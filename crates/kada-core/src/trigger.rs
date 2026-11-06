// 触发单元（C-02 从 lib.rs 搬出）：组合 / 序列 / 和弦的解析与时序，
// 以及文本扩展的键→字符映射。超时 / Esc 判定与注入由壳层负责，这里只放纯逻辑。

use std::collections::BTreeSet;

use crate::keys::{
    format_shortcut, matches, modifier_key, parse, Key, Modifier, ParseError, RawEvent, Shortcut,
};
// ---------------------------------------------------------------------------
// 键序列（leader key）触发：触发键从「单次组合」扩展为「组合 或 按键序列」。
// 核心只放纯逻辑（解析 + 匹配时序），超时/Esc 判定与注入由壳层负责。
// ---------------------------------------------------------------------------

/// 触发键单元：单个组合键，一串按键序列，或一组同时按住的键（和弦）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Trigger {
    /// 单次组合键（如 `Ctrl+K`）。
    Combo(Shortcut),
    /// 按键序列（如 `F9 J K`：依次按下 F9 → J → K）。首步即 leader 键。
    Sequence(Vec<Shortcut>),
    /// 和弦（如 `F&J`、`Ctrl+F&J`：同时按住这些键）。无序。成员可带修饰键，
    /// 也可全是修饰键（只作「要求按住」），但至少要有一个非修饰键成员。
    Chord(Vec<Shortcut>),
}

impl Trigger {
    /// 解析触发键字符串：单个 token → 组合键，多个 token（空白分隔）→ 序列，
    /// 含 `&` 的单个 token → 和弦。约定：组合键用 `+` 且不含空格；序列步之间
    /// 用空格；和弦成员之间用 `&`。`"Space"`（空格键）是单 token 仍为组合，
    /// `"F9 Space"` 则是「F9 后接空格」的序列。
    pub fn parse(s: &str) -> Result<Trigger, ParseError> {
        let tokens: Vec<&str> = s.split_whitespace().collect();
        match tokens.as_slice() {
            [] => Err(ParseError::Empty),
            [one] => {
                if one.contains('&') {
                    return parse_chord(one);
                }
                Ok(Trigger::Combo(one.parse()?))
            }
            _ => {
                let steps = tokens
                    .iter()
                    .map(|t| t.parse::<Shortcut>())
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Trigger::Sequence(steps))
            }
        }
    }

    /// 是否为序列（≥2 步）。
    pub fn is_sequence(&self) -> bool {
        matches!(self, Trigger::Sequence(_))
    }

    /// 是否为和弦（同时按住多个键）。
    pub fn is_chord(&self) -> bool {
        matches!(self, Trigger::Chord(_))
    }

    /// 各步（组合键返回单元素切片，便于统一遍历）。
    pub fn steps(&self) -> &[Shortcut] {
        match self {
            Trigger::Combo(s) => std::slice::from_ref(s),
            Trigger::Sequence(steps) => steps,
            Trigger::Chord(members) => members,
        }
    }

    /// 首步（leader 键）。
    pub fn first(&self) -> &Shortcut {
        &self.steps()[0]
    }
}

/// 解析和弦触发键（`&` 分隔同时按住的键，如 `F&J`、`Ctrl+F&J`、`Ctrl+Alt&J`）。
///
/// 成员是 [`Shortcut`]：`key` = **要一起按住的键**，`mods` = **凑齐那一刻必须按住的修饰键**。
/// 于是 `Ctrl+F&J` = 「按住 Ctrl 的同时把 F、J 一起按住」。成员可以全是修饰要求
/// （`Ctrl+Alt&J` 的前半），但**至少要有一个非修饰键成员**——纯修饰键只表示「要求按住」，
/// 没有可吞的键，也就没有「同时按下」的判定时机。
fn parse_chord(s: &str) -> Result<Trigger, ParseError> {
    let raw: Vec<&str> = s.split('&').collect();
    if raw.iter().any(|m| m.trim().is_empty()) {
        return Err(ParseError::ChordInvalid("和弦成员不能为空".into()));
    }
    let members: Vec<Shortcut> = raw
        .iter()
        .map(|m| parse_chord_member(m))
        .collect::<Result<_, _>>()?;
    if members.len() < 2 {
        return Err(ParseError::ChordInvalid("和弦至少需要两个键".into()));
    }
    for (i, m) in members.iter().enumerate() {
        if members[..i].contains(m) {
            return Err(ParseError::ChordInvalid(format!(
                "和弦成员「{}」重复",
                format_shortcut(m)
            )));
        }
    }
    if !members.iter().any(|m| !m.key.is_modifier()) {
        return Err(ParseError::ChordInvalid(
            "和弦至少要有一个非修饰键成员（修饰键只作「要求按住」用）".into(),
        ));
    }
    Ok(Trigger::Chord(members))
}

/// 解析一个和弦成员：常规组合（`Ctrl+F`）直接走 [`Shortcut`]；纯修饰键组合（`Ctrl+Alt`）
/// 没有主键，交给 [`modifier_only_member`] 折叠。解析失败时保留 [`Shortcut`] 那条更具体的
/// 报错（「未知按键: K」比笼统的「和弦无效」有用）。
fn parse_chord_member(s: &str) -> Result<Shortcut, ParseError> {
    match s.parse::<Shortcut>() {
        Ok(sc) => Ok(sc),
        Err(e) => modifier_only_member(s).ok_or(e),
    }
}

/// 纯修饰键成员（`Ctrl+Alt`：每个 token 都是修饰键名、且至少两个）折叠成
/// `{mods: 前面的, key: 最后一个}`。取最后一个当「成员键」是为了沿用同一套 `Shortcut`
/// 表示，且 [`format_shortcut`] 按 Ctrl→Alt→Shift→Meta 渲染，显示会原样回到 `Ctrl+Alt`。
fn modifier_only_member(s: &str) -> Option<Shortcut> {
    let parts: Vec<&str> = s
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let mut mods: Vec<Modifier> = parts
        .iter()
        .map(|p| parse::modifier(p))
        .collect::<Option<Vec<_>>>()?;
    if mods.len() < 2 {
        return None;
    }
    let key = modifier_key(mods.pop()?);
    Some(Shortcut {
        mods: mods.into_iter().collect(),
        key,
    })
}

/// 键序列匹配的结果。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeqAdvance {
    /// 事件与任何序列无关（未开始 / 断链后重置）。
    NoMatch,
    /// 事件推进了某条序列（应吞掉，继续等待下一键）。
    Advance,
    /// 事件完成了一条序列，`usize` 为命中序列在传入列表中的下标（应吞掉并触发）。
    Complete(usize),
}

/// 键序列匹配的运行时状态（纯逻辑，时序由壳层驱动）。记录当前激活的候选序列
/// 及其已匹配步数，天然支持共享前缀（如 `F9 J K` 与 `F9 J L` 并存）。
#[derive(Clone, Debug, Default)]
pub struct SequenceTracker {
    /// (序列下标, 完整步骤, 已匹配步数)。空 = 未处于序列模式。
    candidates: Vec<(usize, Vec<Shortcut>, usize)>,
}

/// 键序列等待窗的默认毫秒数（leader 后超过该时长未按下一步即回退）。
///
/// 实际取值来自 [`Settings::sequence_timeout_ms`]（设置页可改，`0` = 不限时）；
/// 本常量只是缺省值：老配置（写于该字段出现之前）与 `Settings::default` 都用它。
pub const DEFAULT_SEQUENCE_TIMEOUT_MS: u64 = 1000;

/// 和弦等待窗的默认毫秒数（按下部分成员后，等其余成员的最长时间）。
///
/// 实际取值来自 [`Settings::chord_timeout_ms`]（设置页可改，`0` = 不限时：等成员键
/// 全部抬起才定论）；本常量只是缺省值。
pub const DEFAULT_CHORD_TIMEOUT_MS: u64 = 1000;

impl SequenceTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否有正在进行的序列。
    pub fn is_active(&self) -> bool {
        !self.candidates.is_empty()
    }

    /// 清空序列状态（超时 / Esc / 触发完成后由壳层调用）。
    pub fn reset(&mut self) {
        self.candidates.clear();
    }

    /// 喂入一个按键事件，返回推进结果。`sequences` 是当前层生效的所有序列步骤，
    /// 顺序与调用方的触发器列表一致（`Complete(usize)` 的 `usize` 即其下标）。
    pub fn advance(&mut self, ev: &RawEvent, sequences: &[Vec<Shortcut>]) -> SeqAdvance {
        if self.candidates.is_empty() {
            // 以本事件作为某序列首步（leader）开启序列。共享同一 leader 的多条序列
            // （如「F9 J K」与「F9 J L」）必须一并建立候选，否则只有第一条能走到下一步。
            for (idx, steps) in sequences.iter().enumerate() {
                let Some(first) = steps.first() else { continue };
                if matches(ev, first) {
                    self.candidates.push((idx, steps.clone(), 1));
                }
            }
            return if self.candidates.is_empty() {
                SeqAdvance::NoMatch
            } else {
                SeqAdvance::Advance
            };
        }

        // 推进所有命中下一步的候选；任一走完即完成，全都不命中则断链重置。
        let mut advanced: Vec<(usize, Vec<Shortcut>, usize)> = Vec::new();
        for (idx, steps, progress) in self.candidates.iter() {
            let idx = *idx;
            let progress = *progress;
            let Some(next) = steps.get(progress) else {
                continue;
            };
            if matches(ev, next) {
                if progress + 1 == steps.len() {
                    self.reset();
                    return SeqAdvance::Complete(idx);
                }
                advanced.push((idx, steps.clone(), progress + 1));
            }
        }
        if advanced.is_empty() {
            // 所有候选都断链：重置，事件放行给普通 decide。
            self.reset();
            return SeqAdvance::NoMatch;
        }
        self.candidates = advanced;
        SeqAdvance::Advance
    }
}

/// 和弦（同时按住多个键）匹配的结果。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChordAdvance {
    /// 该键不是任何和弦的成员（集合不变）。
    NoMatch,
    /// 该键是某和弦成员但尚未凑齐（应吞掉，继续等待其它成员）。
    Await,
    /// 该键凑齐了某和弦，`usize` 为命中和弦在传入列表中的下标（应吞掉并触发）。
    Complete(usize),
}

/// 和弦（同时按住多个键）匹配的运行时状态（纯逻辑，时序由壳层驱动）。
/// 记录当前按住的成员键集合，凑齐某和弦的所有成员即触发。成员键按下即被吞，
/// 集合由 [`ChordTracker::press`] / [`ChordTracker::release`] 成对维护：抬起即移出，
/// 保证「同时按住」判定准确，也让壳层能据此判定「没凑成 → 原键回放」的时机。
#[derive(Clone, Debug, Default)]
pub struct ChordTracker {
    /// 当前按住、尚未凑齐的成员键。**只放非修饰键**：修饰键成员（`Ctrl+F` 里的 Ctrl、
    /// `Ctrl+Alt&J` 里的 Ctrl/Alt）是「要求按住」而非「可吞的成员」，它们从不被吞，
    /// 其按住状态由传入的 `mods` 判定。
    held: BTreeSet<Key>,
}

impl ChordTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否有正在等待凑齐的和弦。
    pub fn is_active(&self) -> bool {
        !self.held.is_empty()
    }

    /// 当前按住的成员键（壳层据此判断「成员已全部抬起 → 定论回放」）。
    pub fn held(&self) -> &BTreeSet<Key> {
        &self.held
    }

    /// 清空和弦状态（超时 / 触发完成后由壳层调用）。
    pub fn reset(&mut self) {
        self.held.clear();
    }

    /// 喂入一个成员键按下。`chords` 是当前层生效的所有和弦成员集合，顺序与调用方的
    /// 触发器列表一致（`Complete(usize)` 的 `usize` 即其下标）。和弦成员是无序的
    /// 「同时按住」：所有成员键都在按住集合中即命中。
    ///
    /// `mods` 是**按下这一刻**按住的修饰键（钩子层按物理状态填入，已含刚按下的那个
    /// 修饰键）。成员的修饰要求按它判：凑齐那一刻必须都按着——`Ctrl+F&J` 里 F 的 Ctrl
    /// 要求、`Ctrl+Alt&J` 里那两个纯修饰键成员都走这里，故修饰键无需进按住集合。
    pub fn press(
        &mut self,
        key: Key,
        mods: &BTreeSet<Modifier>,
        chords: &[Vec<Shortcut>],
    ) -> ChordAdvance {
        // 修饰键成员（`Ctrl+F&J` 里的 Ctrl、`Ctrl+Alt&J` 里的 Ctrl/Alt）只作「要求按住」用，
        // 不是可吞的成员：壳层从不把它交给这里（吞掉修饰键会把 `Ctrl+Alt+Tab` 变成
        // `Ctrl+Tab`），这里再挡一道，保证按住集合里只有非修饰键。
        if key.is_modifier() {
            return ChordAdvance::NoMatch;
        }
        // 事件键不是任何和弦成员：集合不变（断链与否由壳层决定，成员键不回放）。
        if !chords.iter().any(|c| c.iter().any(|m| m.key == key)) {
            return ChordAdvance::NoMatch;
        }
        self.held.insert(key);
        for (idx, chord) in chords.iter().enumerate() {
            if chord.iter().all(|m| self.member_held(m, mods)) {
                self.held.clear();
                return ChordAdvance::Complete(idx);
            }
        }
        ChordAdvance::Await
    }

    /// 一个成员是否已满足：主键要么在按住集合里，要么本身是修饰键（看 `mods`），
    /// 且成员的修饰要求全部在 `mods` 里。
    fn member_held(&self, m: &Shortcut, mods: &BTreeSet<Modifier>) -> bool {
        let key_ok = match m.key.as_modifier() {
            Some(md) => mods.contains(&md),
            None => self.held.contains(&m.key),
        };
        key_ok && m.mods.is_subset(mods)
    }

    /// 喂入一个成员键抬起。返回该键原本是否处于按住集合中（即确实是待凑齐的成员）。
    /// 命中触发的和弦时集合已在 [`ChordTracker::press`] 里清空，故返回 false。
    pub fn release(&mut self, key: Key) -> bool {
        self.held.remove(&key)
    }
}

// ---------------------------------------------------------------------------
// 文本扩展（hotstring）：输入触发词 + 后缀自动展开。核心只放纯逻辑（键→字符
// 映射、后缀判定、触发词匹配），缓冲与注入由壳层负责。
// ---------------------------------------------------------------------------

/// 热串触发后缀：空格 / 回车 / Tab。输入这些键且前面的可打印字符命中触发词时展开。
pub fn is_hotstring_terminator(key: Key) -> bool {
    matches!(key, Key::Space | Key::Enter | Key::Tab)
}

/// [`Key`] + Shift → 可打印字符（US 布局语义，与 `kada-hook` 的键码映射一致）。
/// 非可打印键（方向键/功能键/编辑键/修饰键/媒体键/鼠标键等）返回 None。
/// 注意：`Space` 虽可打印，但调用方应先判 `is_hotstring_terminator` 再调用本函数。
pub fn key_to_char(key: Key, shift: bool) -> Option<char> {
    use Key::*;
    let c = match key {
        A => {
            if shift {
                'A'
            } else {
                'a'
            }
        }
        B => {
            if shift {
                'B'
            } else {
                'b'
            }
        }
        C => {
            if shift {
                'C'
            } else {
                'c'
            }
        }
        D => {
            if shift {
                'D'
            } else {
                'd'
            }
        }
        E => {
            if shift {
                'E'
            } else {
                'e'
            }
        }
        F => {
            if shift {
                'F'
            } else {
                'f'
            }
        }
        G => {
            if shift {
                'G'
            } else {
                'g'
            }
        }
        H => {
            if shift {
                'H'
            } else {
                'h'
            }
        }
        I => {
            if shift {
                'I'
            } else {
                'i'
            }
        }
        J => {
            if shift {
                'J'
            } else {
                'j'
            }
        }
        K => {
            if shift {
                'K'
            } else {
                'k'
            }
        }
        L => {
            if shift {
                'L'
            } else {
                'l'
            }
        }
        M => {
            if shift {
                'M'
            } else {
                'm'
            }
        }
        N => {
            if shift {
                'N'
            } else {
                'n'
            }
        }
        O => {
            if shift {
                'O'
            } else {
                'o'
            }
        }
        P => {
            if shift {
                'P'
            } else {
                'p'
            }
        }
        Q => {
            if shift {
                'Q'
            } else {
                'q'
            }
        }
        R => {
            if shift {
                'R'
            } else {
                'r'
            }
        }
        S => {
            if shift {
                'S'
            } else {
                's'
            }
        }
        T => {
            if shift {
                'T'
            } else {
                't'
            }
        }
        U => {
            if shift {
                'U'
            } else {
                'u'
            }
        }
        V => {
            if shift {
                'V'
            } else {
                'v'
            }
        }
        W => {
            if shift {
                'W'
            } else {
                'w'
            }
        }
        X => {
            if shift {
                'X'
            } else {
                'x'
            }
        }
        Y => {
            if shift {
                'Y'
            } else {
                'y'
            }
        }
        Z => {
            if shift {
                'Z'
            } else {
                'z'
            }
        }
        Digit0 => {
            if shift {
                ')'
            } else {
                '0'
            }
        }
        Digit1 => {
            if shift {
                '!'
            } else {
                '1'
            }
        }
        Digit2 => {
            if shift {
                '@'
            } else {
                '2'
            }
        }
        Digit3 => {
            if shift {
                '#'
            } else {
                '3'
            }
        }
        Digit4 => {
            if shift {
                '$'
            } else {
                '4'
            }
        }
        Digit5 => {
            if shift {
                '%'
            } else {
                '5'
            }
        }
        Digit6 => {
            if shift {
                '^'
            } else {
                '6'
            }
        }
        Digit7 => {
            if shift {
                '&'
            } else {
                '7'
            }
        }
        Digit8 => {
            if shift {
                '*'
            } else {
                '8'
            }
        }
        Digit9 => {
            if shift {
                '('
            } else {
                '9'
            }
        }
        Comma => {
            if shift {
                '<'
            } else {
                ','
            }
        }
        Period => {
            if shift {
                '>'
            } else {
                '.'
            }
        }
        Slash => {
            if shift {
                '?'
            } else {
                '/'
            }
        }
        Backslash => {
            if shift {
                '|'
            } else {
                '\\'
            }
        }
        Semicolon => {
            if shift {
                ':'
            } else {
                ';'
            }
        }
        Quote => {
            if shift {
                '"'
            } else {
                '\''
            }
        }
        Backquote => {
            if shift {
                '~'
            } else {
                '`'
            }
        }
        Minus => {
            if shift {
                '_'
            } else {
                '-'
            }
        }
        Equal => {
            if shift {
                '+'
            } else {
                '='
            }
        }
        BracketLeft => {
            if shift {
                '{'
            } else {
                '['
            }
        }
        BracketRight => {
            if shift {
                '}'
            } else {
                ']'
            }
        }
        Space => ' ',
        _ => return None,
    };
    Some(c)
}
