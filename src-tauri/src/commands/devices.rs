// 平台 / 设备诊断命令（T-04 从 lib.rs 搬出）：冲突列表、输入环境探测、输入设备、HID++、提权通道状态。
// 都只回答「系统现在是什么情况」，不做输入判定（见 说明书 §3 / §4.6 / §4.8）。

use serde::Serialize;

use crate::*;

/// 提权窗口注入的现状（设置页卡片的数据源，规划 7.2-㉑a）。
#[derive(Serialize)]
pub(crate) struct ElevatedStatus {
    /// 本平台有没有这套能力（只有 Windows 有）。
    supported: bool,
    /// 开关（`settings.elevated_inject`）是否打开。
    enabled: bool,
    /// 提权进程是否已连上。
    connected: bool,
}

/// 提权窗口注入的现状。
#[tauri::command]
pub(crate) fn elevated_status() -> ElevatedStatus {
    let s = input::elevated::status();
    ElevatedStatus {
        supported: s.supported,
        enabled: s.enabled,
        connected: s.connected,
    }
}

/// 拉起提权进程并连上通道（**唯一会弹 UAC 的地方**，只由设置页的按钮触发；连接有超时，
/// 用户不点 UAC 也不会把界面挂住）。
#[tauri::command]
pub(crate) fn elevated_start() -> Result<String, String> {
    input::elevated::start()
}

/// 断开提权通道（提权进程随之退出；开关状态不动，下次点「启动」或重启应用即可再来）。
#[tauri::command]
pub(crate) fn elevated_stop() -> Result<(), String> {
    input::elevated::stop();
    Ok(())
}

/// 计算给定配置的冲突列表（前端把当前正在编辑的配置传入，实时展示警告）。
/// 四部分：① 内部冲突（重复触发键/改键遮蔽/超集重叠）；② 系统快捷键清单命中；
/// ③ Windows 下 RegisterHotKey 探测「其他应用/系统已注册」的真实占用；
/// ④ 平台能力缺失（`PlatformCaps`：本平台监听不到的触发键、注入不了的目标键、
///   恒不成立的前台条件——从 Windows 同步过来的配置在 Linux/macOS 上会静默失效，
///   在这里点出来，别让用户自己猜，见规划 7.3-⑲）；
/// ⑤ 输入层环境（`diagnose_input_env`，规划 7.2-㊽：探测到会 divert 按键的厂商代理且
///   配置用到了鼠标键时给一条「为什么按了没反应」——与 ④ 同一口径，归在同一组里）。
#[tauri::command]
pub(crate) fn get_conflicts(config: Config) -> Vec<Conflict> {
    let mut out = detect_conflicts(&config, &platform_caps());

    // ⑤ 输入层环境诊断（可关：设置页「识别厂商接管的按键」）。关掉后探测整条不跑，
    //    不只是不显示——这类提示的误报余地在那儿摆着，用户关它就该是真静默。
    if config.settings.input_env_diag {
        out.extend(diagnose_input_env(&config, &input_env_cached()));
    }

    // ② 系统快捷键清单（跨平台）：精确匹配；命中的触发键记下，供③跳过避免重复提示。
    let mut sys_hits: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in &config.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            let Ok(sc) = t.parse::<Shortcut>() else {
                continue;
            };
            for (combo, desc) in SYSTEM_SHORTCUTS {
                if let Ok(sys) = combo.parse::<Shortcut>() {
                    if sys == sc {
                        out.push(Conflict {
                            severity: Severity::Warn,
                            platform: false,
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
                        platform: false,
                        message: format!("「{t}」已被系统或其他应用占用"),
                        name: s.name.clone().unwrap_or_default(),
                    });
                }
            }
        }
    }

    out
}

/// 输入层环境诊断的明细报告（设置页「输入层环境」卡片 / 帮助页用，规划 7.2-㊽）。
///
/// 与 [`get_conflicts`] 第⑤段的区别是**口径**：那边只在「探测到接管者**且**配置用到鼠标键」
/// 时才出声（避免噪音），这边是用户主动来看的，所以把探测到的接管者、以及「有没有扫过」
/// 一并交代清楚——包括「本平台还没做」和「查过了没问题」这两种截然不同的结论。
#[derive(Serialize)]
pub(crate) struct InputEnvReport {
    /// 本平台**是否实现了**探测。false = 「这个平台没做」，不是「清理干净了」。
    supported: bool,
    /// 此次探测的结论（接管者）。
    diverters: Vec<InputEnvHit>,
    /// 扫过的接管者类别数（让「未检测到」有分量：不是没查，是查了 N 类都没中）。
    checked: usize,
    /// Windows 内存完整性（HVCI）：`Some(true/false)` = 已开启/已关闭；`None` = 读不到。
    hvci: Option<bool>,
    /// 本平台是否支持「一键解锁罗技侧键」（档位 C）。只在 Windows 为 true；前端据此
    /// 决定要不要显示那一组按钮（**不在这里做 HID 枚举**——那要几百毫秒，不能拖慢卡片）。
    hidpp_supported: bool,
}

/// 一个命中的接管者（展示名 + 处置办法都从 [`kada_core::DIVERTERS`] 来，前端不再抄一份）。
#[derive(Serialize)]
struct InputEnvHit {
    id: String,
    name: String,
    advice: String,
    /// 用户对这个接管者点过「忽略」：只影响冲突列表的提示，卡片里仍列出来（可恢复）。
    ignored: bool,
}

/// 「设备是」条件可选的输入设备（设置页下拉）。
///
/// 之前只能手写标识：Windows 的标识是一整条 `\\?\HID#VID_…&PID_…&MI_00#7&b3c8a2e&0&0000
/// #{…}`，抄不出来也记不住。这里把本机输入设备（**键盘 / 鼠标 / 触摸板 / 数位板**）列出来，
/// 选一台就填进配置——**存的仍是模式**（[`kada_core::device_pattern`] 压出的稳定片段），
/// 手写通配照样可用。
#[derive(Serialize)]
pub(crate) struct InputDeviceList {
    /// 本平台能不能认设备（macOS 恒 false：`CGEventTap` 不带设备信息，见 7.2-㊷）。
    supported: bool,
    devices: Vec<DeviceChoice>,
}

/// 下拉里的一台设备。
#[derive(Serialize)]
pub(crate) struct DeviceChoice {
    /// 存进配置的值（稳定片段：`VID_046D&PID_C52B` / `ACPI#MSI0007` / evdev 名字）。
    pub(crate) id: String,
    /// 下拉里显示的文字。
    pub(crate) label: String,
    /// 完整设备路径（悬停提示，手写通配时可以直接抄）。
    pub(crate) detail: String,
    /// 大类（`keyboard` / `mouse` / `touchpad` / `tablet` / `other`）：界面据此分组。
    pub(crate) kind: &'static str,
}

/// 本机输入设备（设置页「设备是」下拉的数据源）。
#[tauri::command]
pub(crate) fn input_devices() -> InputDeviceList {
    InputDeviceList {
        supported: platform_caps().device_conditions,
        devices: device_choices(input::input_devices()),
    }
}

/// 设备路径 / evdev 名字 → 下拉项。
///
/// 值取 [`kada_core::device_pattern`] 压出的稳定片段（实例号换口就变，不能存）。两种「撞值」
/// 分开处理：
/// - **同一块硬件的多个口**（触摸板 / 数位板同时暴露「触摸口」与「鼠标口」，片段相同）：
///   鼠标口并进更具体的那条（[`kada_core::DeviceKind::rank`]）。**只认同一块硬件**
///   （[`kada_core::device_hardware_key`]）——同一颗罗技接收器下的键盘与鼠标也常共用
///   VID/PID，靠片段相等就合并会把键盘从下拉里删掉；
/// - **同型号两台**（真·两台设备）：片段相同、硬件也不同，只能退回完整路径——否则选哪台
///   都命中两台，等于没得选。
pub(crate) fn device_choices(raw: Vec<kada_core::InputDevice>) -> Vec<DeviceChoice> {
    // 中间形态：类别留着数值排序，最后才转成给前端的稳定 id。
    struct Pending {
        id: String,
        detail: String,
        hardware: String,
        kind: kada_core::DeviceKind,
    }
    let mut items: Vec<Pending> = raw
        .into_iter()
        .map(|d| {
            let stable = kada_core::device_pattern(&d.name);
            let id = if stable.is_empty() {
                d.name.clone()
            } else {
                stable
            };
            let hardware = kada_core::device_hardware_key(&d.name);
            Pending {
                id,
                detail: d.name,
                hardware,
                kind: d.kind,
            }
        })
        .collect();
    // 同一块硬件里，触摸板 / 数位板的鼠标口让位给更具体的那条（否则下拉多一条同名同值的
    // 「鼠标」，选它反而匹配不到笔迹 / 触摸事件）。
    let surface: std::collections::HashSet<(String, String)> = items
        .iter()
        .filter(|it| {
            matches!(
                it.kind,
                kada_core::DeviceKind::Touchpad | kada_core::DeviceKind::Tablet
            )
        })
        .map(|it| (it.hardware.clone(), it.id.clone()))
        .collect();
    items.retain(|it| {
        !(it.kind == kada_core::DeviceKind::Mouse
            && surface.contains(&(it.hardware.clone(), it.id.clone())))
    });
    // 同一块硬件上压出同一个值、同一类别的多条（触摸板的 Col01 / Col02 两个集合）只留一条：
    // 存的模式本来就会同时命中它们，列两条只是让用户挑花眼。
    let mut seen: std::collections::HashSet<(String, String, &'static str)> =
        std::collections::HashSet::new();
    items.retain(|it| seen.insert((it.hardware.clone(), it.id.clone(), it.kind.id())));
    // 剩下的同值项就是真·同型号两台：退回完整路径。
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for it in &items {
        *counts.entry(it.id.clone()).or_default() += 1;
    }
    let mut out: Vec<DeviceChoice> = items
        .into_iter()
        .map(|it| {
            let id = if counts.get(&it.id).copied().unwrap_or(0) > 1 {
                it.detail
                    .split("#{")
                    .next()
                    .unwrap_or(&it.detail)
                    .to_string()
            } else {
                it.id
            };
            DeviceChoice {
                label: id.clone(),
                id,
                detail: it.detail,
                kind: it.kind.id(),
            }
        })
        .collect();
    out.sort_by(|a, b| {
        // 分组顺序：键盘 → 鼠标 → 触摸板 → 数位板 → 其它（与界面 optgroup 的出现顺序一致）。
        let order = |k: &str| match k {
            "keyboard" => 0,
            "mouse" => 1,
            "touchpad" => 2,
            "tablet" => 3,
            _ => 4,
        };
        order(a.kind)
            .cmp(&order(b.kind))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

#[tauri::command]
pub(crate) fn input_env_report(config: Config) -> InputEnvReport {
    let supported = input::input_env_supported();
    // 不开诊断就不探测：前端据此显示「已关闭」，也省下这次进程表扫描。
    let env = if supported && config.settings.input_env_diag {
        input_env_cached()
    } else {
        kada_core::InputEnv::default()
    };
    let diverters = kada_core::DIVERTERS
        .iter()
        .filter(|d| env.diverters.iter().any(|id| id == d.id))
        .map(|d| InputEnvHit {
            id: d.id.to_string(),
            name: d.name.to_string(),
            advice: d.advice.to_string(),
            ignored: config
                .settings
                .input_env_ignored
                .iter()
                .any(|id| id == d.id),
        })
        .collect();
    InputEnvReport {
        supported,
        diverters,
        checked: kada_core::DIVERTERS.len(),
        hvci: env.hvci,
        hidpp_supported: kada_hook::hidpp::supported(),
    }
}

/// 本机正在运行的外部工具（目录见 [`kada_core::EXTERNAL_TOOLS`]）。
/// 只做进程名探测：动作里先出现占位，执行会明确报「尚未接入」。
#[tauri::command]
pub(crate) fn running_tools() -> Vec<kada_core::ExternalToolInfo> {
    let ids = kada_hook::platform_running_tools();
    kada_core::EXTERNAL_TOOLS
        .iter()
        .filter(|t| ids.iter().any(|id| id == t.id))
        .cloned()
        .collect()
}

// ---- 罗技 HID++ 解锁（`docs/底层输入能力规划.md` 档位 C）----
//
// 比档位 A 更进一步：不只告诉用户「谁拿走了按键」，而是直接把 divert 关掉，让按键回到
// 标准报文本。只在 Windows 有实现（见 `kada_hook::hidpp`），其余平台返回 supported=false。

/// 只读探测视图（设置页「罗技设备」小字）。
#[derive(Serialize)]
pub(crate) struct HidppStatusView {
    supported: bool,
    /// 是否正在后台保持解锁（本次 Kada 运行期间）。
    keeper_active: bool,
    keeper_error: Option<String>,
    /// 通道整体打不开之类的原因。
    error: Option<String>,
    devices: Vec<HidppDeviceView>,
}

#[derive(Serialize)]
pub(crate) struct HidppDeviceView {
    name: String,
    product_id: u16,
    /// HID++ 设备索引（`255` = 直连）。
    index: u8,
    control_count: u8,
    channel: String,
    controls: Vec<HidppControlView>,
}

#[derive(Serialize)]
pub(crate) struct HidppControlView {
    cid: u16,
    label: String,
    present: bool,
    diverted: bool,
    persistent: bool,
}

/// 解锁结果视图（前端直接铺在卡片里）。
#[derive(Serialize)]
pub(crate) struct HidppUnlockView {
    error: Option<String>,
    devices: Vec<HidppUnlockDeviceView>,
    /// 是否开始后台保持（有键被真正改掉时才是 true）。
    keeping: bool,
}

#[derive(Serialize)]
pub(crate) struct HidppUnlockDeviceView {
    name: String,
    cleared: Vec<String>,
    already: Vec<String>,
    missing: Vec<String>,
    failed: Vec<String>,
}

fn hidpp_status_view() -> HidppStatusView {
    let s = kada_hook::hidpp::status();
    HidppStatusView {
        supported: s.supported,
        keeper_active: s.keeper_active,
        keeper_error: s.keeper_error,
        error: s.error,
        devices: s
            .devices
            .into_iter()
            .map(|d| HidppDeviceView {
                name: d.name,
                product_id: d.product_id,
                index: d.index,
                control_count: d.control_count,
                channel: d.channel.to_string(),
                controls: d
                    .controls
                    .into_iter()
                    .map(|c| HidppControlView {
                        cid: c.cid,
                        label: c.label.to_string(),
                        present: c.present,
                        diverted: c.diverted,
                        persistent: c.persistent,
                    })
                    .collect(),
            })
            .collect(),
    }
}

#[tauri::command]
pub(crate) fn hidpp_status() -> HidppStatusView {
    hidpp_status_view()
}

/// 把 divert 关掉，并把结论同时推进消息中心（用户点完按钮后多半会离开设置页）。
#[tauri::command]
pub(crate) fn hidpp_unlock(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
) -> HidppUnlockView {
    let r = kada_hook::hidpp::unlock();
    let keeping = r.devices.iter().any(|d| !d.cleared.is_empty());

    let mut lines: Vec<String> = Vec::new();
    for d in &r.devices {
        lines.push(format!("设备：{}", d.name));
        if !d.cleared.is_empty() {
            lines.push(format!("  已解锁：{}", d.cleared.join("、")));
        }
        if !d.already.is_empty() {
            lines.push(format!("  本来就是标准报文：{}", d.already.join("、")));
        }
        if !d.missing.is_empty() {
            lines.push(format!("  设备上没有这些控件：{}", d.missing.join("、")));
        }
        for f in &d.failed {
            lines.push(format!("  失败：{f}"));
        }
    }
    let (headline, detail) = match &r.error {
        Some(err) => (
            "罗技鼠标侧键解锁失败".to_string(),
            format!(
                "{err}\n\n控制接口打不开时，先在 Options+ / G HUB 里把该按键设为「无操作 / 默认功能」，或直接退出它。"
            ),
        ),
        None if keeping => (
            "已解锁罗技鼠标侧键".to_string(),
            format!(
                "{}\n\n本次 Kada 运行期间会保持（设备重连或厂商软件重启后会再关一次）。现在按侧键应能触发快捷键。",
                lines.join("\n")
            ),
        ),
        None => (
            "罗技鼠标侧键本来就是标准报文".to_string(),
            format!("{}\n\n不需要解锁：钩子已经能收到这些按键。", lines.join("\n")),
        ),
    };
    commit_result(
        &app,
        &state.results,
        &state.unread,
        CommandResult {
            kind: "hidpp".into(),
            label: "输入层".into(),
            trigger: "罗技 HID++".into(),
            name: String::new(),
            command: headline,
            stdout: detail,
            stderr: String::new(),
            exit_code: None,
            detached: false,
            log_path: None,
            stdout_file: None,
            stderr_file: None,
            // 不弹窗：用户就在设置页看着结果，弹窗只会多一步要关。
            show_output: false,
            time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            run_id: None,
        },
    );

    HidppUnlockView {
        error: r.error,
        keeping,
        devices: r
            .devices
            .into_iter()
            .map(|d| HidppUnlockDeviceView {
                name: d.name,
                cleared: d.cleared.into_iter().map(str::to_string).collect(),
                already: d.already.into_iter().map(str::to_string).collect(),
                missing: d.missing.into_iter().map(str::to_string).collect(),
                failed: d.failed,
            })
            .collect(),
    }
}
