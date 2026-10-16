//! 输入层环境诊断探针（规划 7.2-㊽ / `docs/底层输入能力规划.md` 档位 A）。
//!
//! 打印本机探测结果：认出了哪些「按键接管者」、内在完整性（HVCI）开没开。真机上
//! 「到底认出谁、有没有误报」只能靠它核对——应用里那张卡片只显示结论，不显示匹配细节。
//!
//! 用法：`cargo run -p kada-hook --example input_env_probe`
//!
//! 核对要点：
//! - 装了 Logi Options+ 的机器上应认出 `logi-options-plus`；退出 Options+ 再跑一次应为空。
//! - 「没认出来」分两种：真没装（正常），或装了但进程名不在目录里（要补
//!   [`kada_core::DIVERTERS`] 的匹配片段，见该结构体 `processes`）。

fn main() {
    let env = kada_hook::platform_input_env();
    println!("== 输入层环境诊断 ==");
    if env.diverters.is_empty() {
        println!("接管者：未检测到");
    } else {
        println!("接管者：检测到 {} 个", env.diverters.len());
        for id in &env.diverters {
            let name = kada_core::DIVERTERS
                .iter()
                .find(|d| d.id == id)
                .map(|d| d.name)
                .unwrap_or("<目录里没有这个 id>");
            println!("  - {id}（{name}）");
        }
    }
    match env.hvci {
        Some(true) => println!("内存完整性（HVCI）：已开启"),
        Some(false) => println!("内存完整性（HVCI）：已关闭"),
        None => println!("内存完整性（HVCI）：读不到（非 Windows，或键不可读）"),
    }
    println!("\n目录共 {} 类；要认的别的接管者往 `kada_core::DIVERTERS` 里加。", kada_core::DIVERTERS.len());
}
