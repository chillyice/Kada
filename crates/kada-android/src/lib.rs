//! 咔哒安卓桥（S1-a）：Kotlin ↔ `kada-core`。
//!
//! 只做三件事，判定与配置语义**全部在 Rust 里**（与桌面同一份 core，不另写一套）：
//! 1. 配置的读写（`kada_core::config_io`：原子写 + 损坏自愈，与桌面完全一致）；
//! 2. 配置报告（条数 / 冲突 / 本机不支持的条目 / 动作支持情况）；
//! 3. 按键判定（`engine::AndroidEngine`）→ 吞不吞 + 要执行的动作。
//!
//! 所有 `native*` 函数都是 **JSON 进、JSON 出**：Kotlin 只负责按键映射与动作执行，
//! 不解析触发键字符串、不重复实现判重 / 层语义。

mod action_support;
mod engine;
mod report;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use kada_core::{config_io, FrontmostContext, Key, Modifier, ShortcutItem, Trigger};
use kada_core::{Action, Config};

use engine::{AndroidEngine, Rules};

/// 帧内全局状态：无障碍服务单线程调用，一把锁足够。
static STATE: Mutex<Option<Runtime>> = Mutex::new(None);

struct Runtime {
    path: PathBuf,
    tier: String,
    cfg: Config,
    rules: Rules,
    engine: AndroidEngine,
    frontmost: Option<FrontmostContext>,
    /// 上次加载产生的告警（损坏自愈 / 备份恢复）——推进 UI 的消息区。
    warnings: Vec<(String, String)>,
}

impl Runtime {
    fn report(&self) -> String {
        report::config_report(&self.path, &self.tier, &self.cfg, &self.warnings)
    }
}

fn config_path(dir: &Path) -> PathBuf {
    dir.join("config.json")
}

/// 首次启动（没有配置文件）落一份安卓示例：让「按一下就出效果」不用先学格式。
fn ensure_config(dir: &Path) -> Result<(), String> {
    let path = config_path(dir);
    if path.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败：{e}"))?;
    let cfg = sample_config();
    config_io::save(&path, &cfg)
}

/// 安卓示例配置：只有「组合键 → 动作」这一档（无 root 档能落地的部分）。
///
/// 动作只用 `open_url`：它是无 root 档**唯一无需权限、无需输入框**就能端到端验证的
/// 动作；其余动作（文本 / 剪贴板 / 语义动作）等执行器补齐后再进示例。
fn sample_config() -> Config {
    Config {
        shortcuts: vec![
            shortcut("打开示例页", "F8", "https://example.com/kada"),
            shortcut("打开项目主页", "F7", "https://github.com/"),
            shortcut("和弦 F&J", "F&J", "https://example.com/chord"),
            shortcut("序列 F9 Space", "F9 Space", "https://example.com/sequence"),
            shortcut(
                "组合键 Ctrl+Alt+K",
                "Ctrl+Alt+K",
                "https://example.com/combo",
            ),
        ],
        ..Config::default()
    }
}

fn shortcut(name: &str, trigger: &str, url: &str) -> ShortcutItem {
    ShortcutItem {
        name: Some(name.into()),
        triggers: vec![trigger.into()],
        actions: vec![Action::OpenUrl {
            url: url.into(),
            description: None,
        }],
        enabled: true,
        ..ShortcutItem::default()
    }
}

fn load(dir: &Path, tier: &str) -> Result<Runtime, String> {
    ensure_config(dir)?;
    let path = config_path(dir);
    let outcome = config_io::load(&path);
    let warnings = outcome
        .warnings
        .iter()
        .map(|w| (w.summary.clone(), w.detail.clone()))
        .collect();
    let cfg = outcome.config;
    Ok(Runtime {
        path,
        tier: tier.to_string(),
        rules: Rules::build(&cfg),
        engine: AndroidEngine::new(),
        cfg,
        frontmost: None,
        warnings,
    })
}

/// 重新读盘（导入 / 外部改动后调用）。解析不动就保留内存里的旧配置（铁律：别碰磁盘）。
fn reload(rt: &mut Runtime) {
    let outcome = config_io::load(&rt.path);
    rt.warnings = outcome
        .warnings
        .iter()
        .map(|w| (w.summary.clone(), w.detail.clone()))
        .collect();
    rt.cfg = outcome.config;
    rt.rules = Rules::build(&rt.cfg);
    // 配置整份换掉 = 与物理键盘断过一次线：注入的收回、缓冲的丢弃（架构设计 §3.9）。
    rt.engine.reset();
}

fn with_state<T>(f: impl FnOnce(&mut Runtime) -> T) -> Result<T, String> {
    let mut guard = STATE.lock().map_err(|_| "状态锁已中毒".to_string())?;
    let rt = guard.as_mut().ok_or("尚未初始化（先调 nativeInit）")?;
    Ok(f(rt))
}

// ---------------------------------------------------------------------------
// Kotlin 可调用的 API（Kotlin 侧只看得到本文件的 `pub` 函数）
// ---------------------------------------------------------------------------

pub fn init(files_dir: &str, tier: &str) -> String {
    let dir = PathBuf::from(files_dir);
    match load(&dir, tier) {
        Ok(rt) => {
            let text = rt.report();
            if let Ok(mut guard) = STATE.lock() {
                *guard = Some(rt);
            }
            text
        }
        Err(e) => error_json(&e),
    }
}

pub fn report_json() -> String {
    with_state(|rt| rt.report()).unwrap_or_else(error_json)
}

pub fn reload_now() -> String {
    with_state(|rt| {
        reload(rt);
        rt.report()
    })
    .unwrap_or_else(error_json)
}

pub fn set_foreground(package: Option<String>) {
    if let Ok(mut guard) = STATE.lock() {
        if let Some(rt) = guard.as_mut() {
            rt.frontmost = package.map(|p| FrontmostContext {
                process_name: p,
                window_title: String::new(),
            });
        }
    }
}

pub fn reset() {
    if let Ok(mut guard) = STATE.lock() {
        if let Some(rt) = guard.as_mut() {
            rt.engine.reset();
        }
    }
}

pub fn tick() {
    if let Ok(mut guard) = STATE.lock() {
        if let Some(rt) = guard.as_mut() {
            rt.engine.tick(&rt.cfg);
        }
    }
}

/// 判定一次按键。`key` = kada-core 中性键名（`F8` / `Ctrl` / `VolumeUp` …）；
/// `mods` = 逗号分隔的修饰键（`Ctrl,Alt`）。返回见模块注释。
pub fn decide_key(key: &str, down: bool, repeat: bool, mods: &str) -> String {
    let Ok(key) = key.parse::<Key>() else {
        return serde_json::json!({
            "swallow": false,
            "fired": [],
            "note": format!("unknown-key:{key}"),
        })
        .to_string();
    };
    let mods = parse_mods(mods);
    with_state(|rt| {
        let decision = rt.engine.on_key(
            &rt.cfg,
            &rt.rules,
            key,
            down,
            repeat,
            mods,
            rt.frontmost.as_ref(),
        );
        let fired = decision
            .fired
            .iter()
            .map(|f| {
                serde_json::json!({
                    "name": f.name,
                    "trigger": f.trigger,
                    "actions": f.actions,
                    "unsupported": action_support::collect(&f.actions),
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "swallow": decision.swallow,
            "fired": fired,
            "note": decision.note,
        })
        .to_string()
    })
    .unwrap_or_else(|e| {
        serde_json::json!({ "swallow": false, "fired": [], "note": format!("error:{e}") })
            .to_string()
    })
}

/// 导入一份配置 JSON（SAF 选文件后调用）：解析成功才落盘，失败原文件不动。
/// `merge = true` 走 core 的「只增不删」合并；`false` = 整份替换（先留档）。
pub fn import_json(text: &str, merge: bool) -> String {
    let mut incoming: Config = match serde_json::from_str(text) {
        Ok(cfg) => cfg,
        Err(e) => return error_json(format!("解析失败：{e}")),
    };
    incoming.migrate();
    with_state(|rt| {
        let next = if merge {
            kada_core::merge_configs(&rt.cfg, &incoming).0
        } else {
            if config_io::archive_copy(&rt.path, config_io::PRE_IMPORT_MARK).is_none() {
                return error_json("替换导入前留档失败，已中止（原配置未改动）");
            }
            incoming
        };
        if let Err(e) = config_io::save(&rt.path, &next) {
            return error_json(format!("保存失败：{e}"));
        }
        reload(rt);
        rt.report()
    })
    .unwrap_or_else(error_json)
}

/// 导出当前配置文本（SAF 写文件用）。
pub fn export_json() -> String {
    with_state(|rt| serde_json::to_string_pretty(&rt.cfg).unwrap_or_else(|_| "{}".into()))
        .unwrap_or_else(|_| "{}".into())
}

/// 把一份配置 JSON 保存下来（配置页「保存」）：先按桌面同一套清洗规则剔掉不可用条目，
/// 再原子落盘（`config_io::save`）。
pub fn save_json(text: &str) -> String {
    let mut cfg: Config = match serde_json::from_str(text) {
        Ok(cfg) => cfg,
        Err(e) => return error_json(format!("解析失败：{e}")),
    };
    cfg.migrate();
    let (clean, ignored) = kada_core::sanitize_config(&cfg);
    with_state(|rt| {
        if let Err(e) = config_io::save(&rt.path, &clean) {
            return error_json(format!("保存失败：{e}"));
        }
        reload(rt);
        let mut value: serde_json::Value =
            serde_json::from_str(&rt.report()).unwrap_or(serde_json::json!({}));
        if let Some(obj) = value.as_object_mut() {
            obj.insert("ignored".into(), serde_json::json!(ignored));
        }
        value.to_string()
    })
    .unwrap_or_else(error_json)
}

/// 列出当前生效的触发键（报告 / 界面展示用；解析失败的条目带 error）。
pub fn triggers_json() -> String {
    with_state(|rt| {
        let mut out = Vec::new();
        for (idx, s) in rt.cfg.shortcuts.iter().enumerate() {
            if !s.enabled {
                continue;
            }
            for t in &s.triggers {
                out.push(match Trigger::parse(t) {
                    Ok(trig) => serde_json::json!({
                        "item": idx,
                        "name": s.name.clone().unwrap_or_default(),
                        "trigger": t,
                        "kind": trigger_kind(&trig),
                        "layer": s.layer,
                        "supported": s.layer.is_none(),
                    }),
                    Err(e) => serde_json::json!({
                        "item": idx,
                        "name": s.name.clone().unwrap_or_default(),
                        "trigger": t,
                        "kind": "invalid",
                        "error": e.to_string(),
                        "supported": false,
                    }),
                });
            }
        }
        serde_json::json!(out).to_string()
    })
    .unwrap_or_else(|_| "[]".into())
}

fn trigger_kind(t: &Trigger) -> &'static str {
    match t {
        Trigger::Combo(_) => "combo",
        Trigger::Sequence(_) => "sequence",
        Trigger::Chord(_) => "chord",
    }
}

fn parse_mods(text: &str) -> BTreeSet<Modifier> {
    let mut mods = BTreeSet::new();
    for part in text.split(',') {
        match part.trim() {
            "Ctrl" | "Control" => {
                mods.insert(Modifier::Ctrl);
            }
            "Alt" => {
                mods.insert(Modifier::Alt);
            }
            "Shift" => {
                mods.insert(Modifier::Shift);
            }
            "Meta" | "Super" => {
                mods.insert(Modifier::Meta);
            }
            _ => {}
        }
    }
    mods
}

fn error_json(message: impl AsRef<str>) -> String {
    let message = message.as_ref();
    serde_json::json!({ "ok": false, "error": message }).to_string()
}

// ---------------------------------------------------------------------------
// JNI 桥：符号名 = Java_<包名>_<类名>_<方法名>（Kotlin 侧 com.kada.spike.RustBridge）
// 只在 Android 目标编译：桌面跑 `cargo test` 时走上面的 JSON 入口即可。
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod jni_bridge {
    use super::*;
    use jni::objects::{JClass, JString};
    use jni::sys::{jboolean, jstring};
    use jni::JNIEnv;

    fn read(env: &mut JNIEnv, s: &JString) -> String {
        env.get_string(s).map(|js| js.into()).unwrap_or_default()
    }

    fn write(env: &mut JNIEnv, text: String) -> jstring {
        env.new_string(text)
            .map(|s| s.into_raw())
            .unwrap_or(std::ptr::null_mut())
    }

    #[no_mangle]
    pub extern "system" fn Java_com_kada_spike_RustBridge_nativeVersion<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
    ) -> jstring {
        write(&mut env, env!("CARGO_PKG_VERSION").to_string())
    }

    #[no_mangle]
    pub extern "system" fn Java_com_kada_spike_RustBridge_nativeInit<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        files_dir: JString<'local>,
        tier: JString<'local>,
    ) -> jstring {
        let dir = read(&mut env, &files_dir);
        let tier = read(&mut env, &tier);
        write(&mut env, init(&dir, &tier))
    }

    #[no_mangle]
    pub extern "system" fn Java_com_kada_spike_RustBridge_nativeReport<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
    ) -> jstring {
        write(&mut env, report_json())
    }

    #[no_mangle]
    pub extern "system" fn Java_com_kada_spike_RustBridge_nativeReload<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
    ) -> jstring {
        write(&mut env, reload_now())
    }

    #[no_mangle]
    pub extern "system" fn Java_com_kada_spike_RustBridge_nativeDecide<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        key: JString<'local>,
        down: jboolean,
        repeat: jboolean,
        mods: JString<'local>,
    ) -> jstring {
        let key = read(&mut env, &key);
        let mods = read(&mut env, &mods);
        write(&mut env, decide_key(&key, down != 0, repeat != 0, &mods))
    }

    #[no_mangle]
    pub extern "system" fn Java_com_kada_spike_RustBridge_nativeTick<'local>(
        _env: JNIEnv<'local>,
        _class: JClass<'local>,
    ) {
        tick();
    }

    #[no_mangle]
    pub extern "system" fn Java_com_kada_spike_RustBridge_nativeReset<'local>(
        _env: JNIEnv<'local>,
        _class: JClass<'local>,
    ) {
        reset();
    }

    #[no_mangle]
    pub extern "system" fn Java_com_kada_spike_RustBridge_nativeSetForeground<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        package: JString<'local>,
    ) -> jstring {
        let pkg = read(&mut env, &package);
        set_foreground(if pkg.is_empty() { None } else { Some(pkg) });
        write(&mut env, "{}".into())
    }

    #[no_mangle]
    pub extern "system" fn Java_com_kada_spike_RustBridge_nativeTriggers<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
    ) -> jstring {
        write(&mut env, triggers_json())
    }

    #[no_mangle]
    pub extern "system" fn Java_com_kada_spike_RustBridge_nativeImport<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        text: JString<'local>,
        merge: jboolean,
    ) -> jstring {
        let text = read(&mut env, &text);
        write(&mut env, import_json(&text, merge != 0))
    }

    #[no_mangle]
    pub extern "system" fn Java_com_kada_spike_RustBridge_nativeExport<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
    ) -> jstring {
        write(&mut env, export_json())
    }

    #[no_mangle]
    pub extern "system" fn Java_com_kada_spike_RustBridge_nativeSave<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        text: JString<'local>,
    ) -> jstring {
        let text = read(&mut env, &text);
        write(&mut env, save_json(&text))
    }
}
