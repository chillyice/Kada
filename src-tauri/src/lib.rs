#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! 咔哒桌面壳。
//!
//! 配置驱动：启动时加载 JSON 配置 → 把平台钩子接入配置匹配 → 托盘常驻
//! （关窗不退出）。命令层承载 UI 的读写配置。
//!
//! 跨平台要点：事件管道用自有的 [`Ev`]，各平台钩子都把它喂给 [`decide`] /
//! [`Recorder`] 判定。Windows / Linux 已有钩子实现，其它平台（macOS）壳仍
//! 可编译运行（改键/快捷键/录制暂不可用，托盘与配置界面可用）。

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{Emitter, Manager, WindowEvent};

use kada_core::{
    detect_conflicts, matches, Action, Config, Conflict, Key, Modifier, RawEvent, Severity,
    Shortcut,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

/// 平台输入层。
#[cfg(target_os = "windows")]
mod input {
    //! Windows：全局低层键盘钩子（kada-hook）。
    pub use kada_hook::win::{simulate, start, Action as HookAction, HookHandle, KeyEvent};

    pub fn hooks_supported() -> bool {
        true
    }
}

#[cfg(target_os = "linux")]
mod input {
    //! Linux：evdev + uinput 全局钩子（kada-hook），X11 / Wayland 通用。
    pub use kada_hook::linux::{simulate, start, Action as HookAction, HookHandle, KeyEvent};

    pub fn hooks_supported() -> bool {
        true
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod input {
    //! 其它平台（macOS）：输入层待实现（M5）。占位类型保证壳可编译。
    /// 平台事件（无实现时不可构造）。
    pub enum KeyEvent {}

    pub struct HookHandle;

    pub fn hooks_supported() -> bool {
        false
    }
}

/// 跨平台键盘事件：各平台钩子统一喂给决策器/录制器。
#[derive(Clone, Debug)]
enum Ev {
    Down { key: Key, mods: BTreeSet<Modifier>, repeat: bool },
    Up { key: Key },
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn to_ev(ev: &input::KeyEvent) -> Ev {
    match ev {
        input::KeyEvent::Down { key, mods, repeat } => Ev::Down {
            key: *key,
            mods: mods.clone(),
            repeat: *repeat,
        },
        input::KeyEvent::Up { key, .. } => Ev::Up { key: *key },
    }
}

/// 一次命令类动作（CMD / PowerShell）的执行结果，进入「消息中心」。
#[derive(Clone, Serialize)]
struct CommandResult {
    /// 动作种类："cmd" | "powershell"。
    kind: String,
    /// 展示标签："CMD" / "PowerShell"。
    label: String,
    /// 实际执行的命令文本。
    command: String,
    /// 触发它的快捷键（已渲染为 "Ctrl+Alt+C" 形式）。
    trigger: String,
    /// 快捷键名称（无名称时为空字符串）。
    name: String,
    /// 标准输出（UTF-8）。
    stdout: String,
    /// 标准错误（UTF-8）。
    stderr: String,
    /// 进程退出码；进程未能启动时为 None。
    exit_code: Option<i32>,
    /// 是否弹出结果弹窗（仅影响展示，不影响记录）。
    show_output: bool,
}

/// 运行时状态：钩子持有的配置 + 配置落盘路径 + 消息中心。
struct KadaState {
    config: Arc<RwLock<Config>>,
    paused: Arc<AtomicBool>,
    /// 录制器：Some = 正在录制（此时快捷键/改键全暂停）。
    rec: Arc<Mutex<Option<Recorder>>>,
    file: PathBuf,
    _hook: Mutex<Option<input::HookHandle>>,
    /// 消息中心：命令结果（内存态，重启清空）。
    results: Arc<Mutex<Vec<CommandResult>>>,
    /// 是否有未读命令结果（红点）。
    unread: Arc<AtomicBool>,
    /// 托盘图标（用于运行时叠 / 去红点）。
    tray: Mutex<Option<TrayIcon>>,
    /// 托盘基础图标（无红点）。
    tray_base: Option<tauri::image::Image<'static>>,
    /// 托盘带红点图标（未读态）。
    tray_unread: Option<tauri::image::Image<'static>>,
}

/// 单条事件的决定。
enum Outcome {
    /// 命中快捷键：返回其全部动作（按顺序执行）+ 触发键与名称（供消息中心展示）。
    Shortcut { actions: Vec<Action>, trigger: String, name: String },
    /// 命中改键：改发某个键。
    Replace(Key),
    /// 放行。
    Pass,
}

fn decide(ev: &Ev, cfg: &Config) -> Outcome {
    match ev {
        Ev::Down { key, mods, repeat } => {
            // 改键优先于快捷键：先消耗掉原生键，避免改键后键再触发快捷键。
            for r in &cfg.remaps {
                if !r.enabled {
                    continue;
                }
                if let (Ok(from), Ok(to)) = (r.from.parse::<Key>(), r.to.parse::<Key>()) {
                    if from == *key {
                        return Outcome::Replace(to);
                    }
                }
            }
            let raw = RawEvent { key: *key, mods: mods.clone(), pressed: true };
            for s in &cfg.shortcuts {
                if !s.enabled || *repeat {
                    continue;
                }
                for t in &s.triggers {
                    if let Ok(sc) = t.parse::<Shortcut>() {
                        if matches(&raw, &sc) {
                            return Outcome::Shortcut {
                                actions: s.actions.clone(),
                                trigger: t.clone(),
                                name: s.name.clone().unwrap_or_default(),
                            };
                        }
                    }
                }
            }
            Outcome::Pass
        }
        Ev::Up { .. } => Outcome::Pass,
    }
}

/// 执行一串动作（异步跑，避免阻塞钩子回调）；命令类动作的结果进消息中心。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn fire(
    app: tauri::AppHandle,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    actions: Vec<Action>,
    trigger: String,
    name: String,
) {
    show_toast(&app, &name, &trigger);
    std::thread::spawn(move || {
        for action in actions {
            match run_action(&action, &trigger, &name) {
                Ok(Some(result)) => commit_result(&app, &results, &unread, result),
                Ok(None) => {}
                Err(e) => eprintln!("动作执行失败: {e}"),
            }
        }
    });
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn fire(
    _app: tauri::AppHandle,
    _results: Arc<Mutex<Vec<CommandResult>>>,
    _unread: Arc<AtomicBool>,
    _actions: Vec<Action>,
    _trigger: String,
    _name: String,
) {
    // 无钩子即无触发入口，本分支不会运行（macOS 占位）。
}

/// 触发气泡载荷：名称 + 触发组合键。
#[derive(Clone, Serialize)]
struct ToastPayload {
    name: String,
    trigger: String,
}

/// 触发序号：连按多个快捷键时，只有最后一次气泡到点后隐藏。
static TOAST_SERIAL: AtomicU64 = AtomicU64::new(0);

/// 在屏幕右下角弹一个短暂的气泡（常驻 3 秒后自动消失）。
fn show_toast(app: &tauri::AppHandle, name: &str, trigger: &str) {
    let Some(w) = app.get_webview_window("toast") else { return };
    let _ = w.emit(
        "toast-show",
        ToastPayload { name: name.to_string(), trigger: trigger.to_string() },
    );
    let _ = w.set_ignore_cursor_events(true);
    let _ = w.show();
    let serial = TOAST_SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(3000));
        if TOAST_SERIAL.load(Ordering::Relaxed) == serial {
            if let Some(w) = app.get_webview_window("toast") {
                let _ = w.hide();
            }
        }
    });
}

/// 记录一条命令结果：写入消息中心；弹窗开则唤起主窗口，否则亮未读红点。
/// 两种情况都会向主窗口发 `command-result` 事件。
fn commit_result(
    app: &tauri::AppHandle,
    results: &Mutex<Vec<CommandResult>>,
    unread: &AtomicBool,
    result: CommandResult,
) {
    results.lock().unwrap().push(result.clone());
    if result.show_output {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.show();
            let _ = w.set_focus();
        }
    } else {
        unread.store(true, Ordering::Relaxed);
        set_tray_unread(app, true);
    }
    let _ = app.emit("command-result", &result);
}

/// 切换托盘图标的红点（未读态带红点，已读态还原基础图标）。
fn set_tray_unread(app: &tauri::AppHandle, unread: bool) {
    let state = app.state::<KadaState>();
    let tray = state.tray.lock().unwrap();
    if let Some(tray) = tray.as_ref() {
        let icon = if unread { state.tray_unread.clone() } else { state.tray_base.clone() };
        let _ = tray.set_icon(icon);
    }
}

/// 顺序执行一个动作。文本/按键走模拟输入，进程类走 std::process。
/// 命令类动作（CMD/PowerShell）捕获输出并返回 `Some(CommandResult)`，其余返回 `None`。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn run_action(action: &Action, trigger: &str, name: &str) -> Result<Option<CommandResult>, String> {
    match action {
        Action::Text { text } => input::simulate::type_text(text).map_err(|e| e.to_string())?,
        Action::Keys { keys } => {
            let ks: Vec<Key> = keys
                .iter()
                .map(|k| k.parse::<Key>().map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            // 全部按下 → 稍停 → 逆序松开
            for k in &ks {
                input::simulate::down(*k);
            }
            std::thread::sleep(Duration::from_millis(30));
            for k in ks.iter().rev() {
                input::simulate::up(*k);
            }
        }
        Action::PauseMs { ms } => std::thread::sleep(Duration::from_millis(*ms)),
        Action::Cmd { command, show_output } => {
            let (stdout, stderr, exit_code) = run_cmd("CMD", false, command)?;
            return Ok(Some(CommandResult {
                kind: "cmd".into(),
                label: "CMD".into(),
                command: command.clone(),
                trigger: trigger.into(),
                name: name.into(),
                stdout,
                stderr,
                exit_code,
                show_output: *show_output,
            }));
        }
        Action::Powershell { command, show_output } => {
            if !cfg!(target_os = "windows") {
                return Err("PowerShell 动作仅 Windows 可用".into());
            }
            let (stdout, stderr, exit_code) = run_cmd("PowerShell", true, command)?;
            return Ok(Some(CommandResult {
                kind: "powershell".into(),
                label: "PowerShell".into(),
                command: command.clone(),
                trigger: trigger.into(),
                name: name.into(),
                stdout,
                stderr,
                exit_code,
                show_output: *show_output,
            }));
        }
        Action::Launch { program, args } => {
            std::process::Command::new(program)
                .args(args)
                .spawn()
                .map_err(|e| format!("启动程序「{program}」失败: {e}"))?;
        }
        Action::CloseProgram { program } => {
            let cmd = if cfg!(target_os = "windows") {
                format!("taskkill /IM {program} /F /T")
            } else {
                format!("pkill -f {program}")
            };
            let (stdout, stderr, exit_code) = run_cmd("关闭程序", false, &cmd)?;
            return Ok(Some(CommandResult {
                kind: "close_program".into(),
                label: "关闭程序".into(),
                command: cmd,
                trigger: trigger.into(),
                name: name.into(),
                stdout,
                stderr,
                exit_code,
                show_output: false,
            }));
        }
        Action::OpenFolder { path } => {
            if cfg!(target_os = "windows") {
                std::process::Command::new("explorer")
                    .arg(path)
                    .spawn()
                    .map_err(|e| format!("打开目录失败: {e}"))?;
            } else {
                std::process::Command::new("xdg-open")
                    .arg(path)
                    .spawn()
                    .map_err(|e| format!("打开目录失败: {e}"))?;
            }
        }
    }
    Ok(None)
}

/// 执行 shell 命令并捕获 UTF-8 输出。
/// Windows 下 cmd 前缀 `chcp 65001`、PowerShell 前缀设置输出编码，保证中文不乱码；
/// Linux 下 CMD 走 `sh -c`。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn run_cmd(label: &str, is_powershell: bool, command: &str) -> Result<(String, String, Option<i32>), String> {
    let output = if cfg!(target_os = "windows") {
        if is_powershell {
            let full = format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8; {command}");
            std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", full.as_str()])
                .output()
        } else {
            let full = format!("chcp 65001>nul & {command}");
            std::process::Command::new("cmd")
                .args(["/C", full.as_str()])
                .output()
        }
    } else {
        std::process::Command::new("sh").args(["-c", command]).output()
    }
    .map_err(|e| format!("执行 {label} 命令失败: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((stdout, stderr, output.status.code()))
}

/// 在图标右上角叠加一个红色圆点（未读标记），返回新图像。
fn with_red_dot(icon: &tauri::image::Image<'_>) -> tauri::image::Image<'static> {
    let w = icon.width() as usize;
    let h = icon.height() as usize;
    let mut rgba = icon.rgba().to_vec();
    let r = ((w.min(h) as f32) * 0.20).max(1.0) as usize;
    let cx = w.saturating_sub(r + 1);
    let cy = r + 1;
    for y in 0..h {
        for x in 0..w {
            let dx = x as isize - cx as isize;
            let dy = y as isize - cy as isize;
            if dx * dx + dy * dy <= (r as isize) * (r as isize) {
                let i = (y * w + x) * 4;
                rgba[i] = 0xe0; // R
                rgba[i + 1] = 0x50; // G
                rgba[i + 2] = 0x40; // B
                rgba[i + 3] = 0xff; // A
            }
        }
    }
    tauri::image::Image::new_owned(rgba, icon.width(), icon.height())
}

/// 宏录制器：把全局键盘事件转录成动作列表 [`Action`]。
/// 同一次按下的多个键合并成一条 `Keys`（组合键），事件间 ≥20ms 间隔记 `PauseMs`。
struct Recorder {
    started: std::time::Instant,
    last_ms: u64,
    held: Vec<Key>,
    chord: Vec<Key>,
    actions: Vec<Action>,
}

impl Recorder {
    fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            last_ms: 0,
            held: vec![],
            chord: vec![],
            actions: vec![],
        }
    }

    fn now(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// 事件 → 动作；同一次按下的键并成一条组合键，间隙 ≥20ms 记延迟。
    fn push(&mut self, ev: &Ev) {
        let (key, down) = match ev {
            Ev::Down { key, repeat, .. } if !*repeat => (*key, true),
            Ev::Up { key, .. } => (*key, false),
            _ => return, // 重复 down 是按住，折叠
        };
        let ms = self.now();
        let gap = ms.saturating_sub(self.last_ms);
        self.last_ms = ms;
        if down {
            // 一组新组合键的开始（之前无按住的键）：先记下间隔
            if self.held.is_empty() && gap >= 20 {
                self.actions.push(Action::PauseMs { ms: gap });
            }
            self.held.push(key);
            self.chord.push(key);
        } else {
            self.held.retain(|k| *k != key);
            // 组合键全部松开 → 落成一条 Keys 动作
            if self.held.is_empty() {
                let keys: Vec<String> = self.chord.drain(..).map(|k| key_name(k).to_string()).collect();
                if !keys.is_empty() {
                    self.actions.push(Action::Keys { keys });
                }
            }
        }
    }

    /// 结束：仍在按住的键按剩余组合键收尾，返回录制所得动作。
    fn finish(mut self) -> Vec<Action> {
        if !self.chord.is_empty() {
            let keys: Vec<String> = self.chord.drain(..).map(|k| key_name(k).to_string()).collect();
            self.actions.push(Action::Keys { keys });
        }
        self.actions
    }
}

fn key_name(k: Key) -> &'static str {
    kada_core::key_name(k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_transcribes_events() {
        let down = |key: Key| Ev::Down { key, mods: BTreeSet::new(), repeat: false };
        let up = |key: Key| Ev::Up { key };

        let mut r = Recorder::new();
        r.push(&down(Key::K));
        r.push(&up(Key::K));
        r.push(&down(Key::Control));
        r.push(&down(Key::C));
        r.push(&up(Key::C));
        r.push(&up(Key::Control));
        let actions = r.finish();

        // K 单键成一条组合键，Ctrl+C 合并成一条组合键（间隔若 ≥20ms 会有 PauseMs，忽略之）
        let combos: Vec<&Vec<String>> = actions
            .iter()
            .filter_map(|a| match a {
                Action::Keys { keys } => Some(keys),
                _ => None,
            })
            .collect();
        assert!(combos.iter().any(|k| *k == &vec!["K".to_string()]));
        assert!(combos.iter().any(|k| *k == &vec!["Ctrl".to_string(), "C".to_string()]));

        // 重复 down 折叠：按住 Ctrl 只产生一次组合键
        let mut r = Recorder::new();
        r.push(&down(Key::Control));
        r.push(&Ev::Down { key: Key::Control, mods: BTreeSet::new(), repeat: true });
        r.push(&up(Key::Control));
        let ctrl = r
            .finish()
            .iter()
            .filter(|a| matches!(a, Action::Keys { keys } if keys == &vec!["Ctrl".to_string()]))
            .count();
        assert_eq!(ctrl, 1, "重复 down 必须折叠");
    }
}

fn load_config(file: &PathBuf) -> Config {
    match fs::read_to_string(file) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("配置解析失败 {e}，已回退为默认空配置");
                Config::default()
            }
        },
        Err(_) => Config::default(),
    }
}

fn save_config(file: &PathBuf, cfg: &Config) -> Result<(), String> {
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(file, json).map_err(|e| e.to_string())
}

fn validate(cfg: &Config) -> Result<(), String> {
    for s in &cfg.shortcuts {
        for t in &s.triggers {
            t.parse::<Shortcut>().map_err(|e| format!("触发器「{t}」无效：{e}"))?;
        }
        for a in &s.actions {
            a.validate()?;
        }
    }
    for r in &cfg.remaps {
        r.from.parse::<Key>().map_err(|e| format!("改键来源「{}」无效：{e}", r.from))?;
        r.to.parse::<Key>().map_err(|e| format!("改键目标「{}」无效：{e}", r.to))?;
    }
    for c in detect_conflicts(cfg) {
        if c.severity == Severity::Error {
            return Err(c.message);
        }
    }
    Ok(())
}

/// 读取当前配置。
#[tauri::command]
fn get_config(state: tauri::State<'_, KadaState>) -> Config {
    state.config.read().unwrap().clone()
}

/// 校验并保存配置（写入磁盘 + 即时生效，钩子无需重启）。
#[tauri::command]
fn set_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
    config: Config,
) -> Result<(), String> {
    validate(&config)?;
    let autostart = config.settings.autostart;
    save_config(&state.file, &config)?;
    *state.config.write().unwrap() = config;
    sync_autostart(&app, autostart);
    Ok(())
}

/// 把「开机自启」设置同步到系统（Windows 写 HKCU\...\CurrentVersion\Run，无需管理员权限）。
fn sync_autostart(app: &tauri::AppHandle, enabled: bool) {
    let res = if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    };
    if let Err(e) = res {
        eprintln!("同步开机自启失败: {e}");
    }
}

/// 计算给定配置的冲突列表（前端把当前正在编辑的配置传入，实时展示警告）。
#[tauri::command]
fn get_conflicts(config: Config) -> Vec<Conflict> {
    detect_conflicts(&config)
}

/// 导出当前配置到指定路径（JSON）。
#[tauri::command]
fn export_config(state: tauri::State<'_, KadaState>, path: String) -> Result<(), String> {
    let cfg = state.config.read().unwrap().clone();
    let json = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("写入失败：{e}"))
}

/// 从指定路径导入配置（解析 + 校验 + 硬冲突检查，通过后替换并落盘）。
#[tauri::command]
fn import_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
    path: String,
) -> Result<(), String> {
    let s = std::fs::read_to_string(&path).map_err(|e| format!("读取失败：{e}"))?;
    let config: Config = serde_json::from_str(&s).map_err(|e| format!("解析失败：{e}"))?;
    validate(&config)?;
    let autostart = config.settings.autostart;
    save_config(&state.file, &config)?;
    *state.config.write().unwrap() = config;
    sync_autostart(&app, autostart);
    Ok(())
}

/// 暂停/恢复快捷键触发：录入组合键时暂停，避免自触发。
#[tauri::command]
fn set_paused(state: tauri::State<'_, KadaState>, paused: bool) {
    state.paused.store(paused, Ordering::Relaxed);
}

/// 开始录制宏：快捷键/改键随即暂停，所有按键进时间线。
#[tauri::command]
fn start_record(state: tauri::State<'_, KadaState>) -> Result<(), String> {
    if !input::hooks_supported() {
        return Err("当前平台暂不支持宏录制".into());
    }
    let mut g = state.rec.lock().unwrap();
    if g.is_some() {
        return Err("已在录制中".into());
    }
    *g = Some(Recorder::new());
    state.paused.store(true, Ordering::Relaxed);
    Ok(())
}

/// 停止录制，返回时间线步骤（可直接作为 Sequence 动作保存）。
#[tauri::command]
fn stop_record(state: tauri::State<'_, KadaState>) -> Result<Vec<Action>, String> {
    if !input::hooks_supported() {
        return Err("当前平台暂不支持宏录制".into());
    }
    let mut g = state.rec.lock().unwrap();
    let Some(rec) = g.take() else {
        return Err("没有正在进行的录制".into());
    };
    state.paused.store(false, Ordering::Relaxed);
    Ok(rec.finish())
}

/// 读取消息中心全部命令结果（最新在前）。
#[tauri::command]
fn get_command_results(state: tauri::State<'_, KadaState>) -> Vec<CommandResult> {
    let mut v = state.results.lock().unwrap().clone();
    v.reverse();
    v
}

/// 当前是否有未读命令结果（红点）。
#[tauri::command]
fn get_unread(state: tauri::State<'_, KadaState>) -> bool {
    state.unread.load(Ordering::Relaxed)
}

/// 标记全部已读（熄灭红点，还原托盘图标）。
#[tauri::command]
fn mark_results_read(app: tauri::AppHandle, state: tauri::State<'_, KadaState>) {
    state.unread.store(false, Ordering::Relaxed);
    set_tray_unread(&app, false);
}

/// 清空消息中心（同时熄灭红点）。
#[tauri::command]
fn clear_command_results(app: tauri::AppHandle, state: tauri::State<'_, KadaState>) {
    state.results.lock().unwrap().clear();
    state.unread.store(false, Ordering::Relaxed);
    set_tray_unread(&app, false);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_config,
            set_config,
            get_conflicts,
            export_config,
            import_config,
            set_paused,
            start_record,
            stop_record,
            get_command_results,
            get_unread,
            mark_results_read,
            clear_command_results
        ])
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let file = dir.join("config.json");

            let config = Arc::new(RwLock::new(load_config(&file)));
            let paused = Arc::new(AtomicBool::new(false));
            let rec = Arc::new(Mutex::new(None::<Recorder>));
            let results: Arc<Mutex<Vec<CommandResult>>> = Arc::new(Mutex::new(Vec::new()));
            let unread = Arc::new(AtomicBool::new(false));

            // 托盘图标：基础 + 带红点（未读态）。转为 owned 以存入 state。
            let base_icon: Option<tauri::image::Image<'static>> = app
                .default_window_icon()
                .map(|img| tauri::image::Image::new_owned(img.rgba().to_vec(), img.width(), img.height()));
            let unread_icon = base_icon.as_ref().map(with_red_dot);

            // 钩子接线：事件即时查表；（Windows / Linux 有实现）
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            let hook_handle: Option<input::HookHandle> = Some(
                {
                    let cfg = config.clone();
                    let p = paused.clone();
                    let r = rec.clone();
                    let app_handle = app.handle().clone();
                    let results = results.clone();
                    let unread = unread.clone();
                    input::start(move |ev: input::KeyEvent| {
                        let ev = to_ev(&ev);
                        // 录制中：所有事件进时间线、放行；快捷键/改键全暂停。
                        {
                            let mut g = r.lock().unwrap();
                            if let Some(recorder) = g.as_mut() {
                                recorder.push(&ev);
                                return input::HookAction::Allow;
                            }
                        }
                        let guard = cfg.read().unwrap();
                        if p.load(Ordering::Relaxed) || guard.settings.paused {
                            return input::HookAction::Allow;
                        }
                        match decide(&ev, &guard) {
                            Outcome::Shortcut { actions, trigger, name } => {
                                fire(
                                    app_handle.clone(),
                                    results.clone(),
                                    unread.clone(),
                                    actions,
                                    trigger,
                                    name,
                                );
                                input::HookAction::Block
                            }
                            Outcome::Replace(to) => input::HookAction::Replace(to),
                            Outcome::Pass => input::HookAction::Allow,
                        }
                    })
                }
                .map_err(|e: std::io::Error| e.to_string())?,
            );

            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            let hook_handle: Option<input::HookHandle> = None;

            let state = KadaState {
                config,
                paused,
                rec,
                file,
                _hook: Mutex::new(hook_handle),
                results,
                unread,
                tray: Mutex::new(None),
                tray_base: base_icon.clone(),
                tray_unread: unread_icon,
            };
            app.manage(state);

            // 启动后最小化到托盘：窗口默认隐藏（tauri.conf.json visible=false），
            // 未开启「启动最小化」时才显示主窗口。
            let launch_minimized = app
                .state::<KadaState>()
                .config
                .read()
                .map(|c| c.settings.launch_minimized)
                .unwrap_or(false);
            if !launch_minimized {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }

            // 触发气泡窗口：右下角小浮层，无边框置顶、不抢焦点，隐藏态常驻。
            let toast = {
                let tw = tauri::WebviewWindowBuilder::new(
                    app,
                    "toast",
                    tauri::WebviewUrl::App("index.html#toast".into()),
                )
                .decorations(false)
                .transparent(true)
                .skip_taskbar(true)
                .always_on_top(true)
                .resizable(false)
                .focused(false)
                .inner_size(300.0, 60.0)
                .visible(false)
                .build()?;
                tw.set_ignore_cursor_events(true)
                    .map_err(|e| format!("toast 窗口穿透失败: {e}"))?;
                if let Ok(Some(m)) = app.primary_monitor() {
                    use tauri::LogicalPosition;
                    let scale = m.scale_factor();
                    let work = m.work_area();
                    let x = (work.position.x as f64 + work.size.width as f64 - 300.0 - 16.0) / scale;
                    let y = (work.position.y as f64 + work.size.height as f64 - 60.0 - 16.0) / scale;
                    let _ = tw.set_position(LogicalPosition::new(x, y));
                }
                tw
            };
            let _ = toast;

            // 托盘：常驻后台，关窗不退出。
            let show_i = MenuItem::with_id(app, "show", "打开咔哒", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;
            let tray_icon = TrayIconBuilder::new()
                .icon(base_icon.unwrap())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            *app.state::<KadaState>().tray.lock().unwrap() = Some(tray_icon);

            Ok(())
        })
        .on_window_event(|window, event| {
            // 关窗 = 隐藏到托盘，应用继续跑。
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Kada");
}