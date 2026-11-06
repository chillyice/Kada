// 同步存档 / 自启 / 输入环境探测（T-05 从 lib.rs 搬出）：都是「拿配置或平台问一句」的叶子函数。
// 同步存档与本地配置同一套原子写 + `.bak`；自启走 tauri-plugin-autostart（见 说明书 §5.6 / §5.1）。

use crate::*;

/// 同步存档目录里写死的文件名：换机器只要在设置里选同一个目录就能找到同一份配置。
pub(crate) const SYNC_FILE_NAME: &str = "kada-config.json";

/// 同步存档文件的完整路径；未配置（`None` / 空白）时返回 `None`。
pub(crate) fn sync_file(cfg: &Config) -> Option<PathBuf> {
    let dir = cfg.settings.sync_dir.as_deref()?.trim();
    (!dir.is_empty()).then(|| Path::new(dir).join(SYNC_FILE_NAME))
}

/// 把配置同步写一份到「同步存档」目录（OneDrive 等）；未配置则什么都不做。
///
/// 与本地配置同一套 `config_io::save`（原子写 + `.bak`）：同步目录里也留一份可回滚副本，
/// 半截 JSON 不会被同步客户端传到别处。**尽力而为**——失败只打日志、绝不拦本地保存
/// （与 [`sync_autostart`] 同一取舍：OneDrive 目录临时不在 / 被占用，都不该让「改个快捷键」
/// 失败）。需要明确反馈时走 `sync_now` 命令。
pub(crate) fn push_sync(cfg: &Config) {
    let Some(file) = sync_file(cfg) else { return };
    if let Err(e) = config_io::save(&file, cfg) {
        eprintln!("同步存档写入失败（{}）: {e}", file.display());
    }
}

/// 把「开机自启」设置同步到系统（Windows 写 HKCU\...\CurrentVersion\Run，无需管理员权限）。
pub(crate) fn sync_autostart(app: &tauri::AppHandle, enabled: bool) {
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
pub(crate) fn launched_by_autostart() -> bool {
    std::env::args().any(|a| a == "--autostart")
}

/// 本机输入环境探测（接管者 + HVCI）的短时缓存。
///
/// [`get_conflicts`] 在**每次编辑配置**时都会被调（实时显示警告），而探测要走一次
/// Toolhelp 快照 / 扫一遍 `/proc`——几百个进程，单次几毫秒，不该每敲一个字重来一遍。
/// 缓存 3 秒：够覆盖连续编辑，也能在「退出厂商软件后回到界面」时及时翻篇（界面刷新
/// 走 [`input_env_report`]，那里也读同一份缓存）。
///
/// 与看门狗 / 定时器线程无锁关系：这是独立的一把锁，不参与「配置读锁必须先于 Engine 锁」
/// 那个顺序约束（见 `spawn_engine_ticker`）。
pub(crate) fn input_env_cached() -> kada_core::InputEnv {
    use std::sync::OnceLock;

    const TTL: Duration = Duration::from_secs(3);
    static CACHE: OnceLock<Mutex<Option<(Instant, kada_core::InputEnv)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    // 中毒（某次探测 panic）不该让诊断功能连带失效：拿回内层值继续用。返回的是**拷贝**，
    // 不把锁带出去（探测结果很小：几个 id + 一个 `Option<bool>`）。
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, env)) = guard.as_ref() {
        if at.elapsed() < TTL {
            return env.clone();
        }
    }
    let env = input::input_env();
    *guard = Some((Instant::now(), env.clone()));
    env
}

/// 计算给定配置的冲突列表（前端把当前正在编辑的配置传入，实时展示警告）。
///
/// 各平台口径（与钩子层的实际能力一一对应，改钩子时同步这里）：
/// - **Windows**：键 / 前台 / 设备条件全支持——设备识别随 7.2-㊷ 落地（Raw Input 旁路，
///   见 `kada-hook::win::current_device`）。
/// - **Linux**：前台窗口取不到（X11 未接 / Wayland 无通用协议）→ 前台类条件恒不成立，
///   但设备条件可用（evdev 设备名）；键盘与鼠标键（中键/侧键）监听 + 注入都可用（7.3-㉔）。
/// - **macOS**：媒体键走独立的 `NX_SYSDEFINED` tap（7.1-㊹，监听与注入都可用）；
///   F21~F24 无对应键码；前台走 AX 可用；设备条件仍不可用（CGEventTap 不带设备信息，
///   见 7.2-㊷）。
pub(crate) fn platform_caps() -> kada_core::PlatformCaps {
    #[cfg(target_os = "linux")]
    {
        // 前台条件 false；设备条件可用（DeviceIs 在 Linux 真生效），故只覆盖前台。
        kada_core::PlatformCaps {
            frontmost_conditions: false,
            window_actions: false,
            window_wait: false,
            mouse_absolute: false,
            background_input: false,
            shell_actions: false,
            hot_corners: false,
            wheel_edges: false,
            brightness: false,
            ..Default::default()
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut keys = std::collections::BTreeSet::new();
        for k in [Key::F21, Key::F22, Key::F23, Key::F24] {
            keys.insert(k);
        }
        kada_core::PlatformCaps {
            listen_unsupported: keys.clone(),
            inject_unsupported: keys,
            frontmost_conditions: true,
            device_conditions: false,
            window_actions: false,
            window_wait: false,
            mouse_absolute: true,
            background_input: false,
            shell_actions: false,
            hot_corners: false,
            wheel_edges: false,
            brightness: false,
            ..Default::default()
        }
    }
    // Windows：键 / 前台 / 设备条件全支持——设备识别随 7.2-㊷ 落地（Raw Input 旁路，
    // 见 `kada-hook::win::current_device`）。
    #[cfg(target_os = "windows")]
    {
        kada_core::PlatformCaps::default()
    }
    // 其它无输入层平台：拿不到设备 → 设备条件恒不成立。
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        kada_core::PlatformCaps {
            device_conditions: false,
            window_actions: false,
            window_wait: false,
            background_input: false,
            shell_actions: false,
            hot_corners: false,
            wheel_edges: false,
            touchpad_gestures: false,
            brightness: false,
            ..Default::default()
        }
    }
}
