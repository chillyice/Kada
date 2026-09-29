//! 钩子引擎冒烟：安装 → 消息循环跑起来 → 看门狗静默 / 重装 → 卸载回收。
//! 触发路径（swallow/Replace/注入）需真人按键，由 examples/demo.rs 验证。

#![cfg(windows)]

use std::time::{Duration, Instant};

use kada_hook::win::{reinstall_count, request_reinstall, start, Action, KeyEvent};

#[test]
fn install_reinstall_and_uninstall() {
    let before = reinstall_count();
    let handle = start(|_ev: KeyEvent| Action::Allow).expect("安装钩子");

    // 静置 2.5s（看门狗巡检 ≥2 轮）：钩子活着时看门狗必须一声不吭。它一旦误判就会
    // 每秒重装一次，重装窗口里的按键被丢掉——用户看到的是「偶尔按了没反应」。
    std::thread::sleep(Duration::from_millis(2_500));
    assert_eq!(reinstall_count(), before, "闲置期间不该触发自愈重装");

    // 主动触发一次重装：真正换钩子句柄的那段 unsafe（卸旧 → 装新，必须成对，否则
    // 每个按键被处理两遍）只有走过这条路才算验证过。
    assert!(request_reinstall(), "重装请求应能投递到钩子线程");
    let deadline = Instant::now() + Duration::from_secs(2);
    while reinstall_count() == before && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(reinstall_count(), before + 1, "重装应已在钩子线程落地");

    drop(handle); // 停看门狗 → 发 WM_QUIT → join 两个线程
}
