// 录制器（T-05 从 lib.rs 搬出）：全局键盘事件 → `Action` 序列；鼠标触发 → `Action::Mouse` 序列。
// 只做转录，不碰钩子与状态机（见 说明书 §5.2）。

use crate::*;

/// 宏录制器：把全局键盘事件转录成动作列表 [`Action`]。
/// 同一次按下的多个键合并成一条 `Keys`（组合键），事件间 ≥20ms 间隔记 `PauseMs`。
pub(crate) struct Recorder {
    started: std::time::Instant,
    last_ms: u64,
    held: Vec<Key>,
    chord: Vec<Key>,
    actions: Vec<Action>,
}

impl Recorder {
    pub(crate) fn new() -> Self {
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
    pub(crate) fn push(&mut self, ev: &Ev) {
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
                self.actions.push(Action::PauseMs {
                    ms: gap,
                    description: None,
                });
            }
            self.held.push(key);
            self.chord.push(key);
        } else {
            self.held.retain(|k| *k != key);
            // 组合键全部松开 → 落成一条 Keys 动作
            if self.held.is_empty() {
                let keys: Vec<String> = self
                    .chord
                    .drain(..)
                    .map(|k| key_name(k).to_string())
                    .collect();
                if !keys.is_empty() {
                    self.actions.push(Action::Keys {
                        keys,
                        mode: kada_core::KeysMode::Tap,
                        description: None,
                    });
                }
            }
        }
    }

    /// 结束：仍在按住的键按剩余组合键收尾，返回录制所得动作。
    pub(crate) fn finish(mut self) -> Vec<Action> {
        if !self.chord.is_empty() {
            let keys: Vec<String> = self
                .chord
                .drain(..)
                .map(|k| key_name(k).to_string())
                .collect();
            self.actions.push(Action::Keys {
                keys,
                mode: kada_core::KeysMode::Tap,
                description: None,
            });
        }
        self.actions
    }
}

pub(crate) fn key_name(k: Key) -> &'static str {
    kada_core::key_name(k)
}

/// 鼠标操作录制器：把钩子层上报的鼠标触发表成 [`Action::Mouse`] 序列——先按位移补一条
/// `Move`（为 0 则省，见吞键 / 判重口径），再补触发本身（`Click` / `Scroll`）。
///
/// 与键盘 [`Recorder`] 独立：鼠标录制只转录鼠标，按键一律不记。位移由钩子层算好
/// （Windows / macOS 取两次触发坐标之差、Linux 取累积相对量），壳层无平台分叉。
#[derive(Default)]
pub(crate) struct MouseRecorder {
    actions: Vec<Action>,
}

impl MouseRecorder {
    pub(crate) fn push(&mut self, ev: &kada_hook::MouseRecord) {
        // 滚轮量为 0（高精度滚轮的次格信号）不算一次触发，直接丢弃——`MouseOp::Scroll`
        // 的偏移不能同时为 0，落盘时会被当成坏条目。
        let op = match ev.kind {
            kada_hook::MouseRecordKind::Click(button) => kada_core::MouseOp::Click { button },
            kada_hook::MouseRecordKind::Scroll { dx, dy } if dx == 0 && dy == 0 => return,
            kada_hook::MouseRecordKind::Scroll { dx, dy } => kada_core::MouseOp::Scroll { dx, dy },
        };
        if ev.move_dx != 0 || ev.move_dy != 0 {
            self.actions.push(Action::Mouse {
                op: kada_core::MouseOp::Move {
                    dx: ev.move_dx,
                    dy: ev.move_dy,
                },
                description: None,
            });
        }
        self.actions.push(Action::Mouse {
            op,
            description: None,
        });
    }

    pub(crate) fn into_actions(self) -> Vec<Action> {
        self.actions
    }
}
