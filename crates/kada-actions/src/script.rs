//! 内置脚本（规划 7.2-㉟）：在 Rhai 沙箱里跑一小段脚本，给动作链做计算 / 拼字符串。
//!
//! **为什么是 Rhai 而不是 Lua**：Rhai 是纯 Rust 引擎，不引入 C 编译器 / 动态库，
//! 三平台（含 aarch64-apple-darwin 交叉检查）照旧 `cargo check` 即可；Lua（mlua）
//! 要么带 vendored C 源码、要么要求本机有 Lua，会让现有交叉检查与发布流水线变复杂。
//!
//! **沙箱边界**（改了这里先想清楚）：
//! - 引擎开 `no_module`：脚本里没有 `import`，读不到任何文件；
//! - 不注册任何文件 / 网络 / 进程函数，只有 [`run`] 里列出的几个 Kada 函数；
//! - 操作数上限 [`kada_core::INLINE_SCRIPT_MAX_OPS`]，字符串 / 数组 / 映射 / 调用深度都有上限；
//! - 另受动作链超时（`settings.action_timeout_ms`）与「停止执行」约束——`on_progress`
//!   每若干条指令检查一次，到点就取消（不是等脚本自己跑完）。
//!
//! **变量语义**：脚本读的是本次执行开始时的变量快照；`set_var` 先写在本地，脚本
//! **正常结束后**才合并回动作链（失败 / 超时 / 中止都不留半成品）。
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kada_core::{Value, Vars, INLINE_SCRIPT_MAX_OPS};
use rhai::{Dynamic, Engine, EvalAltResult, Map, Scope};

/// 单次脚本输出的上限（字节）：消息中心与执行记录都不该被一步脚本撑爆。
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// 单个变量值的上限（字节）：变量会被后续动作整段注入，太长只会拖慢执行。
const MAX_VAR_BYTES: usize = 64 * 1024;
/// 一次脚本最多能写多少个变量（防止 `for i in 0..100000 { set_var(...) }` 撑爆变量表）。
const MAX_VAR_WRITES: usize = 100;

/// 脚本执行结果（由调用方组装成 [`crate::CommandResult`]）。
pub struct ScriptOutput {
    /// 本步输出：`print` 的行 + 脚本返回值（见 [`run`] 的输出口径）。
    pub stdout: String,
    /// 失败原因（成功时为空）。
    pub stderr: String,
    /// 是否失败（语法 / 运行时错误 / 超时 / 中止）。
    pub failed: bool,
    /// 脚本写入的变量（正常结束后才合并）。
    pub vars: Vec<(String, String)>,
    /// 脚本返回值：`None` = 返回 `()`（没有返回值）；`Some(s)` = 返回了值（`s` 可以是空串）。
    /// 区分这两者才能让「返回空串」不退回 `print` 输出（CS-06）。
    ret: Option<String>,
}

impl ScriptOutput {
    /// 存进 `var` 变量的文本：**优先脚本返回值**，没有返回值（返回 `()`）才退回 `print`
    /// 输出。`print("debug"); 42` 得到 `42` 而不是 `debug\n42`——调试信息看消息中心，
    /// 变量只装结果。
    pub fn value(&self) -> String {
        self.ret.clone().unwrap_or_else(|| self.stdout.clone())
    }
}

/// 脚本运行期的共享状态（`print` 收集 + 变量读写）。
struct State {
    /// 变量快照（只读）与脚本写入（成功后才合并）。
    vars: BTreeMap<String, String>,
    writes: BTreeMap<String, String>,
    /// `print` / `debug` 的输出。
    prints: String,
    /// 输出是否已截断（只提示一次）。
    truncated: bool,
    /// `on_progress` 主动取消时写这里：`(原因)`。
    terminated: Option<String>,
}

impl State {
    /// 追加一段输出（超上限就截断并留一行说明）。
    fn push_out(&mut self, text: &str, newline: bool) {
        if self.truncated {
            return;
        }
        if self.prints.len() + text.len() + 1 > MAX_OUTPUT_BYTES {
            let room = MAX_OUTPUT_BYTES.saturating_sub(self.prints.len());
            let mut cut = room.min(text.len());
            while cut > 0 && !text.is_char_boundary(cut) {
                cut -= 1;
            }
            self.prints.push_str(&text[..cut]);
            self.prints.push_str("\n…（输出过长，已截断）");
            self.truncated = true;
            return;
        }
        self.prints.push_str(text);
        if newline {
            self.prints.push('\n');
        }
    }
}

/// 执行一段内置脚本。
///
/// `timeout` 为 [`Duration::ZERO`] 表示不限时（仍受操作数上限约束）；`gen` 是本次动作链
/// 开始时的中止代数（`abort::generation()`），用户点「停止执行」后脚本会在下一次进度检查时取消。
///
/// 输出口径：`print` 的每一行按顺序输出；脚本返回值（非 `()`）非空时接在后面。
/// 写进变量（动作的 `var` 字段）时优先取返回值，没有返回值就用 `print` 的输出。
pub fn run(
    source: &str,
    trigger: &str,
    name: &str,
    vars: &Vars,
    timeout: Duration,
    gen: u64,
) -> Result<ScriptOutput, String> {
    run_with_limits(
        source,
        trigger,
        name,
        vars,
        timeout,
        gen,
        INLINE_SCRIPT_MAX_OPS,
    )
}

/// 同 [`run`]，但可以指定操作数上限（单测用它把超时 / 中止的路径跑稳）。
fn run_with_limits(
    source: &str,
    trigger: &str,
    name: &str,
    vars: &Vars,
    timeout: Duration,
    gen: u64,
    max_ops: u64,
) -> Result<ScriptOutput, String> {
    if source.len() > kada_core::INLINE_SCRIPT_MAX_BYTES {
        return Err(format!(
            "脚本内容超过上限（{} KB）",
            kada_core::INLINE_SCRIPT_MAX_BYTES / 1024
        ));
    }

    let state = Arc::new(Mutex::new(State {
        vars: vars.iter().map(|(k, v)| (k.clone(), text_of(v))).collect(),
        writes: BTreeMap::new(),
        prints: String::new(),
        truncated: false,
        terminated: None,
    }));

    let mut engine = Engine::new();
    engine.set_max_operations(max_ops);
    engine.set_max_string_size(MAX_VAR_BYTES);
    engine.set_max_array_size(10_000);
    engine.set_max_map_size(1_000);
    engine.set_max_call_levels(32);
    engine.set_max_expr_depths(32, 32);

    // 读：`get_var("名")` / `has_var("名")` / `vars()`（整张表）。
    // 注意不能叫 `var`——那是 Rhai 的保留字。
    {
        let st = state.clone();
        engine.register_fn("get_var", move |name: &str| -> String {
            st.lock()
                .unwrap_or_else(|e| e.into_inner())
                .vars
                .get(name.trim())
                .cloned()
                .unwrap_or_default()
        });
    }
    {
        let st = state.clone();
        engine.register_fn("has_var", move |name: &str| -> bool {
            st.lock()
                .unwrap_or_else(|e| e.into_inner())
                .vars
                .contains_key(name.trim())
        });
    }
    {
        let st = state.clone();
        engine.register_fn("vars", move || -> Map {
            let st = st.lock().unwrap_or_else(|e| e.into_inner());
            let mut map = Map::new();
            for (k, v) in &st.vars {
                map.insert(k.as_str().into(), Dynamic::from(v.clone()));
            }
            map
        });
    }
    // 只读 JSON 取值（CS-19）：`json_get(text, "file.name")`。
    // 同样不碰文件 / 网络——文本是脚本自己拼出来的（通常是 get_var 读到的命令输出）。
    engine.register_fn(
        "json_get",
        |json: &str, path: &str| -> Result<String, Box<EvalAltResult>> {
            json_get_text(json, path).map_err(Into::into)
        },
    );
    // 写：`set_var("名", 值)`，值可以是字符串 / 整数 / 布尔。
    {
        let st = state.clone();
        engine.register_fn(
            "set_var",
            move |name: &str, value: &str| -> Result<(), Box<EvalAltResult>> {
                write_var(&st, name, value)
            },
        );
    }
    {
        let st = state.clone();
        engine.register_fn(
            "set_var",
            move |name: &str, value: i64| -> Result<(), Box<EvalAltResult>> {
                write_var(&st, name, &value.to_string())
            },
        );
    }
    {
        let st = state.clone();
        engine.register_fn(
            "set_var",
            move |name: &str, value: bool| -> Result<(), Box<EvalAltResult>> {
                write_var(&st, name, if value { "true" } else { "false" })
            },
        );
    }
    // 上下文：谁触发的。
    {
        let trigger = trigger.to_string();
        engine.register_fn("trigger", move || -> String { trigger.clone() });
    }
    {
        let name = name.to_string();
        engine.register_fn("shortcut_name", move || -> String { name.clone() });
    }
    // `print` / `debug` 一律进本步输出——桌面应用没有控制台，写 stdout 等于丢掉。
    {
        let st = state.clone();
        engine.on_print(move |s| {
            st.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_out(s, true);
        });
    }
    {
        let st = state.clone();
        engine.on_debug(move |s, _, _| {
            st.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_out(s, true);
        });
    }

    // 超时 / 中止：每 256 条指令看一次表（`Instant::now()` 没必要每条都取）。
    {
        let st = state.clone();
        let start = Instant::now();
        engine.on_progress(move |ops| {
            if ops % 256 != 0 {
                return None;
            }
            let mut st = st.lock().unwrap_or_else(|e| e.into_inner());
            if crate::abort::aborted(gen) {
                st.terminated = Some("已停止执行".into());
                return Some("已停止执行".into());
            }
            if !timeout.is_zero() && start.elapsed() >= timeout {
                let secs = timeout.as_secs_f64();
                let reason = if secs >= 1.0 {
                    format!("脚本执行超时（超过 {} 秒）", secs.round() as u64)
                } else {
                    format!("脚本执行超时（超过 {} 毫秒）", timeout.as_millis())
                };
                st.terminated = Some(reason.clone());
                return Some(reason.into());
            }
            None
        });
    }

    let mut scope = Scope::new();
    let result: Result<Dynamic, Box<EvalAltResult>> = engine.eval_with_scope(&mut scope, source);

    let st = state.lock().unwrap_or_else(|e| e.into_inner());
    match result {
        Ok(value) => {
            // `None` = 返回 `()`（无返回值）；`Some` = 返回了值（空串也算）——
            // `value()` 靠这个区分「没返回值，退回 prints」和「返回了空串」（CS-06）。
            let ret = if value.is_unit() {
                None
            } else {
                Some(value.to_string())
            };
            let ret_text = ret.clone().unwrap_or_default();
            let prints = st.prints.trim_end().to_string();
            let stdout = match (prints.is_empty(), ret_text.is_empty()) {
                (true, true) => String::new(),
                (true, false) => ret_text.clone(),
                (false, true) => prints,
                (false, false) => format!("{prints}\n{ret_text}"),
            };
            let writes: Vec<(String, String)> = st
                .writes
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            Ok(ScriptOutput {
                stdout: truncate(&stdout),
                stderr: String::new(),
                failed: false,
                vars: writes,
                ret,
            })
        }
        Err(err) => {
            // `on_progress` 的取消原因是我们自己写的中文，比 Rhai 的英文报错更好读。
            let reason = st
                .terminated
                .clone()
                .unwrap_or_else(|| format_eval_error(&err));
            let stdout = truncate(st.prints.trim_end());
            Ok(ScriptOutput {
                stderr: format!("〔咔哒〕{reason}"),
                stdout,
                failed: true,
                vars: Vec::new(),
                // 失败没有返回值可言：`value()` 退回 prints（失败时不写变量，调用方有闸门）。
                ret: None,
            })
        }
    }
}

/// 写一个变量（带校验）：名字非空、长度 / 条数 / 值大小都有上限。
fn write_var(st: &Arc<Mutex<State>>, name: &str, value: &str) -> Result<(), Box<EvalAltResult>> {
    let name = name.trim();
    if name.is_empty() {
        return Err("变量名不能为空".into());
    }
    if name.chars().count() > 128 {
        return Err("变量名过长（最多 128 个字符）".into());
    }
    if value.len() > MAX_VAR_BYTES {
        return Err(format!("变量值过长（最多 {} KB）", MAX_VAR_BYTES / 1024).into());
    }
    let mut st = st.lock().unwrap_or_else(|e| e.into_inner());
    if !st.writes.contains_key(name) && st.writes.len() >= MAX_VAR_WRITES {
        return Err(format!("一次脚本最多写 {MAX_VAR_WRITES} 个变量").into());
    }
    st.writes.insert(name.to_string(), value.to_string());
    Ok(())
}

/// 从 JSON 文本里按点分路径取值（CS-19 的 `json_get`）。
///
/// 路径用 `.` 分隔，对象按键取、数组按下标取（`items.0.name`）；取不到 = 空串。
/// **只解析传进来的文本**：不读文件、不联网，沙箱边界没变。
/// 非法 JSON 返回错误（让脚本作者知道自己的文本不对，而不是悄悄拿到空串）。
fn json_get_text(json: &str, path: &str) -> Result<String, String> {
    let root: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("不是合法 JSON：{e}"))?;
    let mut cur = &root;
    for seg in path.split('.').filter(|s| !s.trim().is_empty()) {
        cur = match cur {
            serde_json::Value::Object(map) => match map.get(seg.trim()) {
                Some(v) => v,
                None => return Ok(String::new()),
            },
            serde_json::Value::Array(items) => match seg.trim().parse::<usize>() {
                Ok(i) => match items.get(i) {
                    Some(v) => v,
                    None => return Ok(String::new()),
                },
                Err(_) => return Ok(String::new()),
            },
            _ => return Ok(String::new()),
        };
    }
    Ok(match cur {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    })
}

/// 变量值的文本形态：文件变量取完整路径，布尔取 true/false。
fn text_of(v: &Value) -> String {
    match v {
        Value::Text(t) => t.text.clone(),
        Value::Bool(b) => b.to_string(),
        Value::File(f) => f.path.clone(),
    }
}

/// 截断到输出上限（按字符边界）。
fn truncate(s: &str) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        return s.to_string();
    }
    let mut cut = MAX_OUTPUT_BYTES;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}\n…（输出过长，已截断）", &s[..cut])
}

/// 把 Rhai 的英文错误整理成「第 N 行：…」；没有位置信息就不带行号。
fn format_eval_error(err: &EvalAltResult) -> String {
    let pos = err.position();
    let line = pos.line().unwrap_or(0);
    let text = err.to_string();
    if line > 0 {
        format!("脚本第 {line} 行：{text}")
    } else {
        format!("脚本错误：{text}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kada_core::TextValue;
    use std::collections::BTreeMap;

    fn vars() -> Vars {
        let mut v: Vars = BTreeMap::new();
        v.insert(
            "who".into(),
            Value::Text(TextValue {
                text: "Kada".into(),
                exit_code: None,
            }),
        );
        v
    }

    #[test]
    fn reads_vars_and_returns_value() {
        let _g = crate::abort::test_lock();
        let out = run(
            r#"let w = get_var("who"); "你好，" + w"#,
            "Ctrl+Alt+K",
            "测试",
            &vars(),
            Duration::ZERO,
            crate::abort::generation(),
        )
        .unwrap();
        assert!(!out.failed, "{}", out.stderr);
        assert_eq!(out.stdout, "你好，Kada");
    }

    #[test]
    fn print_and_return_are_both_kept() {
        let _g = crate::abort::test_lock();
        let out = run(
            r#"print("第一行"); print("第二行"); 42"#,
            "",
            "",
            &vars(),
            Duration::ZERO,
            crate::abort::generation(),
        )
        .unwrap();
        assert_eq!(out.stdout, "第一行\n第二行\n42");
    }

    /// `value()` 返回值优先（CS-06）：`print` 是调试输出，不该混进变量。
    #[test]
    fn value_prefers_return_over_prints() {
        let _g = crate::abort::test_lock();
        let out = run(
            r#"print("debug"); 42"#,
            "",
            "",
            &vars(),
            Duration::ZERO,
            crate::abort::generation(),
        )
        .unwrap();
        assert!(!out.failed, "{}", out.stderr);
        assert_eq!(out.stdout, "debug\n42", "stdout 仍是展示文本");
        assert_eq!(out.value(), "42", "变量只拿返回值");
    }

    /// 无返回值（返回 `()`）时 `value()` 退回 `print` 输出。
    #[test]
    fn value_falls_back_to_prints_without_return() {
        let _g = crate::abort::test_lock();
        let out = run(
            r#"print("debug")"#,
            "",
            "",
            &vars(),
            Duration::ZERO,
            crate::abort::generation(),
        )
        .unwrap();
        assert!(!out.failed, "{}", out.stderr);
        assert_eq!(out.value(), "debug");
    }

    /// 「返回空串」与「无返回值」是两回事：前者 `value()` 得空串（不退回 prints），
    /// 后者才退回 prints。
    #[test]
    fn value_distinguishes_empty_return_from_no_return() {
        let _g = crate::abort::test_lock();
        let empty = run(
            "print(\"debug\"); \"\"",
            "",
            "",
            &vars(),
            Duration::ZERO,
            crate::abort::generation(),
        )
        .unwrap();
        assert!(!empty.failed, "{}", empty.stderr);
        assert_eq!(empty.value(), "", "返回空串就给空串，不退回 prints");

        let none = run(
            r#"print("debug"); let x = 1"#,
            "",
            "",
            &vars(),
            Duration::ZERO,
            crate::abort::generation(),
        )
        .unwrap();
        assert!(!none.failed, "{}", none.stderr);
        assert_eq!(none.value(), "debug", "无返回值才退回 prints");
    }

    #[test]
    fn set_var_is_committed_only_on_success() {
        let _g = crate::abort::test_lock();
        let out = run(
            r#"set_var("greeting", "hi " + get_var("who")); set_var("count", 3)"#,
            "",
            "",
            &vars(),
            Duration::ZERO,
            crate::abort::generation(),
        )
        .unwrap();
        assert!(!out.failed);
        assert_eq!(
            out.vars,
            vec![
                ("count".to_string(), "3".to_string()),
                ("greeting".to_string(), "hi Kada".to_string())
            ]
        );

        let failed = run(
            r#"set_var("half", "written"); nope()"#,
            "",
            "",
            &vars(),
            Duration::ZERO,
            crate::abort::generation(),
        )
        .unwrap();
        assert!(failed.failed);
        assert!(failed.vars.is_empty(), "失败时不许留半成品变量");
    }

    #[test]
    fn json_get_reads_nested_values() {
        let _g = crate::abort::test_lock();
        let out = run(
            r#"let j = json_get(get_var("json"), "file.name"); j"#,
            "",
            "",
            &{
                let mut v = vars();
                v.insert(
                    "json".into(),
                    Value::Text(TextValue {
                        text: r#"{"file":{"name":"a.txt","tags":["x","y"]},"n":3,"ok":true}"#
                            .into(),
                        exit_code: None,
                    }),
                );
                v
            },
            Duration::ZERO,
            crate::abort::generation(),
        )
        .unwrap();
        assert!(!out.failed, "{}", out.stderr);
        assert_eq!(out.stdout, "a.txt");
    }

    #[test]
    fn json_get_rejects_invalid_json() {
        let _g = crate::abort::test_lock();
        let out = run(
            r#"json_get("{nope", "a")"#,
            "",
            "",
            &vars(),
            Duration::ZERO,
            crate::abort::generation(),
        )
        .unwrap();
        assert!(out.failed);
        assert!(out.stderr.contains("不是合法 JSON"), "{}", out.stderr);
    }

    #[test]
    fn empty_name_reports_error() {
        let _g = crate::abort::test_lock();
        let out = run(
            r#"set_var("  ", "x")"#,
            "",
            "",
            &vars(),
            Duration::ZERO,
            crate::abort::generation(),
        )
        .unwrap();
        assert!(out.failed);
        assert!(out.stderr.contains("变量名不能为空"), "{}", out.stderr);
    }

    #[test]
    fn syntax_error_has_line_number() {
        let _g = crate::abort::test_lock();
        let out = run(
            "let x = ;",
            "",
            "",
            &vars(),
            Duration::ZERO,
            crate::abort::generation(),
        )
        .unwrap();
        assert!(out.failed);
        assert!(out.stderr.contains("第 1 行"), "{}", out.stderr);
    }

    #[test]
    fn runaway_loop_hits_operation_limit() {
        let _g = crate::abort::test_lock();
        let out = run_with_limits(
            "let i = 0; while true { i += 1; }",
            "",
            "",
            &vars(),
            Duration::ZERO,
            crate::abort::generation(),
            2_000,
        )
        .unwrap();
        assert!(out.failed);
        assert!(
            out.stderr.contains("operations") || out.stderr.contains("Too many"),
            "应撞上操作数上限：{}",
            out.stderr
        );
    }

    #[test]
    fn progress_cancels_on_timeout() {
        let _g = crate::abort::test_lock();
        let out = run_with_limits(
            "let i = 0; while true { i += 1; }",
            "",
            "",
            &vars(),
            Duration::from_millis(5),
            crate::abort::generation(),
            u64::MAX,
        )
        .unwrap();
        assert!(out.failed);
        assert!(out.stderr.contains("超时"), "{}", out.stderr);
    }

    #[test]
    fn imports_are_unavailable() {
        let _g = crate::abort::test_lock();
        // `no_module` 让 `import` 直接编译失败：脚本读不到任何文件。
        let out = run(
            r#"import "/etc/passwd" as p; p"#,
            "",
            "",
            &vars(),
            Duration::ZERO,
            crate::abort::generation(),
        )
        .unwrap();
        assert!(out.failed);
    }

    #[test]
    fn output_is_truncated() {
        let s = "x".repeat(MAX_OUTPUT_BYTES + 100);
        let t = truncate(&s);
        assert!(t.len() <= MAX_OUTPUT_BYTES + 64);
        assert!(t.ends_with("已截断）"), "{}", &t[t.len() - 40..]);
    }
}
