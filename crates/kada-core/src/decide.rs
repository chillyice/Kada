// 决策层（Android 版接入时从桌面壳搬出）：跨平台事件模型 `Ev` / 单条事件的决定 /
// 快捷键与改键匹配。
//
// 纯判定，不碰窗口与 IO：`decide` 只回答「吞掉 / 改发 / 触发」，真正的执行在调用方。
// 桌面壳与安卓壳共用这一份——两边唯一的差别是「谁去执行」以及能不能注入。

use std::collections::BTreeSet;

use crate::{matches, Action, Config, Key, Modifier, RawEvent, Shortcut, TriggerContext};

/// 跨平台键盘事件：各平台钩子统一喂给决策器/录制器。
#[derive(Clone, Debug)]
pub enum Ev {
    Down {
        key: Key,
        mods: BTreeSet<Modifier>,
        repeat: bool,
    },
    Up {
        key: Key,
    },
}

impl Ev {
    /// 事件来自哪个键（按下 / 抬起都算）。
    pub fn key(&self) -> Key {
        match self {
            Ev::Down { key, .. } | Ev::Up { key } => *key,
        }
    }

    /// 是不是**指针**（鼠标 / 触摸板 / 数位板）输入：中键 / 侧键 / 滚轮。
    /// 「设备是」据此在键盘设备与指针设备之间选一条（两条分开记）。
    pub fn is_pointer_input(&self) -> bool {
        self.key().is_mouse()
    }
}

/// 单条事件的决定。
#[derive(Clone, Debug)]
pub enum Outcome {
    /// 命中快捷键：返回其全部动作（按顺序执行）+ 触发键与名称（供消息中心展示）。
    Shortcut {
        actions: Vec<Action>,
        trigger: String,
        name: String,
    },
    /// 命中改键：改发某个键。
    Replace(Key),
    /// 放行。
    Pass,
}

/// 层语义匹配普通改键（非 tap-hold）：激活层条目优先、基础层条目兜底。
/// `ctx` 是触发侧 `when` 门控上下文：条件不成立的条目与「不存在」等价（跳过、落到下一条）。
pub fn match_plain_remap(
    cfg: &Config,
    key: Key,
    active_layer: Option<&str>,
    ctx: &TriggerContext,
) -> Option<Key> {
    for r in &cfg.remaps {
        if !r.enabled || r.needs_timing_state() || r.layer.as_deref() != active_layer {
            continue;
        }
        if let (Ok(from), Ok(to)) = (r.from.parse::<Key>(), r.to.parse::<Key>()) {
            if from == key && r.when_matches(ctx) {
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
                if from == key && r.when_matches(ctx) {
                    return Some(to);
                }
            }
        }
    }
    None
}

/// 层语义匹配快捷键：激活层条目优先、基础层条目兜底。返回 (动作, 触发键, 名称)。
pub fn match_shortcut(
    cfg: &Config,
    raw: &RawEvent,
    active_layer: Option<&str>,
    ctx: &TriggerContext,
) -> Option<(Vec<Action>, String, String)> {
    for s in &cfg.shortcuts {
        if !s.enabled || s.layer.as_deref() != active_layer {
            continue;
        }
        for t in &s.triggers {
            if let Ok(sc) = t.parse::<Shortcut>() {
                if matches(raw, &sc) && s.when_matches(ctx) {
                    return Some((
                        s.actions.clone(),
                        t.clone(),
                        s.name.clone().unwrap_or_default(),
                    ));
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
                    if matches(raw, &sc) && s.when_matches(ctx) {
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

/// 单条事件的判定：改键优先于快捷键。
pub fn decide(ev: &Ev, cfg: &Config, active_layer: Option<&str>, ctx: &TriggerContext) -> Outcome {
    match ev {
        Ev::Down { key, mods, repeat } => {
            // 改键优先于快捷键：先消耗掉原生键，避免改键后键再触发快捷键。
            if let Some(to) = match_plain_remap(cfg, *key, active_layer, ctx) {
                return Outcome::Replace(to);
            }
            if !*repeat {
                let raw = RawEvent {
                    key: *key,
                    mods: mods.clone(),
                    pressed: true,
                };
                if let Some((actions, trigger, name)) = match_shortcut(cfg, &raw, active_layer, ctx)
                {
                    return Outcome::Shortcut {
                        actions,
                        trigger,
                        name,
                    };
                }
            }
            Outcome::Pass
        }
        Ev::Up { .. } => Outcome::Pass,
    }
}
