//! 键盘诊断（规划 7.2-KD）：把「硬件看到了什么」与「Kada 拿它怎么办」拼成一条时间线，
//! 供设置页实时显示。回答三个问题——**键到得了吗**（钩子有没有看见、扫描码是多少、
//! 是物理键还是注入）、**迟不迟滞**（事件排队延迟、回调停留耗时、键间隔 / 自动重复）、
//! **为什么这条快捷键没反应**（命中了哪条、被哪条拦下、还是根本没匹配）。
//!
//! 数据分两段：原始事件由 `kada-hook` 的探针在**判定之前**记录（`KeyProbe`，只有 Windows
//! 实现），处置由钩子回调在判定之后补上（[`note_key`]）。两边在同一个环形缓冲里按
//! 「同一个键 + 同一个方向 + 尚未配对」就近配对，不靠时间窗猜。
//!
//! 约束：探针回调跑在钩子回调里，只许往缓冲区追加——不建窗、不做 IO、不等别的锁
//! （见 `crates/kada-hook/src/win.rs` 模块文档与 AGENTS.md 铁律 1.4）。

use crate::*;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

/// 环形缓冲上限：设置页最多回看这么多条，再多也只占固定内存（诊断不能把内存拖起来）。
const MAX_LINES: usize = 300;
/// 配对时最多往回找几行：探针与判定是同一次回调里的前后脚，正常只差 1 行；
/// 留一点余量给「上一次的抬起还没配对」这类交叉。
const PAIR_LOOKBACK: usize = 8;

/// 时间线上的一条（序列化给前端）。
#[derive(Clone, Debug, Serialize)]
pub(crate) struct KeyDiagLine {
    /// 全局递增序号：前端按 `since` 增量拉取，判断要不要重绘看它。
    pub(crate) seq: u64,
    /// 事件时刻（毫秒，系统 tick，与 `lag_ms` 同一时钟基准）。
    pub(crate) t_ms: u32,
    /// 键的中性名；`None` = 这个扫描码 / 虚拟键码 Kada 不认识。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) key: Option<String>,
    /// 扫描码（同一键名在不同物理位置的辨别依据）。
    pub(crate) scan: u32,
    /// 扩展键标记（小键盘与方向键扫描码相同，靠它区分）。
    pub(crate) extended: bool,
    /// 按下 vs 抬起。
    pub(crate) down: bool,
    /// 自动重复（按住不放时系统反复发的 down）。
    pub(crate) repeat: bool,
    /// 注入产生（Kada 自己或别的注入工具）：注入事件不进 Kada 判定。
    pub(crate) injected: bool,
    /// 事件产生 → 钩子回调的排队延迟（毫秒）。**「迟滞」最直接的读数。**
    pub(crate) lag_ms: u32,
    /// Kada 的处置（判定后补上）：放行 / 吞掉 / 命中哪条 / 哪条让路规则生效。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) disposition: Option<String>,
}

/// 钩子体检（顶部一行）。非 Windows 恒默认值 + `supported: false`。
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub(crate) struct KeyDiagHealth {
    pub(crate) installed: bool,
    pub(crate) last_key_age_ms: u32,
    pub(crate) reinstalls: u64,
    pub(crate) last_dwell_us: u64,
    pub(crate) max_dwell_us: u64,
    pub(crate) slow_callbacks: u64,
}

/// 一次轮询的返回：增量事件 + 体检 + 是否在跑。
#[derive(Clone, Debug, Serialize)]
pub(crate) struct KeyDiagSnapshot {
    /// 本平台是否实现了诊断（非 Windows 为 false，界面据此说「暂不支持」）。
    pub(crate) supported: bool,
    pub(crate) active: bool,
    /// `seq > since` 的新事件（按时间顺序）。
    pub(crate) lines: Vec<KeyDiagLine>,
    pub(crate) latest_seq: u64,
    pub(crate) health: KeyDiagHealth,
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static SEQ: AtomicU64 = AtomicU64::new(0);
static LINES: LazyLock<Mutex<VecDeque<KeyDiagLine>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

/// 诊断是否开着（钩子回调据此决定要不要记录处置；关着时只有一次原子读）。
pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// 开始诊断：装探针、清空缓冲。非 Windows 直接报错（界面显示「暂不支持」）。
pub(crate) fn start() -> Result<(), String> {
    if !input::key_diag_supported() {
        return Err("键盘诊断目前只实现了 Windows 版本。".to_string());
    }
    clear();
    #[cfg(target_os = "windows")]
    input::set_key_probe(Some(std::sync::Arc::new(|p: &input::KeyProbe| note_raw(p))));
    ACTIVE.store(true, Ordering::Relaxed);
    Ok(())
}

/// 结束诊断：先关旗标再撤探针（撤掉后钩子回调不再产生原始事件）。
pub(crate) fn stop() {
    ACTIVE.store(false, Ordering::Relaxed);
    #[cfg(target_os = "windows")]
    input::set_key_probe(None);
}

/// 清空时间线（序号不重置：前端靠它判断增量，重置会让旧序号与新序号撞车）。
pub(crate) fn clear() {
    if let Ok(mut g) = LINES.lock() {
        g.clear();
    }
}

/// 把一条原始事件追加到时间线（探针回调里调用：**只追加**，别加别的活）。
#[cfg(target_os = "windows")]
fn note_raw(p: &input::KeyProbe) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let Ok(mut g) = LINES.lock() else { return };
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    g.push_back(KeyDiagLine {
        seq,
        t_ms: p.t_ms,
        key: p.key.map(|k| key_name(k).to_string()),
        scan: p.scan,
        extended: p.extended,
        down: p.down,
        repeat: p.repeat,
        injected: p.injected,
        lag_ms: p.lag_ms,
        disposition: None,
    });
    while g.len() > MAX_LINES {
        g.pop_front();
    }
}

/// 记录一条「Kada 拿这个键怎么办」。由钩子回调在判定之后调用。
///
/// 配对规则：从新到旧找**同一个键、同一个方向、还没配过对**的那一行（只回看几行），
/// 找到就填上处置。找不到就丢掉——诊断是旁路，绝不能因为配不上对就影响判定。
pub(crate) fn note_key(key: Key, down: bool, text: impl Into<String>) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let name = key_name(key);
    let Ok(mut g) = LINES.lock() else { return };
    for line in g.iter_mut().rev().take(PAIR_LOOKBACK) {
        if line.disposition.is_some() {
            continue;
        }
        if line.down == down && line.key.as_deref() == Some(name) {
            line.disposition = Some(text.into());
            return;
        }
    }
}

/// 取增量快照（前端每 ~150ms 轮询一次）。
pub(crate) fn snapshot(since: u64) -> KeyDiagSnapshot {
    let (lines, latest_seq) = match LINES.lock() {
        Ok(g) => (
            g.iter().filter(|l| l.seq > since).cloned().collect(),
            g.back().map(|l| l.seq).unwrap_or(0),
        ),
        Err(_) => (Vec::new(), 0),
    };
    KeyDiagSnapshot {
        supported: input::key_diag_supported(),
        active: active(),
        lines,
        latest_seq,
        health: health(),
    }
}

#[cfg(target_os = "windows")]
fn health() -> KeyDiagHealth {
    let h = input::key_health();
    KeyDiagHealth {
        installed: h.installed,
        last_key_age_ms: h.last_key_age_ms,
        reinstalls: h.reinstalls,
        last_dwell_us: h.last_dwell_us,
        max_dwell_us: h.max_dwell_us,
        slow_callbacks: h.slow_callbacks,
    }
}

#[cfg(not(target_os = "windows"))]
fn health() -> KeyDiagHealth {
    KeyDiagHealth::default()
}

#[tauri::command]
pub(crate) fn key_diag_start() -> Result<(), String> {
    start()
}

#[tauri::command]
pub(crate) fn key_diag_stop() {
    stop();
}

#[tauri::command]
pub(crate) fn key_diag_clear() {
    clear();
}

#[tauri::command]
pub(crate) fn key_diag_poll(since: u64) -> KeyDiagSnapshot {
    snapshot(since)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 处置要挂在「同一个键 + 同一个方向」的那一行上，不能串到别的键；没有原始事件时
    /// 直接丢弃（不能凭空造一行）。两个断言放在同一个测试里：它们共用全局缓冲与旗标，
    /// 分开写会被并行的测试线程互相清空。
    #[test]
    fn disposition_pairs_with_matching_key_and_direction() {
        clear();
        ACTIVE.store(true, Ordering::Relaxed);
        // 先验「配不上就丢弃」。
        note_key(Key::A, true, "无事件");
        assert!(LINES.lock().unwrap().is_empty());
        // 再造两行：A 按下、B 按下，只给 A 的按下记处置。
        for key in [Key::A, Key::B] {
            let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
            LINES.lock().unwrap().push_back(KeyDiagLine {
                seq,
                t_ms: 0,
                key: Some(key_name(key).to_string()),
                scan: 0,
                extended: false,
                down: true,
                repeat: false,
                injected: false,
                lag_ms: 0,
                disposition: None,
            });
        }
        note_key(Key::A, true, "命中「测试」");
        {
            let g = LINES.lock().unwrap();
            let a = g.iter().find(|l| l.key.as_deref() == Some("A")).unwrap();
            let b = g.iter().find(|l| l.key.as_deref() == Some("B")).unwrap();
            assert_eq!(a.disposition.as_deref(), Some("命中「测试」"));
            assert!(b.disposition.is_none());
        }
        clear();
        ACTIVE.store(false, Ordering::Relaxed);
    }
}
