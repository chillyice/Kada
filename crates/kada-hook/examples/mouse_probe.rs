//! 鼠标侧键「按了没反应」的分诊探针（临时诊断工具）。
//!
//! ```text
//! cargo run -p kada-hook --example mouse_probe
//! ```
//!
//! 把两件事**分开**验证（这两件事混在一起时看不出来是哪一环坏了）：
//!
//! - **A. 按需挂钩子的机制**：`set_mouse_enabled(false)` → `start` → `set_mouse_enabled(true)`
//!   是否真的触发了重装（看 `reinstall_count()` 涨没涨）。不涨 = 壳层「配置里新增鼠标键
//!   触发 → 重挂鼠标钩子」这条链断了。
//! - **B. 侧键事件到底有没有到得了低层钩子**：本探针自己再挂一枚 `WH_MOUSE_LL`，把每一
//!   个中键 / 侧键事件连同 `LLMHF_INJECTED` 标志打出来；同时 `kada-hook` 自己的 handler
//!   也会打印它看到的鼠标键事件。两者对照即可定位：
//!   - 探针看得到（非注入）+ kada handler 没看到 → kada 的鼠标钩子没装上（问题在 A）。
//!   - 探针看得到（**注入**标志置位）→ 侧键是**别的程序（鼠标厂商驱动）注入**的，
//!     `kada-hook` 有意放行注入事件防回环（见 `win.rs` 注释）→ 这类鼠标改键不了。
//!   - 探针什么也没看到 → 这颗鼠标的侧键**根本没产生 `XBUTTON` 消息**（驱动在 HID /
//!     厂商层就把它消化掉了）→ 低层钩子无能为力，换鼠标或改厂商软件里的映射。
//!
//! 跑起来后**请按几下鼠标中键 / 侧键**（两个侧键都按），窗口会实时打印 30 秒。
//!
//! 只在 Windows 上有意义（低层鼠标钩子）；其它平台给个占位 `main`，免得
//! `--all-targets` 交叉检查在这里找不到入口。

#[cfg(not(windows))]
fn main() {
    println!("鼠标侧键探针只覆盖 Windows（低层鼠标钩子是 Windows 侧的实现细节）。");
}

#[cfg(windows)]
fn main() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use kada_hook::win::{reinstall_count, set_mouse_enabled, start, Action, KeyEvent};
    use kada_hook::Key;

    println!("咔哒 · 鼠标侧键分诊探针（Windows）");
    println!("———————————————————————————————————————————");

    let seen_handler = Arc::new(AtomicU64::new(0));

    // ---- A. 按需挂钩子：关 → 起 → 开 ----------------------------------------
    // 与壳层启动时的顺序一致：先 set_mouse_enabled（此刻钩子还没起，只记值），再 start。
    set_mouse_enabled(false);
    let h = {
        let n = seen_handler.clone();
        start(move |ev: KeyEvent| {
            let key = match ev {
                KeyEvent::Down { key, .. } | KeyEvent::Up { key, .. } => key,
            };
            if matches!(key, Key::MouseMiddle | Key::MouseBack | Key::MouseForward) {
                n.fetch_add(1, Ordering::Relaxed);
                println!("[kada-hook] {ev:?}");
            }
            Action::Allow // 只观察，绝不吞——吞掉会干扰下面的对照
        })
        .expect("安装钩子失败")
    };
    std::thread::sleep(std::time::Duration::from_millis(300));
    println!("A1 起钩子（配置无鼠标键）→ reinstall_count = {}", reinstall_count());

    set_mouse_enabled(true);
    std::thread::sleep(std::time::Duration::from_millis(600));
    let after_true = reinstall_count();
    println!("A2 配置新增鼠标键触发 → set_mouse_enabled(true) → reinstall_count = {after_true}");

    set_mouse_enabled(false);
    std::thread::sleep(std::time::Duration::from_millis(600));
    println!("A3 配置移除鼠标键触发 → set_mouse_enabled(false) → reinstall_count = {}", reinstall_count());

    if after_true == 0 {
        println!("   ✗ A 判定：重挂请求**没有**落地（reinstall_count 没涨）——问题在壳层/钩子层的重装链。");
    } else {
        println!("   ✓ A 判定：重装机制本身是通的（+{after_true}）。");
    }
    // 留成「开着」的状态，B 段才有鼠标钩子可用。
    set_mouse_enabled(true);
    std::thread::sleep(std::time::Duration::from_millis(600));
    println!("A4 回到「配置有鼠标键」状态 → reinstall_count = {}", reinstall_count());
    println!("———————————————————————————————————————————");

    // ---- B. 自己再挂一枚裸 WH_MOUSE_LL，看事件本身 ---------------------------
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let raw_thread = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::*;

            static RAW_SEEN: AtomicU64 = AtomicU64::new(0);
            static RAW_INJECTED: AtomicU64 = AtomicU64::new(0);

            unsafe extern "system" fn proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
                if code >= 0 {
                    let ms = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
                    let xb = ms.mouseData >> 16;
                    let name = match wparam.0 as u32 {
                        WM_MBUTTONDOWN => Some("middle down"),
                        WM_MBUTTONUP => Some("middle up"),
                        WM_XBUTTONDOWN => Some(if xb == 1 { "X1(back) down" } else { "X2(forward) down" }),
                        WM_XBUTTONUP => Some(if xb == 1 { "X1(back) up" } else { "X2(forward) up" }),
                        _ => None,
                    };
                    if let Some(name) = name {
                        let injected = ms.flags & LLMHF_INJECTED != 0;
                        println!(
                            "[raw hook]   {name:<16} injected={}",
                            if injected { "是 ✗（别的程序注入的）" } else { "否" }
                        );
                        RAW_SEEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if injected {
                            RAW_INJECTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
                unsafe { CallNextHookEx(None, code, wparam, lparam) }
            }

            let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(proc), None, 0) }
                .expect("挂裸鼠标钩子失败");
            while !stop.load(Ordering::Relaxed) {
                // 低层钩子只在装了它的线程上被回调，得有消息循环。
                let mut msg = MSG::default();
                unsafe {
                    if PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let total = RAW_SEEN.load(Ordering::Relaxed);
            let inj = RAW_INJECTED.load(Ordering::Relaxed);
            unsafe { _ = UnhookWindowsHookEx(hook) };
            (total, inj)
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(300));

    println!("B 现在开始 30 秒：**请按几下中键、侧键后退、侧键前进**（3 个都按）。");
    println!("   （两个钩子都在看同一个事件：`kada-hook` 的会吞掉匹配的键，本探针只观察）");
    for i in (1..=30).rev() {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if i % 10 == 0 {
            println!("   还有 {i} 秒…（已看到 kada-hook={} 个）", seen_handler.load(Ordering::Relaxed));
        }
    }
    stop.store(true, Ordering::Relaxed);
    let (raw_total, raw_inj) = raw_thread.join().unwrap_or((0, 0));
    drop(h);

    // ---- 结论 ---------------------------------------------------------------
    let handler = seen_handler.load(Ordering::Relaxed);
    println!("———————————————————————————————————————————");
    println!("对照结果：kada-hook handler 看到 {handler} 个鼠标键事件；裸钩子看到 {raw_total} 个（其中注入 {raw_inj} 个）");
    if raw_total == 0 {
        println!("✗ 裸钩子一个 XBUTTON 都没看到 → 这颗鼠标的侧键**没产生 XBUTTON 消息**（厂商驱动在更底层就消化了）。低层钩子拦不到 → 只能换鼠标，或在鼠标厂商软件里改映射。");
    } else if raw_inj == raw_total {
        println!("✗ 看到的全是**注入**事件 → 侧键是别的程序（鼠标厂商驱动）注入的；`kada-hook` 有意放行注入事件防回环 → 这类侧键当前改不了。");
    } else if handler == 0 {
        println!("✗ 裸钩子看到了非注入侧键，kada-hook handler 却一个都没看到 → **kada 的鼠标钩子没装上**（问题在按需安装链），这是代码 bug，需要修。");
    } else {
        println!("✓ 两个钩子都看到了侧键事件，且 A 段机制正常 → 事件链路没问题；若应用里仍无反应，问题在状态机/配置匹配那一侧（把这份输出发我）。");
    }
}
