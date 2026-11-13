//! 安卓判定器（S1-a）：把「一次按键事件」判成「吞不吞 + 触发哪些动作」。
//!
//! 与桌面 `Engine` 的关系：**共用 `kada-core` 的匹配与状态机**（`decide` /
//! `SequenceTracker` / `ChordTracker`），但**吞键策略不同**——这是 S0 实测出来的硬约束
//! （docs/安卓版规划.md §7、android/README.md「关键结论」）：
//!
//! - 桌面：可以「先吞下、凑不齐再回放」，所以成员键一律先吞；
//! - 安卓无障碍：`onKeyEvent` **必须同步回答吞不吞**，且**只能吞不能注入**——
//!   吞错的键补不回来。所以这里走**非破坏式**：成员键 / 序列步 / leader **一律放行**，
//!   只有「真正凑齐的那一下」才吞。
//!
//! 代价（写进报告的 notes）：
//! - 目标 app 会看见修饰键 / leader 键的按下（`Ctrl+Alt+K` 里 Ctrl、Alt 会漏给 app）；
//! - 序列 / 和弦「没凑齐」时什么都不发生，不需要回放，天然不丢键；
//! - `remaps`（键 → 键）与层（`hold_layer` 等）依赖注入，无 root 档整块不成立：
//!   命中时**只报告、不吞**，交给报告里的 `unsupported` 计数提示用户。

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use kada_core::{
    decide, Action, ChordAdvance, ChordTracker, Config, Ev, FrontmostContext, Key, Modifier,
    Outcome, RawEvent, SeqAdvance, SequenceTracker, Shortcut, Trigger, TriggerContext,
};

/// 触发一条规则后要执行的东西（Kotlin 按 JSON 里的字段分派）。
#[derive(Clone)]
pub(crate) struct Fired {
    pub(crate) name: String,
    pub(crate) trigger: String,
    pub(crate) actions: Vec<Action>,
}

/// 一次按键事件的判定结果。
#[derive(Default)]
pub(crate) struct Decision {
    /// `true` = 吞掉（`onKeyEvent` 返回 true）。
    pub(crate) swallow: bool,
    pub(crate) fired: Vec<Fired>,
    /// 判定说明（进日志 / 统计，不参与执行）。
    pub(crate) note: String,
}

impl Decision {
    fn pass(note: impl Into<String>) -> Self {
        Self {
            swallow: false,
            fired: Vec::new(),
            note: note.into(),
        }
    }

    fn fire(note: String, rule: &Rule, cfg: &Config) -> Self {
        let item = &cfg.shortcuts[rule.item];
        Self {
            swallow: true,
            fired: vec![Fired {
                name: item.name.clone().unwrap_or_default(),
                trigger: rule.trigger.clone(),
                actions: item.actions.clone(),
            }],
            note,
        }
    }
}

/// 从配置里预解析出来的一条序列 / 和弦规则（`combo` 由 `decide` 直接吃配置，不用预解析）。
pub(crate) struct Rule {
    /// 在 `cfg.shortcuts` 里的下标（`when` 求值要用原条目）。
    pub(crate) item: usize,
    pub(crate) steps: Vec<Shortcut>,
    pub(crate) trigger: String,
}

/// 预解析后的规则表（配置加载 / 导入后重建）。
#[derive(Default)]
pub(crate) struct Rules {
    pub(crate) sequences: Vec<Rule>,
    pub(crate) chords: Vec<Rule>,
}

impl Rules {
    /// 从配置解析出**基础层、启用**的序列与和弦（层在无 root 档不可用，见模块注释）。
    pub(crate) fn build(cfg: &Config) -> Self {
        let mut rules = Rules::default();
        for (item, s) in cfg.shortcuts.iter().enumerate() {
            if !s.enabled || s.layer.is_some() {
                continue;
            }
            for t in &s.triggers {
                match Trigger::parse(t) {
                    Ok(Trigger::Sequence(steps)) => rules.sequences.push(Rule {
                        item,
                        steps,
                        trigger: t.clone(),
                    }),
                    Ok(Trigger::Chord(members)) => rules.chords.push(Rule {
                        item,
                        steps: members,
                        trigger: t.clone(),
                    }),
                    // 组合键走 `decide`；解析失败的条目由报告里的解析错误提示，这里跳过。
                    Ok(Trigger::Combo(_)) | Err(_) => {}
                }
            }
        }
        rules
    }
}

/// 安卓判定器的全部运行时状态（只在无障碍服务线程上串行访问）。
#[derive(Default)]
pub(crate) struct AndroidEngine {
    chord: ChordTracker,
    seq: SequenceTracker,
    /// 物理按住的键（含修饰键，只用于日志 / 统计）。
    held: BTreeSet<Key>,
    /// 按下被吞掉的键：抬起一起吞，别让 app 看到半截按键。
    swallowed: BTreeSet<Key>,
    /// 序列 leader / 和弦第一个成员的按下时刻（超时判定用）。
    seq_at: Option<Instant>,
    chord_at: Option<Instant>,
}

impl AndroidEngine {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 服务断开 / 配置整份换掉：与物理键盘断过一次线，所有等待态作废（架构设计 §3.9）。
    pub(crate) fn reset(&mut self) {
        self.chord.reset();
        self.seq.reset();
        self.held.clear();
        self.swallowed.clear();
        self.seq_at = None;
        self.chord_at = None;
    }

    /// 时间推进：序列 / 和弦的等待窗超时就作废（不回放——本来也没吞过成员键）。
    pub(crate) fn tick(&mut self, cfg: &Config) {
        let now = Instant::now();
        if let (Some(at), Some(window)) =
            (self.seq_at, wait_window(cfg.settings.sequence_timeout_ms))
        {
            if now.duration_since(at) > window {
                self.seq.reset();
                self.seq_at = None;
            }
        }
        if let (Some(at), Some(window)) =
            (self.chord_at, wait_window(cfg.settings.chord_timeout_ms))
        {
            if now.duration_since(at) > window {
                self.chord.reset();
                self.chord_at = None;
            }
        }
    }

    /// 喂一个按键事件，回答「吞不吞 + 触发什么」。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn on_key(
        &mut self,
        cfg: &Config,
        rules: &Rules,
        key: Key,
        down: bool,
        repeat: bool,
        mods: BTreeSet<Modifier>,
        frontmost: Option<&FrontmostContext>,
    ) -> Decision {
        if down {
            self.held.insert(key);
        } else {
            self.held.remove(&key);
            self.chord.release(key);
            self.chord_at = self.chord.is_active().then(Instant::now);
        }

        // 重复键：吞过的继续吞（否则系统连发会把半个键漏给 app）；没吞过的原样放行。
        if down && repeat {
            let swallow = self.swallowed.contains(&key);
            let mut d = Decision::pass(if swallow {
                "repeat:swallowed"
            } else {
                "repeat:pass"
            });
            d.swallow = swallow;
            return d;
        }

        // 抬起：只负责「按下被吞过的键，抬起也吞」。
        if !down {
            let was_swallowed = self.swallowed.remove(&key);
            let mut d = Decision::pass(if was_swallowed {
                "up:swallowed"
            } else {
                "up:pass"
            });
            d.swallow = was_swallowed;
            return d;
        }

        // 以下都是**按下**（且非连发）的事件。
        self.tick(cfg);
        let ctx = TriggerContext::fixed(frontmost, None);

        // 顺序与桌面一致：和弦 → 序列 → 普通判定。
        let chords = rules
            .chords
            .iter()
            .filter(|r| cfg.shortcuts[r.item].when_matches(&ctx))
            .collect::<Vec<_>>();
        let chord_steps = chords.iter().map(|r| r.steps.clone()).collect::<Vec<_>>();
        match self.chord.press(key, &mods, &chord_steps) {
            ChordAdvance::Await => {
                self.chord_at = Some(Instant::now());
                return Decision::pass(format!("chord:await:{key:?}"));
            }
            ChordAdvance::Complete(idx) => {
                if let Some(rule) = chords.get(idx) {
                    self.chord_at = None;
                    self.swallowed.insert(key);
                    return Decision::fire(format!("chord:{}", rule.trigger), rule, cfg);
                }
            }
            ChordAdvance::NoMatch => {}
        }

        let seqs = rules
            .sequences
            .iter()
            .filter(|r| cfg.shortcuts[r.item].when_matches(&ctx))
            .collect::<Vec<_>>();
        let seq_steps = seqs.iter().map(|r| r.steps.clone()).collect::<Vec<_>>();
        let raw = RawEvent {
            key,
            mods: mods.clone(),
            pressed: true,
        };
        match self.seq.advance(&raw, &seq_steps) {
            SeqAdvance::Advance => {
                self.seq_at = Some(Instant::now());
                return Decision::pass(format!("sequence:advance:{key:?}"));
            }
            SeqAdvance::Complete(idx) => {
                if let Some(rule) = seqs.get(idx) {
                    self.seq_at = None;
                    self.swallowed.insert(key);
                    return Decision::fire(format!("sequence:{}", rule.trigger), rule, cfg);
                }
            }
            SeqAdvance::NoMatch => {}
        }

        let ev = Ev::Down {
            key,
            mods: mods.clone(),
            repeat: false,
        };
        match decide(&ev, cfg, None, &ctx) {
            Outcome::Shortcut {
                actions,
                trigger,
                name,
            } => {
                self.swallowed.insert(key);
                Decision {
                    swallow: true,
                    fired: vec![Fired {
                        name,
                        trigger: trigger.clone(),
                        actions,
                    }],
                    note: format!("shortcut:{trigger}"),
                }
            }
            // 改键要注入，无 root 档做不到：**绝不吞**，只留痕（报告里另有计数）。
            Outcome::Replace(to) => Decision::pass(format!("remap:unsupported:{key:?}->{to:?}")),
            Outcome::Pass => Decision::pass("pass"),
        }
    }
}

fn wait_window(ms: u64) -> Option<Duration> {
    (ms > 0).then(|| Duration::from_millis(ms))
}

#[cfg(test)]
mod tests;
