// 冲突检测（C-04 从 lib.rs 搬出）：Severity / Conflict / PlatformCaps / 平台与输入环境诊断。
// 只负责「算出来给界面提示」，不拦保存（软冲突语义见 说明书 §3 / §4.6 / §4.8）。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::*;

/// 冲突严重程度。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warn,
}

/// 一条快捷键/改键冲突。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub severity: Severity,
    pub message: String,
    /// 涉及的主要快捷键名称（无名称为空字符串），供 UI「消息→冲突」页标识是哪个快捷键。
    #[serde(default)]
    pub name: String,
    /// 「平台能力缺失」类（`detect_conflicts` 第 6 段）：配置本身合法、只是当前平台
    /// 支持不了，仍是 [`Severity::Warn`] 不阻止保存；UI 据此单独分组展示，不与遮蔽
    /// 类软冲突混在一起。
    #[serde(default)]
    pub platform: bool,
}

/// 平台输入能力声明（冲突检测据此标注「当前平台用不了」的条目，见规划 7.3-⑲）。
///
/// 同一份配置可能在多平台间同步，而各平台能力不同：Linux 取不到前台窗口（前台类条件
/// 恒不成立）、macOS 绑不了媒体键与 F21~F24、鼠标键在部分平台监听/注入受限。这类
/// 「配置本身合法、只是当前平台不支持」按 [`Severity::Warn`] 提示，**不阻止保存**——
/// 同一份配置在别的平台可能是完全正常的。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformCaps {
    /// 监听不到的键：不能作触发键（组合/序列/和弦）与改键来源，配置了也不会触发。
    pub listen_unsupported: BTreeSet<Key>,
    /// 注入不了的键：不能作改键目标与 `Keys` 动作成员，执行到会静默无输出。
    pub inject_unsupported: BTreeSet<Key>,
    /// 「前台应用 / 窗口标题」类条件是否可用（不可用时恒不成立，`If` 恒走 `otherwise`）。
    pub frontmost_conditions: bool,
    /// 「设备是」类条件是否可用（不可用时恒不成立，`If` 恒走 `otherwise`）。目前仅 Linux
    /// evdev 能给出真实设备标识，Windows / macOS 拿不到设备。
    pub device_conditions: bool,
    /// 「当前窗口」动作（[`Action::Window`]）是否可用。目前只有 Windows 有前台窗口句柄。
    pub window_actions: bool,
    /// 「等待窗口出现」是否可用（[`WaitTarget::Window`]）。目前只有 Windows 实现了
    /// 顶层窗口枚举；其它平台执行到这一步会明确失败。
    pub window_wait: bool,
    /// 鼠标绝对移动（[`MouseOp::MoveTo`] / [`MouseOp::MoveToOrigin`]）是否可用。
    /// Linux 拿不到可靠的全局屏幕范围（尤其 Wayland），暂不支持。
    pub mouse_absolute: bool,
    /// 「后台发送」动作（[`Action::SendTo`]）是否可用。只有 Windows 用 `PostMessage` 实现。
    pub background_input: bool,
    /// Windows Shell 补全动作（定位文件 / 清空回收站 / 关闭显示器 / 切换桌面图标）是否可用。
    pub shell_actions: bool,
    /// 触发角（[`HotCorners`]）是否可用。目前只有 Windows 接了鼠标钩子。
    pub hot_corners: bool,
    /// 边缘滚动（[`WheelEdges`]）是否可用。目前只有 Windows 接了鼠标钩子。
    pub wheel_edges: bool,
    /// 触摸板手势（`TouchpadGesture`）是否可用。采集靠「系统合成的鼠标移动」，
    /// 目前只有 Windows 接了鼠标钩子；其它平台触发键照常输出、但不触发动作。
    pub touchpad_gestures: bool,
    /// 显示器亮度（[`Action::Brightness`]）是否可用。目前只有 Windows 走 DDC/CI。
    pub brightness: bool,
}

impl Default for PlatformCaps {
    /// 全支持（没有任何能力缺失的假想平台；单测「零误报」用它当基准）。
    fn default() -> Self {
        Self {
            listen_unsupported: BTreeSet::new(),
            inject_unsupported: BTreeSet::new(),
            frontmost_conditions: true,
            device_conditions: true,
            window_actions: true,
            window_wait: true,
            mouse_absolute: true,
            background_input: true,
            shell_actions: true,
            hot_corners: true,
            wheel_edges: true,
            touchpad_gestures: true,
            brightness: true,
        }
    }
}

impl PlatformCaps {
    /// 是否全支持：全支持时平台扫描整段跳过（零误报零开销）。
    fn is_full(&self) -> bool {
        self.listen_unsupported.is_empty()
            && self.inject_unsupported.is_empty()
            && self.frontmost_conditions
            && self.device_conditions
            && self.window_actions
            && self.window_wait
            && self.mouse_absolute
            && self.background_input
            && self.shell_actions
            && self.hot_corners
            && self.wheel_edges
            && self.touchpad_gestures
            && self.brightness
    }
}

/// 和弦成员的指纹：`(成员键, 修饰要求)` 表（[`chord_signature`] 排序后成表，无序可比）。
type ChordSignature = Vec<(Key, BTreeSet<Modifier>)>;

/// 一个和弦触发条目：(触发键文本, 成员, 快捷键名)。
type ChordEntry = (String, Vec<Shortcut>, String);

/// 和弦成员的指纹（用于判重 / 遮蔽判定）：`(成员键, 修饰要求)` 排序后成表，无序可比。
fn chord_signature(members: &[Shortcut]) -> ChordSignature {
    let mut sig: Vec<(Key, BTreeSet<Modifier>)> =
        members.iter().map(|m| (m.key, m.mods.clone())).collect();
    sig.sort();
    sig
}

/// 和弦的成员键集合（去重；判「子集」用）。
fn chord_keys(members: &[Shortcut]) -> BTreeSet<Key> {
    members.iter().map(|m| m.key).collect()
}

/// `a` 的要求是否被 `b` 全覆盖：`a` 的每个成员都能在 `b` 里找到「同一个成员键、修饰要求更宽」
/// （`a.mods ⊆ b.mods`）的成员。覆盖意味着 `b` 凑齐时 `a` 一定也凑齐了——两个完全相同的
/// 和弦不算（那是重复触发键，另有硬冲突）。
fn chord_covers(a: &[Shortcut], b: &[Shortcut]) -> bool {
    a.iter().all(|m| {
        b.iter()
            .any(|n| n.key == m.key && m.mods.is_subset(&n.mods))
    }) && chord_signature(a) != chord_signature(b)
}

/// 按「同层」分组收集所有启用的和弦触发键：(触发键文本, 成员, 快捷键名)。
///
/// 用 `Vec` 按配置顺序分组（不是 `HashMap`）：组内顺序就是运行时
/// `collect_chord_items` 的判定顺序（先判到的胜出），遮蔽判定依赖它。
fn chord_groups(cfg: &Config) -> Vec<(Option<String>, Vec<ChordEntry>)> {
    let mut groups: Vec<(Option<String>, Vec<ChordEntry>)> = Vec::new();
    for s in &cfg.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            let Ok(Trigger::Chord(members)) = Trigger::parse(t) else {
                continue;
            };
            let entry = (t.clone(), members, s.name.clone().unwrap_or_default());
            match groups.iter_mut().find(|(layer, _)| layer == &s.layer) {
                Some((_, items)) => items.push(entry),
                None => groups.push((s.layer.clone(), vec![entry])),
            }
        }
    }
    groups
}

/// 检测配置中的快捷键/改键冲突。
///
/// - 硬冲突（[`Severity::Error`]，应阻止保存）：重复触发键（组合/序列/和弦）、
///   序列 leader 遮蔽单组合、重复改键来源、改键来源与快捷键主键相同（改键优先，快捷键将失效）。
/// - 软冲突（[`Severity::Warn`]，仅提示）：触发键超集重叠（更宽松的组合会遮蔽更具体的组合）；
///   和弦之间「一个的要求被另一个全覆盖」（更宽松的那个会先凑齐 → 更严的永远轮不到）；
///   平台能力缺失（[`PlatformCaps`]：监听不到的触发键、注入不了的目标、恒不成立的前台条件）。
///
/// **层口径**：层语义是「激活层优先、基础层兜底」，所以「重复触发键 / 改键重复 / 超集重叠」
/// 都只在**同层内**比较；两个不同的非基础层永远不会同时生效，不构成冲突。唯一跨层比较的是
/// 「改键来源与快捷键主键相同」——基础层（`layer = None`）会兜进任何激活层，所以基础层条目与
/// 层内条目也会互相影响。
pub fn detect_conflicts(cfg: &Config, caps: &PlatformCaps) -> Vec<Conflict> {
    use std::collections::HashMap;

    let mut out: Vec<Conflict> = Vec::new();

    // 1) 重复触发键（同层内 enabled 且跨不同条目；不同层可共用同键）。
    //    单组合按 (层, mods, key) 判重；序列按 (层, 完整字符串) 判重；
    //    和弦按 (层, 排序后的成员（键 + 修饰要求）列表) 判重（F&J 与 J&F 等价，
    //    但 F&J 与 Ctrl+F&J 是两条不同的触发键）。
    let mut seen_combo: HashMap<(Option<String>, BTreeSet<Modifier>, Key, WhenKey), String> =
        HashMap::new();
    let mut seen_seq: HashMap<(Option<String>, String, WhenKey), ()> = HashMap::new();
    let mut seen_chord: HashMap<(Option<String>, ChordSignature, WhenKey), String> = HashMap::new();
    for s in &cfg.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            match Trigger::parse(t) {
                Ok(Trigger::Combo(sc)) => {
                    let k = (s.layer.clone(), sc.mods, sc.key, s.when_key());
                    if let Some(first) = seen_combo.get(&k) {
                        out.push(Conflict {
                            severity: Severity::Error,
                            platform: false,
                            message: format!(
                                "触发键「{t}」与「{first}」重复，多个快捷键共用同一组合"
                            ),
                            name: s.name.clone().unwrap_or_default(),
                        });
                    } else {
                        seen_combo.insert(k, t.clone());
                    }
                }
                Ok(Trigger::Sequence(_)) => {
                    if seen_seq
                        .insert((s.layer.clone(), t.clone(), s.when_key()), ())
                        .is_some()
                    {
                        out.push(Conflict {
                            severity: Severity::Error,
                            platform: false,
                            message: format!("触发序列「{t}」重复，多个快捷键共用同一序列"),
                            name: s.name.clone().unwrap_or_default(),
                        });
                    }
                }
                Ok(Trigger::Chord(members)) => {
                    let k = (s.layer.clone(), chord_signature(&members), s.when_key());
                    if let Some(first) = seen_chord.get(&k) {
                        out.push(Conflict {
                            severity: Severity::Error,
                            platform: false,
                            message: format!(
                                "和弦「{t}」与「{first}」重复，多个快捷键共用同一和弦"
                            ),
                            name: s.name.clone().unwrap_or_default(),
                        });
                    } else {
                        seen_chord.insert(k, t.clone());
                    }
                }
                Err(_) => {}
            }
        }
    }

    // 1b) 序列 leader / 和弦成员遮蔽单组合或序列（同层内）：二者都会吞掉成员键的
    //     down，使「主键 = 该成员键」的单组合或序列 leader 失效。
    let mut combos_by_layer: HashMap<Option<String>, Vec<(String, Shortcut)>> = HashMap::new();
    let mut seq_leaders_by_layer: HashMap<Option<String>, Vec<(String, Shortcut)>> = HashMap::new();
    for s in &cfg.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            match Trigger::parse(t) {
                Ok(Trigger::Combo(sc)) => {
                    combos_by_layer
                        .entry(s.layer.clone())
                        .or_default()
                        .push((t.clone(), sc));
                }
                Ok(Trigger::Sequence(steps)) => {
                    if let Some(leader) = steps.first() {
                        seq_leaders_by_layer
                            .entry(s.layer.clone())
                            .or_default()
                            .push((t.clone(), leader.clone()));
                    }
                }
                _ => {}
            }
        }
    }
    // 序列 leader 遮蔽单组合。
    for s in &cfg.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            if let Ok(Trigger::Sequence(steps)) = Trigger::parse(t) {
                let Some(leader) = steps.first() else {
                    continue;
                };
                if let Some(combos) = combos_by_layer.get(&s.layer) {
                    for (ct, csc) in combos {
                        if csc == leader {
                            out.push(Conflict {
                                severity: Severity::Error,
                                platform: false,
                                message: format!(
                                    "序列「{t}」的 leader「{0}」会吞掉该键，使组合键「{ct}」失效",
                                    format_shortcut(leader)
                                ),
                                name: s.name.clone().unwrap_or_default(),
                            });
                        }
                    }
                }
            }
        }
    }
    // 和弦成员遮蔽单组合 / 序列 leader。只算非修饰键成员：修饰键成员（`Ctrl+F` 里的 Ctrl、
    // `Ctrl+Alt&J` 里的 Ctrl/Alt）从不被吞（吞掉修饰键会把 Ctrl+Alt+Tab 变成 Ctrl+Tab），
    // 也就遮蔽不了以它为「主键」的组合。
    for s in &cfg.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            if let Ok(Trigger::Chord(members)) = Trigger::parse(t) {
                for m in members.iter().filter(|m| !m.key.is_modifier()) {
                    if let Some(combos) = combos_by_layer.get(&s.layer) {
                        for (ct, csc) in combos {
                            if csc.key == m.key {
                                out.push(Conflict {
                                    severity: Severity::Error,
                                    platform: false,
                                    message: format!(
                                        "和弦「{t}」的成员「{0}」会吞掉该键，使组合键「{ct}」失效",
                                        key_name(m.key)
                                    ),
                                    name: s.name.clone().unwrap_or_default(),
                                });
                            }
                        }
                    }
                    if let Some(leaders) = seq_leaders_by_layer.get(&s.layer) {
                        for (st, leader) in leaders {
                            if leader.key == m.key {
                                out.push(Conflict {
                                    severity: Severity::Error,
                                    platform: false,
                                    message: format!(
                                        "和弦「{t}」的成员「{0}」会吞掉该键，使序列「{st}」的 leader 失效",
                                        key_name(m.key)
                                    ),
                                    name: s.name.clone().unwrap_or_default(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    // 1c) 和弦之间的超集 / 子集软冲突（同层内）：一个和弦的要求被另一个全覆盖时，它必然
    //     先凑齐并触发，更严的那个**永远轮不到**（同「层可达性」，两边都是合法配置，
    //     只是有一个不生效，所以是 Warn 不是 Error）。
    //     两种「先凑齐」的来源：① 成员键更少 → 更早凑齐（与书写顺序无关）；② 成员键相同
    //     但修饰要求更宽 → 同一刻凑齐，靠列表靠前先被判到。故按下面两条各判一次。
    for (_layer, items) in chord_groups(cfg) {
        for i in 0..items.len() {
            for j in (i + 1)..items.len() {
                let (ti, ai, ni) = &items[i];
                let (tj, aj, nj) = &items[j];
                // 靠前的那条要求更宽 → 靠后那条永远轮不到。
                if chord_covers(ai, aj) {
                    out.push(Conflict {
                        severity: Severity::Warn,
                        platform: false,
                        message: format!(
                            "和弦「{tj}」永远触发不到：「{ti}」的要求更宽（成员更少或修饰要求更宽），凑齐时会先轮到它"
                        ),
                        name: nj.clone(),
                    });
                } else if chord_covers(aj, ai) && chord_keys(aj).len() < chord_keys(ai).len() {
                    // 靠后的那条成员严格更少 → 必然先凑齐，与书写顺序无关。
                    out.push(Conflict {
                        severity: Severity::Warn,
                        platform: false,
                        message: format!(
                            "和弦「{ti}」永远触发不到：「{tj}」的成员是它的子集，必然先凑齐并触发"
                        ),
                        name: ni.clone(),
                    });
                }
            }
        }
    }

    // 2) 重复改键来源（同层内；不同层可共用同键；同键不同 `when` 是两条独立条目）
    let mut remap_from: HashMap<(Option<String>, Key, WhenKey), String> = HashMap::new();
    for r in &cfg.remaps {
        if !r.enabled {
            continue;
        }
        if let Ok(from) = r.from.parse::<Key>() {
            let k = (r.layer.clone(), from, r.when_key());
            if let Some(first) = remap_from.get(&k) {
                out.push(Conflict {
                    severity: Severity::Error,
                    platform: false,
                    message: format!("改键来源「{first}」重复，多条改键都从「{first}」改起"),
                    name: String::new(),
                });
            } else {
                remap_from.insert(k, r.from.clone());
            }
        }
    }

    // 3) 改键来源与快捷键主键相同。层语义是「激活层优先、基础层兜底」：同层互相影响；
    //    基础层（`None`）会兜进任何激活层，所以它与任一层的条目也会互相影响；**两个不同的
    //    非基础层永远不会同时生效**，不算冲突。
    for r in &cfg.remaps {
        if !r.enabled {
            continue;
        }
        let Ok(from) = r.from.parse::<Key>() else {
            continue;
        };
        let desc = r.describe();
        for s in &cfg.shortcuts {
            if !s.enabled {
                continue;
            }
            if r.layer != s.layer && r.layer.is_some() && s.layer.is_some() {
                continue;
            }
            for t in &s.triggers {
                if let Ok(sc) = t.parse::<Shortcut>() {
                    if sc.key == from {
                        out.push(Conflict {
                            severity: Severity::Error,
                            platform: false,
                            message: format!(
                                "改键「{} → {desc}」会拦截按键「{}」，使快捷键「{t}」失效",
                                r.from,
                                key_name(from)
                            ),
                            name: s.name.clone().unwrap_or_default(),
                        });
                    }
                }
            }
        }
    }

    // 4) 触发键超集重叠（软冲突，**同层内**）。`matches()` 的口径是「事件的修饰键是触发键的
    //    超集」，所以同一层里更宽松的组合（`B`）会先命中，更具体的（`Alt+B`）永远轮不到。
    //    **不同层不比较**：层是局部覆盖（激活层先判、没命中才回落基础层），基础层更具体的
    //    `Alt+B` 只在该层激活期间被该层的 `B` 先命中，退出层就恢复——这是层的本分，不是
    //    「配置写错了」，报进冲突面板只会让每个分层键都挂一条噪声（用户报过这一条）。
    let mut items: Vec<(Option<String>, String, BTreeSet<Modifier>, Key)> = Vec::new();
    for s in &cfg.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            if let Ok(sc) = t.parse::<Shortcut>() {
                items.push((s.layer.clone(), t.clone(), sc.mods, sc.key));
            }
        }
    }
    for i in 0..items.len() {
        for j in 0..items.len() {
            if i == j {
                continue;
            }
            let (li, ti, mi, ki) = &items[i];
            let (lj, tj, mj, kj) = &items[j];
            if li != lj || ki != kj || mi == mj || !mi.is_subset(mj) {
                continue;
            }
            // mi ⊂ mj：更宽松的 ti 会遮蔽更具体的 tj
            out.push(Conflict {
                severity: Severity::Warn,
                platform: false,
                message: format!("「{tj}」被「{ti}」遮蔽：按下 {tj} 时会先命中更宽松的「{ti}」"),
                name: String::new(),
            });
        }
    }

    // 5) 层可达性：层里有启用的条目，却没有任何「长按进入层 / 长按锁定层」的切层键指向它 →
    //    这些条目永远不会生效（`active_layer` 只由切层键驱动）。这是「层内快捷键配了但不
    //    触发」最难自查的原因，所以在清单里直接点出来。
    for l in &cfg.layers {
        let has_entries = cfg
            .shortcuts
            .iter()
            .any(|s| s.enabled && s.layer.as_deref() == Some(l.id.as_str()))
            || cfg
                .remaps
                .iter()
                .any(|r| r.enabled && r.layer.as_deref() == Some(l.id.as_str()));
        if !has_entries {
            continue;
        }
        let reachable = cfg.remaps.iter().any(|r| {
            r.enabled
                && (r.hold_layer.as_deref() == Some(l.id.as_str())
                    || r.lock_layer.as_deref() == Some(l.id.as_str()))
        });
        if !reachable {
            out.push(Conflict {
                severity: Severity::Warn,
                platform: false,
                message: format!(
                    "层「{}」没有切层键：层内的快捷键/改键永远不会生效（在「改键」里设一条「长按进入层」或「长按锁定层」指向它；层内放和弦/序列建议用「长按锁定层」）",
                    l.name
                ),
                name: String::new(),
            });
        }
    }

    // 6) 平台能力缺失：本平台监听不到的触发键/改键来源、注入不了的目标键、恒不成立的
    //    前台条件（7.3-⑲）。跨平台同步的配置在能力短板的平台上会「静默失效」，这些条目
    //    本身合法，只报 Warn 提示「在你这台机器上不会生效」。
    out.extend(platform_conflicts(cfg, caps));

    // 7) 鼠标手势 / 触摸板手势（规划 7.2-MG）：两条启用的手势归一化后是同一个方向串、
    //    `when` 也相同 → 谁先谁后取决于配置顺序，用户会以为「我画了却触发了别的」。只报
    //    Warn（不拦保存）：重复方向串可能是有意为之（同名不同动作），但多数是手滑复制。
    //    不同 `when` 是按条件分流（同快捷键规则）；触摸板手势还要**触发键相同**才算重复。
    let mut seen: Vec<(String, WhenKey, String)> = Vec::new();
    for g in cfg.gestures.iter().filter(|g| g.enabled) {
        let pattern = normalize_pattern(&g.pattern);
        if pattern.is_empty() {
            continue;
        }
        let label = g.label();
        match seen
            .iter()
            .find(|(p, w, _)| *p == pattern && *w == g.when_key())
        {
            Some((_, _, first)) => out.push(Conflict {
                severity: Severity::Warn,
                platform: false,
                message: format!(
                    "鼠标手势「{label}」与「{first}」的方向串「{pattern}」相同：只会触发先配置的那条"
                ),
                name: label,
            }),
            None => seen.push((pattern, g.when_key(), label)),
        }
    }
    let mut seen_pad: Vec<(Key, String, WhenKey, String)> = Vec::new();
    for g in cfg.touchpad_gestures.iter().filter(|g| g.enabled) {
        let Some(trigger) = g.trigger else { continue };
        let pattern = normalize_pattern(&g.pattern);
        if pattern.is_empty() {
            continue;
        }
        let label = g.label();
        match seen_pad
            .iter()
            .find(|(k, p, w, _)| *k == trigger && *p == pattern && *w == g.when_key())
        {
            Some((_, _, _, first)) => out.push(Conflict {
                severity: Severity::Warn,
                platform: false,
                message: format!(
                    "触摸板手势「{label}」与「{first}」都绑在「{}」上画「{pattern}」：只会触发先配置的那条",
                    key_name(trigger)
                ),
                name: label,
            }),
            None => seen_pad.push((trigger, pattern, g.when_key(), label)),
        }
    }

    // 8) 触摸板手势的触发键同时用在快捷键 / 改键的触发侧：按住它会被手势**吞掉**，
    //    那条快捷键 / 改键永远等不到按下。只报 Warn（配置合法，只是不会按用户预期工作）。
    let mut reported: Vec<Key> = Vec::new();
    for g in cfg.touchpad_gestures.iter().filter(|g| g.enabled) {
        let Some(k) = g.trigger else { continue };
        if reported.contains(&k) || !uses_key_as_trigger(cfg, |x| x == k) {
            continue;
        }
        reported.push(k);
        out.push(Conflict {
            severity: Severity::Warn,
            platform: false,
            message: format!(
                "「{}」既被触摸板手势当触发键、又出现在快捷键 / 改键的触发侧：按住它会被手势吞掉，那条快捷键不会触发",
                key_name(k)
            ),
            name: format!("触摸板手势（{}）", key_name(k)),
        });
    }

    out
}

/// 手势 / 触摸板手势的动作列表（平台能力扫描共用一份口径）：两个数组都要扫，
/// 否则同一套动作在「鼠标操作」里报了缺口、在「触摸板手势」里却静默。
fn gesture_action_sets(cfg: &Config) -> impl Iterator<Item = (&[Action], String)> {
    cfg.gestures
        .iter()
        .filter(|g| g.enabled)
        .map(|g| (g.actions.as_slice(), g.label()))
        .chain(
            cfg.touchpad_gestures
                .iter()
                .filter(|g| g.enabled)
                .map(|g| (g.actions.as_slice(), g.label())),
        )
}

/// 平台能力缺失扫描（[`detect_conflicts`] 的第 6 段）。全支持（[`PlatformCaps::is_full`]）
/// 时直接返回空，不影响已有检测。
fn platform_conflicts(cfg: &Config, caps: &PlatformCaps) -> Vec<Conflict> {
    let mut out: Vec<Conflict> = Vec::new();
    if caps.is_full() {
        return out;
    }
    let name_of = |s: &ShortcutItem| s.name.clone().unwrap_or_default();
    let key_list = |keys: &BTreeSet<Key>| {
        keys.iter()
            .map(|k| key_name(*k))
            .collect::<Vec<_>>()
            .join("、")
    };

    for s in &cfg.shortcuts {
        if !s.enabled {
            continue;
        }
        // 触发键（组合/序列/和弦）里监听不到的键。
        for t in &s.triggers {
            let mut bad: BTreeSet<Key> = BTreeSet::new();
            match Trigger::parse(t) {
                Ok(Trigger::Combo(sc)) => collect_unlistenable(&sc, caps, &mut bad),
                Ok(Trigger::Sequence(steps)) => {
                    for sc in &steps {
                        collect_unlistenable(sc, caps, &mut bad);
                    }
                }
                Ok(Trigger::Chord(members)) => {
                    for sc in &members {
                        collect_unlistenable(sc, caps, &mut bad);
                    }
                }
                Err(_) => {}
            }
            if !bad.is_empty() {
                out.push(Conflict {
                    severity: Severity::Warn,
                    platform: true,
                    message: format!(
                        "触发键「{t}」要用到{}，当前平台监听不到这个键，这条触发不会生效",
                        key_list(&bad)
                    ),
                    name: name_of(s),
                });
            }
        }
        // 动作树：注入不了的键、恒不成立的前台/设备条件（`If` 可嵌套，递归扫）。
        let mut bad_inject: BTreeSet<Key> = BTreeSet::new();
        let mut bad_frontmost = false;
        let mut bad_device = false;
        scan_actions_platform(
            &s.actions,
            caps,
            &mut bad_inject,
            &mut bad_frontmost,
            &mut bad_device,
        );
        // 触发侧 `when` 门控（7.2-㊶）：恒不成立的前台 / 设备条件同样标出。
        #[cfg(feature = "automation")]
        if let Some(c) = &s.when {
            if matches!(
                c,
                Condition::FrontmostApp { .. }
                    | Condition::NotFrontmostApp { .. }
                    | Condition::WindowTitleContains { .. }
            ) && !caps.frontmost_conditions
            {
                bad_frontmost = true;
            }
            if matches!(c, Condition::DeviceIs { .. }) && !caps.device_conditions {
                bad_device = true;
            }
        }
        if !bad_inject.is_empty() {
            out.push(Conflict {
                severity: Severity::Warn,
                platform: true,
                message: format!(
                    "动作要用到{}，当前平台注入不了这个键，执行到这一步会没有输出",
                    key_list(&bad_inject)
                ),
                name: name_of(s),
            });
        }
        if bad_frontmost {
            out.push(Conflict {
                severity: Severity::Warn,
                platform: true,
                message: "「前台应用 / 窗口标题」条件（动作或触发门控）在当前平台取不到前台窗口，恒不成立".into(),
                name: name_of(s),
            });
        }
        if bad_device {
            out.push(Conflict {
                severity: Severity::Warn,
                platform: true,
                message: "「设备是」条件（动作或触发门控）在当前平台取不到设备，恒不成立".into(),
                name: name_of(s),
            });
        }
        #[cfg(feature = "automation")]
        if !caps.window_actions && uses_window_action(&s.actions) {
            out.push(Conflict {
                severity: Severity::Warn,
                platform: true,
                message: "「当前窗口」动作在当前平台用不了（只有 Windows 实现了前台窗口操作），执行到这一步会失败".into(),
                name: name_of(s),
            });
        }
        #[cfg(feature = "automation")]
        if !caps.window_wait && uses_window_wait(&s.actions) {
            out.push(Conflict {
                severity: Severity::Warn,
                platform: true,
                message: "「等待窗口出现」在当前平台用不了（只有 Windows 实现了窗口枚举），执行到这一步会失败".into(),
                name: name_of(s),
            });
        }
        if !caps.mouse_absolute && uses_absolute_move(&s.actions) {
            out.push(Conflict {
                severity: Severity::Warn,
                platform: true,
                message: "鼠标绝对移动在当前平台用不了（Linux 暂不支持），执行到这一步会失败"
                    .into(),
                name: name_of(s),
            });
        }
        if !caps.background_input && uses_background_input(&s.actions) {
            out.push(Conflict {
                severity: Severity::Warn,
                platform: true,
                message: "「后台发送」动作在当前平台用不了（只有 Windows 用 PostMessage 实现），执行到这一步会失败".into(),
                name: name_of(s),
            });
        }
        #[cfg(feature = "automation")]
        if !caps.shell_actions && uses_shell_actions(&s.actions) {
            out.push(Conflict {
                severity: Severity::Warn,
                platform: true,
                message: "「定位文件 / 清空回收站 / 关闭显示器 / 切换桌面图标」当前平台用不了（只有 Windows 实现），执行到这一步会失败".into(),
                name: name_of(s),
            });
        }
        #[cfg(feature = "automation")]
        if !caps.brightness && uses_brightness(&s.actions) {
            out.push(Conflict {
                severity: Severity::Warn,
                platform: true,
                message:
                    "「显示器亮度」当前平台用不了（只有 Windows 走 DDC/CI），执行到这一步会失败"
                        .into(),
                name: name_of(s),
            });
        }
    }

    // 手势的动作同样要标平台缺口（手势与快捷键共用一套动作编辑，别在两处给出不同口径）。
    // 鼠标手势与触摸板手势是两个数组，扫描口径共用 `gesture_action_sets`。
    #[cfg(feature = "automation")]
    if !caps.window_actions {
        for (actions, label) in gesture_action_sets(cfg) {
            if uses_window_action(actions) {
                out.push(Conflict {
                    severity: Severity::Warn,
                    platform: true,
                    message: "「当前窗口」动作在当前平台用不了（只有 Windows 实现了前台窗口操作），执行到这一步会失败".into(),
                    name: label,
                });
            }
        }
    }
    #[cfg(feature = "automation")]
    if !caps.window_wait {
        for (actions, label) in gesture_action_sets(cfg) {
            if uses_window_wait(actions) {
                out.push(Conflict {
                    severity: Severity::Warn,
                    platform: true,
                    message: "「等待窗口出现」在当前平台用不了（只有 Windows 实现了窗口枚举），执行到这一步会失败".into(),
                    name: label,
                });
            }
        }
    }
    if !caps.mouse_absolute {
        for (actions, label) in gesture_action_sets(cfg) {
            if uses_absolute_move(actions) {
                out.push(Conflict {
                    severity: Severity::Warn,
                    platform: true,
                    message: "鼠标绝对移动在当前平台用不了（Linux 暂不支持），执行到这一步会失败"
                        .into(),
                    name: label,
                });
            }
        }
    }
    if !caps.background_input {
        for (actions, label) in gesture_action_sets(cfg) {
            if uses_background_input(actions) {
                out.push(Conflict {
                    severity: Severity::Warn,
                    platform: true,
                    message: "「后台发送」动作在当前平台用不了（只有 Windows 用 PostMessage 实现），执行到这一步会失败".into(),
                    name: label,
                });
            }
        }
    }
    #[cfg(feature = "automation")]
    if !caps.shell_actions {
        for (actions, label) in gesture_action_sets(cfg) {
            if uses_shell_actions(actions) {
                out.push(Conflict {
                    severity: Severity::Warn,
                    platform: true,
                    message: "「定位文件 / 清空回收站 / 关闭显示器 / 切换桌面图标」当前平台用不了（只有 Windows 实现），执行到这一步会失败".into(),
                    name: label,
                });
            }
        }
    }
    #[cfg(feature = "automation")]
    if !caps.brightness {
        for (actions, label) in gesture_action_sets(cfg) {
            if uses_brightness(actions) {
                out.push(Conflict {
                    severity: Severity::Warn,
                    platform: true,
                    message:
                        "「显示器亮度」当前平台用不了（只有 Windows 走 DDC/CI），执行到这一步会失败"
                            .into(),
                    name: label,
                });
            }
        }
    }

    // 触发角（EXT-10）：现在只有 Windows 接了鼠标钩子；同一份配置同步到别的平台时
    // 只是不触发，按平台能力缺失给 Warn（不拦保存，执行侧也不静默）。
    if !caps.hot_corners && cfg.hot_corners.iter().any(|r| r.enabled) {
        out.push(Conflict {
            severity: Severity::Warn,
            platform: true,
            message: "触发角在当前平台用不了（只有 Windows 接了鼠标钩子），配置保留但不会触发"
                .into(),
            name: String::new(),
        });
    }

    // 边缘滚动（EXT-11）：同触发角口径，只有 Windows 接了鼠标钩子。
    if !caps.wheel_edges && cfg.wheel_edges.iter().any(|r| r.enabled) {
        out.push(Conflict {
            severity: Severity::Warn,
            platform: true,
            message: "边缘滚动在当前平台用不了（只有 Windows 接了鼠标钩子），配置保留但不会触发"
                .into(),
            name: String::new(),
        });
    }

    // 触摸板手势：采集要能收到「系统合成的鼠标移动」，目前只有 Windows 接了鼠标钩子。
    // 其它平台上触发键照常输出（不静默吞），但画一笔不会触发——按平台能力缺失给 Warn。
    if !caps.touchpad_gestures && cfg.touchpad_gestures.iter().any(|g| g.enabled) {
        out.push(Conflict {
            severity: Severity::Warn,
            platform: true,
            message: "触摸板手势在当前平台用不了（目前只接了 Windows 采集），配置保留但不会触发"
                .into(),
            name: String::new(),
        });
    }

    for r in &cfg.remaps {
        if !r.enabled {
            continue;
        }
        if let Ok(k) = r.from.parse::<Key>() {
            if caps.listen_unsupported.contains(&k) {
                out.push(Conflict {
                    severity: Severity::Warn,
                    platform: true,
                    message: format!("改键来源「{}」当前平台监听不到，这条改键不会生效", r.from),
                    name: String::new(),
                });
            }
        }
        // 各形态的输出目标（to/tap/hold/oneshot/sticky/tap2/tap3）都是要注入的键。
        let mut bad: BTreeSet<Key> = BTreeSet::new();
        let fields = [
            r.to.as_str(),
            r.tap.as_deref().unwrap_or(""),
            r.hold.as_deref().unwrap_or(""),
            r.oneshot.as_deref().unwrap_or(""),
            r.sticky.as_deref().unwrap_or(""),
            r.tap2.as_deref().unwrap_or(""),
            r.tap3.as_deref().unwrap_or(""),
        ];
        for field in fields {
            if let Ok(k) = field.trim().parse::<Key>() {
                if caps.inject_unsupported.contains(&k) {
                    bad.insert(k);
                }
            }
        }
        if !bad.is_empty() {
            out.push(Conflict {
                severity: Severity::Warn,
                platform: true,
                message: format!(
                    "改键「{} → {}」的目标键当前平台注入不了，触发后没有输出",
                    r.from,
                    key_list(&bad)
                ),
                name: String::new(),
            });
        }
        // 触发侧 `when` 门控（7.2-㊶）：恒不成立的前台 / 设备条件标出。
        #[cfg(feature = "automation")]
        if let Some(c) = &r.when {
            if matches!(
                c,
                Condition::FrontmostApp { .. }
                    | Condition::NotFrontmostApp { .. }
                    | Condition::WindowTitleContains { .. }
            ) && !caps.frontmost_conditions
            {
                out.push(Conflict {
                    severity: Severity::Warn,
                    platform: true,
                    message: format!("改键「{}」的触发门控用了「前台应用 / 窗口标题」条件，当前平台取不到前台窗口，恒不成立", r.from),
                    name: String::new(),
                });
            }
            if matches!(c, Condition::DeviceIs { .. }) && !caps.device_conditions {
                out.push(Conflict {
                    severity: Severity::Warn,
                    platform: true,
                    message: format!(
                        "改键「{}」的触发门控用了「设备是」条件，当前平台取不到设备，恒不成立",
                        r.from
                    ),
                    name: String::new(),
                });
            }
        }
    }
    out
}

// ---- 输入层环境诊断（规划 7.2-㊽ / `docs/底层输入能力规划.md` 档位 A）----
//
// 「按了没反应」里有一类根本不是配置问题：厂商代理软件把某个按键 divert 走，设备固件
// 从此不再把它放进标准 HID 报文——Windows 输入栈里没有这次按下，低层钩子拿不到，
// 内核过滤驱动同样拿不到（详见 `docs/底层输入能力规划.md` §3.1）。这里只做两件事：
// 把平台侧探测到的「谁在跑」翻成能照做的提示、并把处置办法一起给出。
//
// 刻意不做：内存完整性（HVCI）只当环境信息（[`InputEnv::hvci`]），不报成冲突——它拦的是
// 内核驱动，本项目的用户态低层钩子不受影响，报出来就是纯误报。

/// 一个已知的「按键接管者」条目。
///
/// 判定只能靠「这个软件在不在跑」：按键被 divert 之后，事件在到达任何软件（含内核）之前
/// 就已经不存在了，没法从事件侧反查是谁干的。
#[derive(Clone, Copy, Debug)]
pub struct DiverterInfo {
    /// 稳定 id（探测结果用它对齐，前端也拿它当 key）。
    pub id: &'static str,
    /// 展示名（写进提示文案）。
    pub name: &'static str,
    /// 进程名匹配片段：与进程名（小写、带 `.exe`）做**子串**匹配。
    ///
    /// 用片段而非全名，是因为同一软件各版本 / 各平台的可执行文件名会变
    /// （`logioptionsplus_agent.exe` / `logioptionsplus.exe`）；片段取得够独特就不会误伤。
    pub processes: &'static [&'static str],
    /// 处置办法（提示文案的第二段，要能照着做）。
    pub advice: &'static str,
}

/// 已知接管者目录。
///
/// 只收**真会抢按键**的两类：把按键 divert 给自己的（罗技系 HID++ 客户端）与独占抓取
/// 输入设备的（Linux 侧的 evdev 抢占者）——这两类都会让 Kada 彻底看不到按键。
/// 纯粹在应用层改键 / 注入的工具不在其列：它们不动设备报文，Kada 照样看得见。
/// **加条目前先确认真有 divert / 独占行为**，否则就是误报来源。
pub const DIVERTERS: &[DiverterInfo] = &[
    DiverterInfo {
        id: "logi-options-plus",
        name: "Logi Options+",
        processes: &["logioptionsplus"],
        advice: "在 Options+ 里把该按键改成「无操作 / 默认功能」，它就会回到标准报文本；不想改设置就直接退出 Options+。",
    },
    DiverterInfo {
        id: "logi-ghub",
        name: "罗技 G HUB",
        processes: &["lghub"],
        advice: "在 G HUB 里把该按键的指派清掉（设为「无操作」），或退出 G HUB。",
    },
    DiverterInfo {
        id: "logi-hidpp-client",
        name: "logiops / Solaar（罗技 HID++ 客户端）",
        processes: &["logid", "solaar"],
        advice: "在它的配置里把这个控件设回「不转交（divert off）」，或退出它。",
    },
    DiverterInfo {
        id: "razer-synapse",
        name: "Razer Synapse（雷云）",
        processes: &["razersynapse", "razer synapse"],
        advice: "在 Synapse 里把该按键的指派清掉（设为「默认 / 无操作」），或退出 Synapse。",
    },
    DiverterInfo {
        id: "steelseries-gg",
        name: "SteelSeries GG / Engine",
        processes: &["steelseries"],
        advice: "在 SteelSeries GG 里把该按键的指派清掉，或退出 GG。",
    },
    DiverterInfo {
        id: "corsair-icue",
        name: "Corsair iCUE",
        processes: &["icue"],
        advice: "在 iCUE 里把该按键的指派清掉，或退出 iCUE。",
    },
    DiverterInfo {
        id: "input-remapper",
        name: "input-remapper",
        processes: &["input-remapper"],
        advice: "它会独占抓取输入设备，和 Kada 互相抢：要用 Kada 就退出它。",
    },
    DiverterInfo {
        id: "keyd",
        name: "keyd",
        processes: &["keyd"],
        advice: "它会在更底层接管键盘，和 Kada 互相抢：要用 Kada 就退出它。",
    },
];

/// 本机输入环境探测结果（平台层填，[`diagnose_input_env`] 消费）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputEnv {
    /// 命中的接管者 id（[`DIVERTERS`] 的 `id`；认不出来的一律丢弃）。
    pub diverters: Vec<String>,
    /// Windows「内存完整性（HVCI）」是否开启；`None` = 读不到该状态或非 Windows。
    ///
    /// **只作环境信息**，不进 [`diagnose_input_env`] 的冲突列表：它拦的是内核驱动，
    /// 不拦本项目的用户态低层钩子，报成「能力缺失」就是误报。
    pub hvci: Option<bool>,
}

/// 配置里是否引用了鼠标键（中键 / 侧键）作为**触发侧**：快捷键触发键（组合 / 序列 /
/// 和弦成员）或改键来源（`from`）。仅作注入目标（`Action::Keys` 成员 / 改键的 `to` 等）不算——
/// 注入不需要监听。
///
/// 两个用途：Windows 按需安装鼠标低层钩子（见规划 7.4-㉛）、输入层环境诊断只在用到
/// 鼠标键时才提醒接管者（见 [`diagnose_input_env`]）。
pub fn config_uses_mouse(cfg: &Config) -> bool {
    uses_key_as_trigger(cfg, |k| {
        matches!(
            k,
            Key::MouseMiddle | Key::MouseBack | Key::MouseForward | Key::WheelUp | Key::WheelDown
        )
    })
}

/// 配置是否把**滚轮**绑成触发键（7.2-MG 余项①）：滚轮当离散键要钩子层逐事件处理，
/// 没用到时钩子层直接放行（不让每次滚动都走一遍判定）。
pub fn config_uses_wheel(cfg: &Config) -> bool {
    uses_key_as_trigger(cfg, |k| matches!(k, Key::WheelUp | Key::WheelDown))
}

/// 配置是否把满足 `hit` 的键用作**触发侧**（快捷键 / 改键来源）。
fn uses_key_as_trigger(cfg: &Config, hit: impl Fn(Key) -> bool) -> bool {
    cfg.shortcuts.iter().any(|s| {
        s.enabled
            && s.triggers.iter().any(|t| {
                Trigger::parse(t)
                    .map(|t| t.steps().iter().any(|s| hit(s.key)))
                    .unwrap_or(false)
            })
    }) || cfg.remaps.iter().any(|r| {
        r.enabled
            && r.from
                .parse::<Shortcut>()
                .map(|s| hit(s.key))
                .unwrap_or(false)
    })
}

/// **要不要挂鼠标钩子**：配置用到鼠标键（[`config_uses_mouse`]）**或**有启用的鼠标手势
/// / 触摸板手势（规划 7.2-MG）时都要。
///
/// 与 [`config_uses_mouse`] 分开是刻意的：后者还兼着「输入层环境诊断要不要跑」的判断，
/// 而厂商接管（Logi Options+ 等）拿的是**侧键**、与右键手势无关——把手势混进去会让
/// 一堆没装那类软件、或只用手势的用户平白收到「鼠标键被接管」的提示。
pub fn config_needs_mouse_hook(cfg: &Config) -> bool {
    config_uses_mouse(cfg)
        || (cfg.settings.experimental
            && (gesture_capture_enabled(cfg)
                || cfg.hot_corners.iter().any(|r| r.enabled)
                || cfg.wheel_edges.iter().any(|r| r.enabled)))
        || (cfg.settings.touchpad && touchpad_gesture_enabled(cfg))
}

/// 输入层环境诊断：把本机探测结果转成与「平台能力缺失」同一口径的软冲突
/// （[`Severity::Warn`] + `platform: true`，只提示、不阻止保存，UI 也归在同一组里）。
///
/// **只在配置真用到鼠标键时才报**：这类接管拿走的典型是侧键 / 额外键，配置里没用到鼠标键时
/// 它不影响任何一条快捷键，报出来就是纯噪音。这是「误报可关闭」的第一道闸；第二道是
/// [`Settings::input_env_diag`] 总开关（关掉后这条链路整个不跑）。
pub fn diagnose_input_env(cfg: &Config, env: &InputEnv) -> Vec<Conflict> {
    if !config_uses_mouse(cfg) {
        return Vec::new();
    }
    // 按目录顺序遍历（而不是按探测结果顺序）：天然去重，且 id 认不出来的一律丢弃。
    DIVERTERS
        .iter()
        .filter(|d| env.diverters.iter().any(|id| id == d.id))
        // 用户点过「忽略」的不再报（设置页卡片里仍能看到并恢复）。
        .filter(|d| !cfg.settings.input_env_ignored.iter().any(|id| id == d.id))
        .map(|d| Conflict {
            severity: Severity::Warn,
            platform: true,
            message: format!(
                "检测到「{}」正在运行：它可能把鼠标侧键 / 额外键转交给自己，Kada 收不到这次按下（配置里用到了鼠标键）。{}",
                d.name, d.advice
            ),
            name: String::new(),
        })
        .collect()
}

/// 一个 [`Shortcut`]（组合键）用到的全部键：主键 + 修饰键。
fn shortcut_keys(sc: &Shortcut) -> impl Iterator<Item = Key> + '_ {
    std::iter::once(sc.key).chain(sc.mods.iter().map(|m| modifier_key(*m)))
}

/// 把组合键里监听不到的键收进 `bad`。
fn collect_unlistenable(sc: &Shortcut, caps: &PlatformCaps, bad: &mut BTreeSet<Key>) {
    for k in shortcut_keys(sc) {
        if caps.listen_unsupported.contains(&k) {
            bad.insert(k);
        }
    }
}

/// 递归扫描动作树，收集注入不了的键（`Keys` 成员）与恒不成立的前台 / 设备条件（`If` 条件）。
/// `If` / `Parallel` 都递归进嵌套动作。基础版没有 `If`/`Condition`，两个 `bad_*` 旗标无从写入。
#[cfg_attr(not(feature = "automation"), allow(unused_variables))]
fn scan_actions_platform(
    actions: &[Action],
    caps: &PlatformCaps,
    bad_inject: &mut BTreeSet<Key>,
    bad_frontmost: &mut bool,
    bad_device: &mut bool,
) {
    for a in actions {
        match a {
            Action::Keys { keys, .. } => {
                for k in keys {
                    if let Ok(key) = k.trim().parse::<Key>() {
                        if caps.inject_unsupported.contains(&key) {
                            bad_inject.insert(key);
                        }
                    }
                }
            }
            #[cfg(feature = "automation")]
            Action::If {
                condition,
                then,
                otherwise,
                ..
            } => {
                if matches!(
                    condition,
                    Condition::FrontmostApp { .. }
                        | Condition::NotFrontmostApp { .. }
                        | Condition::WindowTitleContains { .. }
                ) && !caps.frontmost_conditions
                {
                    *bad_frontmost = true;
                }
                if matches!(condition, Condition::DeviceIs { .. }) && !caps.device_conditions {
                    *bad_device = true;
                }
                scan_actions_platform(then, caps, bad_inject, bad_frontmost, bad_device);
                scan_actions_platform(otherwise, caps, bad_inject, bad_frontmost, bad_device);
            }
            #[cfg(feature = "automation")]
            Action::Parallel { actions, .. } => {
                scan_actions_platform(actions, caps, bad_inject, bad_frontmost, bad_device);
            }
            Action::Group { actions, .. } => {
                scan_actions_platform(actions, caps, bad_inject, bad_frontmost, bad_device);
            }
            _ => {}
        }
    }
}

/// 动作树里有没有「当前窗口」动作（`If` / `Parallel` / 分组递归进去）。
/// 基础版没有这个动作类型，恒 false。
#[cfg(feature = "automation")]
pub(crate) fn uses_window_action(actions: &[Action]) -> bool {
    actions.iter().any(|a| match a {
        Action::Window { .. } => true,
        Action::App {
            operation: AppOperation::CurrentWindow { .. },
            ..
        } => true,
        Action::Group { actions, .. } => uses_window_action(actions),
        Action::If {
            then, otherwise, ..
        } => uses_window_action(then) || uses_window_action(otherwise),
        Action::Parallel { actions, .. } => uses_window_action(actions),
        _ => false,
    })
}

/// 动作树里有没有「等待窗口出现」（`If` / `Parallel` / 分组递归进去）。
/// 基础版没有这个动作类型，恒 false。
#[cfg(feature = "automation")]
fn uses_window_wait(actions: &[Action]) -> bool {
    actions.iter().any(|a| match a {
        Action::WaitFor {
            target: WaitTarget::Window { .. },
            ..
        } => true,
        Action::Group { actions, .. } => uses_window_wait(actions),
        Action::If {
            then, otherwise, ..
        } => uses_window_wait(then) || uses_window_wait(otherwise),
        Action::Parallel { actions, .. } => uses_window_wait(actions),
        _ => false,
    })
}

/// 动作树里有没有绝对鼠标移动（`MoveTo` / `MoveToOrigin`），`If` / `Parallel` / 分组递归。
fn uses_absolute_move(actions: &[Action]) -> bool {
    actions.iter().any(|a| match a {
        Action::Mouse {
            op: MouseOp::MoveTo { .. } | MouseOp::MoveToOrigin,
            ..
        } => true,
        Action::Group { actions, .. } => uses_absolute_move(actions),
        #[cfg(feature = "automation")]
        Action::If {
            then, otherwise, ..
        } => uses_absolute_move(then) || uses_absolute_move(otherwise),
        #[cfg(feature = "automation")]
        Action::Parallel { actions, .. } => uses_absolute_move(actions),
        _ => false,
    })
}

/// 动作树里有没有「后台发送」动作（`If` / `Parallel` / 分组递归进去）。
pub(crate) fn uses_background_input(actions: &[Action]) -> bool {
    actions.iter().any(|a| match a {
        Action::SendTo { .. } => true,
        #[cfg(feature = "automation")]
        Action::System {
            operation: SystemOperation::SendTo { .. },
            ..
        } => true,
        Action::Group { actions, .. } => uses_background_input(actions),
        #[cfg(feature = "automation")]
        Action::If {
            then, otherwise, ..
        } => uses_background_input(then) || uses_background_input(otherwise),
        #[cfg(feature = "automation")]
        Action::Parallel { actions, .. } => uses_background_input(actions),
        _ => false,
    })
}

/// 动作树里有没有 Windows Shell 补全动作（定位文件 / 清空回收站 / 关闭显示器 /
/// 切换桌面图标），`If` / `Parallel` / 分组递归进去。
#[cfg(feature = "automation")]
fn uses_shell_actions(actions: &[Action]) -> bool {
    actions.iter().any(|a| match a {
        Action::Os {
            operation: OsOperation::Reveal { .. },
            ..
        } => true,
        Action::System {
            operation:
                SystemOperation::EmptyRecycleBin
                | SystemOperation::MonitorOff
                | SystemOperation::ToggleDesktopIcons,
            ..
        } => true,
        Action::Group { actions, .. } => uses_shell_actions(actions),
        Action::If {
            then, otherwise, ..
        } => uses_shell_actions(then) || uses_shell_actions(otherwise),
        Action::Parallel { actions, .. } => uses_shell_actions(actions),
        _ => false,
    })
}

/// 动作树里有没有亮度动作（`If` / `Parallel` / 分组递归进去）。
#[cfg(feature = "automation")]
pub(crate) fn uses_brightness(actions: &[Action]) -> bool {
    actions.iter().any(|a| match a {
        Action::Brightness { .. } => true,
        Action::System {
            operation: SystemOperation::Brightness { .. },
            ..
        } => true,
        Action::Group { actions, .. } => uses_brightness(actions),
        Action::If {
            then, otherwise, ..
        } => uses_brightness(then) || uses_brightness(otherwise),
        Action::Parallel { actions, .. } => uses_brightness(actions),
        _ => false,
    })
}

/// 常见系统快捷键（Windows，中性键名：`Meta` = Win / ⌘ 键）+ 用途说明。
/// 用于检测咔哒快捷键与系统快捷键的冲突——精确匹配组合键（修饰键 + 主键都相同才算）。
pub const SYSTEM_SHORTCUTS: &[(&str, &str)] = &[
    ("Meta+E", "文件资源管理器"),
    ("Meta+R", "运行"),
    ("Meta+L", "锁定电脑"),
    ("Meta+D", "显示桌面"),
    ("Meta+I", "设置"),
    ("Meta+S", "搜索"),
    ("Meta+X", "快速链接菜单"),
    ("Meta+V", "剪贴板历史"),
    ("Meta+A", "通知中心"),
    ("Meta+N", "通知"),
    ("Meta+P", "投影"),
    ("Meta+K", "连接设备"),
    ("Meta+G", "游戏栏"),
    ("Meta+Tab", "任务视图"),
    ("Meta+Shift+S", "截图"),
    ("Meta+.", "表情符号面板"),
    ("Meta+,", "桌面预览"),
    ("Alt+Tab", "切换窗口"),
    ("Alt+F4", "关闭窗口"),
    ("Alt+Space", "窗口菜单"),
    ("Alt+Enter", "属性"),
    ("Ctrl+Esc", "开始菜单"),
    ("Ctrl+Shift+Esc", "任务管理器"),
    ("Ctrl+Alt+Delete", "安全选项"),
    ("Ctrl+Alt+Tab", "持久任务切换"),
];
