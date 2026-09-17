//! 钩子引擎冒烟：安装 → 消息循环跑起来 → 卸载回收。
//! 触发路径（swallow/Replace/注入）需真人按键，由 examples/demo.rs 验证。

#![cfg(windows)]

use kada_hook::win::{start, Action, KeyEvent};

#[test]
fn install_and_uninstall() {
    let handle = start(|_ev: KeyEvent| Action::Allow).expect("安装钩子");
    std::thread::sleep(std::time::Duration::from_millis(150));
    drop(handle); // 发 WM_QUIT 并 join 钩子线程
}