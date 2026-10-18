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

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;

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