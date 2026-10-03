//! Linux 键盘 / 鼠标钩子（evdev + uinput）。
//!
//! 职责与 Windows 后端对齐：把键盘 / 鼠标键事件翻译成 [`KeyEvent`]，回调决定
//! [`Action`]，注入由 `simulate` 完成。
//!
//! 机制：
//! - 打开 `/dev/input/event*` 中的键盘与鼠标设备并用 [EVIOCGRAB] 独占抓取：被
//!   抓设备的所有事件只到达我们，原事件不再进应用。
//! - 用 `/dev/uinput` 建一个「键盘 + 鼠标合一」的虚拟设备（键码 1..=0x2ff 已含
//!   `BTN_*`，再加相对轴承载移动 / 滚轮），把 [`Action`] 结果与不认识的原始事件
//!   （左右键、移动、滚轮）全部转发给系统。事件全走我们的 uinput 设备，天然不存在
//!   「收到自己注入事件」的回环。
//! - 鼠标键（中键 / 侧键 MB4 / MB5）与键盘键走同一套状态机；左右键 / 移动 / 滚轮
//!   不进键模型、原样转发（与 Windows 钩子「只处理中键与侧键」同口径）。
//! - 自动重复：物理键长按时内核产生 value=2 的事件，`Allow` 原样转发、
//!   `Replace` 按目标键转发，长按连发不丢。
//! - 修饰键状态按事件流维护（`MODS_DOWN`），吞键/改键不影响真实状态位，
//!   与 Windows 的 `GetKeyState` 语义对齐。
//!
//! 权限（7.3-⑱）：抓取需要 root，或 `input` 组 + udev 放行 `/dev/uinput`。起不来时
//! [`start`] 返回**能照抄的指引**（分清「读不了键盘」「uinput 不在」「被其它工具独占」），
//! 壳层把它记进消息中心、应用照常启动，用户看得见「为什么按键全不响」。
//!
//! 已知天花板（升级路径）：
//! - 触摸板 / 触摸屏 / 指点杆**不抓取**：多点与 ABS 协议原样转发做不到，抓了等于废掉；
//!   它们的中键 / 侧键不能作触发键（键盘与真鼠标不受影响）。
//! - 热插拔不在监听列表里（重启应用即可；7.4-㉜）；Wayland 上同一套代码可用。

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use evdev::{
    AttributeSet, Device, EventType, InputEvent, KeyCode, PropType, RelativeAxisCode,
    uinput::VirtualDevice,
};

use kada_core::{FrontmostContext, Key, Modifier};

/// Linux errno（grab / open 失败原因判定用；跨架构取值一致）。
const EPERM: i32 = 1;
const ENOENT: i32 = 2;
const EACCES: i32 = 13;
const EBUSY: i32 = 16;

/// 一次键盘/鼠标键事件。
#[derive(Clone, Debug)]
pub enum KeyEvent {
    Down { key: Key, mods: BTreeSet<Modifier>, repeat: bool },
    Up { key: Key, mods: BTreeSet<Modifier> },
}

/// 处理函数对事件的处置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// 放行（把原键转发给系统）。
    Allow,
    /// 吞掉本事件（连同后续 keyup）。
    Block,
    /// 吞掉原键、注入目标键。
    Replace(Key),
}

type Handler = Box<dyn FnMut(KeyEvent) -> Action + Send>;

static HANDLER: Mutex<Option<Handler>> = Mutex::new(None);
/// 已决定吞掉的键：后续 keyup 也要吞。
static SWALLOWED: LazyLock<Mutex<HashSet<Key>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// Replace 注入后仍按下的宿主原键 → 目标键。
static REPLACED_DOWN: LazyLock<Mutex<HashMap<Key, Key>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// 物理按下的修饰键码。吞键/改键不动它（与 Windows 的 GetKeyState 对齐）。
static MODS_DOWN: LazyLock<Mutex<HashSet<u16>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// uinput 虚拟设备（键盘 + 鼠标合一）：`start` 创建，事件转发与 simulate 共用。
static VDEV: LazyLock<Mutex<Option<VirtualDevice>>> = LazyLock::new(|| Mutex::new(None));
/// `start` 是否成功（抓到设备 + 虚拟设备建成）。壳层据此拦「录制不可用」。
static STARTED: AtomicBool = AtomicBool::new(false);
/// 「设备线断过」累计次数（诊断 / 壳层输入状态复位用）。
static REINSTALLS: AtomicU64 = AtomicU64::new(0);

/// 输入层是否已启动。Linux 与 macOS 一样存在「起不来但应用该照常活着」的场景
/// （权限没配好）：如实报 false，录制入口给出解释而不是录到一片空白。
pub fn hooks_supported() -> bool {
    STARTED.load(Ordering::Relaxed)
}

/// 「设备线断过」计数：设备拔出等导致事件丢失时 +1。壳层 `ResetWatch` 轮询本计数
/// 跟着复位 `Engine` 状态——拔掉的设备上还按着的键永远等不到抬起，状态机会一直以为
/// 「Ctrl 还按着」。与 Windows 的钩子重装计数同一通道、同一语义。
pub fn reinstall_count() -> u64 {
    REINSTALLS.load(Ordering::Relaxed)
}

/// 钩子句柄。Drop 时停止钩子线程并回收。
pub struct HookHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Drop for HookHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// 安装全局键盘/鼠标钩子。处理函数在钩子线程回调内同步执行。
///
/// 起不来时返回的 [`io::Error`] 带**能照抄的修复指引**（7.3-⑱）：分清没权限读键盘、
/// `/dev/uinput` 缺失 / 没权限、设备被其它独占工具占用。错误串直接面向用户，别再包一层。
pub fn start<F>(handler: F) -> io::Result<HookHandle>
where
    F: FnMut(KeyEvent) -> Action + Send + 'static,
{
    *HANDLER.lock().unwrap() = Some(Box::new(handler));

    // 抓取键盘与鼠标设备。能枚举到 = 打开成功（权限够）；抓取失败按 errno 分诊。
    let mut devices: Vec<Device> = Vec::new();
    let mut saw_keyboard = false;
    let mut keyboard_busy = false;
    for (_, mut dev) in evdev::enumerate() {
        if is_keyboard(&dev) {
            saw_keyboard = true;
            match dev.grab() {
                Ok(()) => devices.push(dev),
                Err(e) if e.raw_os_error() == Some(EBUSY) => keyboard_busy = true,
                Err(e) => eprintln!("kada-hook: 键盘抓取失败，跳过：{e}"),
            }
        } else if is_pointer(&dev) {
            // 鼠标抓不到不致命：中键/侧键改键不可用，键盘照常工作。
            match dev.grab() {
                Ok(()) => devices.push(dev),
                Err(e) => eprintln!("kada-hook: 鼠标抓取失败，跳过：{e}"),
            }
        }
    }
    if devices.is_empty() {
        *HANDLER.lock().unwrap() = None;
        return Err(start_error(saw_keyboard, keyboard_busy));
    }

    // 虚拟设备：键盘 + 鼠标合一。键码 1..=0x2ff 覆盖全部 KEY_* 与 BTN_*（鼠标键注入
    // 不再是缺口，7.3-㉔）；相对轴 0x00..=0x0f 覆盖移动 / 滚轮（含高分辨率滚轮）。
    let keys: AttributeSet<KeyCode> = (1..=0x2ff).map(KeyCode).collect();
    let rels: AttributeSet<RelativeAxisCode> = (0x00..=0x0f).map(RelativeAxisCode).collect();
    let vdev = match VirtualDevice::builder()
        .map_err(|e| uinput_error(&e))?
        .name("kada-virtual-input")
        .with_keys(&keys)
        .and_then(|b| b.with_relative_axes(&rels))
        .and_then(|b| b.build())
    {
        Ok(v) => v,
        Err(e) => {
            *HANDLER.lock().unwrap() = None;
            return Err(uinput_error(&e));
        }
    };
    *VDEV.lock().unwrap() = Some(vdev);

    for dev in &devices {
        dev.set_nonblocking(true)?;
    }
    STARTED.store(true, Ordering::Relaxed);

    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = Arc::clone(&stop);
    let join = thread::Builder::new()
        .name("kada-hook".into())
        .spawn(move || {
            if let Err(e) = poll_loop(&mut devices, &stop2) {
                eprintln!("kada-hook: {e}");
            }
        })?;
    Ok(HookHandle { stop, join: Some(join) })
}

/// 组装「一个设备都没抓到」的错误（[`start`]）。
fn start_error(saw_keyboard: bool, keyboard_busy: bool) -> io::Error {
    if saw_keyboard && keyboard_busy {
        return io::Error::other(
            "键盘设备被其它程序独占（另一个 Kada 实例，或 keyd / xremap 等改键工具在运行），\n\
             关掉它们后重启 Kada",
        );
    }
    if !saw_keyboard && keyboard_files_exist() {
        // /dev/input/event* 就在那里、却一个都枚举不到 = 连打开的权限都没有
        // （无权限的设备在 evdev 枚举阶段就被静默跳过）。
        return io::Error::other(
            "无权限读取键盘设备（/dev/input/event*）。把当前用户加入 input 组并注销重登：\n\
             sudo usermod -aG input $USER\n\
             然后重启 Kada。加组后仍提示无权限的话，检查桌面会话是否重新登录过（组只在新会话生效）",
        );
    }
    io::Error::other("未发现可用的键盘设备（/dev/input/event* 下没有键盘）")
}

/// `/dev/input` 下是否存在 event* 设备文件（权限分诊用：文件在而枚举不到 = 打不开）。
fn keyboard_files_exist() -> bool {
    let Ok(rd) = std::fs::read_dir("/dev/input") else { return false };
    rd.flatten().any(|e| e.file_name().to_string_lossy().starts_with("event"))
}

/// `/dev/uinput` 建不出来时的错误（[`start`]）：缺失与无权限各给一条能照抄的修复命令。
fn uinput_error(e: &io::Error) -> io::Error {
    match e.raw_os_error() {
        Some(ENOENT) => io::Error::other(format!(
            "系统没有 /dev/uinput（虚拟输入设备）。加载 uinput 内核模块并开机自动加载：\n\
             sudo modprobe uinput\n\
             echo uinput | sudo tee /etc/modules-load.d/uinput.conf\n\
             （原始错误：{e}）"
        )),
        Some(EACCES) | Some(EPERM) => io::Error::other(format!(
            "无权限写 /dev/uinput。创建 udev 规则放行（当前用户须已在 input 组，\
             没有就先执行 sudo usermod -aG input $USER）：\n\
             echo 'KERNEL==\"uinput\", MODE=\"0660\", GROUP=\"input\"' | sudo tee /etc/udev/rules.d/60-kada.rules\n\
             sudo udevadm control --reload && sudo udevadm trigger\n\
             然后注销重登并重启 Kada（原始错误：{e}）"
        )),
        _ => io::Error::other(format!("创建虚拟输入设备失败：{e}")),
    }
}

/// 是不是键盘：报出字母键区（排除只有几个功能键的电源 / 蓝牙配对设备）。
fn is_keyboard(dev: &Device) -> bool {
    let Some(keys) = dev.supported_keys() else {
        return false;
    };
    keys.contains(KeyCode::KEY_A) && keys.contains(KeyCode::KEY_Z)
}

/// 是不是该抓取的「真鼠标」：有相对移动轴 + 左键，且**不是**触摸板 / 触摸屏 /
/// 指点杆。触摸类设备协议复杂（多点、ABS 坐标、压力），原样转发做不到，抓了等于
/// 废掉整个触摸板，一律不碰（7.3-㉔ 的边界：这些设备的中键 / 侧键不能作触发键）。
fn is_pointer(dev: &Device) -> bool {
    let Some(rel) = dev.supported_relative_axes() else { return false };
    if !(rel.contains(RelativeAxisCode::REL_X) && rel.contains(RelativeAxisCode::REL_Y)) {
        return false;
    }
    let Some(keys) = dev.supported_keys() else { return false };
    if !keys.contains(KeyCode::BTN_LEFT) {
        return false;
    }
    let props = dev.properties();
    // INPUT_PROP_* 标记；老内核没属性时靠触摸类按键（BTN_TOOL_FINGER / BTN_TOUCH）兜底。
    let touch_props = props.contains(PropType::POINTER)
        && (props.contains(PropType::DIRECT)
            || props.contains(PropType::BUTTONPAD)
            || props.contains(PropType::SEMI_MT)
            || props.contains(PropType::TOPBUTTONPAD));
    if touch_props
        || props.contains(PropType::POINTING_STICK)
        || props.contains(PropType::ACCELEROMETER)
        || keys.contains(KeyCode::BTN_TOOL_FINGER)
        || keys.contains(KeyCode::BTN_TOUCH)
    {
        return false;
    }
    true
}

fn poll_loop(devices: &mut Vec<Device>, stop: &AtomicBool) -> io::Result<()> {
    while !stop.load(Ordering::Relaxed) {
        let mut i = 0;
        while i < devices.len() {
            // 先 collect 再进 match：fetch_events 的迭代器借住 devices[i]，
            // 匹配臂里的 devices.remove(i) 会撞上第二次可变借用。
            let fetched: io::Result<Vec<InputEvent>> =
                devices[i].fetch_events().map(|it| it.collect());
            match fetched {
                Ok(events) => {
                    for ev in events {
                        match ev.event_type() {
                            EventType::KEY => handle_key(ev.code(), ev.value()),
                            // 移动 / 滚轮：不进键模型，原样转发（虚拟设备已声明相对轴）。
                            EventType::RELATIVE => {
                                forward_raw(EventType::RELATIVE.0, ev.code(), ev.value())
                            }
                            // SYN 由 emit 自动补；LED / SND 等不转发（用户态用不到）。
                            _ => {}
                        }
                    }
                    i += 1;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => i += 1,
                Err(_) => {
                    devices.remove(i); // 设备拔出等
                    on_device_lost();
                }
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

/// 设备断线（拔出 / USB 抖动）：这台设备上还按着的键永远等不到抬起，状态机却会一直
/// 以为「Ctrl 还按着」，之后所有快捷键误判。就地复位三张表（与 Windows 重装钩子
/// [`win`] 的 `reset_runtime_state` 同口径）并把已注入的目标键补一个 up，计一次
/// 「线断过」让壳层 `ResetWatch` 跟着复位 `Engine`（序列 / 和弦等等待态）。
fn on_device_lost() {
    for (_, target) in REPLACED_DOWN.lock().unwrap().drain() {
        if let Some(tc) = key_to_code(target) {
            forward_raw(EventType::KEY.0, tc, 0);
        }
    }
    SWALLOWED.lock().unwrap().clear();
    MODS_DOWN.lock().unwrap().clear();
    REINSTALLS.fetch_add(1, Ordering::Relaxed);
    eprintln!("kada-hook: 输入设备断开，已复位按键状态（计数 {}）", reinstall_count());
}

fn handle_key(code: u16, value: i32) {
    let Some(key) = code_to_key(code) else {
        // 未知键（媒体键等）与左右键（BTN_LEFT/RIGHT 不进键模型）：原样转发，
        // 保持设备功能可用。
        forward_raw(EventType::KEY.0, code, value);
        return;
    };
    if value == 0 {
        release(code, key);
    } else {
        press(key, code, value);
    }
}

fn press(key: Key, code: u16, value: i32) {
    if key_as_modifier(key).is_some() {
        MODS_DOWN.lock().unwrap().insert(code);
    }
    let repeat = value == 2;
    let action = {
        let mut h = HANDLER.lock().unwrap();
        match h.as_mut() {
            Some(f) => f(KeyEvent::Down { key, mods: current_mods(key), repeat }),
            None => Action::Allow,
        }
    };
    match action {
        Action::Allow => forward_raw(EventType::KEY.0, code, value),
        Action::Block => {
            SWALLOWED.lock().unwrap().insert(key);
        }
        Action::Replace(target) => {
            SWALLOWED.lock().unwrap().insert(key);
            REPLACED_DOWN.lock().unwrap().insert(key, target);
            if let Some(tc) = key_to_code(target) {
                forward_raw(EventType::KEY.0, tc, value);
            }
        }
    }
}

fn release(code: u16, key: Key) {
    if key_as_modifier(key).is_some() {
        MODS_DOWN.lock().unwrap().remove(&code);
    }
    let swallowed = SWALLOWED.lock().unwrap().remove(&key);
    if swallowed {
        if let Some(target) = REPLACED_DOWN.lock().unwrap().remove(&key) {
            if let Some(tc) = key_to_code(target) {
                forward_raw(EventType::KEY.0, tc, 0);
            }
        }
    }
    // 无论吞掉与否都回调 handler（观察者）：tap-hold 需要在 keyup 时判定 tap/hold。
    if let Some(f) = HANDLER.lock().unwrap().as_mut() {
        let _ = f(KeyEvent::Up { key, mods: current_mods(key) });
    }
    if !swallowed {
        forward_raw(EventType::KEY.0, code, 0);
    }
}

/// 当前按下的修饰键集合；`event_key` 若是修饰键则包含其自身
/// （方向修正，与 Windows 钩子一致）。
fn current_mods(event_key: Key) -> BTreeSet<Modifier> {
    let mods_down = MODS_DOWN.lock().unwrap();
    let mut mods = BTreeSet::new();
    for &code in mods_down.iter() {
        if let Some(m) = code_modifier(code) {
            mods.insert(m);
        }
    }
    if let Some(m) = key_as_modifier(event_key) {
        mods.insert(m);
    }
    mods
}

/// evdev 修饰键码 → 通用修饰键（左右手合并）。
fn code_modifier(code: u16) -> Option<Modifier> {
    if code == KeyCode::KEY_LEFTSHIFT.0 || code == KeyCode::KEY_RIGHTSHIFT.0 {
        Some(Modifier::Shift)
    } else if code == KeyCode::KEY_LEFTCTRL.0 || code == KeyCode::KEY_RIGHTCTRL.0 {
        Some(Modifier::Ctrl)
    } else if code == KeyCode::KEY_LEFTALT.0 || code == KeyCode::KEY_RIGHTALT.0 {
        Some(Modifier::Alt)
    } else if code == KeyCode::KEY_LEFTMETA.0 || code == KeyCode::KEY_RIGHTMETA.0 {
        Some(Modifier::Meta)
    } else {
        None
    }
}

fn key_as_modifier(k: Key) -> Option<Modifier> {
    match k {
        Key::Shift => Some(Modifier::Shift),
        Key::Control => Some(Modifier::Ctrl),
        Key::Alt => Some(Modifier::Alt),
        Key::Meta => Some(Modifier::Meta),
        _ => None,
    }
}

fn forward_raw(type_: u16, code: u16, value: i32) {
    let mut v = VDEV.lock().unwrap();
    if let Some(vdev) = v.as_mut() {
        let _ = vdev.emit(&[InputEvent::new(type_, code, value)]);
    }
}

/// [`Key`] → evdev 键码。通用修饰键归一到左键（与 Windows 的 VK_* 归一一致）；
/// 鼠标键映射到 `BTN_*`（键事件类型，虚拟设备已声明，注入与改键目标都不再是缺口）。
pub fn key_to_code(k: Key) -> Option<u16> {
    use Key::*;
    let code = match k {
        A => KeyCode::KEY_A, B => KeyCode::KEY_B, C => KeyCode::KEY_C,
        D => KeyCode::KEY_D, E => KeyCode::KEY_E, F => KeyCode::KEY_F,
        G => KeyCode::KEY_G, H => KeyCode::KEY_H, I => KeyCode::KEY_I,
        J => KeyCode::KEY_J, K => KeyCode::KEY_K, L => KeyCode::KEY_L,
        M => KeyCode::KEY_M, N => KeyCode::KEY_N, O => KeyCode::KEY_O,
        P => KeyCode::KEY_P, Q => KeyCode::KEY_Q, R => KeyCode::KEY_R,
        S => KeyCode::KEY_S, T => KeyCode::KEY_T, U => KeyCode::KEY_U,
        V => KeyCode::KEY_V, W => KeyCode::KEY_W, X => KeyCode::KEY_X,
        Y => KeyCode::KEY_Y, Z => KeyCode::KEY_Z,
        Digit0 => KeyCode::KEY_0, Digit1 => KeyCode::KEY_1, Digit2 => KeyCode::KEY_2,
        Digit3 => KeyCode::KEY_3, Digit4 => KeyCode::KEY_4, Digit5 => KeyCode::KEY_5,
        Digit6 => KeyCode::KEY_6, Digit7 => KeyCode::KEY_7, Digit8 => KeyCode::KEY_8,
        Digit9 => KeyCode::KEY_9,
        F1 => KeyCode::KEY_F1, F2 => KeyCode::KEY_F2, F3 => KeyCode::KEY_F3,
        F4 => KeyCode::KEY_F4, F5 => KeyCode::KEY_F5, F6 => KeyCode::KEY_F6,
        F7 => KeyCode::KEY_F7, F8 => KeyCode::KEY_F8, F9 => KeyCode::KEY_F9,
        F10 => KeyCode::KEY_F10, F11 => KeyCode::KEY_F11, F12 => KeyCode::KEY_F12,
        Comma => KeyCode::KEY_COMMA, Period => KeyCode::KEY_DOT,
        Slash => KeyCode::KEY_SLASH, Backslash => KeyCode::KEY_BACKSLASH,
        Semicolon => KeyCode::KEY_SEMICOLON, Quote => KeyCode::KEY_APOSTROPHE,
        Backquote => KeyCode::KEY_GRAVE, Minus => KeyCode::KEY_MINUS,
        Equal => KeyCode::KEY_EQUAL, BracketLeft => KeyCode::KEY_LEFTBRACE,
        BracketRight => KeyCode::KEY_RIGHTBRACE,
        Enter => KeyCode::KEY_ENTER, Escape => KeyCode::KEY_ESC,
        Tab => KeyCode::KEY_TAB, Space => KeyCode::KEY_SPACE,
        Backspace => KeyCode::KEY_BACKSPACE, Delete => KeyCode::KEY_DELETE,
        Insert => KeyCode::KEY_INSERT,
        CapsLock => KeyCode::KEY_CAPSLOCK,
        Shift => KeyCode::KEY_LEFTSHIFT, Control => KeyCode::KEY_LEFTCTRL,
        Alt => KeyCode::KEY_LEFTALT, Meta => KeyCode::KEY_LEFTMETA,
        Home => KeyCode::KEY_HOME, End => KeyCode::KEY_END,
        PageUp => KeyCode::KEY_PAGEUP, PageDown => KeyCode::KEY_PAGEDOWN,
        ArrowUp => KeyCode::KEY_UP, ArrowDown => KeyCode::KEY_DOWN,
        ArrowLeft => KeyCode::KEY_LEFT, ArrowRight => KeyCode::KEY_RIGHT,
        F13 => KeyCode::KEY_F13, F14 => KeyCode::KEY_F14, F15 => KeyCode::KEY_F15,
        F16 => KeyCode::KEY_F16, F17 => KeyCode::KEY_F17, F18 => KeyCode::KEY_F18,
        F19 => KeyCode::KEY_F19, F20 => KeyCode::KEY_F20, F21 => KeyCode::KEY_F21,
        F22 => KeyCode::KEY_F22, F23 => KeyCode::KEY_F23, F24 => KeyCode::KEY_F24,
        MediaPlayPause => KeyCode::KEY_PLAYPAUSE, MediaPrev => KeyCode::KEY_PREVIOUSSONG,
        MediaNext => KeyCode::KEY_NEXTSONG, VolumeMute => KeyCode::KEY_MUTE,
        VolumeDown => KeyCode::KEY_VOLUMEDOWN, VolumeUp => KeyCode::KEY_VOLUMEUP,
        Numpad0 => KeyCode::KEY_KP0, Numpad1 => KeyCode::KEY_KP1, Numpad2 => KeyCode::KEY_KP2,
        Numpad3 => KeyCode::KEY_KP3, Numpad4 => KeyCode::KEY_KP4, Numpad5 => KeyCode::KEY_KP5,
        Numpad6 => KeyCode::KEY_KP6, Numpad7 => KeyCode::KEY_KP7, Numpad8 => KeyCode::KEY_KP8,
        Numpad9 => KeyCode::KEY_KP9,
        NumpadAdd => KeyCode::KEY_KPPLUS, NumpadSubtract => KeyCode::KEY_KPMINUS,
        NumpadMultiply => KeyCode::KEY_KPASTERISK, NumpadDivide => KeyCode::KEY_KPSLASH,
        NumpadDecimal => KeyCode::KEY_KPDOT, NumpadEnter => KeyCode::KEY_KPENTER,
        NumLock => KeyCode::KEY_NUMLOCK,
        // 鼠标键：BTN_* 是键事件类型（码位在 0x110..0x117 键码区），改键目标与
        // Keys 动作注入都走虚拟设备的键能力（7.3-㉔）。
        MouseMiddle => KeyCode::BTN_MIDDLE,
        MouseBack => KeyCode::BTN_SIDE,
        MouseForward => KeyCode::BTN_EXTRA,
    };
    Some(code.0)
}

/// evdev 键码 → [`Key`]（左右修饰键归一）。中键 / 侧键来自鼠标设备；左右键
/// （`BTN_LEFT` / `BTN_RIGHT`）不进键模型——返回 `None` 走原样转发。
fn code_to_key(code: u16) -> Option<Key> {
    use Key::*;
    Some(match code {
        c if c == KeyCode::KEY_LEFTSHIFT.0 || c == KeyCode::KEY_RIGHTSHIFT.0 => Shift,
        c if c == KeyCode::KEY_LEFTCTRL.0 || c == KeyCode::KEY_RIGHTCTRL.0 => Control,
        c if c == KeyCode::KEY_LEFTALT.0 || c == KeyCode::KEY_RIGHTALT.0 => Alt,
        c if c == KeyCode::KEY_LEFTMETA.0 || c == KeyCode::KEY_RIGHTMETA.0 => Meta,
        c if c == KeyCode::KEY_A.0 => A, c if c == KeyCode::KEY_B.0 => B,
        c if c == KeyCode::KEY_C.0 => C, c if c == KeyCode::KEY_D.0 => D,
        c if c == KeyCode::KEY_E.0 => E, c if c == KeyCode::KEY_F.0 => F,
        c if c == KeyCode::KEY_G.0 => G, c if c == KeyCode::KEY_H.0 => H,
        c if c == KeyCode::KEY_I.0 => I, c if c == KeyCode::KEY_J.0 => J,
        c if c == KeyCode::KEY_K.0 => K, c if c == KeyCode::KEY_L.0 => L,
        c if c == KeyCode::KEY_M.0 => M, c if c == KeyCode::KEY_N.0 => N,
        c if c == KeyCode::KEY_O.0 => O, c if c == KeyCode::KEY_P.0 => P,
        c if c == KeyCode::KEY_Q.0 => Q, c if c == KeyCode::KEY_R.0 => R,
        c if c == KeyCode::KEY_S.0 => S, c if c == KeyCode::KEY_T.0 => T,
        c if c == KeyCode::KEY_U.0 => U, c if c == KeyCode::KEY_V.0 => V,
        c if c == KeyCode::KEY_W.0 => W, c if c == KeyCode::KEY_X.0 => X,
        c if c == KeyCode::KEY_Y.0 => Y, c if c == KeyCode::KEY_Z.0 => Z,
        c if c == KeyCode::KEY_0.0 => Digit0, c if c == KeyCode::KEY_1.0 => Digit1,
        c if c == KeyCode::KEY_2.0 => Digit2, c if c == KeyCode::KEY_3.0 => Digit3,
        c if c == KeyCode::KEY_4.0 => Digit4, c if c == KeyCode::KEY_5.0 => Digit5,
        c if c == KeyCode::KEY_6.0 => Digit6, c if c == KeyCode::KEY_7.0 => Digit7,
        c if c == KeyCode::KEY_8.0 => Digit8, c if c == KeyCode::KEY_9.0 => Digit9,
        c if c == KeyCode::KEY_F1.0 => F1, c if c == KeyCode::KEY_F2.0 => F2,
        c if c == KeyCode::KEY_F3.0 => F3, c if c == KeyCode::KEY_F4.0 => F4,
        c if c == KeyCode::KEY_F5.0 => F5, c if c == KeyCode::KEY_F6.0 => F6,
        c if c == KeyCode::KEY_F7.0 => F7, c if c == KeyCode::KEY_F8.0 => F8,
        c if c == KeyCode::KEY_F9.0 => F9, c if c == KeyCode::KEY_F10.0 => F10,
        c if c == KeyCode::KEY_F11.0 => F11, c if c == KeyCode::KEY_F12.0 => F12,
        c if c == KeyCode::KEY_COMMA.0 => Comma, c if c == KeyCode::KEY_DOT.0 => Period,
        c if c == KeyCode::KEY_SLASH.0 => Slash, c if c == KeyCode::KEY_BACKSLASH.0 => Backslash,
        c if c == KeyCode::KEY_SEMICOLON.0 => Semicolon,
        c if c == KeyCode::KEY_APOSTROPHE.0 => Quote,
        c if c == KeyCode::KEY_GRAVE.0 => Backquote, c if c == KeyCode::KEY_MINUS.0 => Minus,
        c if c == KeyCode::KEY_EQUAL.0 => Equal, c if c == KeyCode::KEY_LEFTBRACE.0 => BracketLeft,
        c if c == KeyCode::KEY_RIGHTBRACE.0 => BracketRight,
        c if c == KeyCode::KEY_ENTER.0 => Enter, c if c == KeyCode::KEY_ESC.0 => Escape,
        c if c == KeyCode::KEY_TAB.0 => Tab, c if c == KeyCode::KEY_SPACE.0 => Space,
        c if c == KeyCode::KEY_BACKSPACE.0 => Backspace, c if c == KeyCode::KEY_DELETE.0 => Delete,
        c if c == KeyCode::KEY_INSERT.0 => Insert,
        c if c == KeyCode::KEY_CAPSLOCK.0 => CapsLock,
        c if c == KeyCode::KEY_HOME.0 => Home, c if c == KeyCode::KEY_END.0 => End,
        c if c == KeyCode::KEY_PAGEUP.0 => PageUp, c if c == KeyCode::KEY_PAGEDOWN.0 => PageDown,
        c if c == KeyCode::KEY_UP.0 => ArrowUp, c if c == KeyCode::KEY_DOWN.0 => ArrowDown,
        c if c == KeyCode::KEY_LEFT.0 => ArrowLeft, c if c == KeyCode::KEY_RIGHT.0 => ArrowRight,
        c if c == KeyCode::KEY_F13.0 => F13, c if c == KeyCode::KEY_F14.0 => F14,
        c if c == KeyCode::KEY_F15.0 => F15, c if c == KeyCode::KEY_F16.0 => F16,
        c if c == KeyCode::KEY_F17.0 => F17, c if c == KeyCode::KEY_F18.0 => F18,
        c if c == KeyCode::KEY_F19.0 => F19, c if c == KeyCode::KEY_F20.0 => F20,
        c if c == KeyCode::KEY_F21.0 => F21, c if c == KeyCode::KEY_F22.0 => F22,
        c if c == KeyCode::KEY_F23.0 => F23, c if c == KeyCode::KEY_F24.0 => F24,
        c if c == KeyCode::KEY_PLAYPAUSE.0 => MediaPlayPause,
        c if c == KeyCode::KEY_PREVIOUSSONG.0 => MediaPrev,
        c if c == KeyCode::KEY_NEXTSONG.0 => MediaNext,
        c if c == KeyCode::KEY_MUTE.0 => VolumeMute,
        c if c == KeyCode::KEY_VOLUMEDOWN.0 => VolumeDown,
        c if c == KeyCode::KEY_VOLUMEUP.0 => VolumeUp,
        c if c == KeyCode::KEY_KP0.0 => Numpad0, c if c == KeyCode::KEY_KP1.0 => Numpad1,
        c if c == KeyCode::KEY_KP2.0 => Numpad2, c if c == KeyCode::KEY_KP3.0 => Numpad3,
        c if c == KeyCode::KEY_KP4.0 => Numpad4, c if c == KeyCode::KEY_KP5.0 => Numpad5,
        c if c == KeyCode::KEY_KP6.0 => Numpad6, c if c == KeyCode::KEY_KP7.0 => Numpad7,
        c if c == KeyCode::KEY_KP8.0 => Numpad8, c if c == KeyCode::KEY_KP9.0 => Numpad9,
        c if c == KeyCode::KEY_KPPLUS.0 => NumpadAdd,
        c if c == KeyCode::KEY_KPMINUS.0 => NumpadSubtract,
        c if c == KeyCode::KEY_KPASTERISK.0 => NumpadMultiply,
        c if c == KeyCode::KEY_KPSLASH.0 => NumpadDivide,
        c if c == KeyCode::KEY_KPDOT.0 => NumpadDecimal,
        c if c == KeyCode::KEY_KPENTER.0 => NumpadEnter,
        c if c == KeyCode::KEY_NUMLOCK.0 => NumLock,
        c if c == KeyCode::BTN_MIDDLE.0 => MouseMiddle,
        c if c == KeyCode::BTN_SIDE.0 => MouseBack,
        c if c == KeyCode::BTN_EXTRA.0 => MouseForward,
        _ => return None,
    })
}

/// 取当前前台窗口上下文（进程名 + 窗口标题），供「按前台应用/窗口」类条件求值。
///
/// Linux 下取前台窗口依赖桌面环境：X11 可用 `_NET_ACTIVE_WINDOW`（需 `xprop`），
/// Wayland 无通用查询协议。当前返回 None（前台类条件在此平台恒不成立，冲突检测
/// 会据 [`kada_core::PlatformCaps`] 标出，见 7.3-⑲），后续按规划接 X11 + 已知 WM
/// 适配（见 `docs/竞品分析与优化规划.md` 阶段二第 3 项）。
pub fn frontmost_context() -> Option<FrontmostContext> {
    None
}

/// 输入注入：按键/组合键/文本（与 Windows 后端同接口）。
pub mod simulate {
    //! 文本走剪贴板 + `Ctrl+V`（对中文等 Unicode 最稳，与 Windows 一致）；
    //! 副作用是短暂占用并恢复剪贴板。粘贴被吞的极端场景后续可加直发兜底。

    use std::io;
    use std::time::Duration;

    use evdev::{EventType, InputEvent};

    use kada_core::Key;

    use super::{key_to_code, VDEV};

    pub fn down(k: Key) {
        emit(k, 1);
    }

    pub fn up(k: Key) {
        emit(k, 0);
    }

    pub fn tap(k: Key) {
        down(k);
        up(k);
    }

    pub fn chord(keys: &[Key]) {
        for k in keys {
            down(*k);
        }
        // 全部按下后稍停再逆序松开，避免组合键太快导致目标程序收不到。
        std::thread::sleep(Duration::from_millis(30));
        for k in keys.iter().rev() {
            up(*k);
        }
    }

    fn emit(k: Key, value: i32) {
        let Some(code) = key_to_code(k) else {
            return;
        };
        let mut v = VDEV.lock().unwrap();
        if let Some(vdev) = v.as_mut() {
            let _ = vdev.emit(&[InputEvent::new(EventType::KEY.0, code, value)]);
        }
    }

    /// 把文本粘贴到当前焦点控件。
    pub fn type_text(text: &str) -> io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let mut cb = arboard::Clipboard::new().map_err(io::Error::other)?;
        let prev = cb.get_text().ok();
        cb.set_text(text.to_string()).map_err(io::Error::other)?;
        chord(&[Key::Control, Key::V]);
        // 等目标程序处理完粘贴再恢复剪贴板，避免竞态（同 Windows 版）。
        std::thread::sleep(Duration::from_millis(80));
        if let Some(p) = prev {
            let _ = cb.set_text(p);
        }
        Ok(())
    }

    /// 把文本逐字符直发（Linux 无对应通道，回落剪贴板粘贴）。
    ///
    /// 接口与 Windows / macOS 保持统一，但 uinput 虚拟设备只有键码、没有字符层
    /// （X11 的 XTEST 键符号注入到 Wayland 又不可用），「逐字直发」在此平台没有
    /// 通用实现——落回 [`type_text`] 的剪贴板粘贴。规划 7.3-㉒ 的直发兜底只在
    /// Windows / macOS 真正生效。
    pub fn type_text_unicode(text: &str) -> io::Result<()> {
        type_text(text)
    }

    /// 读取剪贴板文本（用于「转大小写」动作：复制选中 → 读剪贴板 → 转换 → 粘贴）。
    pub fn get_clipboard_text() -> io::Result<String> {
        let mut cb = arboard::Clipboard::new().map_err(io::Error::other)?;
        cb.get_text().map_err(io::Error::other)
    }

    /// Linux 版无需等待修饰键释放：物理键盘被 evdev 独占抓取（`EVIOCGRAB`），目标程序
    /// 看不到物理修饰键、只看到注入的虚拟设备事件，不存在「修饰键污染注入」的问题。
    /// 占位接口与 Windows 对齐（`fire` 两平台统一调用）。
    pub fn wait_modifiers_released(_timeout_ms: u64) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kada_core::key_name;

    #[test]
    fn mapping_roundtrip() {
        for k in [
            Key::A, Key::K, Key::Digit9, Key::F12, Key::CapsLock, Key::Control,
            Key::Alt, Key::Shift, Key::Meta, Key::Enter, Key::Escape, Key::Space,
            Key::Backspace, Key::Comma, Key::Minus, Key::Slash, Key::BracketLeft,
            Key::Quote, Key::ArrowUp, Key::PageDown,
            Key::F13, Key::F24, Key::MediaPlayPause, Key::MediaPrev, Key::MediaNext,
            Key::VolumeMute, Key::VolumeDown, Key::VolumeUp, Key::NumLock,
            Key::Numpad0, Key::Numpad9, Key::NumpadAdd, Key::NumpadSubtract,
            Key::NumpadMultiply, Key::NumpadDivide, Key::NumpadDecimal, Key::NumpadEnter,
        ] {
            let code = key_to_code(k).unwrap();
            assert_eq!(code_to_key(code), Some(k), "roundtrip {}", key_name(k));
        }
        // 鼠标键注入与识别（7.3-㉔）：BTN_* 双向映射，左右键不进键模型。
        for k in [Key::MouseMiddle, Key::MouseBack, Key::MouseForward] {
            let code = key_to_code(k).unwrap();
            assert_eq!(code_to_key(code), Some(k), "mouse roundtrip {}", key_name(k));
        }
        assert_eq!(code_to_key(KeyCode::BTN_LEFT.0), None);
        assert_eq!(code_to_key(KeyCode::BTN_RIGHT.0), None);
    }

    #[test]
    fn modifier_left_right_merge() {
        assert_eq!(code_modifier(KeyCode::KEY_LEFTCTRL.0), Some(Modifier::Ctrl));
        assert_eq!(code_modifier(KeyCode::KEY_RIGHTCTRL.0), Some(Modifier::Ctrl));
        assert_eq!(code_modifier(KeyCode::KEY_RIGHTALT.0), Some(Modifier::Alt));
        assert_eq!(code_modifier(KeyCode::KEY_LEFTMETA.0), Some(Modifier::Meta));
        assert_eq!(code_modifier(KeyCode::KEY_BACKSPACE.0), None);
    }

    /// 设备断线复位（7.3-⑱）：三张表清空、被替换的目标键补 up（经假 VDEV 验证）。
    /// 真设备路径（poll_loop 的 Err 分支）只能真机验证，这里锁行为口径。
    #[test]
    fn device_lost_resets_state() {
        SWALLOWED.lock().unwrap().insert(Key::MouseBack);
        REPLACED_DOWN.lock().unwrap().insert(Key::K, Key::Control);
        MODS_DOWN.lock().unwrap().insert(KeyCode::KEY_LEFTCTRL.0);
        let before = reinstall_count();
        on_device_lost();
        assert!(SWALLOWED.lock().unwrap().is_empty());
        assert!(REPLACED_DOWN.lock().unwrap().is_empty());
        assert!(MODS_DOWN.lock().unwrap().is_empty());
        assert_eq!(reinstall_count(), before + 1);
    }
}
