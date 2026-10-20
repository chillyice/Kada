//! 罗技 HID++ 探针（`docs/底层输入能力规划.md` 档位 C）。
//!
//! 打印本机罗技 HID++ 设备与三个鼠标键（中键 / 后退 / 前进）的 divert 状态；
//! 带 `--unlock` 时顺手把 divert 关掉，再回读确认。真机上「到底能不能读、能不能写、
//! 关掉后侧键是否恢复」只能靠它核对——应用里的按钮只显示结论。
//!
//! 用法：
//!
//! ```text
//! cargo run -p kada-hook --example hidpp_probe          # 只读探测
//! cargo run -p kada-hook --example hidpp_probe -- --unlock
//! ```
//!
//! 核对要点：
//! - Logi Options+ 在跑、侧键被它 divert 时，应看到 `diverted=true`（`persistent=true`
//!   说明 Options+ 还写了持久标志，解锁会一并清掉）。
//! - `--unlock` 后回读应为 `diverted=false`；此时按侧键，Windows 应产生标准
//!   XBUTTON1 / XBUTTON2（再用 `mouse_probe` 那个例子看事件有没有到钩子）。
//! - 通道打不开时输出里会有「打不开」的原因（被独占的机器上这属预期，应用会退化为
//!   文字指引）。

fn main() {
    let unlock = std::env::args().any(|a| a == "--unlock");
    if !kada_hook::hidpp::supported() {
        println!("当前平台不支持（这是 Windows 专属能力：HID++ 控制接口只在 Windows 侧共享打开）。");
        return;
    }

    println!("== 只读探测 ==");
    let st = kada_hook::hidpp::status();
    if let Some(e) = &st.error {
        println!("整体错误：{e}");
    }
    if st.devices.is_empty() {
        println!("没有找到可用的罗技 HID++ 设备。");
    }
    for d in &st.devices {
        println!(
            "\n设备：{}（产品号 0x{:04X}，索引 0x{:02X}，{}-report，控件 {} 个）",
            d.name, d.product_id, d.index, d.channel, d.control_count
        );
        for c in &d.controls {
            if !c.present {
                println!("  - {:04X} {}：设备上没有这个控件", c.cid, c.label);
                continue;
            }
            println!(
                "  - {:04X} {}：divert={}{}",
                c.cid,
                c.label,
                c.diverted,
                if c.persistent { "（含持久标志）" } else { "" }
            );
        }
    }
    println!("\n保持解锁：{}", if st.keeper_active { "运行中" } else { "未启动" });
    if let Some(e) = &st.keeper_error {
        println!("保持解锁最近一次错误：{e}");
    }

    if !unlock {
        println!("\n（加 `-- --unlock` 可实际关掉 divert）");
        return;
    }

    println!("\n== 解锁 ==");
    let r = kada_hook::hidpp::unlock();
    if let Some(e) = &r.error {
        println!("失败：{e}");
        return;
    }
    for d in &r.devices {
        println!("\n设备：{}", d.name);
        if !d.cleared.is_empty() {
            println!("  已解除 divert：{}", d.cleared.join("、"));
        }
        if !d.already.is_empty() {
            println!("  本来就是标准报文：{}", d.already.join("、"));
        }
        if !d.missing.is_empty() {
            println!("  设备上没有：{}", d.missing.join("、"));
        }
        for f in &d.failed {
            println!("  失败：{f}");
        }
    }

    println!("\n== 回读确认 ==");
    let after = kada_hook::hidpp::status();
    for d in &after.devices {
        for c in &d.controls {
            if c.present {
                println!("  {} · {}：divert={}", d.name, c.label, c.diverted);
            }
        }
    }
    println!("\n提示：本探针进程退出后「保持解锁」就结束了；应用里的按钮会在 Kada 运行期间一直保持。");
}
