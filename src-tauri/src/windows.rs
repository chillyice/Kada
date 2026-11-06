// 浮窗（T-03 从 lib.rs 搬出）：触发气泡 / 输入状态悬浮窗 / 「设置变量」输入窗。
// 三条硬约束：透明浮窗 `focusable(false)`、`shadow(false)` 自绘投影、可输入的窗反而不挡激活（见 说明书 §6.10）。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{Emitter, Manager, WindowEvent};

use kada_actions::abort as actions_abort;
use serde::Serialize;

use crate::{input, KadaState, StatusPayload, ToastPayload};

/// 触发提醒开关（`Settings.trigger_toast`）：定时器线程每拍按配置同步，`show_toast` 只读它。
/// 放全局是为了让 `fire` 里那条后台线程不重新取配置锁（热路径）。
pub(crate) static TRIGGER_TOAST: AtomicBool = AtomicBool::new(true);

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
    // 浮窗一律 `shadow(false)`：tao 对「无边框 + 有投影」的窗会保留整套 DWM 边框（Win11 上是
    // 一圈 1px 圆角亮边 + 外投影），而这几个窗的内容都比窗口矩形小一圈（透明边是留给 CSS 投影
    // 的），那圈边框就挂在面板外面，看着像凭空多了一个空框。投影由 CSS 自己画，见 `style.css`
    // 里 `.toast-bubble` / `.hud-row` / `.hints-panel` 的 `box-shadow`。
    .shadow(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .resizable(false)
    .focused(false)
    // 同 `ensure_hud`：`focused(false)` 只保证**第一次**显示不抢焦点（tao 首次用
    // `SW_SHOWNOACTIVATE`，之后清掉这个标记、退回 `SW_SHOW`），而气泡每次触发都 show 一次。
    // 加 `focusable(false)`（`WS_EX_NOACTIVATE`）才真的永不成为前台窗口——否则连按两次快捷键，
    // 第二次气泡就会把焦点从用户正在打字的窗口抢走，`ResetWatch` 还会把它当成「前台切换」
    // 白复位一次输入状态。
    .focusable(false)
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

/// 按需创建屏幕下方的输入状态悬浮窗（见规划 7.3-⑬）：真有状态要显示时才建 WebView。
///
/// **必须 `focusable(false)`**——Windows 上就是 `WS_EX_NOACTIVATE`，否则这个窗能被激活，
/// 后果很具体：它在用户按住切层键期间一直挂在最上层，一旦成了前台窗口，① 前台条件（前台应用 /
/// 窗口标题）全读成我们自己这个窗；② `ResetWatch` 把它当作「前台切换」→ 复位输入状态，
/// 按住的 momentary 层当场被踢掉——正是本功能要消除的那种「按了没反应」。它只是一块看板：
/// 不接受点击、不接焦点。位置与尺寸在 [`hud_ready`] 里按内容定（要贴底部居中，宽高得先量出来）。
fn ensure_hud(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window("hud") {
        return Some(w);
    }
    let w = match tauri::WebviewWindowBuilder::new(
        app,
        "hud",
        tauri::WebviewUrl::App("index.html#hud".into()),
    )
    .decorations(false)
    .transparent(true)
    // 同 `ensure_toast`：别借系统投影，那圈 DWM 边框会挂在面板外面。
    .shadow(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .resizable(false)
    .focused(false)
    .focusable(false)
    .inner_size(240.0, 44.0)
    .visible(false)
    .build()
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("创建状态指示窗失败: {e}");
            return None;
        }
    };
    let _ = w.set_ignore_cursor_events(true);
    Some(w)
}

/// 输入状态可视指示（见规划 7.3-⑬）：状态变化时刷新托盘提示，并按需显示 / 隐藏悬浮指示窗。
///
/// 由状态机定时器线程调用，**必须在配置读锁与 `Engine` 锁之外**：这里要动托盘图标、建窗口、
/// 发事件，而钩子回调正等着那把 `Engine` 锁——在锁里干这些就等于把整个输入层挂在界面上。
/// 重量级的那步（建 WebView）另外挪到自己的线程上，见函数末尾。
pub(crate) fn publish_status(app: &tauri::AppHandle, payload: &StatusPayload, hud_enabled: bool) {
    // 定时器线程比 `app.manage(KadaState)` 先起步（差几十毫秒），用 `try_state` 兜一下：
    // 拿不到就等下一拍，别在后台线程里 panic。
    let Some(state) = app.try_state::<KadaState>() else {
        return;
    };
    // 载荷存进 state：悬浮窗是懒创建的，前端加载后据此兜底渲染（与触发气泡同一个套路）。
    state.status.lock().unwrap().replace(payload.clone());
    // 托盘提示：悬停才看得见，但零成本、最不打扰，状态为空时回到应用名。
    if let Some(tray) = state.tray.lock().unwrap().as_ref() {
        let tip = if payload.idle() {
            "咔哒 Kada".to_string()
        } else {
            format!("咔哒 Kada · {}", payload.summary)
        };
        let _ = tray.set_tooltip(Some(&tip));
    }
    // 悬浮指示：设置里关掉了、或没有东西生效 → 收起来（窗口留着重用，不销毁）。
    if !hud_enabled || payload.idle() {
        if let Some(w) = app.get_webview_window("hud") {
            let _ = w.hide();
        }
        return;
    }
    // 建 WebView 是重活（首次上百毫秒），**不能就地做**：这条线程还要落地序列 / 和弦的超时
    // 回放（`Engine::tick`），占住它等于让被吞掉的键晚几百毫秒才补回来。与触发气泡同一条
    // 约定——唤窗一律交给别的线程（`show_toast` 也是在 `std::thread::spawn` 里建窗的）。
    let (app, payload) = (app.clone(), payload.clone());
    std::thread::spawn(move || {
        if let Some(w) = ensure_hud(&app) {
            // 显示交给前端：它渲染完量出内容宽高再调 [`hud_ready`]（要贴底部居中，尺寸得先量）。
            // 这次 emit 晚到也没关系：窗口若已过期，`hud_ready` 会再查一次当前状态并把它藏回去。
            let _ = w.emit("status-change", &payload);
        }
    });
}

// ---- 「设置变量」输入窗（`Action::Ask` 的输入框）----
//
// 以前借系统输入框（Windows 的 PowerShell InputBox / macOS 的 osascript / Linux 的 zenity），
// 灰白一套跟四套主题割裂。改成自己的小窗：文案与配色全走主题变量，跟随主界面
// （同一个 origin，`localStorage["kada.theme"]` 直接生效）。
// 与 toast / hud / hints / 菜单那几只浮窗的**根本差别**：这个窗要能激活——用户要在里面打字。

pub(crate) const PROMPT_WINDOW: &str = "prompt";

/// 发给输入窗的内容；也留在 state 里给懒创建后的前端兜底拉取。
#[derive(Clone, Serialize)]
pub(crate) struct PromptPayload {
    prompt: String,
    default: String,
}

/// 按需创建输入窗。**`focusable` 保持默认（true）**：其余浮窗用 `WS_EX_NOACTIVATE` 挡激活，
/// 这个窗反过来——不激活就收不到键盘。`focused(false)` 只让首次显示不自己抢焦点，
/// 随后的 [`prompt_ready`] 会按尺寸居中并主动 `set_focus`。
fn ensure_prompt(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window(PROMPT_WINDOW) {
        return Some(w);
    }
    let w = match tauri::WebviewWindowBuilder::new(
        app,
        PROMPT_WINDOW,
        tauri::WebviewUrl::App("index.html#prompt".into()),
    )
    .decorations(false)
    .transparent(true)
    // 同其余浮窗：别借系统投影（那圈 DWM 边框会挂在面板外面），投影由 CSS 自己画。
    .shadow(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .resizable(false)
    .focused(false)
    .inner_size(420.0, 190.0)
    .visible(false)
    .build()
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("创建输入框窗口失败: {e}");
            return None;
        }
    };
    // 用户按 Alt+F4 / 系统关窗 = 取消：不给回传就等于动作链一直等一个永远不会来的答案。
    let handle = w.clone();
    w.on_window_event(move |ev| {
        if let WindowEvent::CloseRequested { api, .. } = ev {
            api.prevent_close();
            if let Some(state) = handle.app_handle().try_state::<KadaState>() {
                if let Some(tx) = state.prompt_tx.lock().unwrap().take() {
                    let _ = tx.send(None);
                }
            }
            let _ = handle.hide();
        }
    });
    Some(w)
}

/// 显示带主题的输入框并等用户填完（壳层注册给 `kada_actions::prompt` 的实现）。
///
/// 跑在动作链的执行线程上，**不在钩子回调里**，阻塞等答案是安全的。
pub(crate) fn ui_prompt(
    app: &tauri::AppHandle,
    prompt: &str,
    default: &str,
) -> Result<String, String> {
    // 同一时刻只开一个框：两条链同时要输入时后来者排队，别让两个窗抢同一个回传通道。
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    // 中止代数在**排队之前**取：排队期间被「停止执行」叫停的，拿完锁就该立刻退出。
    let gen = actions_abort::generation();
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let state = app
        .try_state::<KadaState>()
        .ok_or("输入框不可用（状态未就绪）")?;
    let Some(w) = ensure_prompt(app) else {
        return Err("创建输入框窗口失败".into());
    };
    // 触发它的程序：收起窗口后要把焦点还回去（后面的文本注入打的是当前前台窗口）。
    let prev = input::foreground_window();
    let payload = PromptPayload {
        prompt: prompt.to_string(),
        default: default.to_string(),
    };
    state
        .prompt_payload
        .lock()
        .unwrap()
        .replace(payload.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    state.prompt_tx.lock().unwrap().replace(tx);
    // 窗已存在时监听器早就在听；首次建窗时事件可能早于前端注册监听，前端还会自己拉一次载荷。
    let _ = w.emit("prompt-show", &payload);
    let answer = loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(v) => break Some(v),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // 「停止执行」：输入框没有子进程可杀，靠代数比对退出等待。
                if actions_abort::aborted(gen) {
                    break None;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break None,
        }
    };
    state.prompt_tx.lock().unwrap().take();
    // 先还焦点再收起：Windows 隐藏前台窗不会自动把焦点还给下一个窗口。
    if let Some(hwnd) = prev {
        input::focus_window(hwnd);
    }
    let _ = w.hide();
    match answer {
        Some(Some(text)) => Ok(text),
        Some(None) => Err("已取消输入".into()),
        None => Err("已中止".into()),
    }
}

/// 在屏幕右下角弹一个短暂的气泡（常驻 3 秒后自动消失）。
pub(crate) fn show_toast(app: &tauri::AppHandle, name: &str, trigger: &str) {
    if !TRIGGER_TOAST.load(Ordering::Relaxed) {
        return;
    }
    let payload = ToastPayload {
        name: name.to_string(),
        trigger: trigger.to_string(),
        subtitle: None,
    };
    show_toast_payload(app, payload);
}

/// 弹一条自定义副标题的气泡（不含「正在执行…」，用于「发现新版本」这类通知）。
pub(crate) fn show_toast_note(app: &tauri::AppHandle, title: &str, subtitle: &str) {
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
    app.state::<KadaState>()
        .toast
        .lock()
        .unwrap()
        .replace(payload.clone());
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
