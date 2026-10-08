//! 输入决策引擎（Windows / Linux）：把平台事件喂进各状态机（tap-hold/层/和弦/键序列/热串），
//! 决定「吞掉 / 改发 / 注入 / 触发动作」。
//!
//! 与 Tauri 无关：注入走 [`Inject`]（真实实现转发 `kada-hook::simulate`，测试用记录器），
//! 触发动作用 `fire` 回调。这样状态机可以直接单测——它们是历史 bug 的高发区，没有单测
//! 就只能靠真人按键试。**单测里绝不允许真的注入**（会打到跑测试的这台机器上）。
//!
//! 一条铁律：**没有真正组成快捷键/序列/和弦的按键必须原样回放**。按键一旦被吞就再也
//! 补不回来，所以「吞掉」只允许发生在「还在等下一个键来凑齐」的窗口里，窗口一关就回放。

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use kada_core::{
    is_hotstring_terminator, key_to_char, match_expansion, Action, ChordAdvance, ChordTracker,
    Config, Key, Modifier, RawEvent, Remap, SeqAdvance, SequenceTracker, Shortcut, TextExpansion,
    Trigger, TriggerContext,
};

use crate::{EngineStatus, Ev};

/// 注入通道。真实实现转发平台 `simulate`（Windows `SendInput` / Linux uinput）。
pub trait Inject {
    fn down(&mut self, key: Key);
    fn up(&mut self, key: Key);
    /// 按一下再松开。
    fn tap(&mut self, key: Key) {
        self.down(key);
        self.up(key);
    }
}

/// 待定期间最多先吞掉多少个「其它键」：超过就立即定论回放，避免成员键一直按住时
/// 把用户的输入无限攒在缓冲里（表现为打字延迟）。
const MAX_PENDING_BUFFER: usize = 16;

/// 把设置里的毫秒等待窗换算成 `Duration`；`0` = 不限时（`None`，等不到时间到的那天）。
///
/// 序列（`Settings.sequence_timeout_ms`）与和弦（`Settings.chord_timeout_ms`）共用同一约定：
/// 面板上填 0 就是把等待窗关掉，此时定论只由「下一个键」「抬起」这类事件触发。
fn wait_window(ms: u64) -> Option<Duration> {
    (ms > 0).then(|| Duration::from_millis(ms))
}

/// 输入决策引擎的全部运行时状态（钩子回调持有，单线程串行访问）。
pub struct Engine {
    taphold: Option<TapHoldPending>,
    dance: Option<TapDanceState>,
    mods: ModsState,
    layer: LayerState,
    chord: ChordState,
    seq: SequenceState,
    hotstring: String,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {
            taphold: None,
            dance: None,
            mods: ModsState::new(),
            layer: LayerState::default(),
            chord: ChordState::default(),
            seq: SequenceState::new(),
            hotstring: String::new(),
        }
    }

    /// 当前激活的键位层（`None` = 基础层）。
    pub fn active_layer(&self) -> Option<&str> {
        self.layer.active()
    }

    /// 当前输入状态快照（激活层 + 注入中的键），供壳层渲染托盘提示 / 悬浮指示。
    ///
    /// 壳层在状态机定时器线程上每 [`crate::ENGINE_TICK_MS`] 比一次：变了才推给界面。
    /// 状态机那条热路径（钩子回调）一行都不改——**在回调里推 UI 等于把钩子挂在界面上**。
    pub fn status(&self) -> EngineStatus {
        EngineStatus {
            layer: self.layer.active.clone(),
            layer_locked: self.layer.locked,
            hold: self.mods.hold.iter().copied().collect(),
            sticky: self.mods.sticky.iter().copied().collect(),
            oneshot: self.mods.oneshot.iter().copied().collect(),
        }
    }

    /// 前置状态机：tap-hold（含连击/单次/粘滞/切层）→ 和弦 → 键序列。
    /// 返回 `None` = 事件被吞掉（框架层返回 Block）；`Some(ev)` = 继续走 `decide`。
    ///
    /// `ctx` 是触发侧 `when` 门控的求值上下文（前台窗口 / 设备，惰性查询）；
    /// 带 `when` 的条目条件不成立时与「不存在」等价——不吞键、落到下一条。
    pub fn step(
        &mut self,
        ev: &Ev,
        cfg: &Config,
        ctx: &TriggerContext,
        inj: &mut dyn Inject,
        fire: &mut dyn FnMut(Vec<Action>, String, String),
    ) -> Option<Ev> {
        let ev = self.taphold_step(ev, cfg, ctx, inj)?;
        let ev = self.chord_step(&ev, cfg, ctx, inj, fire)?;
        self.sequence_step(&ev, cfg, ctx, inj, fire)
    }

    /// 定时推进（壳层用定时器线程驱动，见 `ENGINE_TICK_MS`）：把「只能靠时间判定」的
    /// 等待态落地——键序列超时回放、和弦等待窗超时回放、连击等待窗超时提交。
    ///
    /// 没有它，超时判定就只能靠「下一个事件」懒触发：单独按一下序列 leader 键（之后不按
    /// 别的键），或改了双击/三击的键只短按一次，回放与输出都要等用户下次敲键盘才发生，
    /// 用户看到的是「这个键按了没反应」。和弦同理：成员键还按着、用户却在打别的字，那些
    /// 字被吞着等成员抬起，等待窗到点就该还回去。
    ///
    /// 等待窗取值来自配置（[`Config::settings`]，`0` = 不限时），所以 `tick` 需要 `cfg`——
    /// 由调用方在**取 `Engine` 锁之前**持有配置读锁传入，别在持锁期间再去读配置（加锁顺序
    /// 与钩子回调路径一致，避免死锁）。
    pub fn tick(&mut self, cfg: &Config, inj: &mut dyn Inject) {
        commit_expired_dance(&mut self.dance, inj);
        self.chord_expire(cfg, inj);
        self.seq_expire(cfg, inj);
    }

    /// 输入状态复位：钩子被摘除后重装、前台窗口切换时由壳层调用（见规划 7.2-④）。
    /// 返回是否真的丢掉了东西（`false` = 本来就是干净状态，调用方可跳过日志）。
    ///
    /// 这些时刻的共同点是「我们与物理键盘之间断了一次线」，Engine 里所有以「按键抬起」
    /// 或「时间窗」为终止条件的等待态都可能永远等不到终止：
    /// - [`ChordTracker`] 的按住集合里那个键再也收不到抬起 → 该键之后每次按下都被判成
    ///   「已在按住中」而**永久变哑**（正是钩子层 `HELD_KEYS` 那份脏状态的镜像）。
    /// - `ModsState` 里的 hold / oneshot / sticky 已经**物理注入**了修饰键 down，配对的
    ///   抬起丢了 → 系统层面认为 Ctrl/Alt 一直按着（「修饰键粘住」），后面敲什么都成组合键。
    /// - 待定的 tap-hold / 连击 / 键序列会把缓冲里的键在超时后回放到一个已经换了主人的窗口。
    /// - 热串缓冲同理：在 A 窗口敲了一半的触发词，切到 B 窗口敲下后缀就展开成一段文本。
    ///
    /// 处理原则：**注入出去的东西必须收回来，缓冲里的东西一律丢弃不回放**。
    /// - 收回：我们注入过的 hold / oneshot / sticky 修饰键补一个 up；momentary 层退出。
    /// - 丢弃：待定 tap-hold、连击等待窗、和弦与序列的待回放键、热串缓冲全部清空。这些键
    ///   在钩子层已被吞掉，但此处**不回放**——复位场景下（锁屏 / 唤醒 / 钩子被摘 / 切窗口）
    ///   前台目标已经变了，把旧按键注入到新窗口（可能是密码框、可能是别的程序）比丢掉它们
    ///   更糟：那会让用户看到「一串莫名其妙的字符被打进当前窗口」。
    /// - 保留：锁定层（`lock_layer`）是用户显式切换的持续状态，与「按键抬起」无关，
    ///   跨前台切换继续生效（和 CapsLock 同理）。
    pub fn reset(&mut self, inj: &mut dyn Inject) -> bool {
        // 注入过的键先收回来（去重：hold 是修饰键时它同时记在 `mods.hold` 里）。
        let mut to_release: BTreeSet<Key> = BTreeSet::new();
        if let Some(p) = self.taphold.as_ref() {
            if p.hold_active {
                to_release.extend(p.hold);
                // oneshot 的 roll：按住 `from` 期间它被当普通 hold 修饰按下。
                to_release.extend(p.oneshot);
            }
        }
        to_release.extend(self.mods.keys());
        for k in &to_release {
            inj.up(*k);
        }

        let dirty = !to_release.is_empty()
            || self.taphold.is_some()
            || self.dance.is_some()
            || !self.chord.pending.is_empty()
            || self.chord.tracker.is_active()
            || !self.seq.pending.is_empty()
            || self.seq.tracker.is_active()
            || !self.hotstring.is_empty()
            || self.layer.restore.is_some();

        self.mods.hold.clear();
        self.mods.sticky.clear();
        self.mods.oneshot.clear();
        self.taphold = None;
        self.dance = None;
        self.chord.pending.clear();
        self.chord.tracker.reset();
        self.chord.settling.clear();
        self.seq.pending.clear();
        self.seq.tracker.reset();
        self.hotstring.clear();
        if self.layer.restore.is_some() {
            // 接管前是锁定层就恢复回去，否则退回基础层（锁定层本身不动）。
            self.layer.leave_momentary();
        }
        dirty
    }

    /// 热串（文本扩展）：在放行事件上累积可打印字符，命中「触发词 + 后缀」时返回要执行的展开
    /// （回删触发词 / 注入替换文本 / 补回后缀键）。注入会阻塞上百毫秒，**必须由调用方放到
    /// 后台线程**执行——钩子回调里跑这么久的活会被系统摘掉钩子。
    pub fn hotstring(&mut self, ev: &Ev, expansions: &[TextExpansion]) -> Option<HotstringHit> {
        let Ev::Down { key, mods, repeat } = ev else { return None };
        if *repeat {
            return None;
        }
        // 修饰键不打断缓冲（输入大写字母需要 Shift 按下）。
        if matches!(key, Key::Shift | Key::Control | Key::Alt | Key::Meta) {
            return None;
        }
        if is_hotstring_terminator(*key) {
            if let Some(exp) = match_expansion(&self.hotstring, expansions) {
                let hit = HotstringHit {
                    trigger: exp.trigger.clone(),
                    backspaces: exp.trigger.chars().count(),
                    replace: exp.replace.clone(),
                    terminator: *key,
                };
                self.hotstring.clear();
                return Some(hit);
            }
            self.hotstring.clear();
            return None;
        }
        if *key == Key::Backspace {
            self.hotstring.pop();
            return None;
        }
        // 可打印字符累积；其余键（方向键/功能键等）打断缓冲。
        let shift = mods.contains(&Modifier::Shift);
        match key_to_char(*key, shift) {
            Some(c) => {
                self.hotstring.push(c);
                if self.hotstring.chars().count() > MAX_HOTSTRING_BUFFER {
                    let skip = self.hotstring.chars().count() - MAX_HOTSTRING_BUFFER;
                    self.hotstring = self.hotstring.chars().skip(skip).collect();
                }
            }
            None => self.hotstring.clear(),
        }
        None
    }
}

/// 命中的文本扩展：回删触发词 + 注入替换文本 + 补回后缀键（调用方后台执行）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotstringHit {
    /// 命中的触发词（诊断用：注入失败时记进消息中心）。
    pub trigger: String,
    /// 要回删的字符数（= 触发词字符数）。
    pub backspaces: usize,
    /// 替换文本（`{date}`/`{time}`/`{clipboard}` 等动态片段由调用方解析）。
    pub replace: String,
    /// 被吞掉的触发后缀键（空格/回车/Tab），回删注入后补回。
    pub terminator: Key,
}

/// 热串输入缓冲最大长度（触发词都很短，64 字符足够）。
const MAX_HOTSTRING_BUFFER: usize = 64;

// ---------------------------------------------------------------------------
// tap-hold / 连击 / 单次 / 粘滞 / 切层
// ---------------------------------------------------------------------------

/// tap-hold 改键的待定状态：按下 `from` 键后，等待判定「短按（tap）/ 长按（hold）/
/// 单次（oneshot）/ 粘滞（sticky）」。字段由命中规则 `Remap` 解析而来。
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
    /// 长按锁定的层（切换式切层）：长按切换该层开/关、层保持生效；与 `hold`/`hold_layer` 互斥。
    lock_layer: Option<String>,
    timeout: Duration,
    down_at: Instant,
    hold_active: bool,
}

/// tap-dance（连击）等待态：短按释放后不立即输出，等待后续连击或超时。
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

/// 运行时注入的键状态：分「长按 hold / 粘滞 / 单次」三类，释放时机各不相同，
/// 但都走同一「物理注入（simulate）+ 后续键 mods 补全」通道。
///
/// 集合里存的是 [`Key`] 而不是 [`Modifier`]：三类形态的值域都是**任意键**（非修饰键就是
/// 「替你按住这个键」，见 7.3-⑭），只有修饰键才参与后续键的 mods 补全（[`ModsState::mods`]）。
struct ModsState {
    /// tap-hold `hold` 键（`from` 松开时释放）。
    hold: BTreeSet<Key>,
    /// 粘滞键（再次单击 `from` 时解锁）。
    sticky: BTreeSet<Key>,
    /// 单次键（下一个非修饰键抬起时释放）。
    oneshot: BTreeSet<Key>,
}

impl ModsState {
    fn new() -> Self {
        Self { hold: BTreeSet::new(), sticky: BTreeSet::new(), oneshot: BTreeSet::new() }
    }

    /// 全部当前注入的键（供状态快照与释放；任意键）。
    fn keys(&self) -> impl Iterator<Item = Key> + '_ {
        self.hold.iter().chain(&self.sticky).chain(&self.oneshot).copied()
    }

    /// 其中的修饰键（供后续键的 mods 补全：非修饰键补不进 `Shortcut.mods`）。
    fn mods(&self) -> impl Iterator<Item = Modifier> + '_ {
        self.keys().filter_map(|k| k.as_modifier())
    }
}

// ---------------------------------------------------------------------------
// 键位层运行态
// ---------------------------------------------------------------------------

/// 键位层运行态（`active` = `None` 即基础层）。两种切层来源：
/// - **momentary**（改键的 `hold_layer`）：按住切层键期间生效、松开退回。层内放和弦/序列
///   时得同时按住切层键凑 3~4 个键，很难按——这类用法该用下面那种。
/// - **lock**（改键的 `lock_layer`，切换式）：长按一次切入、层保持生效，再长按一次退出。
///   「长按切层后腾出手再按层内和弦/序列」正是为它设计的用法。
///
/// 锁定层生效期间又按住一个 momentary 切层键 → 临时接管，松开后退回原锁定层。
#[derive(Default)]
struct LayerState {
    active: Option<String>,
    /// `active` 是否来自锁定：锁定时松开切层键不退层。
    locked: bool,
    /// momentary 接管前的层 `(active, locked)`，松开切层键后恢复。
    restore: Option<(Option<String>, bool)>,
}

impl LayerState {
    fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// 按住 momentary 切层键 → 进入其层（记住原层，松开时恢复）。
    fn enter_momentary(&mut self, id: &str) {
        if self.restore.is_none() {
            self.restore = Some((self.active.clone(), self.locked));
        }
        self.active = Some(id.to_string());
        self.locked = false;
    }

    /// 松开 momentary 切层键 → 回到接管前的层（通常就是基础层）。
    fn leave_momentary(&mut self) {
        match self.restore.take() {
            Some((active, locked)) => {
                self.active = active;
                self.locked = locked;
            }
            None => {
                self.active = None;
                self.locked = false;
            }
        }
    }

    /// 长按切换式切层键 → 该层开/关：已锁定该层则退出，否则切入并锁定。
    fn toggle_lock(&mut self, id: &str) {
        if self.locked && self.active.as_deref() == Some(id) {
            self.active = None;
            self.locked = false;
        } else {
            self.active = Some(id.to_string());
            self.locked = true;
        }
    }
}

impl Engine {
    /// tap-hold 状态机单步推进。
    ///
    /// 返回 `None` 表示事件被吞掉（原键不泄给目标程序）；返回 `Some(ev)` 表示继续走
    /// 普通 [`crate::decide`]，其中 `ev.mods` 已并入当前注入的修饰键（hold ∪ sticky ∪ oneshot）。
    /// 判定规则：
    /// - 按下 `from` → 吞掉并进入待定；短按（阈值内松开）按模式输出 tap / 进入连击 /
    ///   武装 oneshot / 切换 sticky，长按（≥阈值或 roll）输出 hold / 切层（momentary 进入或
    ///   切换式锁定，见 [`LayerState`]）。
    /// - 待定期间按下其它键 → 立即判 hold（roll 判定，缩短等待）。
    /// - 自动重复的 `from` down 被吞掉、不推进判定。
    /// - 连击（tap-dance）等待窗内再次 down 累计击数，超时懒提交。
    fn taphold_step(
        &mut self,
        ev: &Ev,
        cfg: &Config,
        ctx: &TriggerContext,
        inj: &mut dyn Inject,
    ) -> Option<Ev> {
        let (taphold, dance, mods, layers) =
            (&mut self.taphold, &mut self.dance, &mut self.mods, &mut self.layer);
        // 连击等待窗已过期且无按住中的连击键 → 懒提交（输出当前击数对应的键）。
        commit_expired_dance(dance, inj);

        match ev {
            Ev::Down { key, mods: ev_mods, repeat } => {
                // 连击等待中：同键 down → 累计击数并吞掉；异键 down → 提交连击后照常处理。
                if let Some(d) = dance.as_ref() {
                    if d.from == *key {
                        if !*repeat {
                            count_dance_tap(dance);
                        }
                        return None;
                    }
                    commit_dance(dance, inj);
                }

                let is_pending_from = taphold.as_ref().map(|p| p.from == *key).unwrap_or(false);
                if is_pending_from {
                    return None; // 原键的重复 down：吞掉。
                }
                if taphold.is_some() {
                    activate_hold(taphold, mods, layers, inj); // 不同键 down → roll 判定 hold。
                }
                if taphold.is_none() {
                    if let Some(r) = find_taphold_rule(cfg, *key, layers.active(), ctx) {
                        *taphold = Some(make_pending(*key, r));
                        return None;
                    }
                }
                let mut m = ev_mods.clone();
                m.extend(mods.mods());
                Some(Ev::Down { key: *key, mods: m, repeat: *repeat })
            }
            Ev::Up { key } => {
                // 连击等待中的同键 up：吞掉并解除「按住中」。
                if let Some(d) = dance.as_ref() {
                    if d.from == *key {
                        release_dance_tap(dance);
                        return None;
                    }
                }
                // pending 同键 up：结束待定（tap/hold/oneshot 武装/sticky 切换/进入连击）。
                if taphold.as_ref().map(|p| p.from == *key).unwrap_or(false) {
                    finish_taphold(taphold, dance, mods, layers, inj);
                    return None;
                }
                // oneshot 消费：下一个非修饰键 up 时释放武装的键。武装键自己再抬起不算
                // 消费（否则「按住它的那一下」就被当成用掉了）。
                if !mods.oneshot.is_empty()
                    && key.as_modifier().is_none()
                    && !mods.oneshot.contains(key)
                {
                    release_oneshot(mods, inj);
                }
                Some(Ev::Up { key: *key })
            }
        }
    }
}

/// 按层语义查找命中的 tap-hold/切层规则（激活层优先、基础层兜底）。
fn find_taphold_rule<'a>(
    cfg: &'a Config,
    key: Key,
    active_layer: Option<&str>,
    ctx: &TriggerContext,
) -> Option<&'a Remap> {
    for r in &cfg.remaps {
        if !r.enabled || !r.needs_timing_state() || r.layer.as_deref() != active_layer {
            continue;
        }
        if r.from.parse::<Key>().ok() == Some(key) && r.when_matches(ctx) {
            return Some(r);
        }
    }
    if active_layer.is_some() {
        for r in &cfg.remaps {
            if !r.enabled || !r.needs_timing_state() || r.layer.is_some() {
                continue;
            }
            if r.from.parse::<Key>().ok() == Some(key) && r.when_matches(ctx) {
                return Some(r);
            }
        }
    }
    None
}

/// 由命中的 [`Remap`] 规则构造待定状态（各字段解析为 `Key`/层 id）。
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
        lock_layer: r.lock_layer_id().map(String::from),
        timeout: Duration::from_millis(r.tap_timeout()),
        down_at: Instant::now(),
        hold_active: false,
    }
}

/// 判定 hold：注入 hold 键 down、切换/进入键位层；hold 键一律记入 `mods.hold`
/// （修饰键另行参与后续键的 mods 补全；非修饰键同样要登记，否则复位时收不回来）。
fn activate_hold(
    pending: &mut Option<TapHoldPending>,
    mods: &mut ModsState,
    layers: &mut LayerState,
    inj: &mut dyn Inject,
) {
    let Some(p) = pending.as_mut() else { return };
    if p.hold_active {
        return;
    }
    p.hold_active = true;
    if let Some(hk) = p.hold {
        inj.down(hk);
        mods.hold.insert(hk);
    } else if let Some(layer) = &p.lock_layer {
        // 切换式切层：长按（含 roll）即切换该层开/关，松开切层键不退层——层内放和弦/序列
        // 时全靠它，不然得一边按住切层键一边凑齐成员键。
        layers.toggle_lock(layer);
    } else if let Some(layer) = &p.hold_layer {
        layers.enter_momentary(layer);
    } else if let Some(ok) = p.oneshot {
        // oneshot 的 roll：按住 `from` 期间当普通 hold 键按下（松开 `from` 时释放）。
        inj.down(ok);
        mods.hold.insert(ok);
    }
}

/// 结束待定：hold 已激活则释放 hold 键 / 退出 momentary 层；否则按时长判定 tap/hold，或按模式
/// 分发到「短按输出 / 进入连击 / 武装 oneshot / 切换 sticky」。
fn finish_taphold(
    pending: &mut Option<TapHoldPending>,
    tap_dance: &mut Option<TapDanceState>,
    mods: &mut ModsState,
    layers: &mut LayerState,
    inj: &mut dyn Inject,
) {
    let Some(p) = pending.take() else { return };
    if p.hold_active {
        if let Some(hk) = p.hold {
            inj.up(hk);
            mods.hold.remove(&hk);
        } else if p.lock_layer.is_some() {
            // 切换式切层：长按期间已切换过一次，松开什么都不做（层保持生效，再长按一次退出）。
        } else if p.hold_layer.is_some() {
            layers.leave_momentary(); // 松开切层键 → 退回接管前的层。
        } else if let Some(ok) = p.oneshot {
            inj.up(ok); // oneshot roll 的 hold：松开 `from` 释放。
            mods.hold.remove(&ok);
        }
    } else if p.down_at.elapsed() >= p.timeout {
        // 按住超过阈值再松开，且期间没按别的键（没 roll）：有长按输出键就短促输出它；
        // 没有输出（切层键 / 只设了短按键 / 只设了双击三击）则**回放原键**——切层键的层
        // 只在 roll 时真正进过，这里整个按键白吞了，不回放这个键就变哑（吞键铁律）。
        match p.hold {
            Some(hk) => inj.tap(hk),
            // 「长按锁定层」例外：切换式切层就是要靠「长按一下再松开」来开/关，这里做切换、
            // 不回放原键（这个键本身就是切层键，按用户的本意它不该敲出字符）。
            None => match &p.lock_layer {
                Some(id) => layers.toggle_lock(id),
                None => inj.tap(p.from),
            },
        }
    } else if let Some(sk) = p.sticky {
        toggle_sticky(mods, sk, inj); // 快速 tap：切换粘滞修饰。
    } else if let Some(ok) = p.oneshot {
        arm_oneshot(mods, ok, inj); // 快速 tap：武装单次修饰。
    } else if p.tap2.is_some() || p.tap3.is_some() {
        // 快速 tap 且含双击/三击 → 进入连击等待（单/双/三击不同义）。单击缺省是原键、
        // 双击/三击缺省回落上一级：只设了「双击键」时单击不能变成什么都不输出。
        *tap_dance = Some(TapDanceState {
            from: p.from,
            tap_count: 1,
            deadline: Instant::now() + p.timeout,
            timeout: p.timeout,
            outputs: [
                p.tap.or(Some(p.from)),
                p.tap2.or(p.tap).or(Some(p.from)),
                p.tap3.or(p.tap2).or(p.tap).or(Some(p.from)),
            ],
            holding: false,
        });
    } else if let Some(tk) = p.tap {
        inj.tap(tk); // 短按：tap 键。
    } else {
        // 短按没有配置输出（只设了「长按」/「长按进入层」/「长按锁定层」）：回放原键。吞键铁律对
        // tap-hold 同样成立——不回放的话这个键就彻底变哑，用户按一下（比如 CapsLock）什么都不发生。
        inj.tap(p.from);
    }
}

/// 连击等待窗已过期且无按住中的连击键 → 懒提交（输出当前击数对应的键）。
fn commit_expired_dance(tap_dance: &mut Option<TapDanceState>, inj: &mut dyn Inject) {
    let expired = tap_dance.as_ref().is_some_and(|d| !d.holding && Instant::now() >= d.deadline);
    if expired {
        commit_dance(tap_dance, inj);
    }
}

/// 立即提交连击：输出当前击数对应的键并清空等待态。
fn commit_dance(tap_dance: &mut Option<TapDanceState>, inj: &mut dyn Inject) {
    let Some(d) = tap_dance.take() else { return };
    if let Some(k) = d.outputs[(d.tap_count - 1) as usize] {
        inj.tap(k);
    }
}

/// 连击等待窗内再次按下同键：累计击数（≤3）、重置等待窗、标记「按住中」。
fn count_dance_tap(tap_dance: &mut Option<TapDanceState>) {
    let Some(d) = tap_dance.as_mut() else { return };
    if d.tap_count < 3 {
        d.tap_count += 1;
    }
    d.deadline = Instant::now() + d.timeout;
    d.holding = true;
}

/// 连击键抬起：解除「按住中」（其 down 已被吞掉，up 一并吞掉）。
fn release_dance_tap(tap_dance: &mut Option<TapDanceState>) {
    if let Some(d) = tap_dance.as_mut() {
        d.holding = false;
    }
}

/// 武装单次键：物理按下并记入 `mods.oneshot`，供「下一个非修饰键抬起」时释放。
/// 值域不限修饰键（非修饰键 = 替你按住它直到下一个键按完）；修饰键另有 mods 补全。
/// **已武装时重复按下直接忽略**：同一个键再发一次 down 会被目标程序当成第二次按下。
fn arm_oneshot(mods: &mut ModsState, key: Key, inj: &mut dyn Inject) {
    if mods.oneshot.contains(&key) {
        return;
    }
    inj.down(key);
    mods.oneshot.insert(key);
}

/// 切换粘滞键：锁定则物理按下并记入 `mods.sticky`，解锁则物理抬起并移除。
fn toggle_sticky(mods: &mut ModsState, key: Key, inj: &mut dyn Inject) {
    if mods.sticky.contains(&key) {
        inj.up(key);
        mods.sticky.remove(&key);
    } else {
        inj.down(key);
        mods.sticky.insert(key);
    }
}

/// 释放全部武装中的单次键（下一个非修饰键 up 时调用）。
fn release_oneshot(mods: &mut ModsState, inj: &mut dyn Inject) {
    let ones: Vec<Key> = mods.oneshot.iter().copied().collect();
    for k in ones {
        inj.up(k);
    }
    mods.oneshot.clear();
}

// ---------------------------------------------------------------------------
// 和弦（同时按住多个键）
// ---------------------------------------------------------------------------

/// 和弦运行态。
///
/// 成员键按下即吞掉（不吞就漏字符、凑不成和弦），但**没凑成和弦时必须原样回放**——
/// 否则「F&J」会让 F、J 两个键彻底变哑：按下被吞、抬起被吞，目标程序收不到任何事件。
///
/// 定论（回放）的四个时机：
/// - 成员键全部抬起仍未凑齐 → 判定「不是和弦」。
/// - 等待窗（`Settings.chord_timeout_ms`）过期 → 同样判定「不是和弦」（见 [`Engine::chord_expire`]）。
/// - 待定期间按下的其它键把它挤爆缓冲（[`MAX_PENDING_BUFFER`]）→ 立即定论。
/// - 凑齐 → 触发，此时不回放（成员键本身就是触发键）。
///
/// 待定期间按下的其它键也一并吞掉、排在成员键之后回放：直接放行会让回放的成员键
/// 落到它后面——「f 还按着就打 a」会变成「af」。修饰键例外（放行，否则 Ctrl+X 之类
/// 的组合键用不了）。
///
/// 成员可以带修饰要求（`Ctrl+F&J` = 按住 Ctrl 的同时把 F、J 一起按住）或本身就是修饰键
/// （`Ctrl+Alt&J`）：这些修饰键**从不被吞、也不进按住集合**，只在凑齐的那一刻按当时的
/// 修饰状态判定（见 [`ChordTracker::press`]）。
struct ChordState {
    /// 已吞掉、尚未定论的按键（按下顺序 = 回放顺序）。
    pending: Vec<PendingKey>,
    /// 已触发/已回放、但物理仍按住的键：其 keyup 到达前抑制自动重复，避免重复回放。
    settling: BTreeSet<Key>,
    tracker: ChordTracker,
    /// 最近一次**成员键**动作（按下 / 定论回放）的时刻，等待窗从这里起算。
    ///
    /// 刻意不在「其它键被吞进缓冲」时刷新：那样用户一直打字就能把回放无限推后，
    /// 等待窗也就形同虚设（这正是它要治的症状）。
    last_activity: Instant,
}

impl Default for ChordState {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            settling: BTreeSet::new(),
            tracker: ChordTracker::new(),
            last_activity: Instant::now(),
        }
    }
}

/// 待定期间被吞掉的一次按键。
struct PendingKey {
    key: Key,
    /// 是否和弦成员键（成员键在按住判定里；非成员键只借道回放）。
    member: bool,
}

impl ChordState {
    /// 是否处于「等成员键凑齐」的待定态（待定缓冲非空或成员键还按着）。
    fn is_waiting(&self) -> bool {
        !self.pending.is_empty() || self.tracker.is_active()
    }

    /// 把待定期间吞掉的按键按用户输入顺序原样回放，并清空待定状态。
    /// 仍物理按住的键记入 `settling`（抬起前不再参与判定，防止自动重复重复回放）。
    fn replay_pending(&mut self, inj: &mut dyn Inject) {
        let held: Vec<Key> = self.tracker.held().iter().copied().collect();
        let pending = std::mem::take(&mut self.pending);
        for p in &pending {
            inj.tap(p.key);
        }
        self.settling.extend(held);
        self.tracker.reset();
        self.last_activity = Instant::now();
    }
}

impl Engine {
    /// 和弦状态机单步推进（在 tap-hold 之后、键序列之前调用）。
    ///
    /// 返回 `None` 表示事件被吞掉（成员键等待其它成员 / 已触发 / 已定论回放）；
    /// `Some(ev)` 表示继续走键序列与 [`crate::decide`]。只处理非重复 Down；成员键的
    /// keyup 被框架层一并吞掉，但仍以观察者身份回调到这里，用于「全部抬起 → 回放」判定。
    fn chord_step(
        &mut self,
        ev: &Ev,
        cfg: &Config,
        ctx: &TriggerContext,
        inj: &mut dyn Inject,
        fire: &mut dyn FnMut(Vec<Action>, String, String),
    ) -> Option<Ev> {
        // 等待窗过期：判定「不是和弦」，先按输入顺序回放待定的键，再照常处理本次事件
        // （与键序列同构的懒判定；tick 那条定时路径是兜底，单独按住成员键时不按别的键
        // 就只能靠它）。
        self.chord_expire(cfg, inj);

        let items = collect_chord_items(cfg, self.layer.active(), ctx);
        let chords: Vec<Vec<Shortcut>> = items.iter().map(|(c, ..)| c.clone()).collect();

        match ev {
            Ev::Down { key, mods, repeat } => {
                // 成员键只算非修饰键：修饰键成员（`Ctrl+F` 里的 Ctrl、`Ctrl+Alt&J` 里的
                // Ctrl/Alt）只表示「要求按住这个修饰键」，既不吞也不登记——吞掉修饰键会把
                // `Ctrl+Alt+Tab` 变成 `Ctrl+Tab`，登记进按住集合还会让「按着 Ctrl 打字」
                // 全被压进待定缓冲。
                let is_member = chords
                    .iter()
                    .any(|c| c.iter().any(|m| m.key == *key && !m.key.is_modifier()));

                if is_member {
                    // 同一键的重复 down（按住连发）或已触发/已回放的键：吞掉，不重复登记。
                    if self.chord.tracker.held().contains(key) || self.chord.settling.contains(key) {
                        return None;
                    }
                    if *repeat {
                        return None;
                    }
                    let held_before: BTreeSet<Key> = self.chord.tracker.held().clone();
                    return match self.chord.tracker.press(*key, mods, &chords) {
                        ChordAdvance::Complete(i) => {
                            // 命中：成员键就是触发键，不回放；但待定期间被吞掉的其它键
                            // （以及不属于本和弦的残留成员键）要补发，别吞掉用户的输入。
                            let fired: BTreeSet<Key> =
                                chords.get(i).map(|m| m.iter().map(|s| s.key).collect()).unwrap_or_default();
                            let leftovers: Vec<Key> = std::mem::take(&mut self.chord.pending)
                                .into_iter()
                                .filter(|p| !(p.member && fired.contains(&p.key)))
                                .map(|p| p.key)
                                .collect();
                            self.chord.settling.extend(held_before);
                            // 本次按下的成员键也在物理按住中：一并抑制它的自动重复。
                            self.chord.settling.insert(*key);
                            if let Some((_, actions, trigger, name)) = items.get(i) {
                                fire(actions.clone(), trigger.clone(), name.clone());
                            }
                            for k in leftovers {
                                inj.tap(k);
                            }
                            None
                        }
                        ChordAdvance::Await => {
                            self.chord.pending.push(PendingKey { key: *key, member: true });
                            self.chord.last_activity = Instant::now();
                            None
                        }
                        // 理论上到不了（`is_member` 已判过），兜底按「非成员键」放行。
                        ChordAdvance::NoMatch => Some(ev.clone()),
                    };
                }

                // 非成员键：没有待定成员就正常放行；有待定成员则先吞掉，定论时按序回放。
                if self.chord.pending.is_empty() {
                    return Some(ev.clone());
                }
                if key.as_modifier().is_some() {
                    // 修饰键放行（不产生字符，吞掉会破坏 Ctrl+X 之类的组合）。
                    return Some(ev.clone());
                }
                self.chord.pending.push(PendingKey { key: *key, member: false });
                if self.chord.pending.len() >= MAX_PENDING_BUFFER {
                    self.chord.replay_pending(inj);
                }
                None
            }
            Ev::Up { key } => {
                let was_member = self.chord.tracker.release(*key);
                if was_member && !self.chord.tracker.is_active() && !self.chord.pending.is_empty() {
                    // 成员全抬起仍没凑齐 → 不是和弦，原样回放。
                    self.chord.replay_pending(inj);
                } else {
                    self.chord.settling.remove(key);
                }
                // keyup 的返回值不影响框架层（是否放行由框架层已登记的吞键集合决定）。
                Some(ev.clone())
            }
        }
    }

    /// 和弦等待窗过期（`Settings.chord_timeout_ms`，`0` = 不限时）→ 按「不是和弦」定论：
    /// 把待定的键（成员键与借道回放的其它键）按输入顺序原样回放。返回是否过期回放了。
    ///
    /// 为什么需要它：成员键按下即被吞，此后**用户打的每个字都在缓冲里排队**，直到成员键
    /// 全部抬起才一起吐出来。用户其实是「单手按住一个和弦成员、另一只手在打字」时，那些
    /// 字就被压着不动（单键按住不放时更是一直不动）。等待窗就是给这段滞留时间设个上限。
    /// 到点后的定论是**回放**，不是旧实现的「丢弃」——吞键铁律不允许把按键吞掉不还
    /// （见 `docs/架构说明`「吞键铁律」）。
    fn chord_expire(&mut self, cfg: &Config, inj: &mut dyn Inject) -> bool {
        let Some(window) = wait_window(cfg.settings.chord_timeout_ms) else { return false };
        if self.chord.is_waiting() && self.chord.last_activity.elapsed() >= window {
            self.chord.replay_pending(inj);
            return true;
        }
        false
    }
}

/// 收集「当前层生效」的和弦触发条目：(成员键, 动作, 触发键文本, 名称)。
/// 层语义与 [`crate::match_shortcut`] 一致：激活层条目优先、基础层条目兜底。
fn collect_chord_items(
    cfg: &Config,
    active_layer: Option<&str>,
    ctx: &TriggerContext,
) -> Vec<(Vec<Shortcut>, Vec<Action>, String, String)> {
    let mut out: Vec<(Vec<Shortcut>, Vec<Action>, String, String)> = Vec::new();
    for s in &cfg.shortcuts {
        if !s.enabled || s.layer.as_deref() != active_layer || !s.when_matches(ctx) {
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
            if !s.enabled || s.layer.is_some() || !s.when_matches(ctx) {
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

// ---------------------------------------------------------------------------
// 键序列（leader key）
// ---------------------------------------------------------------------------

/// 键序列运行态。
///
/// leader 与已匹配的中间步按下即吞掉（不吞会先漏出字符，序列就废了），但**未命中时必须
/// 原样回放**，否则 leader 键与断链时已吞掉的中间步会变哑：
/// - 命中：不回放（这些键本身就是触发键）。
/// - 断链（下一个键不匹配任何候选）：回放已吞掉的键，再补发当前键，保持输入顺序。
/// - 超时（`Settings.sequence_timeout_ms` 内没有后续按键，`0` = 不限时）：回放已吞掉的键
///   ——由定时 tick 到点落地，下一次事件到来时也懒判一次。
struct SequenceState {
    tracker: SequenceTracker,
    last_activity: Instant,
    /// 已吞掉、尚未定论的按键（leader 与已匹配的中间步，按下顺序 = 回放顺序）。
    pending: Vec<Key>,
}

impl SequenceState {
    fn new() -> Self {
        Self {
            tracker: SequenceTracker::new(),
            last_activity: Instant::now(),
            pending: Vec::new(),
        }
    }

    /// 等待态结束：把已吞掉的按键按输入顺序原样回放。
    fn replay(&mut self, inj: &mut dyn Inject) {
        for k in std::mem::take(&mut self.pending) {
            inj.tap(k);
        }
        self.tracker.reset();
    }
}

impl Engine {
    /// 键序列状态机单步推进（在和弦之后、[`crate::decide`] 之前调用）。
    ///
    /// 返回 `Some(ev)` = 事件继续走 `decide`（未命中/已回放）；`None` = 吞掉（等待下一键、
    /// 命中触发、或已吞掉待回放）。只处理非重复 Down。
    fn sequence_step(
        &mut self,
        ev: &Ev,
        cfg: &Config,
        ctx: &TriggerContext,
        inj: &mut dyn Inject,
        fire: &mut dyn FnMut(Vec<Action>, String, String),
    ) -> Option<Ev> {
        let Ev::Down { key, mods, repeat } = ev else { return Some(ev.clone()) };

        // 懒超时：等待超时后第一次事件到来时回放已吞掉的键。必须在「修饰键放行」之前判，
        // 否则用户下一个动作是按下 Ctrl/Shift（放行、早返回）时回放又被推迟。
        self.seq_expire(cfg, inj);
        // 修饰键不参与序列判定（`Ctrl+K` 这类步骤靠主键 + mods 命中），也从不吞。
        if key.as_modifier().is_some() {
            return Some(ev.clone());
        }

        if *repeat {
            // 已吞掉的键保持按住：重复按下吞掉、不推进序列；其余照常放行。
            return if self.seq.pending.contains(key) { None } else { Some(ev.clone()) };
        }

        let items = collect_sequence_items(cfg, self.layer.active(), ctx);
        let steps: Vec<Vec<Shortcut>> = items.iter().map(|(s, ..)| s.clone()).collect();
        let raw = RawEvent { key: *key, mods: mods.clone(), pressed: true };

        if self.seq.tracker.is_active() {
            return match self.seq.tracker.advance(&raw, &steps) {
                SeqAdvance::Complete(i) => {
                    // 命中：序列本身就是触发键，吞掉不回放。
                    self.seq.pending.clear();
                    if let Some((_, actions, trigger, name)) = items.get(i) {
                        fire(actions.clone(), trigger.clone(), name.clone());
                    }
                    None
                }
                SeqAdvance::Advance => {
                    self.seq.pending.push(*key);
                    self.seq.last_activity = Instant::now();
                    None
                }
                SeqAdvance::NoMatch => {
                    // 断链：已吞掉的键 + 当前键原样回放（当前键排在最后，保持输入顺序）。
                    let pending = std::mem::take(&mut self.seq.pending);
                    for k in pending {
                        inj.tap(k);
                    }
                    inj.tap(*key);
                    None
                }
            };
        }

        // 未激活：本键命中某序列首步（leader）则开启等待，否则放行。
        match self.seq.tracker.advance(&raw, &steps) {
            SeqAdvance::Advance => {
                self.seq.pending.push(*key);
                self.seq.last_activity = Instant::now();
                None
            }
            _ => Some(ev.clone()),
        }
    }

    /// 键序列等待窗过期（`Settings.sequence_timeout_ms`，`0` = 不限时）→ 把已吞掉的 leader
    /// 与中间步按输入顺序原样回放。返回是否过期回放了。
    fn seq_expire(&mut self, cfg: &Config, inj: &mut dyn Inject) -> bool {
        let Some(window) = wait_window(cfg.settings.sequence_timeout_ms) else { return false };
        if self.seq.tracker.is_active() && self.seq.last_activity.elapsed() >= window {
            self.seq.replay(inj);
            return true;
        }
        false
    }
}

/// 收集「当前层生效」的键序列触发条目：(步骤, 动作, 触发键文本, 名称)。
/// 层语义与 [`crate::match_shortcut`] 一致：激活层条目优先、基础层条目兜底。
fn collect_sequence_items(
    cfg: &Config,
    active_layer: Option<&str>,
    ctx: &TriggerContext,
) -> Vec<(Vec<Shortcut>, Vec<Action>, String, String)> {
    let mut out: Vec<(Vec<Shortcut>, Vec<Action>, String, String)> = Vec::new();
    for s in &cfg.shortcuts {
        if !s.enabled || s.layer.as_deref() != active_layer || !s.when_matches(ctx) {
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
            if !s.enabled || s.layer.is_some() || !s.when_matches(ctx) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use kada_core::{key_name, TextMode};

    /// 假注入器：只记录「实际发出的键」，绝不真的发键。
    #[derive(Default)]
    struct FakeInject {
        log: Vec<String>,
    }

    impl FakeInject {
        fn take(&mut self) -> Vec<String> {
            std::mem::take(&mut self.log)
        }
    }

    impl Inject for FakeInject {
        fn down(&mut self, key: Key) {
            self.log.push(format!("down {}", key_name(key)));
        }
        fn up(&mut self, key: Key) {
            self.log.push(format!("up {}", key_name(key)));
        }
    }

    /// 记录触发的动作串（返回「触发键文本」列表）。
    #[derive(Default)]
    struct Fired {
        log: Vec<String>,
    }

    fn down(key: Key) -> Ev {
        Ev::Down { key, mods: BTreeSet::new(), repeat: false }
    }

    fn up(key: Key) -> Ev {
        Ev::Up { key }
    }

    fn key(name: &str) -> Key {
        name.parse::<Key>().unwrap()
    }

    /// 单条快捷键（触发键可能是组合/序列/和弦）的配置。
    fn cfg_with(triggers: &[&str]) -> Config {
        Config {
            shortcuts: vec![kada_core::ShortcutItem {
                triggers: triggers.iter().map(|t| t.to_string()).collect(),
                actions: vec![Action::Text {
                    text: "x".into(),
                    mode: TextMode::Input,
                    description: None,
                }],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn cfg_with_remap(r: Remap) -> Config {
        Config { remaps: vec![r], ..Default::default() }
    }

    fn remap(from: &str, to: &str) -> Remap {
        Remap { from: from.into(), to: to.into(), enabled: true, ..Default::default() }
    }

    /// 走一遍引擎并把注入与触发结果收集出来。
    fn step(
        engine: &mut Engine,
        ev: &Ev,
        cfg: &Config,
        inj: &mut FakeInject,
        fired: &mut Fired,
    ) -> Option<Ev> {
        let mut fire = |_a: Vec<Action>, trigger: String, _n: String| fired.log.push(trigger);
        engine.step(ev, cfg, &TriggerContext::empty(), inj, &mut fire)
    }

    /// 把两个等待窗缩到毫秒级（序列 / 和弦）：超时相关用例不必真等默认的 1 秒。
    fn short_timeout(mut cfg: Config, ms: u64) -> Config {
        cfg.settings.sequence_timeout_ms = ms;
        cfg.settings.chord_timeout_ms = ms;
        cfg
    }

    // ---- 等待窗来自设置（不是写死的 1000ms） ----

    #[test]
    fn sequence_timeout_zero_means_unlimited() {
        // 0 = 不限时：leader 一直等下去，别在 1000ms 后自作主张回放。
        let cfg = short_timeout(cfg_with(&["F9 J K"]), 0);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        assert!(step(&mut engine, &down(Key::F9), &cfg, &mut inj, &mut fired).is_none());
        std::thread::sleep(Duration::from_millis(10));
        engine.tick(&cfg, &mut inj);
        assert!(inj.take().is_empty(), "不限时就不该由 tick 回放");

        // 后续步照常推进（等待窗不是「卡死」的意思）。
        assert!(step(&mut engine, &down(Key::J), &cfg, &mut inj, &mut fired).is_none());
        assert!(step(&mut engine, &down(Key::K), &cfg, &mut inj, &mut fired).is_none());
        assert_eq!(fired.log, vec!["F9 J K"], "不限时只影响回放时机，不影响命中");
        assert!(inj.log.is_empty());
    }

    #[test]
    fn sequence_timeout_comes_from_settings() {
        // 手慢的人把窗口调大：默认 1000ms 内该等的还是要等。
        let mut cfg = cfg_with(&["F9 J K"]);
        cfg.settings.sequence_timeout_ms = 5_000;
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::F9), &cfg, &mut inj, &mut fired);
        std::thread::sleep(Duration::from_millis(10));
        engine.tick(&cfg, &mut inj);
        assert!(inj.take().is_empty(), "窗口 5 秒，10ms 时不该回放");
        assert!(step(&mut engine, &down(Key::J), &cfg, &mut inj, &mut fired).is_none(), "还在等第二步");
    }

    #[test]
    fn chord_timeout_expires_and_replays_in_order() {
        // 成员键还按着、另一只手在打字：等待窗到点就把积压的键按输入顺序还回去。
        let cfg = short_timeout(cfg_with(&["F&J"]), 1);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::F), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::A), &cfg, &mut inj, &mut fired); // 被吞掉待回放
        assert!(inj.take().is_empty(), "等待窗内先压着");

        std::thread::sleep(Duration::from_millis(5));
        engine.tick(&cfg, &mut inj);
        assert_eq!(inj.take(), vec!["down F", "up F", "down A", "up A"], "到点按输入顺序回放");

        // 过期后 F 已定论成普通按键：再按 J（同在和弦里）不再凑成和弦。
        assert!(step(&mut engine, &down(Key::J), &cfg, &mut inj, &mut fired).is_none());
        step(&mut engine, &up(Key::J), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down J", "up J"], "过期后成员键各自回放，不再触发");
        assert!(fired.log.is_empty(), "过期 = 不是和弦");
    }

    #[test]
    fn chord_expiry_is_lazy_on_next_event_too() {
        // tick 只是兜底：等待窗过期后的第一个事件也该先定论，不许把陈旧的待定态
        // 拿去凑和弦（晚按的成员键不该「补上一击」）。
        let cfg = short_timeout(cfg_with(&["F&J"]), 1);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::F), &cfg, &mut inj, &mut fired);
        std::thread::sleep(Duration::from_millis(5));
        let out = step(&mut engine, &down(Key::J), &cfg, &mut inj, &mut fired);
        assert!(out.is_none(), "J 是成员键，自己也要等确认");
        assert_eq!(inj.take(), vec!["down F", "up F"], "先回放已定论的 F");

        step(&mut engine, &up(Key::J), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down J", "up J"], "J 单独按下 → 回放成普通 j");
        assert!(fired.log.is_empty(), "晚了 5ms 的两个键不再是和弦");
    }

    #[test]
    fn chord_timeout_zero_means_unlimited() {
        // 0 = 不限时（＝旧行为）：成员键按着多久都等，「成员全抬起」才定论。
        let cfg = short_timeout(cfg_with(&["F&J"]), 0);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::F), &cfg, &mut inj, &mut fired);
        std::thread::sleep(Duration::from_millis(10));
        engine.tick(&cfg, &mut inj);
        assert!(inj.take().is_empty(), "不限时就不该回放");
        assert!(step(&mut engine, &down(Key::J), &cfg, &mut inj, &mut fired).is_none());
        assert_eq!(fired.log, vec!["F&J"], "不限时：慢一点按也算和弦");
    }

    // ---- 和弦：没凑成必须回放（bug 回归：成员键永久变哑） ----

    #[test]
    fn chord_lone_member_key_is_replayed() {
        let cfg = cfg_with(&["F&J"]);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        // 单按 F：按下先吞掉（万一接着按 J 就是和弦）……
        assert!(step(&mut engine, &down(Key::F), &cfg, &mut inj, &mut fired).is_none());
        assert!(inj.log.is_empty(), "待定期间不发键");
        // ……松开时没凑齐 → 原样回放成普通 f。
        step(&mut engine, &up(Key::F), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down F", "up F"], "没凑成和弦必须回放原键");
        assert!(fired.log.is_empty());
    }

    #[test]
    fn chord_press_release_press_types_twice() {
        // 连打同一个成员键两次（f f）：两次都要出字。
        let cfg = cfg_with(&["F&J"]);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        for _ in 0..2 {
            step(&mut engine, &down(Key::F), &cfg, &mut inj, &mut fired);
            step(&mut engine, &up(Key::F), &cfg, &mut inj, &mut fired);
        }
        assert_eq!(inj.take(), vec!["down F", "up F", "down F", "up F"]);
    }

    #[test]
    fn chord_member_held_while_typing_keeps_input_order() {
        // 和弦成员键还按着就打下一个键：回放顺序必须是 f → a（不能变成 af）。
        let cfg = cfg_with(&["F&J"]);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::F), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::A), &cfg, &mut inj, &mut fired); // 被吞掉待回放
        step(&mut engine, &up(Key::A), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::F), &cfg, &mut inj, &mut fired); // 成员全抬起 → 定论
        assert_eq!(
            inj.take(),
            vec!["down F", "up F", "down A", "up A"],
            "回放顺序要跟用户输入一致"
        );
        // A 的落下与抬起都被吞过，其抬起不会重复补发。
        assert!(step(&mut engine, &up(Key::A), &cfg, &mut inj, &mut fired).is_some());
        assert!(inj.log.is_empty());
    }

    #[test]
    fn chord_member_auto_repeat_after_fire_is_swallowed() {
        // 触发后成员键还按着：自动重复不能被漏给目标程序（否则会多出一串 j）。
        let cfg = cfg_with(&["F&J"]);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::F), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::J), &cfg, &mut inj, &mut fired);
        assert_eq!(fired.log, vec!["F&J"]);
        for _ in 0..3 {
            let repeat = Ev::Down { key: Key::J, mods: BTreeSet::new(), repeat: true };
            assert!(step(&mut engine, &repeat, &cfg, &mut inj, &mut fired).is_none());
        }
        // 抬起后恢复正常：再按仍被吞（等下一个成员），松开时回放。
        step(&mut engine, &up(Key::J), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::F), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::J), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::J), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down J", "up J"], "抬起后该键恢复成普通按键");
    }

    #[test]
    fn chord_completes_without_replay() {
        let cfg = cfg_with(&["F&J"]);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        assert!(step(&mut engine, &down(Key::F), &cfg, &mut inj, &mut fired).is_none());
        assert!(step(&mut engine, &down(Key::J), &cfg, &mut inj, &mut fired).is_none());
        assert!(inj.log.is_empty(), "命中触发不回放成员键");
        assert_eq!(fired.log, vec!["F&J"]);
        // 成员键抬起：不再回放。
        step(&mut engine, &up(Key::F), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::J), &cfg, &mut inj, &mut fired);
        assert!(inj.log.is_empty());
    }

    #[test]
    fn chord_member_auto_repeat_is_swallowed() {
        // 按住成员键不放，自动重复不该被当成新一次按下（否则松开时会多回放一个字符）。
        let cfg = cfg_with(&["F&J"]);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::F), &cfg, &mut inj, &mut fired);
        let repeat = Ev::Down { key: Key::F, mods: BTreeSet::new(), repeat: true };
        assert!(step(&mut engine, &repeat, &cfg, &mut inj, &mut fired).is_none());
        step(&mut engine, &up(Key::F), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down F", "up F"]);
    }

    #[test]
    fn chord_three_members_release_order_replays_in_press_order() {
        let cfg = cfg_with(&["D&F&J"]);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::D), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::F), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::F), &cfg, &mut inj, &mut fired); // 还差 J，继续等
        assert!(inj.log.is_empty(), "还有成员按住时不定论");
        step(&mut engine, &down(Key::J), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::J), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::D), &cfg, &mut inj, &mut fired); // 全部抬起 → 回放
        assert_eq!(
            inj.take(),
            vec!["down D", "up D", "down F", "up F", "down J", "up J"],
            "按按下顺序回放三个成员键"
        );
        assert!(fired.log.is_empty());
    }

    #[test]
    fn chord_unrelated_key_passes_through_when_idle() {
        let cfg = cfg_with(&["F&J"]);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        assert!(step(&mut engine, &down(Key::A), &cfg, &mut inj, &mut fired).is_some());
        assert!(inj.log.is_empty());
    }

    #[test]
    fn chord_modifier_passes_through_while_pending() {
        // 成员键按住时按下 Ctrl：放行（否则 Ctrl+X 之类组合失效），不参与回放缓冲。
        let cfg = cfg_with(&["F&J"]);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::F), &cfg, &mut inj, &mut fired);
        assert!(step(&mut engine, &down(Key::Control), &cfg, &mut inj, &mut fired).is_some());
        step(&mut engine, &up(Key::F), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down F", "up F"], "只回放成员键");
    }

    // ---- 键序列：leader / 断链都要能正常打字 ----

    #[test]
    fn sequence_completes_fires_and_does_not_replay() {
        let cfg = cfg_with(&["F9 J K"]);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        assert!(step(&mut engine, &down(Key::F9), &cfg, &mut inj, &mut fired).is_none());
        assert!(step(&mut engine, &down(Key::J), &cfg, &mut inj, &mut fired).is_none());
        assert!(step(&mut engine, &down(Key::K), &cfg, &mut inj, &mut fired).is_none());
        assert_eq!(fired.log, vec!["F9 J K"]);
        assert!(inj.log.is_empty(), "命中触发不回放序列按键");
    }

    #[test]
    fn sequence_leader_alone_is_replayed_after_timeout() {
        // leader 单独按下（没跟后续键）：超时后要回放，键不能被永久吞掉。
        let cfg = short_timeout(cfg_with(&["F9 J K"]), 1);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        assert!(step(&mut engine, &down(Key::F9), &cfg, &mut inj, &mut fired).is_none());
        step(&mut engine, &up(Key::F9), &cfg, &mut inj, &mut fired);
        assert!(inj.log.is_empty(), "等待期内还没定论");

        std::thread::sleep(Duration::from_millis(5));
        // 下一次事件到来时先回放 leader，再照常处理该事件。
        let out = step(&mut engine, &down(Key::A), &cfg, &mut inj, &mut fired);
        assert!(out.is_some(), "超时后不再吞新键");
        assert_eq!(inj.take(), vec!["down F9", "up F9"], "回放被吞掉的 leader");
    }

    #[test]
    fn tick_replays_expired_sequence_leader() {
        // 单独按一下 leader 键、之后不按别的键：不能靠「下一个事件」懒回放（那要等用户
        // 下次敲键盘，表现为这个键按了没反应），由定时 tick 到点回放。
        let cfg = short_timeout(cfg_with(&["F9 J K"]), 1);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::F9), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::F9), &cfg, &mut inj, &mut fired);
        assert!(inj.take().is_empty(), "等待期内不回放");

        std::thread::sleep(Duration::from_millis(5));
        engine.tick(&cfg, &mut inj);
        assert_eq!(inj.take(), vec!["down F9", "up F9"], "超时后由 tick 回放 leader");
        assert!(fired.log.is_empty(), "回放不等于触发");
    }

    #[test]
    fn tick_commits_expired_tap_dance() {
        // 短按一次「改了双击」的键：等待窗过期后由 tick 提交单击输出，同样不必等下一个按键。
        let mut r = remap("CapsLock", "");
        r.tap = Some("A".into());
        r.tap2 = Some("B".into());
        r.tap_timeout_ms = 20;
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        assert!(inj.take().is_empty(), "进入连击等待窗，先不输出");

        std::thread::sleep(Duration::from_millis(30));
        engine.tick(&cfg, &mut inj);
        assert_eq!(inj.take(), vec!["down A", "up A"], "过期提交单击输出");
    }

    #[test]
    fn sequence_break_replays_in_input_order() {
        // 断链（F9 J 之后按了 X）：F9 与 J 都要回放，且排在 X 之前。
        let cfg = cfg_with(&["F9 J K"]);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::F9), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::J), &cfg, &mut inj, &mut fired);
        assert!(step(&mut engine, &down(Key::X), &cfg, &mut inj, &mut fired).is_none());
        assert_eq!(
            inj.take(),
            vec!["down F9", "up F9", "down J", "up J", "down X", "up X"],
            "断链后按输入顺序回放"
        );
        assert!(fired.log.is_empty());
    }

    #[test]
    fn sequence_non_leader_key_passes_through() {
        let cfg = cfg_with(&["F9 J K"]);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        assert!(step(&mut engine, &down(Key::J), &cfg, &mut inj, &mut fired).is_some());
        assert!(step(&mut engine, &down(Key::A), &cfg, &mut inj, &mut fired).is_some());
        assert!(inj.log.is_empty());
    }

    // ---- 文本扩展 ----

    fn expansions() -> Vec<TextExpansion> {
        vec![TextExpansion {
            trigger: "addr".into(),
            replace: "我的地址".into(),
            folder: None,
            enabled: true,
        }]
    }

    fn type_str(engine: &mut Engine, text: &str, hits: &mut Vec<HotstringHit>) {
        for c in text.chars() {
            let k = match c {
                'a' => Key::A,
                'd' => Key::D,
                'r' => Key::R,
                _ => panic!("测试只用到 a/d/r"),
            };
            if let Some(hit) = engine.hotstring(&down(k), &expansions()) {
                hits.push(hit);
            }
        }
    }

    #[test]
    fn hotstring_hits_trigger_with_terminator() {
        let mut engine = Engine::new();
        let mut hits = Vec::new();
        type_str(&mut engine, "addr", &mut hits);
        assert!(hits.is_empty(), "还没按后缀不展开");
        let hit = engine.hotstring(&down(Key::Space), &expansions()).expect("空格应命中展开");
        assert_eq!(
            hit,
            HotstringHit {
                trigger: "addr".into(),
                backspaces: 4,
                replace: "我的地址".into(),
                terminator: Key::Space,
            }
        );
    }

    #[test]
    fn hotstring_double_letter_trigger_matches() {
        // 触发词里有连打同字母（addr 的 dd）：缓冲不能少字（历史上被自动重复判定吃掉过）。
        let mut engine = Engine::new();
        let mut hits = Vec::new();
        type_str(&mut engine, "ad", &mut hits);
        type_str(&mut engine, "dr", &mut hits);
        assert!(engine.hotstring(&down(Key::Enter), &expansions()).is_some());
    }

    #[test]
    fn hotstring_buffer_cleared_when_no_match() {
        let mut engine = Engine::new();
        let mut hits = Vec::new();
        type_str(&mut engine, "adr", &mut hits);
        assert!(engine.hotstring(&down(Key::Space), &expansions()).is_none());
    }

    // ---- tap-hold / 单次 / 粘滞 / 切层：重构后的行为回归 ----

    #[test]
    fn taphold_short_press_outputs_tap() {
        let mut r = remap("CapsLock", "");
        r.tap = Some("Escape".into());
        r.hold = Some("Ctrl".into());
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        assert!(step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired).is_none());
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down Esc", "up Esc"]);
    }

    // ---- 输入状态快照：托盘提示与悬浮指示的数据源（见规划 7.3-⑬） ----

    #[test]
    fn status_shows_momentary_layer_then_back_to_idle() {
        // 按住式切层键：层生效期间快照里要有层，且标明是「按住」而非「锁定」。
        let mut r = remap("CapsLock", "");
        r.hold_layer = Some("layer-1".into());
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        assert!(engine.status().idle(), "起始无层、无修饰键");
        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::A), &cfg, &mut inj, &mut fired); // roll → 进层
        let st = engine.status();
        assert_eq!(st.layer.as_deref(), Some("layer-1"));
        assert!(!st.layer_locked, "按住式切层不是锁定层");
        assert!(!st.idle());

        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        assert!(engine.status().idle(), "松开切层键后回到基础层");
    }

    #[test]
    fn status_marks_locked_layer() {
        let cfg = cfg_with_locked_layer("K");
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::Tab), &cfg, &mut inj, &mut fired);
        std::thread::sleep(Duration::from_millis(30));
        step(&mut engine, &up(Key::Tab), &cfg, &mut inj, &mut fired);
        let st = engine.status();
        assert_eq!(st.layer.as_deref(), Some("L1"));
        assert!(st.layer_locked, "切换式切层要标成锁定（松开切层键仍生效）");
    }

    #[test]
    fn status_separates_hold_sticky_and_oneshot_modifiers() {
        // 三种注入修饰的释放时机不同，指示上必须分得开：按住 Ctrl / 粘滞 Shift / 单次 Alt。
        let mut hold = remap("CapsLock", "");
        hold.hold = Some("Ctrl".into());
        let mut sticky = remap("F1", "");
        sticky.sticky = Some("Shift".into());
        let mut oneshot = remap("F2", "");
        oneshot.oneshot = Some("Alt".into());
        let cfg = Config { remaps: vec![hold, sticky, oneshot], ..Default::default() };
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        // hold：按住 from 期间按别的键 → roll 判定为长按，Ctrl 注入。
        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::A), &cfg, &mut inj, &mut fired);
        assert_eq!(engine.status().hold, vec![Key::Control]);
        // 松开 from 就释放，快照跟着清掉（这条同时验证「同一时刻只可能有一个待定改键」：
        // 按住 from 不放期间按 F1 不会进入 F1 的待定，故先收尾再点下一颗键）。
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        assert!(engine.status().hold.is_empty());
        assert!(engine.status().sticky.is_empty(), "还没点 F1，粘滞当然是空的");

        // sticky：点一下 F1 锁定 Shift。
        step(&mut engine, &down(Key::F1), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::F1), &cfg, &mut inj, &mut fired);
        assert_eq!(engine.status().sticky, vec![Key::Shift]);
        assert!(engine.status().hold.is_empty(), "粘滞与按住是两套集合，互不串味");

        // oneshot：点一下 F2 武装 Alt。
        step(&mut engine, &down(Key::F2), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::F2), &cfg, &mut inj, &mut fired);
        let st = engine.status();
        assert_eq!(st.sticky, vec![Key::Shift], "武装单次不影响已锁定的粘滞");
        assert_eq!(st.oneshot, vec![Key::Alt]);
        assert_eq!(st.layer, None, "没有层生效时层为空");

        // 解锁粘滞后快照跟着变（指示要能反映「已经退出这个状态」）。
        step(&mut engine, &down(Key::F1), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::F1), &cfg, &mut inj, &mut fired);
        assert!(engine.status().sticky.is_empty());
    }

    #[test]
    fn status_goes_idle_after_reset() {
        // 复位（钩子重装 / 前台切换 / 配置整份重载）之后指示必须跟着熄掉，
        // 否则屏幕上会一直挂着一条早就无效的状态。
        let mut r = remap("CapsLock", "");
        r.oneshot = Some("Ctrl".into());
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        assert!(!engine.status().idle());

        engine.reset(&mut inj);
        assert!(engine.status().idle(), "复位后快照回到空");
    }

    /// 快照可以直接比较：定时器线程靠它决定「要不要推给界面」，抖动一下就会让指示闪。
    #[test]
    fn status_compares_by_value() {
        let mut r = remap("CapsLock", "");
        r.oneshot = Some("Ctrl".into());
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        let before = engine.status();
        assert_eq!(before, Engine::new().status(), "同样的空状态要比得出相等");
        step(&mut engine, &down(Key::A), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::A), &cfg, &mut inj, &mut fired);
        assert_eq!(engine.status(), before, "与状态无关的普通按键不该改变快照");
    }

    #[test]
    fn taphold_long_press_holds_modifier_and_passes_others() {
        let mut r = remap("CapsLock", "");
        r.tap = Some("Escape".into());
        r.hold = Some("Ctrl".into());
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        // 按住 CapsLock 期间按下 A：roll 判定为 hold → Ctrl 注入，A 带 Ctrl 修饰继续走。
        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        let ev = step(&mut engine, &down(Key::A), &cfg, &mut inj, &mut fired).expect("A 应放行");
        match ev {
            Ev::Down { mods, .. } => assert!(mods.contains(&Modifier::Ctrl), "A 要带上 hold 的 Ctrl"),
            _ => panic!("应为 Down"),
        }
        assert_eq!(inj.take(), vec!["down Ctrl"]);
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["up Ctrl"], "松开 from 释放 hold 修饰");
    }

    #[test]
    fn oneshot_arms_then_releases_on_next_key() {
        let mut r = remap("CapsLock", "");
        r.oneshot = Some("Ctrl".into());
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down Ctrl"], "短按武装单次修饰");

        let ev = step(&mut engine, &down(Key::C), &cfg, &mut inj, &mut fired).expect("C 应放行");
        match ev {
            Ev::Down { mods, .. } => assert!(mods.contains(&Modifier::Ctrl)),
            _ => panic!("应为 Down"),
        }
        step(&mut engine, &up(Key::C), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["up Ctrl"], "下一个非修饰键抬起后释放");
    }

    #[test]
    fn sticky_toggles_on_each_tap() {
        let mut r = remap("CapsLock", "");
        r.sticky = Some("Shift".into());
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down Shift"], "单击锁定");
        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["up Shift"], "再击解锁");
    }

    #[test]
    fn hold_layer_activates_while_held() {
        let mut r = remap("CapsLock", "");
        r.hold_layer = Some("layer-1".into());
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        assert_eq!(engine.active_layer(), None);
        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::A), &cfg, &mut inj, &mut fired); // roll → 进层
        assert_eq!(engine.active_layer(), Some("layer-1"));
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        assert_eq!(engine.active_layer(), None, "松开切层键退回基础层");
    }

    #[test]
    fn taphold_short_press_without_tap_output_replays_original_key() {
        // 只设了长按键（短按键留空）：短按必须回放原键，否则这个键短按一下就变哑。
        let mut r = remap("CapsLock", "");
        r.hold = Some("Ctrl".into());
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down CapsLock", "up CapsLock"], "短按回放原键");
    }

    #[test]
    fn taphold_long_press_without_hold_output_replays_original_key() {
        // 只设了短按键：按住超过阈值再松开同样什么输出都没有 → 必须回放原键，
        // 不然「按久了一点」就等于这个键变哑。
        let mut r = remap("CapsLock", "");
        r.tap = Some("Escape".into());
        r.tap_timeout_ms = 20;
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        std::thread::sleep(Duration::from_millis(30));
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down CapsLock", "up CapsLock"], "长按无输出时回放原键");
    }

    #[test]
    fn hold_layer_key_without_other_key_replays_original_key() {
        // 切层键：无论短按还是「按久一点再松开」（期间没按别的键，层从没真正进过），
        // 都必须回放原键——否则这个键按下去什么都不发生。
        let mut r = remap("CapsLock", "");
        r.hold_layer = Some("layer-1".into());
        r.tap_timeout_ms = 20;
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        for hold_ms in [0u64, 30] {
            step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
            if hold_ms > 0 {
                std::thread::sleep(Duration::from_millis(hold_ms));
            }
            step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
            assert_eq!(
                inj.take(),
                vec!["down CapsLock", "up CapsLock"],
                "按住 {hold_ms}ms 后松开：回放原键"
            );
            assert_eq!(engine.active_layer(), None, "没按别的键就不进层");
        }
    }

    #[test]
    fn tap_dance_without_tap_output_falls_back_to_original_key() {
        // 只设了「双击键」、单击留空：单击同样要回放原键。
        let mut r = remap("CapsLock", "");
        r.tap2 = Some("B".into());
        r.tap_timeout_ms = 20;
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        assert!(inj.take().is_empty(), "进入连击等待窗，先不输出");

        std::thread::sleep(Duration::from_millis(30));
        engine.tick(&cfg, &mut inj);
        assert_eq!(inj.take(), vec!["down CapsLock", "up CapsLock"], "单击缺省回放原键");
    }

    #[test]
    fn tap_dance_counts_quick_taps() {
        let mut r = remap("CapsLock", "");
        r.tap = Some("A".into());
        r.tap2 = Some("B".into());
        r.tap3 = Some("C".into());
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        // 两次快击 → 第二击累计后超时提交 tap2 的键。
        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        std::thread::sleep(Duration::from_millis(5));
        step(&mut engine, &down(Key::Z), &cfg, &mut inj, &mut fired); // 下一次事件触发懒提交
        assert!(
            inj.log.contains(&"down B".to_string()),
            "双击应输出 tap2 键，实际：{:?}",
            inj.log
        );
    }

    #[test]
    fn stored_key_name_is_stable() {
        // key_name 是回放日志的可读名，改名会悄悄改测试预期。
        assert_eq!(key_name(key("F")), "F");
    }

    // ---- 长按锁定层（切换式切层）：长按后腾出手按层内和弦 / 序列 ----

    /// 「长按锁定层」的切层键（Tab → L1）+ 该层内一条快捷键（触发键由参数给出）。
    /// 阈值压到 20ms，免得测试真等 200ms。
    fn cfg_with_locked_layer(trigger: &str) -> Config {
        let mut cfg = cfg_with(&[trigger]);
        cfg.layers = vec![kada_core::Layer { id: "L1".into(), name: "层1".into() }];
        cfg.shortcuts[0].layer = Some("L1".into());
        cfg.remaps = vec![Remap {
            from: "Tab".into(),
            to: String::new(),
            lock_layer: Some("L1".into()),
            tap_timeout_ms: 20,
            enabled: true,
            ..Default::default()
        }];
        cfg
    }

    #[test]
    fn lock_layer_long_press_toggles_and_survives_release() {
        let cfg = cfg_with_locked_layer("K");
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        assert_eq!(engine.active_layer(), None);
        // 长按切层键（期间没按别的键）再松开 → 切入并锁定：层不随松开消失。
        step(&mut engine, &down(Key::Tab), &cfg, &mut inj, &mut fired);
        std::thread::sleep(Duration::from_millis(30));
        step(&mut engine, &up(Key::Tab), &cfg, &mut inj, &mut fired);
        assert_eq!(engine.active_layer(), Some("L1"), "长按后层保持生效");
        assert!(inj.log.is_empty(), "切层键本身不回放、也不注入");

        // 再长按一次 → 退出锁定层，回到基础层。
        step(&mut engine, &down(Key::Tab), &cfg, &mut inj, &mut fired);
        std::thread::sleep(Duration::from_millis(30));
        step(&mut engine, &up(Key::Tab), &cfg, &mut inj, &mut fired);
        assert_eq!(engine.active_layer(), None, "再长按一次退出切层");
        assert!(inj.log.is_empty());
    }

    #[test]
    fn lock_layer_quick_tap_replays_original_key() {
        // 只设了「长按锁定层」、没设短按输出：快速点击必须回放原键（吞键铁律），
        // 不然这个键轻轻一按就没反应；同时不能顺手把层切了（切换要长按）。
        let cfg = cfg_with_locked_layer("K");
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::Tab), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::Tab), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down Tab", "up Tab"], "短按回放原键");
        assert_eq!(engine.active_layer(), None, "短按不切层");
    }

    #[test]
    fn locked_layer_roll_toggles_layer() {
        // 切层键与别的键一起（roll）：同样算长按 → 切换层，且切换发生在该键判定之前
        //（层内条目要能命中这个刚按下的键）。
        let cfg = cfg_with_locked_layer("F&J");
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::Tab), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::F), &cfg, &mut inj, &mut fired); // roll → 锁定并进层
        assert_eq!(engine.active_layer(), Some("L1"), "roll 也切换层");
        step(&mut engine, &down(Key::J), &cfg, &mut inj, &mut fired);
        assert_eq!(fired.log, vec!["F&J"], "同一个按键就吃到新层的和弦");
        step(&mut engine, &up(Key::Tab), &cfg, &mut inj, &mut fired);
        assert_eq!(engine.active_layer(), Some("L1"), "松开切层键不退层（切换式）");
    }

    #[test]
    fn locked_layer_chord_fires_after_layer_key_released() {
        // 用户真实用法（2026-09-24）：长按 Tab 切到「和弦层」→ 松开 → 按 `5&Y`。
        // momentary 层做不到这个（层随松手消失），所以层内的和弦/序列要靠切换式切层。
        let cfg = cfg_with_locked_layer("5&Y");
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::Tab), &cfg, &mut inj, &mut fired);
        std::thread::sleep(Duration::from_millis(30));
        step(&mut engine, &up(Key::Tab), &cfg, &mut inj, &mut fired);
        assert_eq!(engine.active_layer(), Some("L1"));

        step(&mut engine, &down(Key::Digit5), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::Y), &cfg, &mut inj, &mut fired);
        assert_eq!(fired.log, vec!["5&Y"], "层内和弦应触发");
        assert!(inj.log.is_empty(), "命中触发不回放成员键");

        // 成员键抬起不重复回放；层仍在（切换式，直到再长按一次切层键）。
        step(&mut engine, &up(Key::Digit5), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::Y), &cfg, &mut inj, &mut fired);
        assert!(inj.log.is_empty());
        assert_eq!(engine.active_layer(), Some("L1"));
    }

    #[test]
    fn locked_layer_sequence_fires_after_layer_key_released() {
        let cfg = cfg_with_locked_layer("F9 J K");
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::Tab), &cfg, &mut inj, &mut fired);
        std::thread::sleep(Duration::from_millis(30));
        step(&mut engine, &up(Key::Tab), &cfg, &mut inj, &mut fired);

        step(&mut engine, &down(Key::F9), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::J), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::K), &cfg, &mut inj, &mut fired);
        assert_eq!(fired.log, vec!["F9 J K"], "层内序列应触发");
        assert!(inj.log.is_empty(), "命中触发不回放序列按键");
    }

    #[test]
    fn momentary_layer_over_locked_layer_restores_on_release() {
        // 锁定层 L1 生效期间又按住一条 momentary 切层键（CapsLock → L2）→ 临时接管，
        // 松开后退回 L1（而不是掉回基础层）。
        let cfg = Config {
            layers: vec![
                kada_core::Layer { id: "L1".into(), name: "层1".into() },
                kada_core::Layer { id: "L2".into(), name: "层2".into() },
            ],
            remaps: vec![
                Remap {
                    from: "Tab".into(),
                    to: String::new(),
                    lock_layer: Some("L1".into()),
                    tap_timeout_ms: 20,
                    enabled: true,
                    ..Default::default()
                },
                Remap {
                    from: "CapsLock".into(),
                    to: String::new(),
                    hold_layer: Some("L2".into()),
                    enabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::Tab), &cfg, &mut inj, &mut fired);
        std::thread::sleep(Duration::from_millis(30));
        step(&mut engine, &up(Key::Tab), &cfg, &mut inj, &mut fired);
        assert_eq!(engine.active_layer(), Some("L1"));

        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::A), &cfg, &mut inj, &mut fired); // roll → 临时进 L2
        assert_eq!(engine.active_layer(), Some("L2"), "momentary 临时接管");
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        assert_eq!(engine.active_layer(), Some("L1"), "松开 momentary 键退回锁定层");

        // 再长按一次切层键 → 退出锁定层，回到基础层。
        step(&mut engine, &down(Key::Tab), &cfg, &mut inj, &mut fired);
        std::thread::sleep(Duration::from_millis(30));
        step(&mut engine, &up(Key::Tab), &cfg, &mut inj, &mut fired);
        assert_eq!(engine.active_layer(), None);
    }

    // ---- 输入状态复位（钩子重装 / 前台切换，见规划 7.2-④） ----

    #[test]
    fn reset_releases_injected_hold_modifier() {
        // 按住切层/tap-hold 键期间钩子被摘或切了窗口：注入的 Ctrl 必须收回，
        // 否则系统层面 Ctrl 一直按着，之后敲什么都是组合键（「修饰键粘住」）。
        let mut r = remap("CapsLock", "");
        r.hold = Some("Ctrl".into());
        r.tap = Some("Escape".into());
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::K), &cfg, &mut inj, &mut fired); // roll → 注入 Ctrl down
        assert_eq!(inj.take(), vec!["down Ctrl"]);

        assert!(engine.reset(&mut inj), "有待定态就该被判为「脏」");
        assert_eq!(inj.take(), vec!["up Ctrl"], "注入的修饰键必须补一个 up");
        // 复位后再按住同一个键：能重新判 hold（没被卡在「已按住」里）。
        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down(Key::K), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down Ctrl"], "复位后该键恢复正常");
    }

    #[test]
    fn reset_releases_sticky_and_oneshot() {
        let mut r = remap("CapsLock", "");
        r.sticky = Some("Ctrl".into());
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired); // 单击 → 锁定粘滞
        assert_eq!(inj.take(), vec!["down Ctrl"]);

        assert!(engine.reset(&mut inj));
        assert_eq!(inj.take(), vec!["up Ctrl"], "粘滞修饰也要收回（否则永远粘住）");
    }

    #[test]
    fn reset_drops_chord_pending_without_replay() {
        // 和弦待定期丢过一次 keyup：按住集合必须清空，否则那个键之后每次按下都被判成
        // 「已在按住中」吞掉 → 永久变哑。但**不回放**缓冲里的键（前台目标可能已经变了）。
        let cfg = cfg_with(&["F&J"]);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        assert!(step(&mut engine, &down(Key::F), &cfg, &mut inj, &mut fired).is_none());
        assert!(engine.reset(&mut inj));
        assert!(inj.take().is_empty(), "复位不回放缓冲里的键（避免打进新窗口）");

        // 复位后 F 是干净的：重新按 F、J 仍能凑成和弦。
        assert!(step(&mut engine, &down(Key::F), &cfg, &mut inj, &mut fired).is_none());
        assert!(step(&mut engine, &down(Key::J), &cfg, &mut inj, &mut fired).is_none());
        assert_eq!(fired.log, vec!["F&J"], "复位后该键还能重新凑和弦");
    }

    #[test]
    fn reset_drops_sequence_pending_without_replay() {
        let cfg = cfg_with(&["F9 J K"]);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        assert!(step(&mut engine, &down(Key::F9), &cfg, &mut inj, &mut fired).is_none());
        assert!(engine.reset(&mut inj));
        assert!(inj.take().is_empty(), "复位不回放 leader");
        // 超时线程之后也不会再回放（tracker 与 pending 都已清空）。
        engine.tick(&cfg, &mut inj);
        assert!(inj.take().is_empty());
        assert!(step(&mut engine, &down(Key::A), &cfg, &mut inj, &mut fired).is_some(), "普通键照常放行");
    }

    #[test]
    fn reset_drops_momentary_layer_but_keeps_locked_one() {
        // 前台切换后：按住式（momentary）层的「松开切层键」事件可能永远不来，必须退出；
        // 锁定层是用户显式切换的持续状态，与按键抬起无关，继续生效。
        let cfg = cfg_with_locked_layer("K");
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        // 长按 Tab → 锁定 L1。
        step(&mut engine, &down(Key::Tab), &cfg, &mut inj, &mut fired);
        std::thread::sleep(Duration::from_millis(30));
        step(&mut engine, &up(Key::Tab), &cfg, &mut inj, &mut fired);
        assert_eq!(engine.active_layer(), Some("L1"));
        // 纯锁定层没有「会变脏的等待态」，复位报告 false（调用方据此不刷日志）。
        assert!(!engine.reset(&mut inj), "锁定层不算待复位的脏状态");
        assert_eq!(engine.active_layer(), Some("L1"), "锁定层跨前台切换继续生效");

        // 锁定层之上按住 CapsLock 临时进 L2（momentary 接管）→ 复位后回到锁定层。
        let mut cfg2 = cfg.clone();
        cfg2.layers.push(kada_core::Layer { id: "L2".into(), name: "层2".into() });
        cfg2.remaps.push(Remap {
            from: "CapsLock".into(),
            to: String::new(),
            hold_layer: Some("L2".into()),
            tap_timeout_ms: 20,
            enabled: true,
            ..Default::default()
        });
        step(&mut engine, &down(Key::CapsLock), &cfg2, &mut inj, &mut fired);
        step(&mut engine, &down(Key::A), &cfg2, &mut inj, &mut fired); // roll → 进 L2
        assert_eq!(engine.active_layer(), Some("L2"));
        assert!(engine.reset(&mut inj));
        assert_eq!(engine.active_layer(), Some("L1"), "momentary 层退出，锁定层恢复");
    }

    #[test]
    fn reset_clears_hotstring_buffer() {
        // 在 A 窗口敲了一半的触发词，切到 B 窗口后敲后缀不该在 B 里展开出一段文本。
        let mut engine = Engine::new();
        let (mut inj, mut _fired) = (FakeInject::default(), Fired::default());
        let mut hits = Vec::new();
        type_str(&mut engine, "addr", &mut hits);

        assert!(engine.reset(&mut inj), "缓冲里有内容就算「脏」");
        assert!(inj.log.is_empty(), "热串缓冲只是壳层状态，不涉及注入");
        assert!(engine.hotstring(&down(Key::Space), &expansions()).is_none(), "复位后不该再展开");
    }

    #[test]
    fn reset_is_noop_when_idle() {
        let mut engine = Engine::new();
        let (mut inj, _fired) = (FakeInject::default(), Fired::default());
        assert!(!engine.reset(&mut inj), "干净状态不该被报告成「脏」（否则白白刷日志）");
        assert!(inj.log.is_empty());
    }

    // ---- 7.3-⑭：单次 / 粘滞的值域扩到任意键、和弦成员可带修饰 ----

    /// 带修饰键按下一个键（和弦成员的修饰要求按它判定）。
    fn down_mods(key: Key, mods: &[Modifier]) -> Ev {
        Ev::Down { key, mods: mods.iter().copied().collect(), repeat: false }
    }

    #[test]
    fn oneshot_plain_key_is_held_until_next_key_released() {
        // 单次的非修饰键 = 「替你按住它，直到下一个键按完」：注入 down 后一直按着，
        // 下一个非修饰键抬起才释放（目标程序据此看到 A 先于 B 落下）。
        let mut r = remap("CapsLock", "");
        r.oneshot = Some("A".into());
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down A"], "短按武装：物理按住 A");
        assert_eq!(engine.status().oneshot, vec![Key::A], "指示要如实说「单次 A」");

        // 下一个键照常放行且不带修饰（A 不是修饰键，补不进 mods）。
        let ev = step(&mut engine, &down(Key::B), &cfg, &mut inj, &mut fired).expect("B 应放行");
        match ev {
            Ev::Down { mods, .. } => assert!(mods.is_empty(), "非修饰键的武装不该给 B 加修饰"),
            _ => panic!("应为 Down"),
        }
        assert!(inj.log.is_empty(), "B 抬起前 A 仍按着");
        step(&mut engine, &up(Key::B), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["up A"], "下一个非修饰键抬起后释放");
        assert!(engine.status().oneshot.is_empty());
    }

    #[test]
    fn oneshot_rearming_the_same_key_does_not_double_inject() {
        // 武装的键自己再按一次不该被当成「用掉」，也不该重复 down（会被目标程序当成连按两次）。
        let mut r = remap("CapsLock", "");
        r.oneshot = Some("A".into());
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down A"]);
        step(&mut engine, &down(Key::A), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::A), &cfg, &mut inj, &mut fired);
        assert_eq!(engine.status().oneshot, vec![Key::A], "武装的键自己抬起不算消费");
        assert!(inj.log.is_empty(), "不重复注入");
    }

    #[test]
    fn sticky_plain_key_holds_until_next_tap() {
        // 粘滞的非修饰键 = 点一下按住它（侧键做「按住说话」这类用法），再点一下松开。
        let mut r = remap("CapsLock", "");
        r.sticky = Some("MouseBack".into());
        let cfg = cfg_with_remap(r);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        for _ in 0..2 {
            step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired);
            step(&mut engine, &up(Key::CapsLock), &cfg, &mut inj, &mut fired);
        }
        assert_eq!(inj.take(), vec!["down MouseBack", "up MouseBack"], "第一次锁定、第二次解锁");
        assert!(engine.status().sticky.is_empty());
    }

    #[test]
    fn chord_member_with_modifier_requires_it_held() {
        // Ctrl+F&J：Ctrl 按着才凑齐；没按 Ctrl 时成员键照吞键铁律回放（不能变哑）。
        let cfg = cfg_with(&["Ctrl+F&J"]);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        // 没有 Ctrl：F 按下被吞（等凑齐），F 抬起 → 判定「不是和弦」→ 原样回放。
        assert!(step(&mut engine, &down(Key::F), &cfg, &mut inj, &mut fired).is_none());
        step(&mut engine, &up(Key::F), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down F", "up F"], "缺修饰没凑齐要回放");
        assert!(fired.log.is_empty());

        // 按住 Ctrl 再凑 F、J → 触发。
        step(&mut engine, &down(Key::Control), &cfg, &mut inj, &mut fired);
        step(&mut engine, &down_mods(Key::F, &[Modifier::Ctrl]), &cfg, &mut inj, &mut fired);
        assert!(
            step(&mut engine, &down_mods(Key::J, &[Modifier::Ctrl]), &cfg, &mut inj, &mut fired)
                .is_none()
        );
        assert_eq!(fired.log, vec!["Ctrl+F&J"], "修饰键按住时凑齐");
        assert!(inj.log.is_empty(), "成员键就是触发键，不回放");
    }

    #[test]
    fn chord_modifier_only_member_is_never_swallowed() {
        // Ctrl+Alt&J：Ctrl/Alt 只是「要求按住」，按下照样放行、不进回放缓冲
        // （吞掉修饰键会把 Ctrl+Alt+Tab 变成 Ctrl+Tab）。
        let cfg = cfg_with(&["Ctrl+Alt&J"]);
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        assert!(step(&mut engine, &down(Key::J), &cfg, &mut inj, &mut fired).is_none(), "J 待定");
        assert!(
            step(&mut engine, &down(Key::Alt), &cfg, &mut inj, &mut fired).is_some(),
            "修饰键成员要放行"
        );
        assert!(step(&mut engine, &up(Key::Alt), &cfg, &mut inj, &mut fired).is_some());
        step(&mut engine, &up(Key::J), &cfg, &mut inj, &mut fired);
        assert_eq!(inj.take(), vec!["down J", "up J"], "只回放成员键，修饰键不参与回放");

        // 两个修饰键都按住 → 凑齐触发。
        step(
            &mut engine,
            &down_mods(Key::J, &[Modifier::Ctrl, Modifier::Alt]),
            &cfg,
            &mut inj,
            &mut fired,
        );
        assert_eq!(fired.log, vec!["Ctrl+Alt&J"]);
    }

    #[cfg(feature = "automation")]
    #[test]
    fn when_gate_skips_chord_when_condition_false() {
        use kada_core::{Condition, FrontmostContext, TriggerContext};
        let mut cfg = cfg_with(&["F&J"]);
        cfg.shortcuts[0].when = Some(Condition::FrontmostApp { app: "chrome".into() });

        // 条件不成立（空上下文）：F 不是可吞的成员，原样放行、不成和弦。
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());
        let mut fire = |_a: Vec<Action>, trigger: String, _n: String| fired.log.push(trigger);
        let out = engine.step(&down(Key::F), &cfg, &TriggerContext::empty(), &mut inj, &mut fire);
        assert!(out.is_some(), "门控不成立时成员键应放行");
        assert!(fired.log.is_empty());
        assert!(inj.log.is_empty());

        // 条件成立：正常凑齐和弦触发。
        let chrome =
            FrontmostContext { process_name: "chrome.exe".into(), window_title: String::new() };
        let ctx = TriggerContext::fixed(Some(&chrome), None);
        let mut engine2 = Engine::new();
        let (mut inj2, mut fired2) = (FakeInject::default(), Fired::default());
        let mut fire2 = |_a: Vec<Action>, trigger: String, _n: String| fired2.log.push(trigger);
        assert!(engine2.step(&down(Key::F), &cfg, &ctx, &mut inj2, &mut fire2).is_none());
        assert!(engine2.step(&down(Key::J), &cfg, &ctx, &mut inj2, &mut fire2).is_none());
        assert_eq!(fired2.log, vec!["F&J"]);
    }

    // ---- 层内和弦：按住切层键后按成员键凑和弦 ----

    /// 层 `sym` 里放一条和弦 `U&Y`，切层键为 `CapsLock`。
    fn cfg_layer_chord(layer_field: &str) -> Config {
        let mut remap = Remap { from: "CapsLock".into(), enabled: true, ..Default::default() };
        if layer_field == "hold" {
            remap.hold_layer = Some("sym".into());
        } else {
            remap.lock_layer = Some("sym".into());
        }
        Config {
            layers: vec![kada_core::Layer { id: "sym".into(), name: "符号".into() }],
            shortcuts: vec![kada_core::ShortcutItem {
                triggers: vec!["U&Y".into()],
                actions: vec![Action::Text {
                    text: "x".into(),
                    mode: TextMode::Input,
                    description: None,
                }],
                enabled: true,
                layer: Some("sym".into()),
                ..Default::default()
            }],
            remaps: vec![remap],
            ..Default::default()
        }
    }

    #[test]
    fn chord_in_momentary_layer_triggers() {
        let cfg = cfg_layer_chord("hold");
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        // 按住切层键（待定）。
        assert!(step(&mut engine, &down(Key::CapsLock), &cfg, &mut inj, &mut fired).is_none());
        // 第一个成员键：应 roll 进入 momentary 层并被吞掉。
        assert!(
            step(&mut engine, &down(Key::U), &cfg, &mut inj, &mut fired).is_none(),
            "U 应被当成层内和弦成员吞掉，而不是放行"
        );
        assert_eq!(engine.active_layer(), Some("sym"), "按住切层键 + 按别的键应进入层");
        // 第二个成员键：应凑齐触发。
        assert!(step(&mut engine, &down(Key::Y), &cfg, &mut inj, &mut fired).is_none());
        assert_eq!(fired.log, vec!["U&Y"], "层内和弦应触发");
        assert!(inj.log.is_empty(), "命中不回放成员键");
    }

    #[test]
    fn chord_in_locked_layer_triggers() {
        let cfg = cfg_layer_chord("lock");
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        // 长按切层键（超过 tap 阈值）后松开 → 锁定进入层。
        let mut cfg_slow = cfg.clone();
        cfg_slow.remaps[0].tap_timeout_ms = 10; // 确保阈值短
        assert!(step(&mut engine, &down(Key::CapsLock), &cfg_slow, &mut inj, &mut fired).is_none());
        std::thread::sleep(Duration::from_millis(20));
        step(&mut engine, &up(Key::CapsLock), &cfg_slow, &mut inj, &mut fired);
        assert_eq!(engine.active_layer(), Some("sym"), "长按锁定后层应生效");

        // 腾出手来按和弦成员。
        assert!(step(&mut engine, &down(Key::U), &cfg, &mut inj, &mut fired).is_none());
        assert!(step(&mut engine, &down(Key::Y), &cfg, &mut inj, &mut fired).is_none());
        assert_eq!(fired.log, vec!["U&Y"], "锁定层内和弦应触发");
    }

    /// 用户实配复现：`` ` `` 长按进层（`to` 为空 + `hold_layer`），层内 `U&Y`。
    #[test]
    fn user_config_layer_chord() {
        const LID: &str = "20d08484-41a4-48ab-90cf-bb8633f779df";
        let cfg = Config {
            layers: vec![kada_core::Layer { id: LID.into(), name: "和弦层".into() }],
            shortcuts: vec![kada_core::ShortcutItem {
                name: Some("输入密码".into()),
                layer: Some(LID.into()),
                triggers: vec!["Alt+Z".into(), "U&Y".into()],
                actions: vec![Action::Text {
                    text: "Y5jYS!uK".into(),
                    mode: TextMode::Input,
                    description: None,
                }],
                enabled: true,
                ..Default::default()
            }],
            remaps: vec![Remap {
                from: "`".into(),
                to: String::new(),
                hold_layer: Some(LID.into()),
                tap_timeout_ms: 200,
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut engine = Engine::new();
        let (mut inj, mut fired) = (FakeInject::default(), Fired::default());

        assert!(cfg.remaps[0].needs_timing_state(), "hold_layer 必须走 tap-hold 状态机");
        assert!(step(&mut engine, &down(Key::Backquote), &cfg, &mut inj, &mut fired).is_none());
        assert!(
            step(&mut engine, &down(Key::U), &cfg, &mut inj, &mut fired).is_none(),
            "U 应在层内被吞成和弦成员"
        );
        assert_eq!(engine.active_layer(), Some(LID));
        assert!(step(&mut engine, &down(Key::Y), &cfg, &mut inj, &mut fired).is_none());
        assert_eq!(fired.log, vec!["U&Y"], "层内和弦应触发");
        assert!(inj.log.is_empty(), "命中不回放成员键（不应漏出 u/y）");

        // 抬起三个键：成员键的 keyup 被吞后不得回放，切层键抬起才退层。
        step(&mut engine, &up(Key::U), &cfg, &mut inj, &mut fired);
        step(&mut engine, &up(Key::Y), &cfg, &mut inj, &mut fired);
        assert!(inj.log.is_empty(), "触发后抬起成员键不得回放出 u/y");
        step(&mut engine, &up(Key::Backquote), &cfg, &mut inj, &mut fired);
        assert_eq!(engine.active_layer(), None, "松开切层键退回基础层");
        assert!(inj.log.is_empty(), "整个过程一个字符都不该漏出");
        assert_eq!(fired.log, vec!["U&Y"]);
    }
}
