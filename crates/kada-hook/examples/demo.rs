//! M1 冒烟 demo：手动验证钩子引擎（Windows / Linux / macOS）。
//!
//! ```text
//! cargo run -p kada-hook --example demo
//! ```
//!
//! 打开记事本 / 任意输入框后：
//! - `Ctrl+Alt+K` —— 输入文本「咔哒 Kada」（剪贴板粘贴；macOS 上是 ⌃⌥K）
//! - `Ctrl+Alt+M` —— 改键演示：吞掉 M，改发 N（按住可重复）
//! - `Ctrl+Alt+Q` —— 退出
//!
//! 平台前提（起不来时先看这条，`start` 的报错会把同一个说法再讲一遍）：
//! - **Windows**：无需额外权限。
//! - **Linux**：要 root，或加入 `input` 组并放行 `/dev/uinput`。
//! - **macOS**：要在「系统设置 → 隐私与安全性 → 辅助功能」里勾选**运行本 demo 的终端**
//!   并重启终端。`cargo run` 跑的是裸二进制而不是 .app，授权面板里认的是终端这个宿主程序。
//!
//! 这个 demo 是输入层「真人按键」验证的入口：低层钩子与事件 tap 都会忽略合成事件，
//! 状态机没法用脚本驱动，只能人来按。

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
use std::collections::BTreeSet;
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
use std::sync::Arc;

#[cfg(windows)]
use kada_hook::win::{simulate, start, Action, KeyEvent};
#[cfg(target_os = "linux")]
use kada_hook::linux::{simulate, start, Action, KeyEvent};
#[cfg(target_os = "macos")]
use kada_hook::macos::{simulate, start, Action, KeyEvent};

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
use kada_hook::{Key, Modifier};

#[cfg(windows)]
const PLATFORM: &str = "Windows";
#[cfg(target_os = "linux")]
const PLATFORM: &str = "Linux";
#[cfg(target_os = "macos")]
const PLATFORM: &str = "macOS";

fn main() {
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    run();

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    println!("本平台没有钩子引擎实现（demo 只覆盖 Windows / Linux / macOS）。");
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
fn run() {
    println!("咔哒 M1 · {PLATFORM} 钩子引擎 demo");
    println!("  Ctrl+Alt+K  输入文本「咔哒 Kada」");
    println!("  Ctrl+Alt+M  按下后实际输出 N");
    println!("  Ctrl+Alt+Q  退出");
    println!("（焦点放到可输入文本的地方再按）");

    let quit = Arc::new(AtomicBool::new(false));
    let q = quit.clone();

    let handle = start(move |ev| {
        let ctrl_alt = |mods: &BTreeSet<Modifier>| {
            mods.contains(&Modifier::Ctrl) && mods.contains(&Modifier::Alt)
        };
        match ev {
            KeyEvent::Down { key: Key::K, mods, repeat: false } if ctrl_alt(&mods) => {
                eprintln!("[触发] Ctrl+Alt+K -> 输入文本");
                // 注入必须离开回调线程：文本走剪贴板 + Ctrl+V / ⌘V，要上百毫秒，跑在钩子
                // 回调里会被系统判超时（Windows 摘掉钩子 / macOS 停用 tap）。壳层同样是把
                // 动作执行丢到后台线程，这里照做，demo 才与真实路径一致。
                std::thread::spawn(|| {
                    // 触发键的修饰键此刻还物理按着，不等它松开，粘贴会被污染成 Ctrl+Alt+V。
                    simulate::wait_modifiers_released(300);
                    if let Err(e) = simulate::type_text("咔哒 Kada") {
                        eprintln!("type_text 失败: {e}");
                    }
                });
                Action::Block
            }
            KeyEvent::Down { key: Key::M, mods, repeat } if ctrl_alt(&mods) => {
                if !repeat {
                    eprintln!("[改键] M -> N");
                }
                Action::Replace(Key::N)
            }
            KeyEvent::Down { key: Key::Q, mods, repeat: false } if ctrl_alt(&mods) => {
                eprintln!("[退出]");
                q.store(true, Ordering::Relaxed);
                Action::Block
            }
            _ => Action::Allow,
        }
    })
    .expect("安装钩子失败");

    while !quit.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    drop(handle);
    println!("已退出。");
}
