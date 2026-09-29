#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! 咔哒桌面壳。
//!
//! 配置驱动：启动时加载 JSON 配置 → 把平台钩子接入配置匹配 → 托盘常驻
//! （关窗不退出）。命令层承载 UI 的读写配置。
//!
//! 跨平台要点：事件管道用自有的 [`Ev`]，各平台钩子都把它喂给 [`decide`] /
//! [`Recorder`] 判定。Windows / Linux 已有钩子实现，其它平台（macOS）壳仍
//! 可编译运行（改键/快捷键/录制暂不可用，托盘与配置界面可用）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WindowEvent};

use kada_core::{
    detect_conflicts, matches, sanitize_config, Action, Config, Conflict, Key, Modifier, RawEvent,
    Shortcut, Severity, Vars, SYSTEM_SHORTCUTS,
};
use kada_actions::{run_actions, CommandResult};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

/// 输入决策引擎（tap-hold/层/和弦/键序列/热串的状态机），与 Tauri 解耦、可单测。
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod engine;

/// 软件更新（Tauri updater + GitHub Releases + Ed25519 签名），与 Tauri 壳解耦、可单测。
mod update;

/// 配置读写（原子写 + 损坏自愈），与 Tauri 壳解耦、可单测。
mod config_io;

#[cfg(any(target_os = "windows", target_os = "linux"))]
use engine::{Engine, HotstringHit, Inject};

/// 把平台注入（SendInput / uinput）接到引擎的 [`Inject`] 通道。
#[cfg(any(target_os = "windows", target_os = "linux"))]
struct SimulatedInject;

#[cfg(any(target_os = "windows", target_os = "linux"))]
impl Inject for SimulatedInject {
    fn down(&mut self, key: Key) {
        input::simulate::down(key);
    }

    fn up(&mut self, key: Key) {
        input::simulate::up(key);
    }
}

/// 平台输入层。
#[cfg(target_os = "windows")]
mod input {
    //! Windows：全局低层键盘钩子（kada-hook）。
    pub use kada_hook::win::{
        frontmost_context, hotkey_occupied, simulate, start, Action as HookAction, HookHandle,
        KeyEvent,
    };

    pub fn hooks_supported() -> bool {
        true
    }
}

#[cfg(target_os = "linux")]
mod input {
    //! Linux：evdev + uinput 全局钩子（kada-hook），X11 / Wayland 通用。
    pub use kada_hook::linux::{frontmost_context, simulate, start, Action as HookAction, HookHandle, KeyEvent};

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

/// 触发气泡载荷：名称 + 触发组合键。
#[derive(Clone, Serialize)]
struct ToastPayload {
    name: String,
    trigger: String,
    /// 自定义副标题：`Some` 时前端直接显示它（「发现新版本」等非执行中气泡用）；
    /// 缺省时前端按「触发键 · 正在执行…」渲染。
    #[serde(skip_serializing_if = "Option::is_none")]
    subtitle: Option<String>,
}

/// 运行时状态：钩子持有的配置 + 配置落盘路径 + 消息中心。
struct KadaState {
    config: Arc<RwLock<Config>>,
    /// 录入捕获暂停的截止时刻（进程启动后的毫秒数；0 = 未暂停）。见 [`pause_active`]。
    paused_until: Arc<AtomicU64>,
    /// 录制器：Some = 正在录制（此时快捷键/改键全暂停）。
    rec: Arc<Mutex<Option<Recorder>>>,
    file: PathBuf,
    _hook: Mutex<Option<input::HookHandle>>,
    /// 消息中心：命令结果（内存态，重启清空）。
    results: Arc<Mutex<Vec<CommandResult>>>,
    /// 是否有未读命令结果（红点）。
    unread: Arc<AtomicBool>,
    /// 最近一次触发气泡的载荷（懒创建 toast 窗口时，供前端加载后兜底读取）。
    toast: Arc<Mutex<Option<ToastPayload>>>,
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

/// 暂停租约时长（毫秒）：录入捕获最多暂停这么久，超时自动恢复。
///
/// 暂停本来是「录入组合键/序列/和弦期间屏蔽触发，避免自己触发自己」，但前端若被中断
/// （切走窗口、关掉详情、窗口收进托盘）可能来不及解除，布尔量会永久卡在暂停态——表现
/// 为整个应用静默失效（快捷键、改键、文本扩展全不响应），用户完全看不出原因。
/// 改成租约：到期自动失效，前端正常解除仍即时生效。
const PAUSE_LEASE_MS: u64 = 60_000;

/// 状态机定时推进间隔（毫秒）。键序列超时、连击等待窗这类「只能靠时间判定」的等待态，
/// 必须由定时器驱动落地——只靠「下一个事件」懒判定的后果是：单独按一下序列 leader 键
/// （之后不按别的键）回放永远不发生，用户看到的是「这个键按了没反应」。见 [`Engine::tick`]。
#[cfg(any(target_os = "windows", target_os = "linux"))]
const ENGINE_TICK_MS: u64 = 60;

/// 进程启动时刻（暂停租约的时间基准）。
static PROCESS_START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);

/// 进程启动至今的毫秒数。
fn now_ms() -> u64 {
    PROCESS_START.elapsed().as_millis() as u64
}

/// 暂停租约是否生效中。
fn pause_active(until: &AtomicU64) -> bool {
    let t = until.load(Ordering::Relaxed);
    t != 0 && now_ms() < t
}

/// 设置暂停租约（`true` = 暂停 [`PAUSE_LEASE_MS`]，`false` = 立即解除）。
fn set_pause_lease(until: &AtomicU64, paused: bool) {
    let deadline = if paused { now_ms() + PAUSE_LEASE_MS } else { 0 };
    until.store(deadline, Ordering::Relaxed);
}

/// 层语义匹配普通改键（非 tap-hold）：激活层条目优先、基础层条目兜底。
fn match_plain_remap(cfg: &Config, key: Key, active_layer: Option<&str>) -> Option<Key> {
    for r in &cfg.remaps {
        if !r.enabled || r.needs_timing_state() || r.layer.as_deref() != active_layer {
            continue;
        }
        if let (Ok(from), Ok(to)) = (r.from.parse::<Key>(), r.to.parse::<Key>()) {
            if from == key {
                return Some(to);
            }
        }
    }
    if active_layer.is_some() {
        for r in &cfg.remaps {
            if !r.enabled || r.needs_timing_state() || r.layer.is_some() {
                continue;
            }
            if let (Ok(from), Ok(to)) = (r.from.parse::<Key>(), r.to.parse::<Key>()) {
                if from == key {
                    return Some(to);
                }
            }
        }
    }
    None
}

/// 层语义匹配快捷键：激活层条目优先、基础层条目兜底。返回 (动作, 触发键, 名称)。
fn match_shortcut(
    cfg: &Config,
    raw: &RawEvent,
    active_layer: Option<&str>,
) -> Option<(Vec<Action>, String, String)> {
    for s in &cfg.shortcuts {
        if !s.enabled || s.layer.as_deref() != active_layer {
            continue;
        }
        for t in &s.triggers {
            if let Ok(sc) = t.parse::<Shortcut>() {
                if matches(raw, &sc) {
                    return Some((s.actions.clone(), t.clone(), s.name.clone().unwrap_or_default()));
                }
            }
        }
    }
    if active_layer.is_some() {
        for s in &cfg.shortcuts {
            if !s.enabled || s.layer.is_some() {
                continue;
            }
            for t in &s.triggers {
                if let Ok(sc) = t.parse::<Shortcut>() {
                    if matches(raw, &sc) {
                        return Some((
                            s.actions.clone(),
                            t.clone(),
                            s.name.clone().unwrap_or_default(),
                        ));
                    }
                }
            }
        }
    }
    None
}

fn decide(ev: &Ev, cfg: &Config, active_layer: Option<&str>) -> Outcome {
    match ev {
        Ev::Down { key, mods, repeat } => {
            // 改键优先于快捷键：先消耗掉原生键，避免改键后键再触发快捷键。
            if let Some(to) = match_plain_remap(cfg, *key, active_layer) {
                return Outcome::Replace(to);
            }
            if !*repeat {
                let raw = RawEvent { key: *key, mods: mods.clone(), pressed: true };
                if let Some((actions, trigger, name)) = match_shortcut(cfg, &raw, active_layer) {
                    return Outcome::Shortcut { actions, trigger, name };
                }
            }
            Outcome::Pass
        }
        Ev::Up { .. } => Outcome::Pass,
    }
}

/// 后台执行一次文本扩展：回删触发词 → 注入替换文本 → 补回后缀键。
///
/// 必须另起线程：注入走剪贴板 + `SendInput`，要上百毫秒，跑在钩子回调里会被系统判超时
/// 摘掉钩子（之后快捷键/改键/文本扩展全部失效）。后缀键在判定命中时已被吞掉，这里补回，
/// 保证「addr␣」展开成「我的地址␣」。注入失败会记进消息中心（没有控制台时不再无声无息）。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn spawn_hotstring(
    app: tauri::AppHandle,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    hit: HotstringHit,
) {
    std::thread::spawn(move || {
        for _ in 0..hit.backspaces {
            input::simulate::tap(Key::Backspace);
        }
        let expanded = resolve_hotstring(&hit.replace);
        if let Err(e) = input::simulate::type_text(&expanded) {
            commit_result(
                &app,
                &results,
                &unread,
                CommandResult {
                    kind: "hotstring".into(),
                    label: "文本扩展".into(),
                    command: hit.trigger.clone(),
                    trigger: hit.trigger.clone(),
                    name: String::new(),
                    stdout: String::new(),
                    stderr: format!("注入替换文本失败：{e}（触发词已被回删）"),
                    exit_code: None,
                    show_output: false,
                    time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                },
            );
            return;
        }
        input::simulate::tap(hit.terminator);
    });
}

/// 展开热串替换文本的动态片段：`{date}` / `{time}` / `{clipboard}`。
/// 未知占位符保持原样（不破坏用户输入的字面 `{...}`）。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn resolve_hotstring(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                let token = &after[..end];
                let val = match token {
                    "date" => Some(chrono::Local::now().format("%Y-%m-%d").to_string()),
                    "time" => Some(chrono::Local::now().format("%H:%M:%S").to_string()),
                    "clipboard" => input::simulate::get_clipboard_text().ok(),
                    _ => None,
                };
                match val {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push('{');
                        out.push_str(token);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 双击唤醒的判定窗口：两次「唤醒键按下」的间隔在此之内才算双击。
const WAKE_DOUBLE_TAP_MS: u64 = 400;

/// 快速唤醒判定：这一击是否构成「双击唤醒键」。
///
/// 被动检测——只报告命中、不拦截按键：唤醒键（UI 里建议选 Alt）常同时是修饰键，拦截会
/// 破坏 Alt+Tab / Alt+字母 等组合。两次按下（非自动重复）间隔 ≤[`WAKE_DOUBLE_TAP_MS`]
/// 判为双击；其间按下其它键则取消上一次单击计数（视作组合键的一部分）。唤醒键由
/// `Settings::wake_key` 指定，**`None` = 关闭快速唤醒**（是 `Settings` 的默认值）。
///
/// **只判定，不建窗口**：调用方命中后必须把唤窗丢到后台线程（见 `run` 里的接线）——
/// 冷启动时建 WebView 要几百毫秒，跑在钩子回调里会被系统判超时摘掉钩子，之后快捷键 /
/// 改键 / 热串全部静默失效。这里不持有 `AppHandle`，顺带也就能单测了。
fn wake_double_tap(cfg: &RwLock<Config>, last_tap: &mut Option<Instant>, ev: &Ev) -> bool {
    let Ev::Down { key, repeat, .. } = ev else { return false };
    if *repeat {
        return false;
    }
    {
        let guard = cfg.read().unwrap();
        let Some(wake) = guard.settings.wake_key.as_ref() else { return false };
        let Ok(wk) = wake.parse::<Key>() else { return false };
        if *key != wk {
            // 按下其它键 → 上一次唤醒键单击不算数（可能是组合键的一部分）。
            last_tap.take();
            return false;
        }
    }
    let now = Instant::now();
    match *last_tap {
        Some(prev) if now.duration_since(prev) <= Duration::from_millis(WAKE_DOUBLE_TAP_MS) => {
            // 计完这一双击就归零：再要唤醒得重新点两下。
            *last_tap = None;
            true
        }
        _ => {
            *last_tap = Some(now);
            false
        }
    }
}

/// 状态机的超时推进线程：每 [`ENGINE_TICK_MS`] 调一次 [`Engine::tick`]，只在「等待态
/// 已过期」时动作（回放被吞的 leader / 中间步、提交连击等待窗），不会注入别的东西。
/// 录制中与暂停中不推进：那段时间事件不进状态机，推进只会把陈旧的等待态回放到用户
/// 正在录入的内容里。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn spawn_engine_ticker(
    engine: Arc<Mutex<Engine>>,
    cfg: Arc<RwLock<Config>>,
    paused_until: Arc<AtomicU64>,
    rec: Arc<Mutex<Option<Recorder>>>,
) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(ENGINE_TICK_MS));
        if rec.lock().unwrap().is_some() || pause_active(&paused_until) {
            continue;
        }
        if cfg.read().unwrap().settings.paused {
            continue;
        }
        engine.lock().unwrap().tick(&mut SimulatedInject);
    });
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
    // 气泡窗口首次懒创建较慢（WebView 冷启动），放后台线程，避免阻塞钩子回调——
    // WH_KEYBOARD_LL 回调超时会被系统摘除，导致后续快捷键/热串/改键全部失效。
    {
        let app = app.clone();
        let name = name.clone();
        let trigger = trigger.clone();
        std::thread::spawn(move || show_toast(&app, &name, &trigger));
    }
    std::thread::spawn(move || {
        // 触发带修饰键的快捷键（如 Ctrl+Alt+T）时修饰键仍物理按住，直接注入会被污染成
        // Ctrl+Alt+<键>（粘贴 Ctrl+V 变成 Ctrl+Alt+V）。等修饰键全部释放后再执行动作，
        // 保证文本/按键能正确落到目标程序。
        input::simulate::wait_modifiers_released(300);
        let mut vars: Vars = BTreeMap::new();
        let mut last_copied: Option<String> = None;
        let frontmost = input::frontmost_context();
        let mut commit = |result: CommandResult| commit_result(&app, &results, &unread, result);
        run_actions(
            &mut commit,
            &actions,
            &trigger,
            &name,
            &mut vars,
            &mut last_copied,
            frontmost.as_ref(),
        );
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

/// 触发序号：连按多个快捷键时，只有最后一次气泡到点后隐藏。
static TOAST_SERIAL: AtomicU64 = AtomicU64::new(0);

/// 按需创建右下角触发气泡窗口：首次触发快捷键时才真正建出 WebView（冷启动零成本）。
fn ensure_toast(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window("toast") {
        return Some(w);
    }
    let w = match tauri::WebviewWindowBuilder::new(
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
    .build()
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("创建气泡窗口失败: {e}");
            return None;
        }
    };
    let _ = w.set_ignore_cursor_events(true);
    if let Ok(Some(m)) = app.primary_monitor() {
        use tauri::LogicalPosition;
        let scale = m.scale_factor();
        let work = m.work_area();
        let x = (work.position.x as f64 + work.size.width as f64 - 300.0 - 16.0) / scale;
        let y = (work.position.y as f64 + work.size.height as f64 - 60.0 - 16.0) / scale;
        let _ = w.set_position(LogicalPosition::new(x, y));
    }
    Some(w)
}

/// 在屏幕右下角弹一个短暂的气泡（常驻 3 秒后自动消失）。
fn show_toast(app: &tauri::AppHandle, name: &str, trigger: &str) {
    let payload =
        ToastPayload { name: name.to_string(), trigger: trigger.to_string(), subtitle: None };
    show_toast_payload(app, payload);
}

/// 弹一条自定义副标题的气泡（不含「正在执行…」，用于「发现新版本」这类通知）。
fn show_toast_note(app: &tauri::AppHandle, title: &str, subtitle: &str) {
    let payload = ToastPayload {
        name: title.to_string(),
        trigger: String::new(),
        subtitle: Some(subtitle.to_string()),
    };
    show_toast_payload(app, payload);
}

fn show_toast_payload(app: &tauri::AppHandle, payload: ToastPayload) {
    // 先把载荷写进状态：toast 窗口若是首次懒创建，前端加载后据此兜底渲染，
    // 避免「事件早于监听器注册」导致第一次触发无内容。
    app.state::<KadaState>().toast.lock().unwrap().replace(payload.clone());
    let Some(w) = ensure_toast(app) else { return };
    let _ = w.emit("toast-show", payload);
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

/// 按需创建/显示主窗口：首次打开时才真正建出 WebView，冷启动不加载任何界面。
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    match tauri::WebviewWindowBuilder::new(
        app,
        "main",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("咔哒 Kada")
    .inner_size(860.0, 560.0)
    .resizable(true)
    .center()
    .build()
    {
        Ok(w) => {
            let _ = w.show();
            let _ = w.set_focus();
        }
        Err(e) => {
            // 唤起现在跑在后台线程（见 `wake_double_tap` 的接线），冷启动期间连点两次双击
            // 可能两个线程都走到这里：主线程是串行处理的，后到的那次会撞上「窗口已存在」。
            // 窗口其实已经建好了，补一次显示即可，不必让用户看到一句吓人的创建失败。
            match app.get_webview_window("main") {
                Some(w) => {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
                None => eprintln!("创建主窗口失败: {e}"),
            }
        }
    }
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
        show_main_window(app);
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

// 动作执行引擎（run_action / run_os / run_app / run_cmd / transform_case 等）
// 已迁出到 `kada-actions` crate；壳层通过 `kada_actions::run_actions` 调用。

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
                self.actions.push(Action::PauseMs { ms: gap, description: None });
            }
            self.held.push(key);
            self.chord.push(key);
        } else {
            self.held.retain(|k| *k != key);
            // 组合键全部松开 → 落成一条 Keys 动作
            if self.held.is_empty() {
                let keys: Vec<String> = self.chord.drain(..).map(|k| key_name(k).to_string()).collect();
                if !keys.is_empty() {
                    self.actions.push(Action::Keys { keys, description: None });
                }
            }
        }
    }

    /// 结束：仍在按住的键按剩余组合键收尾，返回录制所得动作。
    fn finish(mut self) -> Vec<Action> {
        if !self.chord.is_empty() {
            let keys: Vec<String> = self.chord.drain(..).map(|k| key_name(k).to_string()).collect();
            self.actions.push(Action::Keys { keys, description: None });
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

    /// 假注入器：只记录发出的键，绝不真的注入（会打到跑测试的这台机器上）。
    #[derive(Default)]
    struct TestInject {
        log: Vec<String>,
    }

    impl engine::Inject for TestInject {
        fn down(&mut self, key: Key) {
            self.log.push(format!("down {}", key_name(key)));
        }
        fn up(&mut self, key: Key) {
            self.log.push(format!("up {}", key_name(key)));
        }
    }

    fn down(key: Key) -> Ev {
        Ev::Down { key, mods: BTreeSet::new(), repeat: false }
    }

    fn up(key: Key) -> Ev {
        Ev::Up { key }
    }

    /// 走一遍钩子回调的决策链（与 `run()` 里的接线一致）：`engine.step` → `decide`。
    fn decide_via_engine(
        engine: &mut Engine,
        inj: &mut TestInject,
        cfg: &Config,
        ev: &Ev,
    ) -> Option<Outcome> {
        let mut fire = |_a: Vec<Action>, _t: String, _n: String| {};
        let ev = engine.step(ev, cfg, inj, &mut fire)?;
        Some(decide(&ev, cfg, engine.active_layer()))
    }

    /// 同 [`decide_via_engine`]，但把触发的触发键收集下来（和弦/序列在 `step` 里就 fire 了）。
    fn step_collecting_fire(
        engine: &mut Engine,
        inj: &mut TestInject,
        cfg: &Config,
        ev: &Ev,
        fired: &mut Vec<String>,
    ) -> Option<Ev> {
        let mut fire = |_a: Vec<Action>, t: String, _n: String| fired.push(t);
        engine.step(ev, cfg, inj, &mut fire)
    }

    /// 「长按进入层」的切层键（CapsLock → L1）+ 该层内一条快捷键（触发键由参数给出）。
    fn cfg_with_layer(trigger: &str) -> Config {
        Config {
            layers: vec![kada_core::Layer { id: "L1".into(), name: "层1".into() }],
            remaps: vec![kada_core::Remap {
                from: "CapsLock".into(),
                hold_layer: Some("L1".into()),
                enabled: true,
                ..Default::default()
            }],
            shortcuts: vec![kada_core::ShortcutItem {
                layer: Some("L1".into()),
                triggers: vec![trigger.into()],
                actions: vec![Action::Text {
                    text: "x".into(),
                    mode: kada_core::TextMode::Input,
                    description: None,
                }],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn layer_key_short_press_replays_original_key() {
        // 只设了「长按进入层」、没设短按输出：短按必须回放原键，否则这个键就变哑了
        //（用户按一下 CapsLock 什么也不发生）。
        let cfg = cfg_with_layer("K");
        let mut engine = Engine::new();
        let mut inj = TestInject::default();

        assert!(decide_via_engine(&mut engine, &mut inj, &cfg, &down(Key::CapsLock)).is_none());
        decide_via_engine(&mut engine, &mut inj, &cfg, &up(Key::CapsLock));
        assert_eq!(inj.log, vec!["down CapsLock", "up CapsLock"], "短按回放原键");
        assert_eq!(engine.active_layer(), None, "短按不进层");
    }

    #[test]
    fn layer_key_long_press_without_other_key_replays_original_key() {
        // 「一直按住切层键、期间没按别的键就松开」（层从没真正进过）：整个按键必须回放原键，
        // 否则这个键只是按久了一点就彻底没反应——用户看到的正是「长按切层键没反应」。
        let mut cfg = cfg_with_layer("K");
        cfg.remaps[0].tap_timeout_ms = 20;
        let mut engine = Engine::new();
        let mut inj = TestInject::default();

        assert!(decide_via_engine(&mut engine, &mut inj, &cfg, &down(Key::CapsLock)).is_none());
        std::thread::sleep(std::time::Duration::from_millis(30));
        decide_via_engine(&mut engine, &mut inj, &cfg, &up(Key::CapsLock));
        assert_eq!(inj.log, vec!["down CapsLock", "up CapsLock"], "长按无输出时回放原键");
        assert_eq!(engine.active_layer(), None, "没按别的键就不进层");
    }

    #[test]
    fn user_real_config_layer_key_tab_with_chord() {
        // 复刻用户真实配置（2026-09-24 14:25）：`Tab` 长按进「和弦层」，层内一条快捷键带
        // `5&Y`（和弦）与 `Alt+Z`（普通组合）。事件流按真人操作补齐：Tab 按住（含自动重复）
        // → 5 → Y → 松开。和弦与组合都应在按住切层键期间命中。
        const LID: &str = "20d08484-41a4-48ab-90cf-bb8633f779df";
        let cfg = Config {
            layers: vec![kada_core::Layer { id: LID.into(), name: "和弦层".into() }],
            remaps: vec![kada_core::Remap {
                from: "Tab".into(),
                to: String::new(),
                hold_layer: Some(LID.into()),
                tap_timeout_ms: 200,
                enabled: true,
                ..Default::default()
            }],
            shortcuts: vec![kada_core::ShortcutItem {
                layer: Some(LID.into()),
                triggers: vec!["5&Y".into(), "Alt+Z".into()],
                actions: vec![Action::Text {
                    text: "pw".into(),
                    mode: kada_core::TextMode::Input,
                    description: None,
                }],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };

        // ① 和弦：按住 Tab（含自动重复）→ 5 → Y
        let mut engine = Engine::new();
        let mut inj = TestInject::default();
        let mut fired: Vec<String> = Vec::new();
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Tab), &mut fired);
        for _ in 0..5 {
            let repeat = Ev::Down { key: Key::Tab, mods: BTreeSet::new(), repeat: true };
            step_collecting_fire(&mut engine, &mut inj, &cfg, &repeat, &mut fired);
        }
        assert_eq!(engine.active_layer(), None, "还没按别的键，未进层");
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Digit5), &mut fired);
        assert_eq!(engine.active_layer(), Some(LID), "按住 Tab + 按 5 → 进层");
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Y), &mut fired);
        assert_eq!(fired, vec!["5&Y"], "层内和弦应触发");

        // ② 普通组合：松开和弦成员后，按住 Tab 再按 Alt+Z（真人按 Alt 时事件带 mods={Alt}）
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Digit5), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Y), &mut fired);
        fired.clear();
        let alt_down = Ev::Down {
            key: Key::Alt,
            mods: BTreeSet::from([Modifier::Alt]),
            repeat: false,
        };
        step_collecting_fire(&mut engine, &mut inj, &cfg, &alt_down, &mut fired);
        let z_down = Ev::Down {
            key: Key::Z,
            mods: BTreeSet::from([Modifier::Alt]),
            repeat: false,
        };
        let out = step_collecting_fire(&mut engine, &mut inj, &cfg, &z_down, &mut fired);
        let layer = engine.active_layer().map(str::to_string);
        match decide(&out.expect("Alt+Z 应放行进 decide"), &cfg, layer.as_deref()) {
            Outcome::Shortcut { trigger, .. } => assert_eq!(trigger, "Alt+Z"),
            other => panic!(
                "层内 Alt+Z 应命中该层快捷键，实际 {:?}",
                matches!(other, Outcome::Pass)
            ),
        }

        // ③ 层内序列：按住 Tab 期间依次按 F9 J K 应触发
        let mut cfg2 = cfg.clone();
        cfg2.shortcuts[0].triggers = vec!["F9 J K".into()];
        let mut engine2 = Engine::new();
        let mut inj2 = TestInject::default();
        let mut fired2: Vec<String> = Vec::new();
        step_collecting_fire(&mut engine2, &mut inj2, &cfg2, &down(Key::Tab), &mut fired2);
        step_collecting_fire(&mut engine2, &mut inj2, &cfg2, &down(Key::F9), &mut fired2);
        assert_eq!(engine2.active_layer(), Some(LID), "按住 Tab + 按 F9 → 进层");
        step_collecting_fire(&mut engine2, &mut inj2, &cfg2, &down(Key::J), &mut fired2);
        step_collecting_fire(&mut engine2, &mut inj2, &cfg2, &down(Key::K), &mut fired2);
        assert_eq!(fired2, vec!["F9 J K"], "层内序列应触发");
    }

    #[test]
    fn layer_key_released_before_chord_does_not_fire() {
        // 层是 momentary 的（hold_layer）：松开切层键后层立即失效，此时按层内和弦不触发（这是
        // 设计，不是 bug）。「长按切层、松开、再按层内和弦/序列」的用法要用切换式切层
        //（lock_layer，见 locked_layer_* 用例）。
        let cfg = cfg_with_layer("5&Y");
        let mut engine = Engine::new();
        let mut inj = TestInject::default();
        let mut fired: Vec<String> = Vec::new();

        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::CapsLock), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::CapsLock), &mut fired);
        assert_eq!(engine.active_layer(), None, "松开切层键即退层");
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Digit5), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Y), &mut fired);
        assert!(fired.is_empty(), "层未激活时层内和弦不触发，实际：{fired:?}");
    }

    #[test]
    fn locked_layer_key_long_press_then_chord_sequence_combo() {
        // 复刻用户真实配置（2026-09-24）：`Tab` 改成「长按锁定层」（切换式）指向「和弦层」，
        // 层内一条快捷键带 `5&Y`（和弦）+ `F9 J K`（序列）+ `Alt+Z`（普通组合），基础层也有一条
        // `Alt+Z`。用户诉求：**长按切层键之后（松手）** 层内的和弦与序列要能触发——momentary
        // 做不到（层随松手消失，层内和弦还得一直按住切层键），切换式才用得起来。
        const LID: &str = "20d08484-41a4-48ab-90cf-bb8633f779df";
        let cfg = Config {
            layers: vec![kada_core::Layer { id: LID.into(), name: "和弦层".into() }],
            remaps: vec![kada_core::Remap {
                from: "Tab".into(),
                to: String::new(),
                lock_layer: Some(LID.into()),
                tap_timeout_ms: 20, // 真实配置是 200；测试里压短，免得真等
                enabled: true,
                ..Default::default()
            }],
            shortcuts: vec![
                kada_core::ShortcutItem {
                    name: Some("基础层 Alt+Z".into()),
                    triggers: vec!["Alt+Z".into()],
                    actions: vec![],
                    enabled: true,
                    ..Default::default()
                },
                kada_core::ShortcutItem {
                    name: Some("输入密码".into()),
                    layer: Some(LID.into()),
                    triggers: vec!["5&Y".into(), "F9 J K".into(), "Alt+Z".into()],
                    actions: vec![Action::Text {
                        text: "pw".into(),
                        mode: kada_core::TextMode::Input,
                        description: None,
                    }],
                    enabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let mut engine = Engine::new();
        let mut inj = TestInject::default();
        let mut fired: Vec<String> = Vec::new();

        // ① 长按 Tab（期间没按别的键）再松开 → 切进「和弦层」并保持生效。
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Tab), &mut fired);
        std::thread::sleep(std::time::Duration::from_millis(30));
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Tab), &mut fired);
        assert_eq!(engine.active_layer(), Some(LID), "长按切层后层保持生效");
        assert!(inj.log.is_empty(), "切层键不回放");

        // ② 层内和弦：松手后直接按 5 与 Y。
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Digit5), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Y), &mut fired);
        assert_eq!(fired, vec!["5&Y"], "长按切层后层内和弦应触发");

        // ③ 层内序列：F9 J K 依次按下。
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Digit5), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Y), &mut fired);
        fired.clear();
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::F9), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::J), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::K), &mut fired);
        assert_eq!(fired, vec!["F9 J K"], "长按切层后层内序列应触发");

        // ④ 层内普通组合：层条目优先于基础层同名条目。
        fired.clear();
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::F9), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::J), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::K), &mut fired);
        let alt_z = Ev::Down { key: Key::Z, mods: BTreeSet::from([Modifier::Alt]), repeat: false };
        let out = step_collecting_fire(&mut engine, &mut inj, &cfg, &alt_z, &mut fired);
        let layer = engine.active_layer().map(str::to_string);
        match decide(&out.expect("Alt+Z 应放行进 decide"), &cfg, layer.as_deref()) {
            Outcome::Shortcut { name, .. } => assert_eq!(name, "输入密码", "层内 Alt+Z 应优先"),
            Outcome::Replace(_) => panic!("不该走改键"),
            Outcome::Pass => panic!("层内 Alt+Z 未命中"),
        }

        // ⑤ 再长按一次切层键 → 退出锁定层，Alt+Z 回到基础层条目。
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Tab), &mut fired);
        std::thread::sleep(std::time::Duration::from_millis(30));
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Tab), &mut fired);
        assert_eq!(engine.active_layer(), None, "再长按一次退出切层");
        let out = step_collecting_fire(&mut engine, &mut inj, &cfg, &alt_z, &mut fired);
        match decide(&out.expect("Alt+Z 应放行进 decide"), &cfg, None) {
            Outcome::Shortcut { name, .. } => assert_eq!(name, "基础层 Alt+Z"),
            Outcome::Replace(_) => panic!("不该走改键"),
            Outcome::Pass => panic!("基础层 Alt+Z 未命中"),
        }
    }

    #[test]
    fn layer_key_long_press_activates_layer_shortcut() {
        // 长按切层键期间按下 K：K 应按「激活层优先」命中层内快捷键（而不是基础层兜底）。
        let cfg = cfg_with_layer("K");
        let mut engine = Engine::new();
        let mut inj = TestInject::default();

        assert!(decide_via_engine(&mut engine, &mut inj, &cfg, &down(Key::CapsLock)).is_none());
        assert_eq!(engine.active_layer(), None, "还没按别的键，未进层");
        let out = decide_via_engine(&mut engine, &mut inj, &cfg, &down(Key::K)).expect("K 应放行");
        assert_eq!(engine.active_layer(), Some("L1"), "按住切层键 + 按 K → 进层");
        match out {
            Outcome::Shortcut { trigger, .. } => assert_eq!(trigger, "K"),
            _ => panic!("层内 K 应命中该层快捷键"),
        }

        // 松开切层键退回基础层，K 不再命中层条目。
        decide_via_engine(&mut engine, &mut inj, &cfg, &up(Key::K));
        decide_via_engine(&mut engine, &mut inj, &cfg, &up(Key::CapsLock));
        assert_eq!(engine.active_layer(), None, "松开切层键退回基础层");
        let out = decide_via_engine(&mut engine, &mut inj, &cfg, &down(Key::K)).expect("K 应放行");
        assert!(matches!(out, Outcome::Pass), "基础层下 K 不该命中层内快捷键");
    }

    #[test]
    fn layer_chord_fires_while_layer_key_held() {
        // 复刻实际配法：层「和弦层」里放一条和弦快捷键（5&Y），用 CapsLock 长按进层
        //（CapsLock 只设了「长按进入层」）。按住切层键 + 同时按 5 与 Y → 该层和弦应触发。
        let cfg = cfg_with_layer("5&Y");
        let mut fired: Vec<String> = Vec::new();
        let mut inj = TestInject::default();
        let mut engine = Engine::new();

        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::CapsLock), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Digit5), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Y), &mut fired);
        assert_eq!(engine.active_layer(), Some("L1"), "按住切层键即进层");
        assert_eq!(fired, vec!["5&Y"], "层内和弦应触发");

        // 不进层时同一个和弦不该触发（层语义：条目只在该层激活时生效）。
        let mut fired2: Vec<String> = Vec::new();
        let mut inj2 = TestInject::default();
        let mut engine2 = Engine::new();
        step_collecting_fire(&mut engine2, &mut inj2, &cfg, &down(Key::Digit5), &mut fired2);
        step_collecting_fire(&mut engine2, &mut inj2, &cfg, &down(Key::Y), &mut fired2);
        assert!(fired2.is_empty(), "基础层下不应命中层内和弦，实际：{fired2:?}");
    }

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
                Action::Keys { keys, .. } => Some(keys),
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
            .filter(|a| matches!(a, Action::Keys { keys, .. } if keys == &vec!["Ctrl".to_string()]))
            .count();
        assert_eq!(ctrl, 1, "重复 down 必须折叠");
    }

    /// 只有「快速唤醒键」一项不同的配置。
    fn wake_cfg(wake_key: Option<&str>) -> RwLock<Config> {
        let mut cfg = Config::default();
        cfg.settings.wake_key = wake_key.map(str::to_string);
        RwLock::new(cfg)
    }

    #[test]
    fn wake_double_tap_needs_two_taps_in_window() {
        // 判定已与「建窗口」解耦（见 wake_double_tap 的注释），所以这层时序规则能单测。
        let cfg = wake_cfg(Some("Alt"));
        let mut last = None;
        assert!(!wake_double_tap(&cfg, &mut last, &down(Key::Alt)), "第一击只是单击");
        assert!(wake_double_tap(&cfg, &mut last, &down(Key::Alt)), "窗口内的第二击命中");
        assert!(!wake_double_tap(&cfg, &mut last, &down(Key::Alt)), "计完归零，第三击重新算单击");
        assert!(wake_double_tap(&cfg, &mut last, &down(Key::Alt)), "第四击又凑成一次双击");
    }

    #[test]
    fn wake_double_tap_window_expires_and_other_keys_reset() {
        let cfg = wake_cfg(Some("Alt"));

        // 上一次单击已落在判定窗口之外：这一击只算新的单击，不该唤窗。
        let mut last = Some(Instant::now() - Duration::from_millis(WAKE_DOUBLE_TAP_MS + 50));
        assert!(!wake_double_tap(&cfg, &mut last, &down(Key::Alt)), "超出窗口不算双击");
        assert!(last.is_some(), "超时后要重新计时（这一击仍是单击）");

        // 中间按了别的键 → 上一次单击作废（那次多半是 Alt+Tab 这类组合的一部分）。
        let mut last = None;
        assert!(!wake_double_tap(&cfg, &mut last, &down(Key::Alt)));
        assert!(!wake_double_tap(&cfg, &mut last, &down(Key::K)));
        assert!(last.is_none(), "按其它键要清掉单击计数");
        assert!(!wake_double_tap(&cfg, &mut last, &down(Key::Alt)), "清空后这一击是新单击");

        // 自动重复的按下不是独立一击：长按唤醒键不该被当成连击而唤窗。
        let mut last = None;
        let repeated = Ev::Down { key: Key::Alt, mods: BTreeSet::new(), repeat: true };
        assert!(!wake_double_tap(&cfg, &mut last, &repeated), "自动重复不算一击");
        assert!(last.is_none(), "自动重复也不该开始计时");
    }

    #[test]
    fn wake_disabled_or_unparsable_key_never_wakes() {
        // 未设唤醒键 = 快速唤醒关闭：按多少次都不唤窗。
        let off = wake_cfg(None);
        let mut last = None;
        assert!(!wake_double_tap(&off, &mut last, &down(Key::Alt)));
        assert!(!wake_double_tap(&off, &mut last, &down(Key::Alt)));

        // 配置里是解析不了的键名：当作关闭，既不 panic 也不唤窗（手工改配置能改出这种值）。
        let bad = wake_cfg(Some("没这个键"));
        let mut last = None;
        assert!(!wake_double_tap(&bad, &mut last, &down(Key::Alt)));
        assert!(!wake_double_tap(&bad, &mut last, &down(Key::Alt)));
    }
}

/// 读取当前配置。
#[tauri::command]
fn get_config(state: tauri::State<'_, KadaState>) -> Config {
    state.config.read().unwrap().clone()
}

/// 保存配置：先逐条清洗（坏触发键/动作/改键被单独忽略，不影响其余配置），
/// 写入磁盘并即时生效。返回被忽略内容的说明（供 UI 提示），只有真正失败（写盘等）才报错。
/// 落盘走 [`config_io::save`]（原子替换 + 留存上次良好副本），不会留下半截 JSON。
#[tauri::command]
fn set_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
    config: Config,
) -> Result<Vec<String>, String> {
    let (clean, ignored) = sanitize_config(&config);
    let autostart = clean.settings.autostart;
    config_io::save(&state.file, &clean)?;
    *state.config.write().unwrap() = clean;
    sync_autostart(&app, autostart);
    Ok(ignored)
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

/// 本次进程是否由「开机自启」拉起（autostart 插件注册的命令带 `--autostart` 参数）。
/// 用于区分「双击 exe 手动启动」（默认打开主窗口）与「随系统启动」（静默到托盘）。
fn launched_by_autostart() -> bool {
    std::env::args().any(|a| a == "--autostart")
}

/// 计算给定配置的冲突列表（前端把当前正在编辑的配置传入，实时展示警告）。
/// 三部分：① 内部冲突（重复触发键/改键遮蔽/超集重叠）；② 系统快捷键清单命中；
/// ③ Windows 下 RegisterHotKey 探测「其他应用/系统已注册」的真实占用。
#[tauri::command]
fn get_conflicts(config: Config) -> Vec<Conflict> {
    let mut out = detect_conflicts(&config);

    // ② 系统快捷键清单（跨平台）：精确匹配；命中的触发键记下，供③跳过避免重复提示。
    let mut sys_hits: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in &config.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            let Ok(sc) = t.parse::<Shortcut>() else { continue };
            for (combo, desc) in SYSTEM_SHORTCUTS {
                if let Ok(sys) = combo.parse::<Shortcut>() {
                    if sys == sc {
                        out.push(Conflict {
                            severity: Severity::Warn,
                            message: format!("「{t}」与系统快捷键 {combo}（{desc}）冲突"),
                            name: s.name.clone().unwrap_or_default(),
                        });
                        sys_hits.insert(t.clone());
                    }
                }
            }
        }
    }

    // ③ Windows：RegisterHotKey 探测其它应用/系统的真实占用。
    #[cfg(windows)]
    for s in &config.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            if sys_hits.contains(t) {
                continue;
            }
            if let Ok(sc) = t.parse::<Shortcut>() {
                if input::hotkey_occupied(&sc) {
                    out.push(Conflict {
                        severity: Severity::Warn,
                        message: format!("「{t}」已被系统或其他应用占用"),
                        name: s.name.clone().unwrap_or_default(),
                    });
                }
            }
        }
    }

    out
}

/// 导出当前配置到指定路径（JSON）。
#[tauri::command]
fn export_config(state: tauri::State<'_, KadaState>, path: String) -> Result<(), String> {
    let cfg = state.config.read().unwrap().clone();
    let json = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("写入失败：{e}"))
}

/// 从指定路径导入配置（解析 + 逐条清洗，坏条目被忽略，通过后替换并落盘）。
#[tauri::command]
fn import_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
    path: String,
) -> Result<Vec<String>, String> {
    let s = std::fs::read_to_string(&path).map_err(|e| format!("读取失败：{e}"))?;
    let config: Config = serde_json::from_str(&s).map_err(|e| format!("解析失败：{e}"))?;
    let (clean, ignored) = sanitize_config(&config);
    let autostart = clean.settings.autostart;
    config_io::save(&state.file, &clean)?;
    *state.config.write().unwrap() = clean;
    sync_autostart(&app, autostart);
    Ok(ignored)
}

/// 暂停/恢复快捷键触发：录入组合键/序列/和弦时暂停，避免自触发。
/// 暂停是限时租约（[`PAUSE_LEASE_MS`]），前端若被中断来不及解除也不会永久卡死。
#[tauri::command]
fn set_paused(state: tauri::State<'_, KadaState>, paused: bool) {
    set_pause_lease(&state.paused_until, paused);
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
    set_pause_lease(&state.paused_until, true);
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
    set_pause_lease(&state.paused_until, false);
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

/// 读取最近一次触发气泡的载荷（toast 窗口懒创建后，前端加载时调用兜底渲染）。
#[tauri::command]
fn get_toast_payload(state: tauri::State<'_, KadaState>) -> Option<ToastPayload> {
    state.toast.lock().unwrap().clone()
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
            Some(vec!["--autostart"]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
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
            get_toast_payload,
            mark_results_read,
            clear_command_results,
            update::get_update_status,
            update::check_update,
            update::install_update
        ])
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let file = dir.join("config.json");

            // 配置读取走 config_io：原子写 + 损坏自愈。解析失败不静默清空——原文件留档，
            // 能从 .bak 恢复就恢复，经过作为告警在托盘建好后推给消息中心（见文件末尾）。
            let config_io::LoadOutcome { config: loaded, warnings: load_warnings } =
                config_io::load(&file);
            let config = Arc::new(RwLock::new(loaded));
            let paused_until = Arc::new(AtomicU64::new(0));
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
            let engine = Arc::new(Mutex::new(Engine::new()));

            #[cfg(any(target_os = "windows", target_os = "linux"))]
            let hook_handle: Option<input::HookHandle> = Some(
                {
                    let cfg = config.clone();
                    let pause = paused_until.clone();
                    let r = rec.clone();
                    let app_handle = app.handle().clone();
                    let results = results.clone();
                    let unread = unread.clone();
                    let engine = engine.clone();
                    let mut last_tap: Option<Instant> = None;
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
                        // 快速唤醒：双击唤醒键唤出主窗口（被动检测，不拦截按键）。
                        // 判定留在回调里（只读配置 + 比时间，很快），**建窗口丢后台线程**：
                        // 冷启动时 WebView 要几百毫秒，同步建会把钩子回调拖过系统超时线、
                        // 钩子被摘掉，之后整个应用静默失效（开机自启时主窗口还没预建，
                        // 首次双击唤醒正好撞在这条路上）。
                        if wake_double_tap(&cfg, &mut last_tap, &ev) {
                            let app = app_handle.clone();
                            std::thread::spawn(move || show_main_window(&app));
                        }
                        let guard = cfg.read().unwrap();
                        if pause_active(&pause) || guard.settings.paused {
                            return input::HookAction::Allow;
                        }
                        // 前置状态机（tap-hold → 和弦 → 键序列）：被吞掉的键不进 decide。
                        // 状态机没凑成快捷键时会自行回放被吞的键，不会让按键变哑。
                        let mut fire_hit = |actions: Vec<Action>, trigger: String, name: String| {
                            fire(
                                app_handle.clone(),
                                results.clone(),
                                unread.clone(),
                                actions,
                                trigger,
                                name,
                            );
                        };
                        let Some(ev) =
                            engine.lock().unwrap().step(&ev, &guard, &mut SimulatedInject, &mut fire_hit)
                        else {
                            return input::HookAction::Block;
                        };
                        let layer = engine.lock().unwrap().active_layer().map(str::to_string);
                        match decide(&ev, &guard, layer.as_deref()) {
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
                            Outcome::Pass => {
                                // 命中文本扩展：后缀键（空格/回车/Tab）吞掉，改由后台线程
                                // 回删触发词 + 注入替换文本 + 补回后缀（注入要上百毫秒，绝不能
                                // 跑在钩子回调里——回调超时会被系统摘掉钩子，之后全部功能失效）。
                                match engine.lock().unwrap().hotstring(&ev, &guard.expansions) {
                                    Some(hit) => {
                                        spawn_hotstring(
                                            app_handle.clone(),
                                            results.clone(),
                                            unread.clone(),
                                            hit,
                                        );
                                        input::HookAction::Block
                                    }
                                    None => input::HookAction::Allow,
                                }
                            }
                        }
                    })
                }
                .map_err(|e: std::io::Error| e.to_string())?,
            );

            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            let hook_handle: Option<input::HookHandle> = None;

            // 定时推进状态机（键序列超时回放 / 连击等待窗提交，见 spawn_engine_ticker）。
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            spawn_engine_ticker(engine, config.clone(), paused_until.clone(), rec.clone());

            let state = KadaState {
                config,
                paused_until,
                rec,
                file,
                _hook: Mutex::new(hook_handle),
                results,
                unread,
                tray: Mutex::new(None),
                tray_base: base_icon.clone(),
                tray_unread: unread_icon,
                toast: Arc::new(Mutex::new(None)),
            };
            app.manage(state);
            // 更新状态单独托管：设置页/托盘都要读，与配置无关（不进 config.json）。
            app.manage(update::UpdateState::new(app.package_info().version.to_string()));

            // 确保开机自启注册项带 `--autostart` 参数（旧版注册的是裸 exe 路径，无法区分启动来源）。
            let autostart_enabled = app
                .state::<KadaState>()
                .config
                .read()
                .map(|c| c.settings.autostart)
                .unwrap_or(false);
            sync_autostart(app.handle(), autostart_enabled);

            // 主窗口按需创建：双击 exe 手动启动时默认打开页面；开机自启（带 --autostart 参数）时静默到托盘。
            // 气泡窗口则等到第一次触发快捷键时才懒创建（见 ensure_toast）。
            if !launched_by_autostart() {
                show_main_window(app.handle());
            }

            // 托盘：常驻后台，关窗不退出。
            let show_i = MenuItem::with_id(app, "show", "打开咔哒", true, None::<&str>)?;
            let update_i = MenuItem::with_id(app, "update", "检查更新", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &update_i, &quit_i])?;
            let tray_icon = TrayIconBuilder::new()
                .icon(base_icon.unwrap())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "update" => update::spawn(app, update::Mode::Manual),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // 双击托盘图标（左键）唤起主窗口。
                    if let TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } = event {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;
            *app.state::<KadaState>().tray.lock().unwrap() = Some(tray_icon);

            // 配置损坏 / 自愈的告警推给消息中心。放在托盘建好之后：commit_result 会叠托盘
            // 红点，此时托盘已存在才叠得上（应用内「消息」入口的红点由前端拉取 unread 得到）。
            if !load_warnings.is_empty() {
                let st = app.state::<KadaState>();
                let time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                for w in load_warnings {
                    commit_result(
                        app.handle(),
                        &st.results,
                        &st.unread,
                        CommandResult {
                            kind: "config".into(),
                            label: "配置".into(),
                            trigger: "启动加载".into(),
                            name: String::new(),
                            command: w.summary,
                            stdout: String::new(),
                            stderr: w.detail,
                            exit_code: None,
                            show_output: false,
                            time: time.clone(),
                        },
                    );
                }
            }

            // 启动后台检查更新：延迟几秒、仅 release 构建、查不到就静默（见 update::spawn_startup_check）。
            update::spawn_startup_check(app.handle());

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