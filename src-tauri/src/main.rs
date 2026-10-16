#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // 提权伴随进程模式（规划 7.2-㉑a / `docs/底层输入能力规划.md` 档位 B）：
    // `kada.exe --kada-agent <父进程 pid>`。**必须在 Tauri 之前分派**——这个模式要的正是
    // 「不建窗口、不初始化套壳、不碰配置」，它只做一件事：在命名管道上按请求重放注入
    // （见 `kada_hook::elevated`）。同一个可执行文件 = 零打包改动、零卸载残留。
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == kada_hook::elevated::AGENT_ARG) {
        let parent = args.get(i + 1).and_then(|s| s.parse::<u32>().ok());
        std::process::exit(kada_hook::elevated::serve(parent));
    }
    kada::run()
}