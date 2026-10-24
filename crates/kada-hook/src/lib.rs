//! 平台钩子引擎：全局键盘事件监听 + 拦截。
//!
//! - Windows 后端：[`win`] 用 `SetWindowsHookEx(WH_KEYBOARD_LL)`，
//!   独立线程跑消息循环，回调内同步决定放行/吞掉/改键（`Action`），
//!   保证顺序与延迟（功耗低：幽灵按键没有）。
//! - Linux 后端：[`linux`] 用 evdev + uinput（内核输入层），X11 / Wayland
//!   桌面都能全局拦截与改键，需要 root 或 `input` 组授权 `/dev/input` 与
//!   `/dev/uinput`。
//! - macOS 后端：[`macos`] 用 `CGEventTap`（CoreGraphics 事件流），需要「辅助功能」
//!   权限；键码是 macOS 虚拟键码（ANSI 位置语义）。
//!
//! 事件统一为轻量的键盘事件，键模型在 [`kada_core`]。

#[cfg(windows)]
pub mod win;

#[cfg(windows)]
mod win_capture;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;

/// 鼠标手势的采集（规划 7.2-MG）：平台无关的状态机 + 轨迹出口，识别在壳层做。
pub mod gesture;

/// 触发角（EXT-10）：平台无关的坐标状态机 + 出口，执行在壳层做。
pub mod hotcorner;
pub mod wheel_edge;

pub use kada_core::{
    format_shortcut, key_name, matches, Key, Modifier, MouseButton, RawEvent, Shortcut,
};

/// 本机输入环境探测（三平台同一入口，规划 7.2-㊽ / `docs/底层输入能力规划.md` 档位 A）：
/// 有哪些已知的「按键接管者」正在运行（厂商代理 divert 走按键 / evdev 抢占者），以及
/// Windows 的内存完整性（HVCI）状态。只读探测，失败一律当作「不知道」（空表 / `None`）。
///
/// 三个平台各自的实现在自己后端里（`win` / `linux` / `macos` 的 `input_env`）；macOS 尚未
/// 实现、恒空——**「这个平台没做」与「探测过了没发现」在界面上是两句话**，别混
/// （见 `macos::input_env`）。
pub fn platform_input_env() -> kada_core::InputEnv {
    #[cfg(windows)]
    {
        win::input_env()
    }
    #[cfg(target_os = "linux")]
    {
        linux::input_env()
    }
    #[cfg(target_os = "macos")]
    {
        macos::input_env()
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        kada_core::InputEnv::default()
    }
}

/// 系统「安全输入」（Secure Input）是否开启（规划 7.2-㊸，macOS 专属；其余平台恒 false）。
///
/// 终端输密码等场景 macOS 会临时关掉所有事件 tap——Kada 的快捷键此时全部失效且无任何
/// 提示。壳层定时器低频轮询这个查询，状态**翻转**时推一条消息中心提示（见
/// `src-tauri` 的 `report_secure_input`），把「静默失灵」变成「知道为什么、知道会自己恢复」。
pub fn secure_input_enabled() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::secure_input_enabled()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// 系统空闲秒数（最后一次键盘 / 鼠标输入至今）：自动更新「长时间不使用电脑时安装」的判据。
///
/// `None` = 本平台查不出来（Linux：X11 / Wayland 没有统一且免提升权的接口），调用方按
/// 「不空闲」处理——**绝不因为查不出来就安装**。
pub fn idle_seconds() -> Option<u64> {
    #[cfg(windows)]
    {
        win::idle_seconds()
    }
    #[cfg(target_os = "macos")]
    {
        macos::idle_seconds()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        None
    }
}

/// 窗口控制操作（应用动作的「最大化 / 最小化 / 放置到前台」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowOp {
    /// 最大化主窗口（已最小化则先还原）。
    Maximize,
    /// 最小化主窗口。
    Minimize,
    /// 把主窗口放到前台（已最小化则先还原）；不启动程序。
    Activate,
}

/// 按程序名 / 应用名找到其主窗口并执行窗口控制。
///
/// 三平台各自实现（`win` / `linux` / `macos`），失败返回能直接展示给用户的中文原因。
/// 本接口不属于输入注入，**不走提权通道**；目标程序若以管理员身份运行，非提权的 Kada
/// 可能改不动它的窗口（Windows 上表现为 `ShowWindow` 无效果）。
pub fn control_window(program: &str, op: WindowOp) -> Result<(), String> {
    #[cfg(windows)]
    {
        win::control_window(program, op)
    }
    #[cfg(target_os = "linux")]
    {
        linux::control_window(program, op)
    }
    #[cfg(target_os = "macos")]
    {
        macos::control_window(program, op)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = (program, op);
        Err("当前平台不支持窗口控制".into())
    }
}


/// 当前窗口动作的操作（[`window_action`]，EXT-14）。与 [`WindowOp`]（按程序名找主窗口）
/// 分开：这里的作用对象是**触发那一刻抓到的前台窗口**，且**一律不激活、不抢焦点**。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowAction {
    /// 最大化 / 还原（按当前是否最大化切换）。
    MaximizeRestore,
    /// 最小化。
    Minimize,
    /// 置顶切换（`WS_EX_TOPMOST` 开关），不改变前后顺序里的激活状态。
    ToggleTopmost,
    /// 在所在显示器工作区里居中（最大化 / 最小化时先还原）。
    Center,
    /// 关闭（`WM_CLOSE`，让目标程序自己走保存 / 确认流程）。
    Close,
    /// 关闭同类窗口：目标窗口所在进程的全部顶层窗口。
    CloseSameApp,
}

/// 对触发时抓到的前台窗口执行操作。
///
/// `hwnd` 是壳层在触发那一刻用 [`win::foreground_window`] 抓的窗口句柄（Windows 为真
/// `HWND`；壳层在其它平台恒传 `None`）。**只有 Windows 实现**：macOS / Linux 返回明确
/// 错误（不静默失败），配置里用到时冲突检测同时标软冲突。
pub fn window_action(hwnd: Option<isize>, op: WindowAction) -> Result<(), String> {
    #[cfg(windows)]
    {
        let hwnd = hwnd.ok_or_else(|| {
            "取不到前台窗口（前台是 Kada 自己的窗口，或当前没有前台窗口）".to_string()
        })?;
        win::window_action(hwnd, op)
    }
    #[cfg(not(windows))]
    {
        let _ = (hwnd, op);
        Err("「当前窗口」动作目前只支持 Windows".into())
    }
}


/// 本机正在运行的外部工具 id（`kada_core::EXTERNAL_TOOLS` 的子集）。
pub fn platform_running_tools() -> Vec<String> {
    #[cfg(windows)]
    {
        win::running_tools()
    }
    #[cfg(target_os = "linux")]
    {
        linux::running_tools()
    }
    #[cfg(target_os = "macos")]
    {
        macos::running_tools()
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Vec::new()
    }
}

/// 一张抓到的图（RGBA8，行序从上到下）。
#[derive(Clone, Debug, Default)]
pub struct CapturedImage {
    /// 宽度（像素）。
    pub width: u32,
    /// 高度（像素）。
    pub height: u32,
    /// `width * height * 4` 字节，RGBA 顺序。
    pub rgba: Vec<u8>,
}

/// 亮度调节方向（EXT-22）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrightnessOp {
    /// 调高。
    Up,
    /// 调低。
    Down,
    /// 恢复本次运行里第一次调节前的原始值。
    Restore,
}

/// 圈选截图：按选区（屏幕物理坐标，左 / 上 / 右 / 下）裁出图。选区由钩子层的
/// 「右键画圈 + 点左键」确认（见 [`gesture::on_left_down`]）。目前只有 Windows 用 GDI 实现。
pub fn capture_rect(rect: (i32, i32, i32, i32)) -> Result<CapturedImage, String> {
    #[cfg(windows)]
    {
        win_capture::capture_rect(rect)
    }
    #[cfg(not(windows))]
    {
        let _ = rect;
        Err("「圈选截图」目前只支持 Windows".into())
    }
}

/// 把截图写进系统图片剪贴板（圈选截图的唯一出口）。
pub fn set_clipboard_image(img: &CapturedImage) -> Result<(), String> {
    #[cfg(windows)]
    {
        win_capture::set_clipboard_image(img)
    }
    #[cfg(not(windows))]
    {
        let _ = img;
        Err("「圈选截图」目前只支持 Windows".into())
    }
}

/// 调主显示器亮度（EXT-22，Windows DDC/CI）。成功返回可展示的中文结果。
pub fn adjust_brightness(op: BrightnessOp, step: i64) -> Result<String, String> {
    #[cfg(windows)]
    {
        win_capture::brightness(op, step)
    }
    #[cfg(not(windows))]
    {
        let _ = (op, step);
        Err("「显示器亮度」目前只支持 Windows（DDC/CI）".into())
    }
}

/// 「后台发送」的内容（[`send_to_window`]，EXT-16 二阶段）。
pub enum WindowInput<'a> {
    /// 按键组合：按顺序按下、逆序抬起。
    Keys(&'a [kada_core::Key]),
    /// 原样文本。
    Text(&'a str),
}

/// 把按键 / 文本发给一个**非前台**窗口，全程不激活、不抢焦点（EXT-16 二阶段）。
///
/// `target` 匹配进程名或窗口标题（忽略大小写，含 `*`/`?` 走通配、否则子串；多个命中取
/// Z 序最上层）。**只有 Windows 实现**（`PostMessage`），macOS / Linux 返回明确错误，
/// 由冲突检测同时标平台缺失。目标恰好是当前前台窗口时直接报错——前台输入只走既有
/// `simulate` 那条路，不给它开第二套注入。
///
/// 已知不生效：Chromium / Electron / UWP / 游戏（靠 Raw Input / 焦点 / IME）、提权窗口
/// （UIPI）、依赖真实修饰键状态的组合键。`PostMessage` 没有回执，返回成功只代表消息进了
/// 目标队列，不代表程序处理了。
pub fn send_to_window(target: &str, input: WindowInput<'_>) -> Result<(), String> {
    #[cfg(windows)]
    {
        win::send_to_window(target, input)
    }
    #[cfg(not(windows))]
    {
        let _ = (target, input);
        Err("「后台发送」目前只支持 Windows".into())
    }
}


/// 提权伴随进程（规划 7.2-㉑a / `docs/底层输入能力规划.md` 档位 B）：补 UIPI 挡住的
/// 「往提权窗口注入」。
///
/// 只有 Windows 有这套东西（提权辅助进程 + 命名管道，见 `win::agent`）；其余平台是**同一个
/// 接口的空实现**，壳层因此不必按平台分叉——`supported()` 恒 false，界面据此说「本平台不
/// 支持」而不是「未启用」。
///
/// 五条不变量（见该文档 §1）在这个模块里的落点：开关默认关（`Settings.elevated_inject`）、
/// 只在设置页点「启动」时才弹 UAC、提权进程「客户端断开 / 父进程退出」即退（不留常驻与
/// 孤儿）、通道失败一律回落本地注入、失败必留一条可见提示（[`elevated::take_notice`]）。
pub mod elevated {
    /// 拉起提权进程后最多等它把通道建起来多久（UAC 弹窗要人点，给足时间）。
    /// 非 Windows 平台没有 [`start`] 的实现，这个值也随之闲置（挂 cfg 免掉一条死代码警告）。
    #[cfg(windows)]
    use std::time::Duration;

    /// 代理模式的命令行标志：`kada.exe --kada-agent <父进程 pid>`（壳层 `main` 据此分派）。
    pub const AGENT_ARG: &str = "--kada-agent";

    /// 拉起提权进程后最多等它把通道建起来多久（UAC 弹窗要人点，给足时间）。
    #[cfg(windows)]
    const START_WAIT: Duration = Duration::from_secs(30);

    /// 本平台有没有这套能力（只有 Windows 有）。
    pub fn supported() -> bool {
        cfg!(windows)
    }

    /// 通道现状（设置页卡片显示它）。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Status {
        /// 本平台是否支持。
        pub supported: bool,
        /// 开关（`settings.elevated_inject`）是否打开。
        pub enabled: bool,
        /// 提权进程是否已连上。
        pub connected: bool,
    }

    /// 「需要提权、但通道没用上」的原因。平台侧只报 id、不出文案，措辞交给壳层。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum NoticeReason {
        /// 开关没开。
        Disabled,
        /// 开关开着，但提权进程没在跑。
        NotRunning,
        /// 通道在，但这一批没送成。
        Failed,
    }

    /// 一次「该走提权通道却没走成」的记录（壳层取走它推消息中心）。
    #[derive(Clone, Debug)]
    pub struct Notice {
        /// 前台那个提权窗口的进程名（如 `notepad.exe`）；取不到时为空串。
        pub app: String,
        /// 为什么没用上。
        pub reason: NoticeReason,
    }

    /// 通道现状。
    pub fn status() -> Status {
        #[cfg(windows)]
        {
            let s = super::win::agent::status();
            Status { supported: true, enabled: s.enabled, connected: s.connected }
        }
        #[cfg(not(windows))]
        {
            Status { supported: false, enabled: false, connected: false }
        }
    }

    /// 配置里这个开关现在是什么（壳层每次配置变动都同步一次）。关掉时顺带断开通道——
    /// 提权进程随之退出。
    pub fn set_enabled(on: bool) {
        #[cfg(windows)]
        super::win::agent::set_enabled(on);
        #[cfg(not(windows))]
        let _ = on;
    }

    /// 拉起提权进程并连上通道（**唯一会弹 UAC 的入口**，只由设置页的动作调用）。
    pub fn start() -> Result<String, String> {
        #[cfg(windows)]
        {
            super::win::agent::start(START_WAIT).map_err(|e| e.to_string())
        }
        #[cfg(not(windows))]
        {
            Err("本平台暂不支持提权窗口注入".into())
        }
    }

    /// 断开通道（提权进程随即自行退出）。
    pub fn stop() {
        #[cfg(windows)]
        super::win::agent::stop();
    }

    /// 取走一条待报提示（壳层定时器线程每拍看一眼）。
    pub fn take_notice() -> Option<Notice> {
        #[cfg(windows)]
        {
            super::win::agent::take_notice().map(|n| Notice {
                app: n.app,
                reason: match n.reason {
                    super::win::agent::Reason::Disabled => NoticeReason::Disabled,
                    super::win::agent::Reason::NotRunning => NoticeReason::NotRunning,
                    super::win::agent::Reason::Failed => NoticeReason::Failed,
                },
            })
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// 以代理模式运行（`main` 在 `--kada-agent` 时调用）并返回进程退出码。
    ///
    /// 它**不碰** Tauri 壳、配置、窗口：提权进程的能力面只到这里为止。参数是拉起自己的
    /// Kada 进程 pid（`None` = 手工运行，不守望父进程）。
    pub fn serve(parent: Option<u32>) -> i32 {
        #[cfg(windows)]
        {
            let name = match super::win::agent::default_pipe_name() {
                Ok(name) => name,
                Err(e) => {
                    eprintln!("kada-agent: 取通道名失败：{e}");
                    return 3;
                }
            };
            match super::win::agent::serve(&name, parent) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("kada-agent: {e}");
                    3
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = parent;
            eprintln!("kada-agent: 本平台不支持提权伴随进程");
            3
        }
    }
}

/// 罗技 HID++ 客户端（`docs/底层输入能力规划.md` 档位 C）：把被厂商代理 divert 走的鼠标
/// 侧键**交回标准 HID 报文**，让现成的钩子链直接看见它。
///
/// 被 divert 的成因见 [`kada_core::DIVERTERS`] 与 `docs/需求设计说明书.md` §4.6；只有
/// Windows 有实现（共享打开厂商控制接口，见 `win::hidpp`），其余平台是空实现，界面据此说
/// 「本平台不支持」而不是「没找到设备」。两个入口：
/// - [`hidpp::status`]：只读探测（设备 / 控件 / divert 状态），**不写设备**；
/// - [`hidpp::unlock`]：把中键 / 后退 / 前进三个控件里处于 divert 的关掉，成功后后台保持
///   （设备重连或代理重启把 divert 设回时自动再关）。
///
/// 五条边界（与档位 C 的取舍一致）：只关 divert、**不接管事件流**（Kada 退出后按键仍是
/// 原生行为）；只动 Kada 键模型里的三个鼠标键（手势键等不动，免得破坏厂商软件的设置）；
/// 只管罗技自家设备；通道打不开如实报错、退化为档位 A 的文字指引；保持只在本次会话内有效。
pub mod hidpp {
    /// 一个鼠标控件的当前状态。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct ControlStatus {
        /// 控件 ID（HID++ CID）。
        pub cid: u16,
        /// 展示名（中键 / 后退侧键 / 前进侧键）。
        pub label: &'static str,
        /// 设备上是否有这个控件（读 reporting 成功即视为有）。
        pub present: bool,
        /// 是否正被 divert（厂商软件拿走了它）。
        pub diverted: bool,
        /// 是否「持久 divert」（重启 / 重连后仍然生效）。
        pub persistent: bool,
    }

    /// 一台设备的只读状态。
    #[derive(Clone, Debug)]
    pub struct DeviceStatus {
        /// 展示名（产品名 + 槽位）。
        pub name: String,
        pub product_id: u16,
        /// HID++ 设备索引（`0xFF` = 直连）。
        pub index: u8,
        /// 控件表里登记了多少个控件（读不到时为 0）。
        pub control_count: u8,
        pub controls: Vec<ControlStatus>,
        /// 走的是长报文还是短报文通道（诊断用）。
        pub channel: &'static str,
    }

    /// 只读探测结果。
    #[derive(Clone, Debug, Default)]
    pub struct Status {
        /// 本平台是否实现了这套能力（非 Windows 恒 false）。
        pub supported: bool,
        /// 是否正在后台保持解锁。
        pub keeper_active: bool,
        /// 保持解锁的最近一次错误（`None` = 正常）。
        pub keeper_error: Option<String>,
        /// 打不开通道之类的整体错误（例如被独占）。
        pub error: Option<String>,
        pub devices: Vec<DeviceStatus>,
    }

    /// 一台设备的解锁结果。
    #[derive(Clone, Debug, Default)]
    pub struct UnlockDevice {
        pub name: String,
        /// 这次真正解除了 divert 的键名。
        pub cleared: Vec<&'static str>,
        /// 本来就是标准报文（无需处理）。
        pub already: Vec<&'static str>,
        /// 设备上没有这个控件。
        pub missing: Vec<&'static str>,
        /// 读 / 写失败的原因（逐键）。
        pub failed: Vec<String>,
    }

    /// 解锁结果（`error` 非空 = 整个通道都没打通）。
    #[derive(Clone, Debug, Default)]
    pub struct UnlockReport {
        pub devices: Vec<UnlockDevice>,
        pub error: Option<String>,
    }

    /// 本平台是否实现了这套能力（只有 Windows 有）。
    pub fn supported() -> bool {
        cfg!(windows)
    }

    /// 只读探测：列出本机罗技设备与三个鼠标键的 divert 状态。**不写设备**。
    pub fn status() -> Status {
        #[cfg(windows)]
        {
            super::win::hidpp::status()
        }
        #[cfg(not(windows))]
        {
            Status::default()
        }
    }

    /// 解锁：把处于 divert 的鼠标键交回标准报文，并在成功后开始后台保持。
    pub fn unlock() -> UnlockReport {
        #[cfg(windows)]
        {
            super::win::hidpp::unlock()
        }
        #[cfg(not(windows))]
        {
            UnlockReport {
                error: Some("本平台暂不支持罗技 HID++ 解锁（Windows 专属能力）".into()),
                ..UnlockReport::default()
            }
        }
    }
}

/// 鼠标录制事件（鼠标操作录制的输入源，见壳体 `MouseRecorder`）：一次触发（点击 / 滚轮），
/// 附「自上一次触发以来的光标位移」。
///
/// 位移口径：Windows / macOS 取两次触发时光标位置之差（钩子事件自带坐标）；Linux 拿不到
/// 光标绝对位置，取两次触发之间累积的相对位移。三者对「移动到某处再触发」的语义一致，
/// 故壳层统一按增量处理。
#[derive(Clone, Copy, Debug)]
pub struct MouseRecord {
    /// 触发类型。
    pub kind: MouseRecordKind,
    /// 自上次触发以来的横向位移（像素）。
    pub move_dx: i32,
    /// 自上次触发以来的纵向位移（像素）。
    pub move_dy: i32,
}

/// 一次鼠标触发的类型。
#[derive(Clone, Copy, Debug)]
pub enum MouseRecordKind {
    /// 按下某个鼠标键（左/右/中/侧键，在按下那一刻记录）。
    Click(MouseButton),
    /// 滚轮滚动（`dy>0` 上滚 / `dy<0` 下滚，`dx` 横向，单位「格」）。
    Scroll { dx: i32, dy: i32 },
}