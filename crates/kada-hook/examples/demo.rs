//! M1 冒烟 demo：手动验证钩子引擎。
//!
//! ```
//! cargo run -p kada-hook --example demo
//! ```
//!
//! 打开记事本 / 任意输入框后：
//! - `Ctrl+Alt+K` —— 输入文本「咔哒 Kada」（剪贴板粘贴）
//! - `Ctrl+Alt+M` —— 改键演示：吞掉 M，改发 N（按住可重复）
//! - `Ctrl+Alt+Q` —— 退出

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use kada_hook::win::simulate;
use kada_hook::win::{start, Action, KeyEvent};
use kada_hook::{Key, Modifier};

fn main() {
    println!("咔哒 M1 · Windows 钩子引擎 demo");
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
                if let Err(e) = simulate::type_text("咔哒 Kada") {
                    eprintln!("type_text 失败: {e}");
                }
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