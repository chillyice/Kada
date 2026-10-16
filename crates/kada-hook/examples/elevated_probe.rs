//! 提权通道探针（Windows）：把**不需要 UAC 就能验的那一段**验掉。
//!
//! 在本进程里起一个「代理」（线程里跑 [`kada_hook::win::agent::serve`]），再用客户端连上去
//! 发几批 op，覆盖：
//! - 管道 DACL 放行本用户、双方身份校验（镜像路径 / 用户 SID）通过；
//! - 握手行、行协议往返、批次回复（`ok` / `err`）；
//! - 坏 op 被整批拒掉（不是静默略过）；
//! - 一次上百个 op 的分片读写；
//! - 路由闸门：开关没开 / 前台不是提权窗口时 `forward` 返回 `None`（照旧本地注入）。
//!
//! **验不到的那一段**：UAC 拉起、提权进程真的把注入送进提权窗口——那要真人点一次 UAC，
//! 见 `docs/人工验证清单.md` §31。
//!
//! 跑法：`cargo run -p kada-hook --example elevated_probe`

#[cfg(windows)]
fn main() {
    use kada_hook::win::agent::{self, Op};
    use kada_core::{Key, MouseOp};

    let name = match agent::probe_pipe_name() {
        Ok(name) => name,
        Err(e) => {
            println!("取探针通道名失败：{e}");
            std::process::exit(1);
        }
    };
    println!("通道名：{name}");

    // 1) 路由闸门：开关没开、前台也不是提权窗口 —— 不该路由，也不该留提示。
    assert!(agent::forward(&[Op::Pause(0)]).is_none(), "开关没开时不该路由");
    assert!(agent::take_notice().is_none(), "前台非提权窗口不该留提示");
    println!("✓ 开关没开 / 前台非提权窗口：不路由，不留提示");

    // 2) 起一个代理（同一进程里的线程；镜像路径与客户端相同，身份校验应当通过）。
    let server_name = name.clone();
    std::thread::spawn(move || {
        if let Err(e) = agent::serve(&server_name, None) {
            println!("代理退出：{e}");
        }
    });

    // 3) 先验「闸门本身」：探针里的代理**不是提权**运行的，带提权要求去连必须被拒
    //    （真提权进程由 UAC 拉起，本机自动验不了，但这条校验逻辑能验）。
    match agent::connect_for_probe(&name, std::time::Duration::from_secs(3), true) {
        Ok(_) => panic!("非提权对端不该被认下"),
        Err(e) => println!("✓ 对端的提权校验生效（非提权对端被拒：{e}）"),
    }

    // 4) 放开提权校验再连：验 DACL、双方身份、握手与行协议往返（真提权那一段人工验）。
    let client = match agent::connect_for_probe(&name, std::time::Duration::from_secs(3), false) {
        Ok(c) => c,
        Err(e) => {
            println!("连不上代理：{e}");
            std::process::exit(1);
        }
    };
    println!("✓ 已连上代理（pid {}）：DACL + 双方身份校验都过了", client.agent_pid());

    // 3) 正常一批：都不产生真实注入（零位移移动、空文本），纯验往返。
    client
        .request(&[Op::Pause(0), Op::Mouse(MouseOp::Move { dx: 0, dy: 0 }), Op::Text(String::new())])
        .expect("正常批次该成功");
    println!("✓ 一批 op 往返成功（ok）");

    // 4) 一批按键 op（按下 + 抬起 + 停顿）——注入对象是当前前台窗口，跑这个探针时请别在
    //    乎多按几下 Esc（无修饰、无副作用）。
    client
        .request(&[Op::Down(Key::Escape), Op::Pause(10), Op::Up(Key::Escape)])
        .expect("按键批次该成功");
    println!("✓ 按键批次往返成功（ok）");

    // 5) 坏 op：整批拒绝，不能静默略过。
    let bad = agent::decode_batch(&["run calc".to_string()]);
    assert!(bad.is_err(), "坏行必须报错");
    println!("✓ 坏 op 被拒：{}", bad.unwrap_err());

    // 6) 上百个 op 的分片读写（每行一个 op，跨多次 ReadFile / WriteFile）。
    let many: Vec<Op> = (0..200).map(|_| Op::Pause(0)).collect();
    client.request(&many).expect("大批次该成功");
    println!("✓ {} 个 op 的大批次往返成功", many.len());

    // 7) 断开 = 代理自行退出（「平时不运行」）。
    drop(client);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let again = agent::connect_for_probe(&name, std::time::Duration::ZERO, false);
    assert!(again.is_err(), "客户端断开后代理该退出，通道不该还开着");
    println!("✓ 客户端一断开，代理退出（通道已关）");

    println!("\n探针全过。真提权那一段（UAC + 往提权窗口注入）请按 人工验证清单 §31 人工验。");
}

#[cfg(not(windows))]
fn main() {
    println!("提权通道是 Windows 专属（见 docs/底层输入能力规划.md 档位 B），本平台无此能力。");
}
