//! 动作执行引擎：把「触发 → 执行一串动作」的落地逻辑从桌面壳迁出，独立成 crate。
//!
//! 依赖 `kada-core`（动作/配置模型）与 `kada-hook`（文本/按键模拟注入）。
//! 命令类动作的执行结果以 [`CommandResult`] 形式产出，由调用方（桌面壳）通过
//! 回调提交进消息中心；本 crate 不依赖 Tauri，可独立单测。
//!
//! **执行有界**（见规划 7.2-⑦）：命令 / 脚本类动作用 [`RunOptions::action_timeout`] 限时，
//! 超时连同子进程树一起强制终止；用户随时可以用 [`abort::request`] 叫停正在跑的动作链
//! （剩余动作不再执行、正在跑的命令立即被杀）。

use std::time::{Duration, Instant};

use serde::Serialize;

#[cfg(feature = "automation")]
use std::fs;
#[cfg(feature = "automation")]
use std::io::Read;
#[cfg(feature = "automation")]
use std::path::Path;
#[cfg(all(feature = "automation", any(target_os = "windows", target_os = "linux", target_os = "macos")))]
use std::process::{Child, Stdio};
#[cfg(feature = "automation")]
use std::sync::atomic::{AtomicU64, Ordering};

pub mod abort;

use kada_core::{substitute_vars, Action, FrontmostContext, Key, TextInjectMode, TextMode, Vars};

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
use kada_core::{MouseButton, MouseOp};

#[cfg(feature = "automation")]
use kada_core::{AppOperation, FileObject, OsOperation, Shell, TextValue, Value};

#[cfg(target_os = "windows")]
use kada_hook::win::simulate;
#[cfg(target_os = "linux")]
use kada_hook::linux::simulate;
#[cfg(target_os = "macos")]
use kada_hook::macos::simulate;

/// 一次命令类动作（CMD / PowerShell / 关闭程序 / 脚本）的执行结果，进入消息中心。
#[derive(Clone, Serialize)]
pub struct CommandResult {
    /// 动作种类："cmd" | "powershell" | "close_program" | "script"。
    pub kind: String,
    /// 展示标签："CMD" / "PowerShell" / "关闭程序" / "脚本"。
    pub label: String,
    /// 实际执行的命令文本。
    pub command: String,
    /// 触发它的快捷键（已渲染为 "Ctrl+Alt+C" 形式）。
    pub trigger: String,
    /// 快捷键名称（无名称时为空字符串）。
    pub name: String,
    /// 标准输出（UTF-8）。
    pub stdout: String,
    /// 标准错误（UTF-8）。
    pub stderr: String,
    /// 进程退出码；进程未能启动时为 None。
    pub exit_code: Option<i32>,
    /// 是否弹出结果弹窗（仅影响展示，不影响记录）。
    pub show_output: bool,
    /// 执行时间（本地时区，"YYYY-MM-DD HH:MM:SS"）。
    pub time: String,
}

/// 一次动作链执行的运行参数（由调用方从配置读出后传入）。
#[derive(Clone, Copy, Debug)]
pub struct RunOptions {
    /// 命令 / 脚本类动作的执行超时；[`Duration::ZERO`] = 不限时。
    ///
    /// 超时不是「性能选项」而是可靠性兜底：`.output()` 会一直等下去，脚本挂住（等输入、
    /// 死循环、弹了个看不见的确认框）就永久占住这条触发的执行线程——后续动作永不执行、
    /// 线程也收不回来。
    pub action_timeout: Duration,
    /// 文本注入方式（读自 `Settings.text_inject_mode`，规划 7.3-㉒）：文本动作与
    /// 「转大小写」按它选「剪贴板粘贴」或「逐字直发」。
    pub text_inject_mode: TextInjectMode,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            action_timeout: Duration::from_millis(kada_core::DEFAULT_ACTION_TIMEOUT_MS),
            text_inject_mode: TextInjectMode::Clipboard,
        }
    }
}

/// 一次触发的现场：谁触发的（触发键 / 名称）与触发瞬间的前台窗口、触发设备。
///
/// 与 [`RunOptions`]（执行边界，读自配置）分开：这些是「这一次触发」的事实，而前台窗口 /
/// 设备类条件是按**触发那一刻**的现场求值的，所以它和超时一样要整条链共享同一份快照。
#[derive(Clone, Copy)]
pub struct TriggerCtx<'a> {
    pub trigger: &'a str,
    pub name: &'a str,
    pub frontmost: Option<&'a FrontmostContext>,
    /// 触发键来自哪台设备（Linux evdev 设备名）；Windows / macOS 恒 `None`。
    pub device: Option<&'a str>,
}

/// 一次动作链的执行上下文：整条链共享（触发现场 / 超时 / 中止代数快照）。
struct Ctx<'a> {
    t: TriggerCtx<'a>,
    /// 见 [`RunOptions::action_timeout`]。
    #[cfg_attr(not(feature = "automation"), allow(dead_code))]
    timeout: Duration,
    /// 见 [`RunOptions::text_inject_mode`]（文本动作与「转大小写」用）。
    text_mode: TextInjectMode,
    /// 本次执行开始时记下的中止代数（[`abort::aborted`] 的比对基准）。
    gen: u64,
}

/// 递归执行一串动作：遇到「条件判断」动作时按条件求值选择 `then` / `otherwise` 分支继续。
/// 命令类动作的结果通过 `commit` 回调产出（由调用方决定如何进消息中心）。
/// `t` 是本次触发的现场（触发键 / 名称 / 前台窗口，供「前台应用/窗口」类条件求值），
/// `vars` 承载本次触发内的变量，`last_copied` 记录最近一次复制/剪切的来源。
/// `opts` 提供超时等执行边界；用户请求中止时停止执行剩余动作并记一条结果。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[cfg_attr(not(feature = "automation"), allow(unused_variables))]
pub fn run_actions(
    commit: &mut dyn FnMut(CommandResult),
    actions: &[Action],
    t: TriggerCtx<'_>,
    vars: &mut Vars,
    last_copied: &mut Option<String>,
    opts: &RunOptions,
) {
    // 「正在执行」计数（UI 据此回答「有没有东西可以停」）；整个动作链期间有效。
    let _running = abort::RunGuard::new();
    let ctx = Ctx { t, timeout: opts.action_timeout, text_mode: opts.text_inject_mode, gen: abort::generation() };
    run_chain(commit, actions, &ctx, vars, last_copied);
}

/// 动作链的实际执行（`If` 分支递归时复用同一 [`Ctx`]：超时与中止代数整链共享）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[cfg_attr(not(feature = "automation"), allow(unused_variables))]
fn run_chain(
    commit: &mut dyn FnMut(CommandResult),
    actions: &[Action],
    ctx: &Ctx,
    vars: &mut Vars,
    last_copied: &mut Option<String>,
) {
    for action in actions {
        // 中止在执行前判一次：正在跑的进程由 `run_with_limits` 的轮询杀掉，这里保证
        // 「剩余动作不再执行」——否则点停止后还会继续注入按键 / 启动程序。
        if abort::aborted(ctx.gen) {
            commit(abort_result(ctx));
            return;
        }
        #[cfg(feature = "automation")]
        if let Action::If { condition, then, otherwise, .. } = action {
            let branch = if condition.matches(vars, ctx.t.frontmost, ctx.t.device) { then } else { otherwise };
            run_chain(commit, branch, ctx, vars, last_copied);
            // 分支里已经因中止收尾过（并记了一条结果）：外层别再记一遍。
            if abort::aborted(ctx.gen) {
                return;
            }
            continue;
        }
        match run_action(action, ctx, vars, last_copied) {
            Ok(Some(result)) => commit(result),
            Ok(None) => {}
            Err(e) => {
                // 失败同样进消息中心：此前只写 stderr 日志，而发布版是没有控制台的 GUI 程序，
                // 用户看到的现象就是「按了没反应」——注入失败、路径不存在、程序起不来全都查不到原因。
                eprintln!("动作执行失败: {e}");
                commit(failure_result(action, ctx, vars, &e));
            }
        }
    }
}

/// 用户中止时补记的一条结果：进消息中心，让「点了停止」有可见的回应
/// （否则用户只能靠「它不再继续了」猜）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn abort_result(ctx: &Ctx) -> CommandResult {
    CommandResult {
        kind: "abort".into(),
        label: "中止".into(),
        command: "（用户请求停止）".into(),
        trigger: ctx.t.trigger.into(),
        name: ctx.t.name.into(),
        stdout: String::new(),
        stderr: "已停止执行：剩余动作不再执行，正在运行的命令 / 脚本已强制终止。".into(),
        exit_code: None,
        show_output: false,
        time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    }
}

/// 动作失败时进消息中心的一条记录。
///
/// 展示上用「哪种动作失败」当标签（tab 标题是「标签 · 名称」，一眼能看出坏在哪一步），
/// 「命令」一栏放动作摘要（用户写的说明优先，其次是动作本体渲染，如 `删除 D:\x.txt`）——
/// 同一个快捷键挂了多个同类动作时，只给类型是分不清哪一条炸了的。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn failure_result(action: &Action, ctx: &Ctx, vars: &Vars, err: &str) -> CommandResult {
    CommandResult {
        kind: "error".into(),
        label: action_label(action).into(),
        command: action_summary(action, vars),
        trigger: ctx.t.trigger.into(),
        name: ctx.t.name.into(),
        stdout: String::new(),
        stderr: err.to_string(),
        exit_code: None,
        show_output: action_show_output(action),
        time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    }
}

/// 失败记录的展示标签（消息中心 tab 上「标签 · 名称」的左半）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn action_label(action: &Action) -> &'static str {
    match action {
        Action::Text { mode: TextMode::Input, .. } => "文本注入失败",
        Action::Text { .. } => "大小写转换失败",
        Action::Keys { .. } => "按键注入失败",
        Action::PauseMs { .. } => "暂停失败",
        Action::Mouse { .. } => "鼠标模拟失败",
        #[cfg(feature = "automation")]
        Action::Command { .. } | Action::Cmd { .. } | Action::Powershell { .. } => "命令失败",
        #[cfg(feature = "automation")]
        Action::Launch { .. } | Action::App { operation: AppOperation::Launch { .. }, .. } => {
            "启动程序失败"
        }
        #[cfg(feature = "automation")]
        Action::OpenFolder { .. }
        | Action::Os { operation: OsOperation::OpenFolder { .. }, .. } => "打开目录失败",
        #[cfg(feature = "automation")]
        Action::CloseProgram { .. } | Action::App { operation: AppOperation::Close { .. }, .. } => {
            "关闭程序失败"
        }
        #[cfg(feature = "automation")]
        Action::App { operation: AppOperation::Restart { .. }, .. } => "重启程序失败",
        #[cfg(feature = "automation")]
        Action::App { operation: AppOperation::Status { .. }, .. } => "查询程序失败",
        #[cfg(feature = "automation")]
        Action::Script { .. } => "脚本失败",
        #[cfg(feature = "automation")]
        Action::OpenUrl { .. } => "打开网址失败",
        #[cfg(feature = "automation")]
        Action::Os { .. } => "文件操作失败",
        #[cfg(feature = "automation")]
        Action::If { .. } => "条件判断失败",
    }
}

/// 失败记录是否弹窗：沿用动作自己的 `show_output`——用户给命令 / 脚本开了「显示输出」，
/// 那它连启动都没成功这件事同样该弹出来；其余动作静默记入消息中心 + 未读红点。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn action_show_output(action: &Action) -> bool {
    match action {
        #[cfg(feature = "automation")]
        Action::Command { show_output, .. }
        | Action::Cmd { show_output, .. }
        | Action::Powershell { show_output, .. }
        | Action::Script { show_output, .. } => *show_output,
        _ => false,
    }
}

/// 「命令」一栏的内容：用户写的动作说明优先（它才是用户认得的那句话），
/// 没有说明则渲染动作本体摘要。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn action_summary(action: &Action, vars: &Vars) -> String {
    let body = action_body(action, vars);
    match action_description(action).map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => format!("{d}（{body}）"),
        None => body,
    }
}

/// 动作本体摘要（变量已按本次触发的取值代入，展示的是「实际做了什么」）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn action_body(action: &Action, vars: &Vars) -> String {
    let sub = |s: &str| substitute_vars(s, vars);
    match action {
        Action::Text { text, mode, .. } => match mode {
            TextMode::Input => format!("输入文本 {}", preview(&sub(text))),
            TextMode::ToUpper => "把选中/剪贴板文本转为大写".into(),
            TextMode::ToLower => "把选中/剪贴板文本转为小写".into(),
        },
        Action::Keys { keys, .. } => format!("按下 {}", keys.join("+")),
        Action::PauseMs { ms, .. } => format!("暂停 {ms} 毫秒"),
        Action::Mouse { op, .. } => mouse_body(op),
        #[cfg(feature = "automation")]
        Action::Command { command, .. }
        | Action::Cmd { command, .. }
        | Action::Powershell { command, .. } => preview(&sub(command)),
        #[cfg(feature = "automation")]
        Action::Launch { program, args, .. } => {
            format!("启动程序 {}", join_args(&sub(program), args, vars))
        }
        #[cfg(feature = "automation")]
        Action::OpenFolder { path, .. } => format!("打开目录 {}", sub(path)),
        #[cfg(feature = "automation")]
        Action::CloseProgram { program, .. } => format!("关闭程序 {}", sub(program)),
        #[cfg(feature = "automation")]
        Action::Os { operation, .. } => os_body(operation, vars),
        #[cfg(feature = "automation")]
        Action::App { operation, .. } => app_body(operation, vars),
        #[cfg(feature = "automation")]
        Action::OpenUrl { url, .. } => format!("打开网址 {}", sub(url)),
        #[cfg(feature = "automation")]
        Action::Script { path, interpreter, .. } => match interpreter {
            Some(i) => format!("运行 {} {}", sub(i), sub(path)),
            None => format!("运行脚本 {}", sub(path)),
        },
        #[cfg(feature = "automation")]
        Action::If { .. } => "条件判断".into(),
    }
}

/// 鼠标动作摘要（消息中心「命令」一栏）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn mouse_body(op: &MouseOp) -> String {
    match op {
        MouseOp::Move { dx, dy } => format!("鼠标移动 ({dx}, {dy})"),
        MouseOp::Click { button } => {
            let b = match button {
                MouseButton::Left => "左键",
                MouseButton::Right => "右键",
                MouseButton::Middle => "中键",
            };
            format!("鼠标点击{b}")
        }
        MouseOp::Scroll { dx, dy } => format!("鼠标滚轮 ({dx}, {dy})"),
    }
}

#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn os_body(op: &OsOperation, vars: &Vars) -> String {
    let sub = |s: &str| substitute_vars(s, vars);
    match op {
        OsOperation::Copy { source, dest } => format!("复制 {} → {}", sub(source), sub(dest)),
        OsOperation::Cut { source, dest } => format!("剪切 {} → {}", sub(source), sub(dest)),
        OsOperation::Paste { dest } => format!("粘贴到 {}", sub(dest)),
        OsOperation::Delete { path } => format!("删除 {}", sub(path)),
        OsOperation::NewFile { path } => format!("新建文件 {}", sub(path)),
        OsOperation::NewFolder { path } => format!("新建目录 {}", sub(path)),
        OsOperation::OpenFolder { path } => format!("打开目录 {}", sub(path)),
        OsOperation::Zip { source, dest } => format!("压缩 {} → {}", sub(source), sub(dest)),
        OsOperation::Unzip { source, dest } => format!("解压 {} → {}", sub(source), sub(dest)),
        OsOperation::GetFileProps { path, var } => {
            format!("读取 {} 的属性写入变量 {var}", sub(path))
        }
    }
}

#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn app_body(op: &AppOperation, vars: &Vars) -> String {
    let sub = |s: &str| substitute_vars(s, vars);
    match op {
        AppOperation::Launch { program, args } => {
            format!("启动程序 {}", join_args(&sub(program), args, vars))
        }
        AppOperation::Close { program } => format!("关闭程序 {}", sub(program)),
        AppOperation::Status { program, .. } => format!("查询程序 {} 是否在运行", sub(program)),
        AppOperation::Restart { program, args } => {
            format!("重启程序 {}", join_args(&sub(program), args, vars))
        }
    }
}

/// 程序名 + 参数渲染成一行（参数同样代入变量）。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn join_args(program: &str, args: &[String], vars: &Vars) -> String {
    let args: Vec<String> = args.iter().map(|a| substitute_vars(a, vars)).collect();
    if args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {}", args.join(" "))
    }
}

/// 取动作上用户写的说明（`Action` 各变体都带可选 `description`）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn action_description(action: &Action) -> Option<&str> {
    match action {
        Action::Text { description, .. }
        | Action::Keys { description, .. }
        | Action::PauseMs { description, .. }
        | Action::Mouse { description, .. } => description.as_deref(),
        #[cfg(feature = "automation")]
        Action::Command { description, .. }
        | Action::Cmd { description, .. }
        | Action::Powershell { description, .. }
        | Action::Launch { description, .. }
        | Action::OpenFolder { description, .. }
        | Action::CloseProgram { description, .. }
        | Action::Os { description, .. }
        | Action::App { description, .. }
        | Action::OpenUrl { description, .. }
        | Action::Script { description, .. }
        | Action::If { description, .. } => description.as_deref(),
    }
}

/// 摘要里字段长度的上限：这一栏是给用户认「哪一步」的短上下文，
/// 整段替换文本 / 长命令原样铺开会把消息卡片撑得没法看。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
const PREVIEW_MAX_CHARS: usize = 160;

/// 压成单行并截断（换行在卡片里是多行，摘要只需要一眼能认出来）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn preview(s: &str) -> String {
    let one_line: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    match one_line.char_indices().nth(PREVIEW_MAX_CHARS) {
        Some((idx, _)) => format!("{}…", &one_line[..idx]),
        None => one_line,
    }
}

/// 顺序执行一个动作。文本/按键走模拟输入，进程类走 std::process，文件类走 std::fs。
/// 命令类动作（CMD/PowerShell/关闭程序）捕获输出并返回 `Some(CommandResult)`，其余返回 `None`。
/// `vars` 承载本次触发内的变量（`GetFileProps` 写 File、`App::Status` 写 Bool、
/// 命令动作 `var` 非空时写 Text），供后续动作用占位符引用；
/// `last_copied` 记录最近一次复制/剪切的来源，供「粘贴」动作使用。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[cfg_attr(not(feature = "automation"), allow(unused_variables))]
fn run_action(
    action: &Action,
    ctx: &Ctx,
    vars: &mut Vars,
    last_copied: &mut Option<String>,
) -> Result<Option<CommandResult>, String> {
    match action {
        Action::Text { text, mode, .. } => match mode {
            TextMode::Input => {
                let text = substitute_vars(text, vars);
                inject_text(&text, ctx.text_mode)?;
            }
            TextMode::ToUpper | TextMode::ToLower => {
                transform_case(matches!(mode, TextMode::ToUpper), ctx.text_mode)?;
            }
        },
        Action::Keys { keys, .. } => {
            let ks: Vec<Key> = keys
                .iter()
                .map(|k| k.parse::<Key>().map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            // 全部按下 → 稍停 → 逆序松开
            for k in &ks {
                simulate::down(*k);
            }
            std::thread::sleep(Duration::from_millis(30));
            for k in ks.iter().rev() {
                simulate::up(*k);
            }
        }
        Action::Mouse { op, .. } => {
            simulate::mouse(op).map_err(|e| e.to_string())?;
        }
        // 长等待也要能被中止：一次 `PauseMs` 可以是几十秒，睡死在里面的话「停止」要等到
        // 它自然睡醒才生效（用户看到的是「点了停止还在继续」）。切成小片睡，每片查一次。
        Action::PauseMs { ms, .. } => {
            let deadline = Instant::now() + Duration::from_millis(*ms);
            while Instant::now() < deadline {
                if abort::aborted(ctx.gen) {
                    return Ok(None);
                }
                let left = deadline.saturating_duration_since(Instant::now());
                std::thread::sleep(left.min(Duration::from_millis(50)));
            }
        }
        #[cfg(feature = "automation")]
        Action::Command { shell, command, show_output, var, .. } => {
            let (label, kind, is_powershell) = match shell {
                Shell::Cmd => ("CMD", "cmd", false),
                Shell::Powershell => ("PowerShell", "powershell", true),
            };
            if is_powershell && !cfg!(target_os = "windows") {
                return Err("PowerShell 命令仅 Windows 可用".into());
            }
            let command = substitute_vars(command, vars);
            let out = run_cmd(label, is_powershell, &command, ctx)?;
            if !var.trim().is_empty() {
                vars.insert(
                    var.trim().to_string(),
                    Value::Text(TextValue {
                        text: out.stdout.trim().to_string(),
                        exit_code: out.exit_code,
                    }),
                );
            }
            return Ok(Some(out.into_result(kind, label, command, ctx, *show_output)));
        }
        #[cfg(feature = "automation")]
        Action::Cmd { command, show_output, var, .. } => {
            let command = substitute_vars(command, vars);
            let out = run_cmd("CMD", false, &command, ctx)?;
            if !var.trim().is_empty() {
                vars.insert(
                    var.trim().to_string(),
                    Value::Text(TextValue {
                        text: out.stdout.trim().to_string(),
                        exit_code: out.exit_code,
                    }),
                );
            }
            return Ok(Some(out.into_result("cmd", "CMD", command, ctx, *show_output)));
        }
        #[cfg(feature = "automation")]
        Action::Powershell { command, show_output, var, .. } => {
            if !cfg!(target_os = "windows") {
                return Err("PowerShell 动作仅 Windows 可用".into());
            }
            let command = substitute_vars(command, vars);
            let out = run_cmd("PowerShell", true, &command, ctx)?;
            if !var.trim().is_empty() {
                vars.insert(
                    var.trim().to_string(),
                    Value::Text(TextValue {
                        text: out.stdout.trim().to_string(),
                        exit_code: out.exit_code,
                    }),
                );
            }
            return Ok(Some(out.into_result("powershell", "PowerShell", command, ctx, *show_output)));
        }
        #[cfg(feature = "automation")]
        Action::Launch { program, args, .. } => {
            // 旧版动作（正常流程已在加载时迁移到 App，此处兜底）。
            let program = substitute_vars(program, vars);
            let args: Vec<String> = args.iter().map(|a| substitute_vars(a, vars)).collect();
            launch_program(&program, &args)?;
        }
        #[cfg(feature = "automation")]
        Action::CloseProgram { program, .. } => {
            // 旧版动作（正常流程已在加载时迁移到 App，此处兜底）。
            let program = substitute_vars(program, vars);
            return Ok(Some(close_program(&program, ctx)?));
        }
        #[cfg(feature = "automation")]
        Action::OpenFolder { path, .. } => {
            // 旧版动作（正常流程已在加载时迁移到 Os::OpenFolder，此处兜底）。
            run_os(&OsOperation::OpenFolder { path: path.clone() }, vars, last_copied)?;
        }
        #[cfg(feature = "automation")]
        Action::Os { operation, .. } => {
            run_os(operation, vars, last_copied)?;
        }
        #[cfg(feature = "automation")]
        Action::App { operation, .. } => {
            return run_app(operation, ctx, vars);
        }
        #[cfg(feature = "automation")]
        Action::OpenUrl { url, .. } => {
            let url = substitute_vars(url, vars);
            open_url(&url)?;
        }
        #[cfg(feature = "automation")]
        Action::If { .. } => {
            // 条件判断动作由 run_chain 拦截处理，不会到达这里。
            return Err("内部错误：条件判断动作应在运行器内处理".into());
        }
        #[cfg(feature = "automation")]
        Action::Script { path, interpreter, show_output, var, .. } => {
            let path = substitute_vars(path, vars);
            let out = run_script(interpreter.as_deref(), &path, ctx, vars)?;
            if !var.trim().is_empty() {
                vars.insert(
                    var.trim().to_string(),
                    Value::Text(TextValue {
                        text: out.stdout.trim().to_string(),
                        exit_code: out.exit_code,
                    }),
                );
            }
            let command = match interpreter {
                Some(i) => format!("{i} \"{path}\""),
                None => path.clone(),
            };
            return Ok(Some(out.into_result("script", "脚本", command, ctx, *show_output)));
        }
    }
    Ok(None)
}

/// 执行一个操作系统动作（文件复制/剪切/粘贴/删除/新建/压缩/取属性）。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn run_os(
    op: &OsOperation,
    vars: &mut Vars,
    last_copied: &mut Option<String>,
) -> Result<(), String> {
    match op {
        OsOperation::Copy { source, dest } => {
            let s = substitute_vars(source, vars);
            let d = substitute_vars(dest, vars);
            copy_path(&s, &d)?;
            *last_copied = Some(s);
        }
        OsOperation::Cut { source, dest } => {
            let s = substitute_vars(source, vars);
            let d = substitute_vars(dest, vars);
            move_path(&s, &d)?;
            *last_copied = Some(d);
        }
        OsOperation::Paste { dest } => {
            let d = substitute_vars(dest, vars);
            let s = last_copied
                .clone()
                .ok_or_else(|| "粘贴失败：尚无复制/剪切的来源".to_string())?;
            copy_path(&s, &d)?;
        }
        OsOperation::Delete { path } => {
            let p = substitute_vars(path, vars);
            delete_path(&p)?;
        }
        OsOperation::NewFile { path } => {
            let p = substitute_vars(path, vars);
            new_file(&p)?;
        }
        OsOperation::NewFolder { path } => {
            let p = substitute_vars(path, vars);
            std::fs::create_dir_all(&p).map_err(|e| format!("新建目录失败: {e}"))?;
        }
        OsOperation::OpenFolder { path } => {
            let p = substitute_vars(path, vars);
            if cfg!(target_os = "windows") {
                std::process::Command::new("explorer")
                    .arg(&p)
                    .spawn()
                    .map_err(|e| format!("打开目录失败: {e}"))?;
            } else if cfg!(target_os = "macos") {
                // macOS 的 `open` 一个命令同时管「打开目录」与「打开网址」（见 open_url）。
                std::process::Command::new("open")
                    .arg(&p)
                    .spawn()
                    .map_err(|e| format!("打开目录失败: {e}"))?;
            } else {
                std::process::Command::new("xdg-open")
                    .arg(&p)
                    .spawn()
                    .map_err(|e| format!("打开目录失败: {e}"))?;
            }
        }
        OsOperation::Zip { source, dest } => {
            let s = substitute_vars(source, vars);
            let d = substitute_vars(dest, vars);
            zip_path(&s, &d)?;
        }
        OsOperation::Unzip { source, dest } => {
            let s = substitute_vars(source, vars);
            let d = substitute_vars(dest, vars);
            unzip_path(&s, &d)?;
        }
        OsOperation::GetFileProps { path, var } => {
            let p = substitute_vars(path, vars);
            let obj = file_object(&p)?;
            let name = if var.trim().is_empty() { "file".to_string() } else { var.clone() };
            vars.insert(name, Value::File(obj));
        }
    }
    Ok(())
}

/// 按配置的注入方式发文本（规划 7.3-㉒）：默认剪贴板粘贴；目标程序吞粘贴
/// （游戏 / 终端）时用「逐字直发」绕过剪贴板。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn inject_text(text: &str, mode: TextInjectMode) -> Result<(), String> {
    match mode {
        TextInjectMode::Clipboard => simulate::type_text(text).map_err(|e| e.to_string()),
        TextInjectMode::Unicode => simulate::type_text_unicode(text).map_err(|e| e.to_string()),
    }
}

/// 把当前选中/剪贴板文本转为大写或小写后粘贴回原处：
/// 复制选中（平台组合键）→ 读剪贴板 → 转换 → 粘贴（注入方式同文本动作，见 [`inject_text`]）。
/// 无选中时复制通常不改剪贴板，即退化为「转换剪贴板文本」。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn transform_case(upper: bool, text_mode: TextInjectMode) -> Result<(), String> {
    copy_chord();
    std::thread::sleep(Duration::from_millis(80));
    let clip = simulate::get_clipboard_text().map_err(|e| e.to_string())?;
    let out = if upper { clip.to_uppercase() } else { clip.to_lowercase() };
    inject_text(&out, text_mode)?;
    Ok(())
}

/// 「复制选中内容」的组合键：macOS 是 ⌘C，Windows / Linux 是 Ctrl+C。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn copy_chord() {
    if cfg!(target_os = "macos") {
        simulate::chord(&[Key::Meta, Key::C]);
    } else {
        simulate::chord(&[Key::Control, Key::C]);
    }
}

/// 执行一个应用动作（打开/关闭/查询状态/重启）。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn run_app(
    op: &AppOperation,
    ctx: &Ctx,
    vars: &mut Vars,
) -> Result<Option<CommandResult>, String> {
    match op {
        AppOperation::Launch { program, args } => {
            let program = substitute_vars(program, vars);
            let args: Vec<String> = args.iter().map(|a| substitute_vars(a, vars)).collect();
            launch_program(&program, &args)?;
            Ok(None)
        }
        AppOperation::Close { program } => {
            let program = substitute_vars(program, vars);
            Ok(Some(close_program(&program, ctx)?))
        }
        AppOperation::Status { program, var, retries, interval_ms } => {
            let program = substitute_vars(program, vars);
            let image = image_name(&program);
            // 未运行则按 retries/interval_ms 轮询，等程序起来（如「启动后等它就绪」）。
            let mut running = app_running(&image);
            for _ in 0..*retries {
                if running || abort::aborted(ctx.gen) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(*interval_ms));
                running = app_running(&image);
            }
            let name = if var.trim().is_empty() { "app".to_string() } else { var.clone() };
            vars.insert(name, Value::Bool(running));
            Ok(None)
        }
        AppOperation::Restart { program, args } => {
            let program = substitute_vars(program, vars);
            let args: Vec<String> = args.iter().map(|a| substitute_vars(a, vars)).collect();
            restart_program(&program, &args, ctx)?;
            Ok(None)
        }
    }
}

/// 启动程序（可选参数）。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn launch_program(program: &str, args: &[String]) -> Result<(), String> {
    std::process::Command::new(program)
        .args(args)
        .spawn()
        .map_err(|e| format!("启动程序「{program}」失败: {e}"))?;
    Ok(())
}

/// 用系统默认浏览器打开网址。Windows 走 `explorer`（直接以参数传入，不经 shell，
/// 避免 URL 里的 `&`/`?` 等被 cmd 二次解析）；macOS 走 `open`；Linux 走 `xdg-open`。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn open_url(url: &str) -> Result<(), String> {
    let res = if cfg!(target_os = "windows") {
        std::process::Command::new("explorer").arg(url).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    res.map(|_| ()).map_err(|e| format!("打开网址「{url}」失败: {e}"))
}

/// 关闭程序（结束所有同名进程），返回命令结果供消息中心展示。
///
/// Windows 走 `taskkill`；macOS / Linux 都走 `pkill`（两边都自带这个命令）。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn close_program(program: &str, ctx: &Ctx) -> Result<CommandResult, String> {
    let image = image_name(program);
    let cmd = if cfg!(target_os = "windows") {
        format!("taskkill /IM {image} /F /T")
    } else {
        format!("pkill -f {image}")
    };
    let out = run_cmd("关闭程序", false, &cmd, ctx)?;
    Ok(out.into_result("close_program", "关闭程序", cmd, ctx, false))
}

/// 重启程序：先关闭（忽略「未在运行」），再启动。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn restart_program(program: &str, args: &[String], ctx: &Ctx) -> Result<(), String> {
    let _ = close_program(program, ctx);
    launch_program(program, args)?;
    Ok(())
}

/// 判断程序是否正在运行。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn app_running(program: &str) -> bool {
    if cfg!(target_os = "windows") {
        match std::process::Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {program}"), "/NH"])
            .output()
        {
            Ok(o) => String::from_utf8_lossy(&o.stdout)
                .to_lowercase()
                .contains(&program.to_lowercase()),
            Err(_) => false,
        }
    } else {
        std::process::Command::new("pgrep")
            .args(["-x", program])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// 取程序名（镜像名）：`program` 为带路径形式时取最后一段文件名，否则原样返回。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn image_name(program: &str) -> String {
    Path::new(program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_string())
}

/// 复制文件或目录（目录递归）。`dest` 为已存在目录时复制到其下，否则视为完整目标路径。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn copy_path(source: &str, dest: &str) -> Result<(), String> {
    let src = Path::new(source);
    let dst = Path::new(dest);
    if !src.exists() {
        return Err(format!("复制失败：源「{source}」不存在"));
    }
    let target = if dst.is_dir() {
        dst.join(src.file_name().ok_or("源路径无效")?)
    } else {
        dst.to_path_buf()
    };
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if src.is_dir() {
        copy_dir_rec(src, &target).map_err(|e| format!("复制目录失败: {e}"))
    } else {
        fs::copy(src, &target).map_err(|e| format!("复制文件失败: {e}"))?;
        Ok(())
    }
}

#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn copy_dir_rec(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_rec(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 移动文件或目录（同盘 `rename`；跨盘回退为复制后删除源）。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn move_path(source: &str, dest: &str) -> Result<(), String> {
    let src = Path::new(source);
    if !src.exists() {
        return Err(format!("剪切失败：源「{source}」不存在"));
    }
    let dst = Path::new(dest);
    let target = if dst.is_dir() {
        dst.join(src.file_name().ok_or("源路径无效")?)
    } else {
        dst.to_path_buf()
    };
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::rename(src, &target).is_ok() {
        return Ok(());
    }
    // 跨盘 rename 失败：复制后删除源。
    copy_path(source, dest)?;
    delete_path(source)
}

/// 删除文件或目录（目录递归删除）。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn delete_path(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("删除失败：「{path}」不存在"));
    }
    if p.is_dir() {
        fs::remove_dir_all(p).map_err(|e| format!("删除目录失败: {e}"))
    } else {
        fs::remove_file(p).map_err(|e| format!("删除文件失败: {e}"))
    }
}

/// 新建空文件（自动创建父目录；已存在则截断为空）。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn new_file(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建父目录失败: {e}"))?;
    }
    fs::write(p, b"").map_err(|e| format!("新建文件失败: {e}"))
}

/// 压缩为 zip：Windows 走 `Compress-Archive`，Linux 走 `zip`。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn zip_path(source: &str, dest: &str) -> Result<(), String> {
    let src = Path::new(source);
    if !src.exists() {
        return Err(format!("压缩失败：源「{source}」不存在"));
    }
    let dest = if dest.to_lowercase().ends_with(".zip") {
        dest.to_string()
    } else {
        format!("{dest}.zip")
    };
    if let Some(parent) = Path::new(&dest).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    }
    let status = if cfg!(target_os = "windows") {
        let full =
            format!("Compress-Archive -Path '{}' -DestinationPath '{}' -Force", source, dest);
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", full.as_str()])
            .status()
    } else {
        std::process::Command::new("zip").args(["-r", &dest, source]).status()
    }
    .map_err(|e| format!("启动压缩命令失败: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("压缩失败（源不存在或系统缺少压缩组件）".into())
    }
}

/// 解压 zip：Windows 走 `Expand-Archive`，Linux 走 `unzip`。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn unzip_path(source: &str, dest: &str) -> Result<(), String> {
    let src = Path::new(source);
    if !src.exists() {
        return Err(format!("解压失败：压缩包「{source}」不存在"));
    }
    fs::create_dir_all(dest).map_err(|e| format!("创建解压目录失败: {e}"))?;
    let status = if cfg!(target_os = "windows") {
        let full =
            format!("Expand-Archive -Path '{}' -DestinationPath '{}' -Force", source, dest);
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", full.as_str()])
            .status()
    } else {
        std::process::Command::new("unzip")
            .args(["-o", source, "-d", dest])
            .status()
    }
    .map_err(|e| format!("启动解压命令失败: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("解压失败（压缩包损坏或系统缺少解压组件）".into())
    }
}

/// 读取文件/目录属性，构造 [`FileObject`]。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn file_object(path: &str) -> Result<FileObject, String> {
    let p = Path::new(path);
    let meta = fs::metadata(p).map_err(|e| format!("读取属性失败：{e}"))?;
    let is_dir = meta.is_dir();
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let size = if is_dir { 0 } else { meta.len() };
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(FileObject { name, path: path.to_string(), dir, stem, ext, size, modified, is_dir })
}

/// 命令 / 脚本的执行结果与「被强制终止」的原因。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
struct CmdOutput {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    /// `Some(说明)` = 因超时 / 用户中止被强制终止（退出码不再是进程自己的语义，置 None）。
    killed: Option<String>,
}

#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
impl CmdOutput {
    /// 组装进消息中心的命令结果；被强制终止时把原因写进 stderr（用户唯一能看到的地方）。
    fn into_result(
        self,
        kind: &str,
        label: &str,
        command: String,
        ctx: &Ctx,
        show_output: bool,
    ) -> CommandResult {
        let stderr = match &self.killed {
            Some(reason) => {
                let note = format!("〔咔哒〕{reason}，已强制终止该进程及其子进程。");
                if self.stderr.is_empty() {
                    note
                } else {
                    format!("{note}\n{}", self.stderr)
                }
            }
            None => self.stderr,
        };
        CommandResult {
            kind: kind.into(),
            label: label.into(),
            command,
            trigger: ctx.t.trigger.into(),
            name: ctx.t.name.into(),
            stdout: self.stdout,
            stderr,
            exit_code: self.exit_code,
            show_output,
            time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        }
    }
}

/// 等待子进程结束时轮询的间隔：既是中止 / 超时的判定粒度，也是 CPU 占用的上界
/// （20ms 一次 `try_wait` 基本无感，用户点「停止」最多 20ms 后进程开始被杀）。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
const WAIT_POLL_MS: u64 = 20;

/// 跑一个子进程并**限时**等它结束（超时或用户中止则连同子进程树一起杀掉），捕获 UTF-8 输出。
///
/// 不用 `Command::output()`：它没有「等多久」，脚本挂住就永久占住本条触发的执行线程
/// （后续动作永不执行、线程泄漏）。这里改为手动收尾：先把两个管道交给读线程，再轮询
/// `try_wait` 判超时 / 中止。
///
/// 管道必须边跑边读：子进程输出超过管道缓冲（Windows 约 4~64KB）而没人读时，它写阻塞、
/// 我们等它退出——双方互等，超时判定也就永远等不到（`output()` 内部正是用读线程避开了
/// 这一点，所以这里也不能省）。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn run_with_limits(
    cmd: &mut std::process::Command,
    label: &str,
    ctx: &Ctx,
) -> Result<CmdOutput, String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    make_process_group(cmd);
    let mut child = cmd.spawn().map_err(|e| format!("执行 {label} 失败: {e}"))?;

    let stdout = child.stdout.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            buf
        })
    });
    let stderr = child.stderr.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            buf
        })
    });

    let deadline = if ctx.timeout.is_zero() { None } else { Some(Instant::now() + ctx.timeout) };
    let killed = loop {
        if abort::aborted(ctx.gen) {
            break Some("已中止".to_string());
        }
        if let Some(d) = deadline {
            if Instant::now() >= d {
                break Some(format!("执行超时（超过 {} 秒）", fmt_secs(ctx.timeout)));
            }
        }
        match child.try_wait() {
            Ok(Some(_)) => break None,
            Ok(None) => std::thread::sleep(Duration::from_millis(WAIT_POLL_MS)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("等待 {label} 结束失败: {e}"));
            }
        }
    };

    if killed.is_some() {
        kill_tree(&mut child);
    }
    let status = child.wait().map_err(|e| format!("回收 {label} 进程失败: {e}"))?;
    // 读线程到这里必然已经结束：进程退出 / 被杀 → 管道写端全部关闭 → read_to_end 返回。
    let join = |h: Option<std::thread::JoinHandle<Vec<u8>>>| {
        h.map(|h| h.join().unwrap_or_default()).unwrap_or_default()
    };
    let stdout = String::from_utf8_lossy(&join(stdout)).into_owned();
    let stderr = String::from_utf8_lossy(&join(stderr)).into_owned();
    let exit_code = if killed.is_some() { None } else { status.code() };
    Ok(CmdOutput { stdout, stderr, exit_code, killed })
}

/// 把超时时长渲染成人话（`90 秒` / `1.5 秒` / `200 毫秒`）。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn fmt_secs(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms} 毫秒")
    } else if ms.is_multiple_of(1000) {
        format!("{}", ms / 1000)
    } else {
        format!("{:.1}", ms as f64 / 1000.0)
    }
}

/// 强制终止子进程**及其子进程**。
///
/// `Child::kill` 只杀直接子进程，而 Windows 上我们跑的是 `cmd /C <临时批处理>` /
/// `powershell -Command`、Linux 上是 `sh -c`：真正干活的是它们的孙进程（脚本、被启动的
/// 程序）。只杀直接子进程 = 命令“看起来”结束了但脚本还在跑，而且它占着 stdout 管道，
/// 连读线程都不会结束。`taskkill /T` 才杀得掉整棵树。
#[cfg(all(target_os = "windows", feature = "automation"))]
fn kill_tree(child: &mut Child) {
    let pid = child.id();
    let ok = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        // taskkill 也可能失败（进程刚好自己退了）：退回只杀直接子进程，聊胜于无。
        let _ = child.kill();
    }
}

/// unix（Linux / macOS）：让子进程**自成进程组**（`setpgid(0,0)`）。
///
/// 没有这一句，子进程与我们同组，「杀整棵树」就无从下手——`sh -c` 的孙进程既不是
/// 我们的直接子进程，也分不出来哪些是它的。自成一组后 [`kill_tree`] 打的是
/// `kill(-pgid)` = 整组一起收。Windows 不需要：`taskkill /T` 自己遍历进程树。
#[cfg(all(unix, feature = "automation"))]
fn make_process_group(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(all(not(unix), feature = "automation"))]
fn make_process_group(_cmd: &mut std::process::Command) {}

/// 强制终止子进程**及其子进程**（Linux / macOS）。
///
/// `Child::kill` 只发 SIGKILL 给直接子进程，而我们跑的是 `sh -c <命令>`：真正干活的是
/// 它的孙进程（脚本本体、被启动的程序）。只杀直接子进程 = 命令「看起来」结束了但脚本
/// 还在跑，而且它占着 stdout 管道，连读线程都不会结束。子进程在 spawn 时已自成进程组
/// （见 [`make_process_group`]），这里 `kill(-pgid)` 一次打整组；失败（组已不存在 /
/// 刚好都退了）退回只杀直接子进程，聊胜于无。
///
/// 天花板：孙进程若自己 `setsid` 脱离（服务化 / nohup 守护），组里找不到它，不追。
#[cfg(all(unix, feature = "automation"))]
fn kill_tree(child: &mut Child) {
    let pgid = child.id() as i32;
    // SAFETY: 只发一个信号；pgid 是本进程刚 spawn 的子进程组 id（`process_group(0)` 定的），
    // 负号 = 打整组——绝不能是 0，0 会打到我们自己所在的组。
    if unsafe { libc::kill(-pgid, libc::SIGKILL) } != 0 {
        let _ = child.kill();
    }
}

/// 执行 shell 命令并捕获 UTF-8 输出（限时、可中止，见 [`run_with_limits`]）。
/// Windows 下 cmd 前缀 `chcp 65001`、PowerShell 前缀设置输出编码，保证中文不乱码；
/// macOS / Linux 下 CMD 走 `sh -c`（两边都有 `/bin/sh`）。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn run_cmd(label: &str, is_powershell: bool, command: &str, ctx: &Ctx) -> Result<CmdOutput, String> {
    // Windows 的 CMD 走临时批处理文件（见下），跑完要删掉——被强杀时也得删。
    let mut temp_bat: Option<std::path::PathBuf> = None;
    let mut cmd = if cfg!(target_os = "windows") {
        if is_powershell {
            let full = format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8; {command}");
            let mut c = std::process::Command::new("powershell");
            c.args(["-NoProfile", "-Command", full.as_str()]);
            c
        } else {
            // Windows：命令可能含嵌套引号（如 `start "" /min cmd /c "…"`），直接 `cmd /C "…"`
            // 会被 Rust 的 argv 转义成 `\"`，而 cmd 不认反斜杠转义，导致「xxx 不是内部或外部命令」。
            // 改为写入临时批处理文件再执行，彻底绕开引号二次解析；chcp 单独一行保证中文输出不乱码。
            static SERIAL: AtomicU64 = AtomicU64::new(0);
            let id = SERIAL.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("kada_cmd_{}_{}.bat", std::process::id(), id));
            let content = format!("@echo off\r\nchcp 65001>nul\r\n{command}\r\n");
            std::fs::write(&path, content.as_bytes())
                .map_err(|e| format!("写入临时批处理失败: {e}"))?;
            let mut c = std::process::Command::new("cmd");
            c.arg("/C").arg(&path);
            temp_bat = Some(path);
            c
        }
    } else {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", command]);
        c
    };
    let out = run_with_limits(&mut cmd, label, ctx);
    if let Some(path) = temp_bat {
        let _ = std::fs::remove_file(path);
    }
    out
}

/// 执行脚本文件：可选解释器（`None` = 直接执行脚本本身），注入环境变量
/// `KADA_TRIGGER`（触发组合）、`KADA_NAME`（快捷键名）、`KADA_VARS`（变量表 JSON）。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn run_script(
    interpreter: Option<&str>,
    path: &str,
    ctx: &Ctx,
    vars: &Vars,
) -> Result<CmdOutput, String> {
    let mut cmd = match interpreter {
        Some(i) => {
            let mut c = std::process::Command::new(i);
            c.arg(path);
            c
        }
        None => std::process::Command::new(path),
    };
    cmd.env("KADA_TRIGGER", ctx.t.trigger)
        .env("KADA_NAME", ctx.t.name)
        .env("KADA_VARS", vars_to_json(vars));
    run_with_limits(&mut cmd, "脚本", ctx)
}

/// 把变量表序列化为 JSON 对象字符串（供脚本环境变量 `KADA_VARS` 使用）。
#[cfg(all(any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
fn vars_to_json(vars: &Vars) -> String {
    let mut map = serde_json::Map::new();
    for (name, value) in vars {
        let v = match value {
            Value::File(f) => serde_json::json!({
                "name": f.name,
                "path": f.path,
                "dir": f.dir,
                "stem": f.stem,
                "ext": f.ext,
                "size": f.size,
                "modified": f.modified,
                "is_dir": f.is_dir,
            }),
            Value::Bool(b) => serde_json::json!(b),
            Value::Text(t) => serde_json::json!({ "text": t.text, "exit_code": t.exit_code }),
        };
        map.insert(name.clone(), v);
    }
    serde_json::Value::Object(map).to_string()
}

#[cfg(all(test, any(target_os = "windows", target_os = "linux", target_os = "macos"), feature = "automation"))]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// 测试用的触发现场（各用例共用同一组固定值，便于断言结果里的触发键 / 名称）。
    fn trigger_ctx() -> TriggerCtx<'static> {
        TriggerCtx { trigger: "Ctrl+Alt+T", name: "测试宏", frontmost: None, device: None }
    }

    fn ctx(timeout_ms: u64) -> Ctx<'static> {
        Ctx {
            t: trigger_ctx(),
            // 0 = 不限时（与配置里的 0 同义），用于只测「中止」的用例。
            timeout: Duration::from_millis(timeout_ms),
            text_mode: TextInjectMode::Clipboard,
            gen: abort::generation(),
        }
    }

    /// 一个必然挂住的子进程（不写任何东西，只睡）。
    /// Windows 上「孙进程」这一层天然存在（cmd / powershell → 脚本本体），正好覆盖
    /// 「只杀直接子进程不够」的场景。
    fn sleeper(secs: u64) -> std::process::Command {
        if cfg!(target_os = "windows") {
            let mut c = std::process::Command::new("powershell");
            c.args(["-NoProfile", "-Command", &format!("Start-Sleep -Seconds {secs}")]);
            c
        } else {
            let mut c = std::process::Command::new("sleep");
            c.arg(secs.to_string());
            c
        }
    }

    #[test]
    fn run_cmd_captures_output_of_normal_command() {
        let _g = abort::test_lock();
        let out = run_cmd("CMD", false, "echo kada", &ctx(10_000)).expect("命令应能执行");
        assert_eq!(out.killed, None, "正常结束不算被打断");
        assert_eq!(out.exit_code, Some(0));
        assert!(out.stdout.contains("kada"), "stdout 应捕获到输出：{:?}", out.stdout);
    }

    #[test]
    fn timeout_kills_hung_process_and_reports_reason() {
        let _g = abort::test_lock();
        let start = Instant::now();
        let out =
            run_with_limits(&mut sleeper(30), "测试", &ctx(800)).expect("进程应能启动");
        assert!(
            matches!(&out.killed, Some(r) if r.contains("超时")),
            "超时必须标记为被强制终止且说明原因：{:?}",
            out.killed
        );
        assert!(start.elapsed() < Duration::from_secs(8), "超时后应立即返回，不再等进程自然结束");
        assert_eq!(out.exit_code, None, "被强杀的进程退出码没有语义，置空由 stderr 说明");
        let note = out.into_result("cmd", "CMD", "sleep".into(), &ctx(800), false);
        assert!(note.stderr.contains("强制终止"), "原因要写进 stderr 才看得到：{}", note.stderr);
    }

    #[test]
    fn timeout_kills_whole_process_tree() {
        // 只杀直接子进程的话，`cmd`/`sh` 下面的脚本还活着，醒来照样建标记文件。
        let _g = abort::test_lock();
        let marker = std::env::temp_dir().join(format!("kada_tree_{}.txt", std::process::id()));
        let _ = fs::remove_file(&marker);
        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = std::process::Command::new("powershell");
            c.args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Start-Sleep -Seconds 3; New-Item -Path '{}' -ItemType File | Out-Null",
                    marker.display()
                ),
            ]);
            c
        } else {
            // 标记必须由**孙进程**来建：直接子进程（外层 `sh`）一死就没人 `touch` 了，
            // 那样「没建出文件」区分不了「树杀干净了」还是「只杀了 `sh`」——见 7.1-㊴。
            let mut c = std::process::Command::new("sh");
            c.args(["-c", &format!("sh -c 'sleep 3; touch \"{}\"' & wait", marker.display())]);
            c
        };
        let out = run_with_limits(&mut cmd, "测试", &ctx(1_200)).expect("进程应能启动");
        assert!(out.killed.is_some(), "1.2 秒还没结束就该判超时");
        std::thread::sleep(Duration::from_millis(3_500));
        assert!(!marker.exists(), "超时必须连子进程树一起杀，否则脚本还在后台继续跑");
        let _ = fs::remove_file(&marker);
    }

    #[test]
    fn abort_kills_running_process() {
        let _g = abort::test_lock();
        let gen = abort::generation();
        let requester = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            abort::request();
        });
        let start = Instant::now();
        // 不限时（0）：能结束只可能是因为中止生效。
        let mut cmd = sleeper(30);
        let out = run_with_limits(&mut cmd, "测试", &ctx(0)).expect("进程应能启动");
        requester.join().unwrap();
        assert_eq!(out.killed.as_deref(), Some("已中止"));
        assert!(start.elapsed() < Duration::from_secs(8), "中止后应立即返回");
        assert!(gen < abort::generation(), "中止会推进代数");
    }

    #[test]
    fn abort_stops_remaining_actions_and_reports() {
        let _g = abort::test_lock();
        let actions = vec![
            Action::PauseMs { ms: 5_000, description: None },
            Action::PauseMs { ms: 5_000, description: None },
        ];
        let requester = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            abort::request();
        });
        let mut results: Vec<CommandResult> = Vec::new();
        let start = Instant::now();
        {
            let mut commit = |r: CommandResult| results.push(r);
            let mut vars: Vars = BTreeMap::new();
            let mut last_copied = None;
            run_actions(
                &mut commit,
                &actions,
                trigger_ctx(),
                &mut vars,
                &mut last_copied,
                &RunOptions { action_timeout: Duration::ZERO, ..Default::default() },
            );
        }
        requester.join().unwrap();
        assert!(start.elapsed() < Duration::from_secs(5), "中止要立刻生效，不能等 PauseMs 睡完");
        assert_eq!(results.len(), 1, "只记一条中止结果，别每步都记");
        assert_eq!(results[0].kind, "abort");
        assert_eq!(results[0].name, "测试宏");
        assert_eq!(abort::running(), 0, "执行结束后计数器要归零");
    }

    #[test]
    fn abort_requested_before_run_does_not_stop_it() {
        // 代数语义：上次的中止不能波及之后新开始的执行（否则点一次停止会「毒死」下一次触发）。
        let _g = abort::test_lock();
        abort::request();
        let mut results: Vec<CommandResult> = Vec::new();
        {
            let mut commit = |r: CommandResult| results.push(r);
            let mut vars: Vars = BTreeMap::new();
            let mut last_copied = None;
            let actions = vec![Action::PauseMs { ms: 10, description: None }];
            run_actions(
                &mut commit,
                &actions,
                trigger_ctx(),
                &mut vars,
                &mut last_copied,
                &RunOptions::default(),
            );
        }
        assert!(results.is_empty(), "新一轮执行不该被上一次的中止影响");
    }

    /// 跑一串动作并收集提交的结果（失败上报用例共用；调用方负责先拿 [`abort::test_lock`]）。
    fn run_collect(actions: &[Action], vars: &mut Vars) -> Vec<CommandResult> {
        let mut results: Vec<CommandResult> = Vec::new();
        {
            let mut commit = |r: CommandResult| results.push(r);
            let mut last_copied = None;
            run_actions(
                &mut commit,
                actions,
                trigger_ctx(),
                vars,
                &mut last_copied,
                &RunOptions::default(),
            );
        }
        results
    }

    /// 一个必然失败的删除动作（路径不存在）。
    fn failing_delete(description: Option<&str>) -> Action {
        Action::Os {
            operation: OsOperation::Delete { path: "D:\\kada_missing_dir\\nope.txt".into() },
            description: description.map(str::to_string),
        }
    }

    #[test]
    fn failed_action_is_reported_with_type_and_summary() {
        let _g = abort::test_lock();
        let actions = vec![failing_delete(Some("清理临时文件"))];
        let mut vars: Vars = BTreeMap::new();
        let results = run_collect(&actions, &mut vars);

        assert_eq!(results.len(), 1, "失败必须产出一条记录（此前只有控制台日志，用户看不到）");
        let r = &results[0];
        assert_eq!(r.kind, "error");
        assert_eq!(r.label, "文件操作失败");
        assert_eq!(r.trigger, "Ctrl+Alt+T");
        assert_eq!(r.name, "测试宏");
        assert!(r.stderr.contains("不存在"), "失败原因要写进 stderr：{}", r.stderr);
        assert_eq!(r.exit_code, None, "没跑起来的动作没有退出码");
        assert!(!r.show_output, "没开「显示输出」的动作失败只记消息中心，不弹窗");
        assert!(!r.time.is_empty());
        assert!(r.command.contains("清理临时文件"), "用户写的说明要展示：{}", r.command);
        assert!(r.command.contains("删除"), "还要看得出是哪一步：{}", r.command);
    }

    #[test]
    fn failed_action_does_not_stop_remaining_actions() {
        let _g = abort::test_lock();
        let actions = vec![
            Action::Keys { keys: vec!["NotAKey".into()], description: None },
            failing_delete(None),
        ];
        let mut vars: Vars = BTreeMap::new();
        let results = run_collect(&actions, &mut vars);

        assert_eq!(results.len(), 2, "一步失败不该吞掉后面的动作");
        assert_eq!(results[0].label, "按键注入失败");
        assert!(results[0].stderr.contains("NotAKey"), "要点出坏在哪个键名：{}", results[0].stderr);
        assert!(results[0].command.contains("NotAKey"), "摘要要有键名：{}", results[0].command);
        assert_eq!(results[1].label, "文件操作失败");
    }

    #[test]
    fn failed_command_start_is_reported_and_honors_show_output() {
        let _g = abort::test_lock();
        // 指向一个不存在的脚本文件：进程根本起不来，走的是 `Err` 而不是「非零退出码」。
        let missing =
            std::env::temp_dir().join("kada_missing_script_never_exists").display().to_string();
        let actions = vec![Action::Script {
            path: missing,
            interpreter: None,
            show_output: true,
            var: String::new(),
            description: None,
        }];
        let mut vars: Vars = BTreeMap::new();
        let results = run_collect(&actions, &mut vars);

        assert_eq!(results.len(), 1, "命令/脚本起不来同样要上报（此前只在控制台留一行）");
        assert_eq!(results[0].label, "脚本失败");
        assert!(results[0].stderr.contains("失败"), "错误原因进 stderr：{}", results[0].stderr);
        assert!(results[0].show_output, "动作开了「显示输出」，连启动失败也该弹出来");
    }

    #[test]
    fn failure_summary_substitutes_variables() {
        let _g = abort::test_lock();
        let missing = "D:\\kada_missing_dir\\nope.txt".to_string();
        let mut vars: Vars = BTreeMap::new();
        vars.insert(
            "target".into(),
            Value::Text(TextValue { text: missing.clone(), exit_code: None }),
        );
        let actions = vec![Action::Os {
            operation: OsOperation::Delete { path: "{target}".into() },
            description: None,
        }];
        let results = run_collect(&actions, &mut vars);

        assert_eq!(results.len(), 1);
        assert!(
            results[0].command.contains("nope.txt"),
            "摘要要展示本次实际生效的取值：{}",
            results[0].command
        );
        assert!(
            !results[0].command.contains("{target}"),
            "占位符不该原样留在给用户看的摘要里：{}",
            results[0].command
        );
    }

    #[test]
    fn long_failure_summary_is_truncated_to_one_line() {
        let _g = abort::test_lock();
        let actions = vec![Action::Text {
            text: format!("第一行\n{}", "长".repeat(500)),
            mode: TextMode::Input,
            description: None,
        }];
        // 文本注入通常成功，这里只验摘要渲染本身：换行压平 + 截断。
        let summary = super::action_summary(&actions[0], &BTreeMap::new());
        assert!(!summary.contains('\n'), "摘要要压成单行：{summary}");
        assert!(summary.contains('…'), "超长文本要截断：{summary}");
        assert!(summary.chars().count() <= PREVIEW_MAX_CHARS + 8, "截断后不该还很长：{summary}");
    }
}
