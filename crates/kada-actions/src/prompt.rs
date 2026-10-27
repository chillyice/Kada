//! 「设置变量」输入框的宿主实现（壳层注册，可选）。
//!
//! 动作执行本身不依赖 Tauri，画不出带主题的窗口；所以把「要用户填一个值」这件事交给
//! 宿主：壳层用 [`install`] 注册一个实现（带主题的独立小窗），本 crate 只管调用。
//! 没注册（单测 / 无壳运行）时调用方回落系统输入框，行为与以前一致。
//!
//! 实现跑在动作链的执行线程上，可以阻塞到用户填完——**调用方保证不在钩子回调里调**。

use std::sync::{Mutex, OnceLock};

/// 宿主实现：`Ok(用户填的内容)`（空串按「留空」处理）、`Err(失败原因)`。
///
/// 取消 / 中止都由实现自己决定文案：用户点取消与「停止执行」用户看到的该是两句话。
pub type Handler = dyn Fn(&str, &str) -> Result<String, String> + Send + Sync;

fn slot() -> &'static Mutex<Option<Box<Handler>>> {
    static SLOT: OnceLock<Mutex<Option<Box<Handler>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// 注册宿主实现（壳层启动时调一次；重复注册以后来的为准）。
pub fn install(h: Box<Handler>) {
    *slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(h);
}

/// 调用宿主实现；没注册返回 `None`（调用方回落系统输入框）。
pub fn ask(prompt: &str, default: &str) -> Option<Result<String, String>> {
    // 持锁调用：注册只发生在启动，调用期与注册期互斥没有代价，也省得克隆 `Box`。
    let guard = slot().lock().unwrap_or_else(|e| e.into_inner());
    guard.as_ref().map(|h| h(prompt, default))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 一条用例跑完全部断言：注册是进程级全局，拆成两条并行跑会互相踩。
    #[test]
    fn installed_handler_answers_and_errors_pass_through() {
        install(Box::new(|prompt, default| Ok(format!("{prompt}|{default}"))));
        assert_eq!(ask("名字", "Kada"), Some(Ok("名字|Kada".to_string())));
        // 注册的实现说了算：取消 / 中止的文案由它给，这里原样透传。
        install(Box::new(|_, _| Err("已中止".into())));
        assert_eq!(ask("名字", ""), Some(Err("已中止".to_string())));
    }
}
