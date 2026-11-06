// 主窗口（T-05 从 lib.rs 搬出）：按需建窗（冷启动不加载界面）/ 拉到最前 / 界面外链交给系统浏览器。
// 跨平台标题栏口径见 说明书 §6.10，外链一律由壳层接住（架构设计 §3.6）。

use crate::*;

/// 把已存在的主窗口拉到最前：单靠 `set_focus` 常常抢不到前台（窗口被别的窗口压着、
/// 最小化、或被 `SetForegroundWindow` 的调用方限制挡住——双击 Alt 唤醒就会表现为
/// 「窗口出来了但没在最前」）。补两步：先取消最小化，再用「临时置顶 + 聚焦 + 撤掉
/// 置顶」把窗口顶到最前。已经在前台就什么都不做，免得凭空闪一下。
fn raise_main_window(w: &tauri::WebviewWindow) {
    let _ = w.unminimize();
    if w.is_focused().unwrap_or(false) {
        return;
    }
    let _ = w.set_always_on_top(true);
    let _ = w.show();
    let _ = w.set_focus();
    let w = w.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        let _ = w.set_always_on_top(false);
    });
}

/// 界面外的链接只放行 http(s) 交给系统浏览器——`about:blank` / `data:` / `javascript:`
/// 之类（新窗口请求里也会出现）不往外丢，免得拿系统默认程序去开一个不是网址的东西。
pub(crate) fn external_link_to_open(url: &str) -> Option<&str> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url)
    } else {
        None
    }
}

/// 按需创建/显示主窗口：首次打开时才真正建出 WebView，冷启动不加载任何界面。
pub(crate) fn show_main_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        raise_main_window(&w);
        return;
    }
    let mut builder =
        tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::App("index.html".into()))
            .title("咔哒 Kada")
            .inner_size(860.0, 560.0)
            // 下限 = 导航栏 64 + 列表栏 300 + 详情区最小可用 540。再窄详情区会挤爆
            // 「模式下拉 + 按键胶囊 + 三个录入图标」那一行（CSS 里没有断点可退）。
            .min_inner_size(900.0, 480.0)
            .resizable(true)
            .center();
    // 标题栏分平台，界面里那条是自绘的（见 `ui/src/main.ts` 的 `mountTitlebar`）：
    // - Windows / Linux 无边框 + **开投影**：tao 对「无边框 + 有投影」的窗保留整套 DWM 帧，
    //   缩放边、圆角、最大化到工作区都还是系统行为，只有标题区交给界面画；
    // - macOS 用 Overlay：红绿灯由系统画、界面铺到标题栏下方，不用自绘按钮，也不必处理
    //   三个交通灯的状态。别照抄浮窗那套 `shadow(false)`——那条是透明浮窗的规矩。
    #[cfg(not(target_os = "macos"))]
    {
        builder = builder.decorations(false).shadow(true);
    }
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
    }
    match builder
        // 界面外链（帮助页落款的 GitHub）交给系统默认浏览器。WebView 的「新窗口请求」
        // 没人接时会被 wry 直接吞掉（WebView2 里 `SetHandled(true)` 后什么都不做），
        // 于是 `<a target="_blank">` 点了毫无反应——必须自己接住。这里一律不建应用内
        // 新窗口（Deny），只把能交给浏览器的 http(s) 链接丢出去。
        .on_new_window(|url, _features| {
            if let Some(target) = external_link_to_open(url.as_str()) {
                let target = target.to_string();
                // 与钩子回调同一条纪律：发起进程调用不当场做，挪到后台线程。
                std::thread::spawn(move || {
                    if let Err(e) = kada_actions::open_url(&target) {
                        eprintln!("打开外链失败: {e}");
                    }
                });
            }
            tauri::webview::NewWindowResponse::Deny
        })
        .build()
    {
        Ok(w) => raise_main_window(&w),
        Err(e) => {
            // 唤起现在跑在后台线程（见 `wake_double_tap` 的接线），冷启动期间连点两次双击
            // 可能两个线程都走到这里：主线程是串行处理的，后到的那次会撞上「窗口已存在」。
            // 窗口其实已经建好了，补一次显示即可，不必让用户看到一句吓人的创建失败。
            match app.get_webview_window("main") {
                Some(w) => raise_main_window(&w),
                None => eprintln!("创建主窗口失败: {e}"),
            }
        }
    }
}
