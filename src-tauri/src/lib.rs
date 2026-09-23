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
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WindowEvent};

use kada_core::{
    detect_conflicts, is_hotstring_terminator, key_to_char, match_expansion, matches,
    sanitize_config, Action, ChordAdvance, ChordTracker, Config, Conflict, Key, Modifier, RawEvent,
    Remap, SeqAdvance, SequenceTracker, Severity, Shortcut, TextExpansion, Trigger, Vars,
    DEFAULT_SEQUENCE_TIMEOUT_MS, SYSTEM_SHORTCUTS,
};
use kada_actions::{run_actions, CommandResult};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

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

/// 层语义匹配普通改键（非 tap-hold）：激活层条目优先、基层层条目兜底。
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

/// 层语义匹配快捷键：激活层条目优先、基层层条目兜底。返回 (动作, 触发键, 名称)。
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

/// tap-hold 改键的待定状态：按下 `from` 键后，等待判定「短按（tap）/ 长按（hold）/
/// 单次（oneshot）/ 粘滞（sticky）」。字段由命中规则 `Remap` 解析而来。
#[cfg(any(target_os = "windows", target_os = "linux"))]
struct TapHoldPending {
    from: Key,
    tap: Option<Key>,
    hold: Option<Key>,
    /// 双击输出键（tap-dance）。
    tap2: Option<Key>,
    /// 三击输出键（tap-dance）。
    tap3: Option<Key>,
    /// 单次修饰键（单击武装、下一个非修饰键后释放）。
    oneshot: Option<Key>,
    /// 粘滞修饰键（单击锁定、再击解锁）。
    sticky: Option<Key>,
    /// 长按进入的层（momentary 切层）；与 `hold`（输出键）互斥。
    hold_layer: Option<String>,
    timeout: Duration,
    down_at: Instant,
    hold_active: bool,
}

/// tap-dance（连击）等待态：短按释放后不立即输出，等待后续连击或超时。
#[cfg(any(target_os = "windows", target_os = "linux"))]
struct TapDanceState {
    from: Key,
    /// 已累计击数（1..=3）。
    tap_count: u8,
    /// 等待下一击的截止时刻（懒提交：下一次事件到来时判定过期）。
    deadline: Instant,
    timeout: Duration,
    /// 击数 1/2/3 对应的输出键（已按缺省回落），`None` = 该击数无输出。
    outputs: [Option<Key>; 3],
    /// 当前连击键是否仍「按下待抬起」（down 已吞、up 待吞）——按住期间不因超时提交。
    holding: bool,
}

/// 运行时注入的修饰键状态：分「长按 hold / 粘滞 / 单次」三类，释放时机各不相同，
/// 但都走同一「物理注入（simulate）+ 后续键 mods 补全」通道。
#[cfg(any(target_os = "windows", target_os = "linux"))]
struct ModsState {
    /// tap-hold `hold` 修饰键（`from` 松开时释放）。
    hold: BTreeSet<Modifier>,
    /// 粘滞修饰键（再次单击 `from` 时解锁）。
    sticky: BTreeSet<Modifier>,
    /// 单次修饰键（下一个非修饰键松开时释放）。
    oneshot: BTreeSet<Modifier>,
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
impl ModsState {
    fn new() -> Self {
        Self { hold: BTreeSet::new(), sticky: BTreeSet::new(), oneshot: BTreeSet::new() }
    }

    /// 全部当前注入的修饰键（供后续键的 mods 补全）。
    fn all(&self) -> impl Iterator<Item = Modifier> + '_ {
        self.hold.iter().chain(self.sticky.iter()).chain(self.oneshot.iter()).copied()
    }
}

/// `Key`（裸修饰键）→ `Modifier`。hold/oneshot/sticky 是修饰键时，后续键的 mods 要补上它。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn key_as_modifier(k: Key) -> Option<Modifier> {
    match k {
        Key::Control => Some(Modifier::Ctrl),
        Key::Alt => Some(Modifier::Alt),
        Key::Shift => Some(Modifier::Shift),
        Key::Meta => Some(Modifier::Meta),
        _ => None,
    }
}

/// `Modifier` → 裸修饰键 `Key`（释放 oneshot/sticky 时把集合里的修饰键映射回可注入的键）。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn modifier_as_key(m: Modifier) -> Key {
    match m {
        Modifier::Ctrl => Key::Control,
        Modifier::Alt => Key::Alt,
        Modifier::Shift => Key::Shift,
        Modifier::Meta => Key::Meta,
    }
}

/// tap-hold 状态机单步推进。
///
/// 返回 `None` 表示事件被吞掉（原键不泄给目标程序）；返回 `Some(ev)` 表示继续走
/// 普通 [`decide`]，其中 `ev.mods` 已并入当前注入的修饰键（hold ∪ sticky ∪ oneshot）。
/// 判定规则：
/// - 按下 `from` → 吞掉并进入待定；短按（阈值内松开）按模式输出 tap / 进入连击 /
///   武装 oneshot / 切换 sticky，长按（≥阈值或 roll）输出 hold。
/// - 待定期间按下其它键 → 立即判 hold（roll 判定，缩短等待）。
/// - 自动重复的 `from` down 被吞掉、不推进判定。
/// - 连击（tap-dance）等待窗内再次 down 累计击数，超时懒提交。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn taphold_step(
    ev: &Ev,
    cfg: &Config,
    pending: &mut Option<TapHoldPending>,
    tap_dance: &mut Option<TapDanceState>,
    mods: &mut ModsState,
    active_layer: &mut Option<String>,
) -> Option<Ev> {
    // 连击等待窗已过期且无按住中的连击键 → 懒提交（输出当前击数对应的键）。
    commit_expired_dance(tap_dance);

    match ev {
        Ev::Down { key, mods: ev_mods, repeat } => {
            // 连击等待中：同键 down → 累计击数并吞掉；异键 down → 提交连击后照常处理。
            if let Some(d) = tap_dance.as_ref() {
                if d.from == *key {
                    if !*repeat {
                        count_dance_tap(tap_dance);
                    }
                    return None;
                }
                commit_dance(tap_dance);
            }

            let is_pending_from = pending.as_ref().map(|p| p.from == *key).unwrap_or(false);
            if is_pending_from {
                return None; // 原键的重复 down：吞掉。
            }
            if pending.is_some() {
                activate_hold(pending, mods, active_layer); // 不同键 down → roll 判定 hold。
            }
            if pending.is_none() {
                if let Some(r) = find_taphold_rule(cfg, *key, active_layer.as_deref()) {
                    *pending = Some(make_pending(*key, r));
                    return None;
                }
            }
            let mut m = ev_mods.clone();
            m.extend(mods.all());
            Some(Ev::Down { key: *key, mods: m, repeat: *repeat })
        }
        Ev::Up { key } => {
            // 连击等待中的同键 up：吞掉并解除「按住中」。
            if let Some(d) = tap_dance.as_ref() {
                if d.from == *key {
                    release_dance_tap(tap_dance);
                    return None;
                }
            }
            // pending 同键 up：结束待定（tap/hold/oneshot 武装/sticky 切换/进入连击）。
            if pending.as_ref().map(|p| p.from == *key).unwrap_or(false) {
                finish_taphold(pending, tap_dance, mods, active_layer);
                return None;
            }
            // oneshot 消费：下一个非修饰键 up 时释放武装修饰。
            if !mods.oneshot.is_empty() && key_as_modifier(*key).is_none() {
                release_oneshot(mods);
            }
            Some(Ev::Up { key: *key })
        }
    }
}

/// 按层语义查找命中的 tap-hold/切层规则（激活层优先、基层层兜底）。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn find_taphold_rule<'a>(cfg: &'a Config, key: Key, active_layer: Option<&str>) -> Option<&'a Remap> {
    for r in &cfg.remaps {
        if !r.enabled || !r.needs_timing_state() || r.layer.as_deref() != active_layer {
            continue;
        }
        if r.from.parse::<Key>().ok() == Some(key) {
            return Some(r);
        }
    }
    if active_layer.is_some() {
        for r in &cfg.remaps {
            if !r.enabled || !r.needs_timing_state() || r.layer.is_some() {
                continue;
            }
            if r.from.parse::<Key>().ok() == Some(key) {
                return Some(r);
            }
        }
    }
    None
}

/// 由命中的 [`Remap`] 规则构造待定状态（各字段解析为 `Key`/层 id）。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn make_pending(key: Key, r: &Remap) -> TapHoldPending {
    TapHoldPending {
        from: key,
        tap: r.tap_key(),
        hold: r.hold_key(),
        tap2: r.tap2_key(),
        tap3: r.tap3_key(),
        oneshot: r.oneshot_key(),
        sticky: r.sticky_key(),
        hold_layer: r.hold_layer_id().map(String::from),
        timeout: Duration::from_millis(r.tap_timeout()),
        down_at: Instant::now(),
        hold_active: false,
    }
}

/// 判定 hold：注入 hold 键 down，或进入切层；hold 是修饰键时记入 `mods.hold`。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn activate_hold(
    pending: &mut Option<TapHoldPending>,
    mods: &mut ModsState,
    active_layer: &mut Option<String>,
) {
    let Some(p) = pending.as_mut() else { return };
    if p.hold_active {
        return;
    }
    p.hold_active = true;
    if let Some(hk) = p.hold {
        input::simulate::down(hk);
        if let Some(md) = key_as_modifier(hk) {
            mods.hold.insert(md);
        }
    } else if let Some(layer) = &p.hold_layer {
        *active_layer = Some(layer.clone());
    } else if let Some(ok) = p.oneshot {
        // oneshot 的 roll：按住 `from` 期间当普通 hold 修饰按下（松开 `from` 时释放）。
        input::simulate::down(ok);
        if let Some(md) = key_as_modifier(ok) {
            mods.hold.insert(md);
        }
    }
}

/// 结束待定：hold 已激活则释放 hold 键 / 退出切层；否则按时长判定 tap/hold，或按模式
/// 分发到「短按输出 / 进入连击 / 武装 oneshot / 切换 sticky」。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn finish_taphold(
    pending: &mut Option<TapHoldPending>,
    tap_dance: &mut Option<TapDanceState>,
    mods: &mut ModsState,
    active_layer: &mut Option<String>,
) {
    let Some(p) = pending.take() else { return };
    if p.hold_active {
        if let Some(hk) = p.hold {
            input::simulate::up(hk);
            if let Some(md) = key_as_modifier(hk) {
                mods.hold.remove(&md);
            }
        } else if p.hold_layer.is_some() {
            *active_layer = None; // 松开切层键 → 退回基层层。
        } else if let Some(ok) = p.oneshot {
            input::simulate::up(ok); // oneshot roll 的 hold：松开 `from` 释放。
            if let Some(md) = key_as_modifier(ok) {
                mods.hold.remove(&md);
            }
        }
    } else if p.down_at.elapsed() >= p.timeout {
        if let Some(hk) = p.hold {
            input::simulate::tap(hk); // 长按后松开：hold 键短促输出一次。
        }
        // 切层键长按后松开（未 roll）：短暂进入又退出，无净效果，无需操作。
    } else if let Some(sk) = p.sticky {
        toggle_sticky(mods, sk); // 快速 tap：切换粘滞修饰。
    } else if let Some(ok) = p.oneshot {
        arm_oneshot(mods, ok); // 快速 tap：武装单次修饰。
    } else if p.tap2.is_some() || p.tap3.is_some() {
        // 快速 tap 且含双击/三击 → 进入连击等待（单/双/三击不同义）。
        *tap_dance = Some(TapDanceState {
            from: p.from,
            tap_count: 1,
            deadline: Instant::now() + p.timeout,
            timeout: p.timeout,
            outputs: [p.tap, p.tap2.or(p.tap), p.tap3.or(p.tap2).or(p.tap)],
            holding: false,
        });
    } else if let Some(tk) = p.tap {
        input::simulate::tap(tk); // 短按：tap 键。
    }
}

/// 连击等待窗已过期且无按住中的连击键 → 懒提交（输出当前击数对应的键）。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn commit_expired_dance(tap_dance: &mut Option<TapDanceState>) {
    let expired = tap_dance.as_ref().is_some_and(|d| !d.holding && Instant::now() >= d.deadline);
    if expired {
        commit_dance(tap_dance);
    }
}

/// 立即提交连击：输出当前击数对应的键并清空等待态。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn commit_dance(tap_dance: &mut Option<TapDanceState>) {
    let Some(d) = tap_dance.take() else { return };
    if let Some(k) = d.outputs[(d.tap_count - 1) as usize] {
        input::simulate::tap(k);
    }
}

/// 连击等待窗内再次按下同键：累计击数（≤3）、重置等待窗、标记「按住中」。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn count_dance_tap(tap_dance: &mut Option<TapDanceState>) {
    let Some(d) = tap_dance.as_mut() else { return };
    if d.tap_count < 3 {
        d.tap_count += 1;
    }
    d.deadline = Instant::now() + d.timeout;
    d.holding = true;
}

/// 连击键抬起：解除「按住中」（其 down 已被吞掉，up 一并吞掉）。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn release_dance_tap(tap_dance: &mut Option<TapDanceState>) {
    if let Some(d) = tap_dance.as_mut() {
        d.holding = false;
    }
}

/// 武装单次修饰：物理按下修饰键并记入 `mods.oneshot`，供后续键 mods 补全。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn arm_oneshot(mods: &mut ModsState, key: Key) {
    if let Some(m) = key_as_modifier(key) {
        input::simulate::down(key);
        mods.oneshot.insert(m);
    }
}

/// 切换粘滞修饰：锁定则物理按下并记入 `mods.sticky`，解锁则物理抬起并移除。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn toggle_sticky(mods: &mut ModsState, key: Key) {
    if let Some(m) = key_as_modifier(key) {
        if mods.sticky.contains(&m) {
            input::simulate::up(key);
            mods.sticky.remove(&m);
        } else {
            input::simulate::down(key);
            mods.sticky.insert(m);
        }
    }
}

/// 释放全部武装中的单次修饰（下一个非修饰键 up 时调用）。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn release_oneshot(mods: &mut ModsState) {
    let ones: Vec<Modifier> = mods.oneshot.iter().copied().collect();
    for m in ones {
        input::simulate::up(modifier_as_key(m));
    }
    mods.oneshot.clear();
}

/// 键序列（leader key）运行态：承载 [`SequenceTracker`] 与超时判据。
#[cfg(any(target_os = "windows", target_os = "linux"))]
struct SequenceState {
    tracker: SequenceTracker,
    last_activity: Instant,
    timeout: Duration,
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
impl SequenceState {
    fn new() -> Self {
        Self {
            tracker: SequenceTracker::new(),
            last_activity: Instant::now(),
            timeout: Duration::from_millis(DEFAULT_SEQUENCE_TIMEOUT_MS),
        }
    }
}

/// 和弦（同时按住多个键）运行态：承载 [`ChordTracker`] 与超时判据。
#[cfg(any(target_os = "windows", target_os = "linux"))]
struct ChordState {
    tracker: ChordTracker,
    last_activity: Instant,
    timeout: Duration,
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
impl ChordState {
    fn new() -> Self {
        Self {
            tracker: ChordTracker::new(),
            last_activity: Instant::now(),
            timeout: Duration::from_millis(DEFAULT_SEQUENCE_TIMEOUT_MS),
        }
    }
}

/// 收集「当前层生效」的键序列触发条目：(步骤, 动作, 触发键文本, 名称)。
/// 层语义与 [`match_shortcut`] 一致：激活层条目优先、基层层条目兜底。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn collect_sequence_items(
    cfg: &Config,
    active_layer: Option<&str>,
) -> Vec<(Vec<Shortcut>, Vec<Action>, String, String)> {
    let mut out: Vec<(Vec<Shortcut>, Vec<Action>, String, String)> = Vec::new();
    for s in &cfg.shortcuts {
        if !s.enabled || s.layer.as_deref() != active_layer {
            continue;
        }
        for t in &s.triggers {
            if let Ok(Trigger::Sequence(steps)) = Trigger::parse(t) {
                out.push((steps, s.actions.clone(), t.clone(), s.name.clone().unwrap_or_default()));
            }
        }
    }
    if active_layer.is_some() {
        for s in &cfg.shortcuts {
            if !s.enabled || s.layer.is_some() {
                continue;
            }
            for t in &s.triggers {
                if let Ok(Trigger::Sequence(steps)) = Trigger::parse(t) {
                    out.push((steps, s.actions.clone(), t.clone(), s.name.clone().unwrap_or_default()));
                }
            }
        }
    }
    out
}

/// 收集「当前层生效」的和弦触发条目：(成员键, 动作, 触发键文本, 名称)。
/// 层语义与 [`match_shortcut`] 一致：激活层条目优先、基层层条目兜底。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn collect_chord_items(
    cfg: &Config,
    active_layer: Option<&str>,
) -> Vec<(Vec<Shortcut>, Vec<Action>, String, String)> {
    let mut out: Vec<(Vec<Shortcut>, Vec<Action>, String, String)> = Vec::new();
    for s in &cfg.shortcuts {
        if !s.enabled || s.layer.as_deref() != active_layer {
            continue;
        }
        for t in &s.triggers {
            if let Ok(Trigger::Chord(members)) = Trigger::parse(t) {
                out.push((members, s.actions.clone(), t.clone(), s.name.clone().unwrap_or_default()));
            }
        }
    }
    if active_layer.is_some() {
        for s in &cfg.shortcuts {
            if !s.enabled || s.layer.is_some() {
                continue;
            }
            for t in &s.triggers {
                if let Ok(Trigger::Chord(members)) = Trigger::parse(t) {
                    out.push((members, s.actions.clone(), t.clone(), s.name.clone().unwrap_or_default()));
                }
            }
        }
    }
    out
}

/// 键序列状态机单步推进（在 tap-hold 之后、普通 [`decide`] 之前调用）。
///
/// 返回 `Some(ev)` 表示事件继续走普通 [`decide`]（断链的键仍可触发单组合）；
/// 返回 `None` 表示事件被吞掉（Block）：leader 已进入等待下一键，或序列已命中。
/// 只处理非重复 Down；`Escape` 取消进行中的序列并吞掉。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn sequence_step(
    app: &tauri::AppHandle,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    ev: &Ev,
    cfg: &Config,
    state: &mut SequenceState,
    active_layer: Option<&str>,
) -> Option<Ev> {
    let Ev::Down { key, mods, repeat } = ev else { return Some(ev.clone()) };
    if *repeat {
        return Some(ev.clone());
    }

    // 超时：过期则重置，事件照走。
    if state.tracker.is_active() && state.last_activity.elapsed() >= state.timeout {
        state.tracker.reset();
    }

    // Escape 取消进行中的序列并吞掉。
    if *key == Key::Escape && state.tracker.is_active() {
        state.tracker.reset();
        return None;
    }

    let raw = RawEvent { key: *key, mods: mods.clone(), pressed: true };
    let items = collect_sequence_items(cfg, active_layer);
    let steps: Vec<Vec<Shortcut>> = items.iter().map(|(s, ..)| s.clone()).collect();
    match state.tracker.advance(&raw, &steps) {
        SeqAdvance::NoMatch => Some(ev.clone()),
        SeqAdvance::Advance => {
            state.last_activity = Instant::now();
            None
        }
        SeqAdvance::Complete(i) => {
            state.tracker.reset();
            if let Some((_, actions, trigger, name)) = items.get(i) {
                fire(app.clone(), results, unread, actions.clone(), trigger.clone(), name.clone());
            }
            None
        }
    }
}

/// 和弦状态机单步推进（在 tap-hold 之后、键序列之前调用）。
///
/// 返回 `Some(ev)` 表示事件继续走键序列 / 普通 [`decide`]；返回 `None` 表示事件被吞掉
/// （Block）：成员键等待其它成员凑齐，或和弦已命中。只处理非重复 Down；
/// 成员键的 keyup 被钩子层一并吞掉，集合靠「凑齐触发」或「超时」清空，不依赖 Up。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn chord_step(
    app: &tauri::AppHandle,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    ev: &Ev,
    cfg: &Config,
    state: &mut ChordState,
    active_layer: Option<&str>,
) -> Option<Ev> {
    let Ev::Down { key, mods, repeat } = ev else { return Some(ev.clone()) };
    if *repeat {
        return Some(ev.clone());
    }

    // 超时：过期则重置（丢弃已按下的成员键），事件照走。
    if state.tracker.is_active() && state.last_activity.elapsed() >= state.timeout {
        state.tracker.reset();
    }

    let raw = RawEvent { key: *key, mods: mods.clone(), pressed: true };
    let items = collect_chord_items(cfg, active_layer);
    let chords: Vec<Vec<Shortcut>> = items.iter().map(|(c, ..)| c.clone()).collect();
    match state.tracker.advance(&raw, &chords) {
        ChordAdvance::NoMatch => Some(ev.clone()),
        ChordAdvance::Await => {
            state.last_activity = Instant::now();
            None
        }
        ChordAdvance::Complete(i) => {
            state.tracker.reset();
            if let Some((_, actions, trigger, name)) = items.get(i) {
                fire(app.clone(), results, unread, actions.clone(), trigger.clone(), name.clone());
            }
            None
        }
    }
}

/// 热串输入缓冲最大长度（触发词都很短，64 字符足够）。
const MAX_HOTSTRING_BUFFER: usize = 64;

/// 处理一个放行的事件用于文本扩展：累积可打印字符、命中触发词时异步回删并注入。
/// 返回 `true` 表示「事件被消费」（命中触发词的后缀键被吞掉，改由后台线程
/// 回删 + 注入 + 补回后缀，避免后缀先落盘与回删并发产生竞态/错位）。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn on_hotstring(ev: &Ev, buffer: &mut String, expansions: &[TextExpansion]) -> bool {
    let Ev::Down { key, mods, repeat } = ev else { return false };
    if *repeat {
        return false;
    }
    // 修饰键不打断缓冲（输入大写字母需要 Shift 按下）。
    if matches!(key, Key::Shift | Key::Control | Key::Alt | Key::Meta) {
        return false;
    }
    if is_hotstring_terminator(*key) {
        if let Some(exp) = match_expansion(buffer, expansions) {
            let backspaces = exp.trigger.chars().count();
            let replace = exp.replace.clone();
            let terminator = *key;
            std::thread::spawn(move || {
                for _ in 0..backspaces {
                    input::simulate::tap(Key::Backspace);
                }
                let expanded = resolve_hotstring(&replace);
                let _ = input::simulate::type_text(&expanded);
                // 后缀键（空格/回车/Tab）已被吞掉，这里补回，保证「sig␣ → signature␣」。
                input::simulate::tap(terminator);
            });
            buffer.clear();
            return true;
        }
        buffer.clear();
        return false;
    }
    if *key == Key::Backspace {
        buffer.pop();
        return false;
    }
    // 可打印字符累积；其余键（方向键/功能键等）打断缓冲。
    let shift = mods.contains(&Modifier::Shift);
    match key_to_char(*key, shift) {
        Some(c) => {
            buffer.push(c);
            if buffer.chars().count() > MAX_HOTSTRING_BUFFER {
                let skip = buffer.chars().count() - MAX_HOTSTRING_BUFFER;
                *buffer = buffer.chars().skip(skip).collect();
            }
        }
        None => buffer.clear(),
    }
    false
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

/// 快速唤醒：双击 `Settings::wake_key` 唤出主窗口。
///
/// 被动检测——只唤出窗口、不拦截按键：唤醒键（默认 Alt）常同时是修饰键，
/// 拦截会破坏 Alt+Tab / Alt+字母 等组合。两次按下（非自动重复）间隔 ≤400ms
/// 判为双击；其间按下其它键则取消上一次单击计数（视作组合键的一部分）。
fn detect_wake(
    cfg: &RwLock<Config>,
    last_tap: &mut Option<Instant>,
    app: &tauri::AppHandle,
    ev: &Ev,
) {
    let Ev::Down { key, repeat, .. } = ev else { return };
    if *repeat {
        return;
    }
    let guard = cfg.read().unwrap();
    let Some(wake) = guard.settings.wake_key.as_ref() else { return };
    let Ok(wk) = wake.parse::<Key>() else { return };
    if *key != wk {
        // 按下其它键 → 上一次唤醒键单击不算数（可能是组合键的一部分）。
        last_tap.take();
        return;
    }
    let now = Instant::now();
    match *last_tap {
        Some(prev) if now.duration_since(prev) <= Duration::from_millis(400) => {
            *last_tap = None;
            show_main_window(app);
        }
        _ => *last_tap = Some(now),
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
    let payload = ToastPayload { name: name.to_string(), trigger: trigger.to_string() };
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
        Err(e) => eprintln!("创建主窗口失败: {e}"),
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
}

fn load_config(file: &PathBuf) -> Config {
    match fs::read_to_string(file) {
        Ok(s) => match serde_json::from_str::<Config>(&s) {
            Ok(mut cfg) => {
                cfg.migrate();
                cfg
            }
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

/// 读取当前配置。
#[tauri::command]
fn get_config(state: tauri::State<'_, KadaState>) -> Config {
    state.config.read().unwrap().clone()
}

/// 保存配置：先逐条清洗（坏触发键/动作/改键被单独忽略，不影响其余配置），
/// 写入磁盘并即时生效。返回被忽略内容的说明（供 UI 提示），只有真正失败（写盘等）才报错。
#[tauri::command]
fn set_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
    config: Config,
) -> Result<Vec<String>, String> {
    let (clean, ignored) = sanitize_config(&config);
    let autostart = clean.settings.autostart;
    save_config(&state.file, &clean)?;
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
    save_config(&state.file, &clean)?;
    *state.config.write().unwrap() = clean;
    sync_autostart(&app, autostart);
    Ok(ignored)
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
                    let mut last_tap: Option<Instant> = None;
                    let mut hotstring_buffer = String::new();
                    let mut taphold: Option<TapHoldPending> = None;
                    let mut tap_dance: Option<TapDanceState> = None;
                    let mut mods_state = ModsState::new();
                    let mut active_layer: Option<String> = None;
                    let mut sequence_state = SequenceState::new();
                    let mut chord_state = ChordState::new();
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
                        detect_wake(&cfg, &mut last_tap, &app_handle, &ev);
                        let guard = cfg.read().unwrap();
                        if p.load(Ordering::Relaxed) || guard.settings.paused {
                            return input::HookAction::Allow;
                        }
                        // tap-hold 状态机（改键优先于快捷键）：被吞掉的键不进 decide。
                        let Some(ev) = taphold_step(
                            &ev,
                            &guard,
                            &mut taphold,
                            &mut tap_dance,
                            &mut mods_state,
                            &mut active_layer,
                        ) else {
                            return input::HookAction::Block;
                        };
                        // 和弦（同时按住多个键）：吞掉成员键，凑齐触发、超时丢弃。
                        let Some(ev) = chord_step(
                            &app_handle,
                            results.clone(),
                            unread.clone(),
                            &ev,
                            &guard,
                            &mut chord_state,
                            active_layer.as_deref(),
                        ) else {
                            return input::HookAction::Block;
                        };
                        // 键序列（leader key）：吞掉进入等待/命中的键，放行断链的键继续走 decide。
                        let Some(ev) = sequence_step(
                            &app_handle,
                            results.clone(),
                            unread.clone(),
                            &ev,
                            &guard,
                            &mut sequence_state,
                            active_layer.as_deref(),
                        ) else {
                            return input::HookAction::Block;
                        };
                        match decide(&ev, &guard, active_layer.as_deref()) {
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
                                // 命中文本扩展时后缀键（空格/回车/Tab）被吞掉，改由后台线程
                                // 回删 + 注入 + 补回后缀（避免后缀先落盘与回删并发产生错位）。
                                if on_hotstring(&ev, &mut hotstring_buffer, &guard.expansions) {
                                    input::HookAction::Block
                                } else {
                                    input::HookAction::Allow
                                }
                            }
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
                toast: Arc::new(Mutex::new(None)),
            };
            app.manage(state);

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
            let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;
            let tray_icon = TrayIconBuilder::new()
                .icon(base_icon.unwrap())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
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