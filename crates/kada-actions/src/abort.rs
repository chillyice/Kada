//! 动作执行的「中止开关」与「正在执行」计数。
//!
//! 中止用**代数**而不是布尔量：布尔量必须在「新一轮执行开始时清零」，而清零与用户按下
//! 停止之间存在竞态——停止请求可能落在清零之前被下一轮吞掉（表现为「点了停止没反应」）；
//! 反过来，若不清零，一次停止会让之后**所有**触发都一启动就自杀。
//! 改成代数计数器：`request()` 只递增，每轮执行开始时记下当时的代数（[`generation`]），
//! 之后两者不等就说明「这一轮被叫停了」。新一轮天然拿到新代数，与旧的停止请求互不干扰。

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// 中止代数。每次 [`request`] 递增；动作执行用开始时的快照比对。
static ABORT_GEN: AtomicU64 = AtomicU64::new(0);

/// 正在执行的动作链数量（供 UI 回答「现在有没有东西可以停」）。
static RUNNING: AtomicUsize = AtomicUsize::new(0);

/// 请求中止当前正在执行的所有动作链，返回递增后的代数（诊断用）。
pub fn request() -> u64 {
    ABORT_GEN.fetch_add(1, Ordering::SeqCst) + 1
}

/// 当前中止代数：动作执行开始时取一次快照。
pub fn generation() -> u64 {
    ABORT_GEN.load(Ordering::SeqCst)
}

/// 相对快照 `gen` 是否已被请求中止。
pub fn aborted(gen: u64) -> bool {
    ABORT_GEN.load(Ordering::SeqCst) != gen
}

/// 正在执行的动作链数量。
pub fn running() -> usize {
    RUNNING.load(Ordering::SeqCst)
}

/// 「正在执行」计数器的作用域守卫：动作链的入口持有它，函数怎么退出都会减回去
/// （动作链里有 `?` 提前返回，手工加减必然会漏，漏一次 UI 就永远显示「执行中」）。
pub(crate) struct RunGuard;

impl RunGuard {
    pub(crate) fn new() -> Self {
        RUNNING.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        RUNNING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 测试用的串行锁：中止是**进程级**全局状态，而 `cargo test` 的用例默认并行跑，
/// 一个用例的 `request()` 会把另一个用例正在等的子进程一起杀掉。所有会读 / 写中止状态的
/// 用例（含其它模块里跑真进程的用例）都必须先拿这把锁。
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_snapshot_is_not_aborted_by_itself() {
        let _g = test_lock();
        let gen = generation();
        assert!(!aborted(gen));
    }

    #[test]
    fn request_aborts_older_generation_only() {
        let _g = test_lock();
        let old = generation();
        request();
        assert!(aborted(old), "旧代数应被判定为已中止");
        assert!(!aborted(generation()), "新一轮拿到的代数不受旧中止影响");
    }

    #[test]
    fn running_counter_returns_to_zero() {
        let _g = test_lock();
        let before = running();
        {
            let _guard = RunGuard::new();
            assert_eq!(running(), before + 1);
        }
        assert_eq!(running(), before, "守卫析构后计数必须回退");
    }
}
