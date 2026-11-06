// 引擎接线（T-05 从 lib.rs 搬出）：状态机定时器 / 输入状态复位 / 配置监听 / 暂停与重载 / 触发执行 `fire`。
// 钩子回调只把事件交给 `Engine::step`，真正的动作执行与状态上报都在这条「宿主」路径上（见 说明书 §4.1 / §5.4）。

use crate::*;

/// 状态机的超时推进线程：每 [`ENGINE_TICK_MS`] 调一次 [`Engine::tick`]，只在「等待态
/// 已过期」时动作（回放被吞的 leader / 中间步、提交连击等待窗），不会注入别的东西。
/// 录制中与暂停中不推进：那段时间事件不进状态机，推进只会把陈旧的等待态回放到用户
/// 正在录入的内容里。
///
/// 同一个线程顺带做**输入状态复位**巡检（钩子重装 / 前台切换，见 [`ResetWatch`]，以及
/// 「配置被整份换掉」见下面 `cfg_stale`）：复位与超时推进都作用于同一个 [`Engine`]，
/// 放在一起就不必为复位再起一个线程。巡检本身照常进行（基线要跟上），只是暂停/录制期间
/// 不落地。
///
/// 还顺带把**输入状态可视指示**推给界面（见规划 7.3-⑬）：状态变了才推（托盘提示 + 悬浮指示窗）。
/// 放在这条线程上是因为它是唯一一处「不在钩子回调里、又拿得到最新 `Engine` 状态」的地方——
/// 在回调里推 UI 等于把输入层挂在界面上。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) fn spawn_engine_ticker(
    engine: Arc<Mutex<Engine>>,
    cfg: Arc<RwLock<Config>>,
    paused_until: Arc<AtomicU64>,
    rec: Arc<Mutex<Option<Recorder>>>,
    cfg_stale: Arc<AtomicBool>,
    app: tauri::AppHandle,
) {
    std::thread::spawn(move || {
        let mut watch = ResetWatch::new();
        // 上一次推给界面的指示（载荷 + 指示开关）：两者任一变了才重新推。开关也参与比对，
        // 否则「关掉悬浮指示」要等到下次状态变化才生效（锁定层能让它一直挂着）。
        let mut last_status: Option<(StatusPayload, bool)> = None;
        // 上一次「配置是否需要鼠标钩子」：变了才通知钩子层（避免每拍都发重装请求）。
        let mut last_mouse: Option<bool> = None;
        // 上一次「滚轮是否作为触发键」的推送值（规划 7.2-MG 余项①）：没绑时钩子层直接放行。
        let mut last_wheel: Option<bool> = None;
        // 上一次「鼠标手势采集是否开着」（规划 7.2-MG）：变了才推给钩子层。
        let mut last_gesture: Option<bool> = None;
        // 上一次「触摸板手势采集是否开着」：只有按住触发键期间才采集，同样是变了才推。
        let mut last_trackpad: Option<bool> = None;
        // 触发角（EXT-10）/ 边缘滚动（EXT-11）：上一次推给钩子层的「总闸 + 四角/四边开关
        // + 显示器矩形」。显示器几何每 ~3 秒重取一次（热插拔 / 改分辨率），两者共用。
        type HotCornerPush = (bool, [bool; 4], Vec<(i32, i32, i32, i32)>);
        let mut last_hot_corners: Option<HotCornerPush> = None;
        let mut last_wheel_edges: Option<HotCornerPush> = None;
        let mut last_monitors: Vec<(i32, i32, i32, i32)> = Vec::new();
        let mut last_edge_on = false;
        let mut hot_corner_tick: u32 = 0;
        // 上一次「是否处于用户暂停」：变了才对齐托盘的「暂停/恢复」勾选（设置页 / 托盘自身 /
        // 外部改配置都能改它，这里是唯一一处常驻且拿得到最新配置的地方）。
        let mut last_paused: Option<bool> = None;
        // 上一次「提权窗口注入开关」：变了才通知钩子层（关掉时钩子层会顺带断开通道，
        // 提权进程随之退出——「平时不运行」，见 `kada_hook::elevated`）。
        let mut last_elevated: Option<bool> = None;
        // 「安全输入」巡检的拍计数与上次状态（macOS 专属，规划 7.2-㊸）：初始 false——
        // 应用启动时「安全输入」若已开着，第一拍（约 2 秒后）就该提示，而不是当作「没变」。
        #[cfg(target_os = "macos")]
        let mut secure_tick: u32 = 0;
        #[cfg(target_os = "macos")]
        let mut last_secure = false;
        loop {
            std::thread::sleep(Duration::from_millis(ENGINE_TICK_MS));
            let reset_reason = watch.poll();
            // 外部改动把配置整份换掉了（见 [`spawn_config_watcher`]）：跟上一条复位通道走同一套
            // 口径——注入的收回来、缓冲里的丢弃不回放、锁定层保留。切换键位层 / 删掉序列 leader
            // 之后，按住的旧层与待定的和弦都属于上一份配置，留着只会打进新配置里。
            let cfg_replaced = cfg_stale.swap(false, Ordering::Relaxed);
            let reason = reset_reason.or(if cfg_replaced {
                Some("配置已重新加载")
            } else {
                None
            });
            // 输入复位 = 「与物理键盘断过一次线」：触摸板手势正在采的缓冲一起丢掉
            // （不回放、不回调出口）。这里不持配置锁，锁序上只会拿采集状态。
            // 轨迹浮层也得跟着收：复位没有出口回调，靠回调收浮层的那条路走不到。
            if reason.is_some() {
                kada_hook::trackpad::reset();
                hide_gesture_overlay(&app);
            }
            // 配置读锁在 `Engine` 锁**之前**取、并一路持有到 `tick`：等待窗取值就来自配置
            // （`Settings.sequence_timeout_ms` / `chord_timeout_ms`），而钩子回调路径也是
            // 「先读配置、再锁引擎」——两条路径的加锁顺序必须一致，反过来会死锁。
            let (
                payload,
                hud_enabled,
                use_mouse,
                use_wheel,
                user_paused,
                elevated,
                gesture_on,
                trackpad_on,
                toast_on,
            ) = {
                let guard = cfg.read().unwrap();
                let frozen = rec.lock().unwrap().is_some()
                    || pause_active(&paused_until)
                    || guard.settings.paused;
                // 暂停 / 录制期间不推进状态机（那段时间事件根本不进状态机），但指示照常反映当前
                // 状态：暂停这个动作本身不该让已经按住的层从指示里消失。
                let status = if frozen {
                    engine.lock().unwrap().status()
                } else {
                    let mut engine = engine.lock().unwrap();
                    if let Some(reason) = reason {
                        // 动作链按住的键也要收回（EXT-16）：输入复位意味着「与物理键盘断过
                        // 一次线」，`Keys press` 留下的修饰键不收回就会一直粘在系统里。
                        release_held_keys();
                        // 只在真的丢掉了状态时留痕：复位本身是无声兜底（用户此刻没有动作可做），
                        // 进消息中心只会变成噪音。
                        if engine.reset(&mut SimulatedInject) {
                            eprintln!("kada: 输入状态已复位（{reason}）");
                        }
                    }
                    engine.tick(&guard, &mut SimulatedInject);
                    engine.status()
                };
                (
                    build_status(&status, &guard),
                    guard.settings.show_status_hud,
                    // 鼠标钩子：「录入触摸板手势」期间也必须挂着——新建第一条时配置里还没有
                    // 启用的触摸板手势，光开 trackpad 闸门不够，普通鼠标移动进不来，一笔都记不到。
                    config_needs_mouse_hook(&guard) || trackpad_recording_key().is_some(),
                    kada_core::config_uses_wheel(&guard),
                    guard.settings.paused,
                    guard.settings.elevated_inject,
                    // 手势采集的闸门：**新功能总开关**（规划 7.2-MG）关了、或没有启用的手势
                    // → 右键完全放行（回到系统行为，一个字节都不多吞）。
                    guard.settings.experimental && gesture_capture_enabled(&guard),
                    // 触摸板手势的闸门：**它自己的总开关**（与鼠标操作的 `experimental` 分开）；
                    // 没有触摸板手势 / 本平台没接鼠标移动采集时不 arm（触发键照常输出，不是静默吞掉）。
                    // **「录一笔」期间强制开闸**：新建第一条时配置里还没有启用的条目。
                    (guard.settings.touchpad
                        && touchpad_gesture_enabled(&guard)
                        && kada_hook::trackpad::supported())
                        || trackpad_recording_key().is_some(),
                    guard.settings.trigger_toast,
                )
            };
            TRIGGER_TOAST.store(toast_on, Ordering::Relaxed);
            // 锁（配置 / 引擎）到此释放：推指示要动托盘图标、建窗口、发事件，不能在锁里做。
            let changed = match last_status.as_ref() {
                Some((last, enabled)) => last != &payload || *enabled != hud_enabled,
                None => true,
            };
            if changed {
                publish_status(&app, &payload, hud_enabled);
                last_status = Some((payload, hud_enabled));
            }
            // 鼠标钩子按需安装：配置里增删了鼠标键触发（含外部改动 / 导入）就通知钩子层重挂。
            if last_mouse != Some(use_mouse) {
                input::set_mouse_enabled(use_mouse);
                last_mouse = Some(use_mouse);
            }
            // 滚轮触发键的闸门（规划 7.2-MG 余项①）：没绑时钩子层不构造事件、不碰判定。
            if last_wheel != Some(use_wheel) {
                input::set_wheel_enabled(use_wheel);
                last_wheel = Some(use_wheel);
            }
            // 鼠标手势（规划 7.2-MG）：开着才拦右键；关掉时钩子层把状态清干净，
            // 右键立刻回到系统行为（总开关在设置页，配置保留）。
            if last_gesture != Some(gesture_on) {
                kada_hook::gesture::set_enabled(gesture_on);
                last_gesture = Some(gesture_on);
            }
            // 触摸板手势（7.2-MG）：只在按住触发键期间采集；关掉时钩子层清干净状态。
            if last_trackpad != Some(trackpad_on) {
                kada_hook::trackpad::set_enabled(trackpad_on);
                if !trackpad_on {
                    // 关闸时若正画到一半，采集状态已被清掉、出口不会再回调：自己收浮层。
                    hide_gesture_overlay(&app);
                }
                last_trackpad = Some(trackpad_on);
            }
            // 触发角（EXT-10）/ 边缘滚动（EXT-11）：总闸 + 四角/四边开关 + 显示器矩形推给
            // 钩子层（回调里只做纯坐标判断）。两者共用同一份显示器几何，任一启用就按 ~3 秒
            // 重取一次，热插拔 / 改分辨率能跟上。
            hot_corner_tick = hot_corner_tick.wrapping_add(1);
            let (hot_on, hot_flags, wheel_on, wheel_flags) = {
                let g = cfg.read().unwrap();
                let any_corner = |spot| g.hot_corners.iter().any(|r| r.enabled && r.spot == spot);
                let any_edge = |edge| g.wheel_edges.iter().any(|r| r.enabled && r.edge == edge);
                (
                    g.settings.experimental && g.hot_corners.iter().any(|r| r.enabled),
                    [
                        any_corner(kada_core::HotCornerSpot::TopLeft),
                        any_corner(kada_core::HotCornerSpot::TopRight),
                        any_corner(kada_core::HotCornerSpot::BottomLeft),
                        any_corner(kada_core::HotCornerSpot::BottomRight),
                    ],
                    g.settings.experimental && g.wheel_edges.iter().any(|r| r.enabled),
                    [
                        any_edge(kada_core::WheelEdgeSpot::Top),
                        any_edge(kada_core::WheelEdgeSpot::Bottom),
                        any_edge(kada_core::WheelEdgeSpot::Left),
                        any_edge(kada_core::WheelEdgeSpot::Right),
                    ],
                )
            };
            let any_edge = hot_on || wheel_on;
            let refetch = any_edge
                && (hot_corner_tick.is_multiple_of(50)
                    || !last_edge_on
                    || last_monitors.is_empty());
            let monitors = if refetch {
                app.available_monitors()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|m| {
                        let pos = m.position();
                        let size = m.size();
                        (
                            pos.x,
                            pos.y,
                            pos.x + size.width as i32,
                            pos.y + size.height as i32,
                        )
                    })
                    .collect()
            } else {
                last_monitors.clone()
            };
            last_monitors = monitors.clone();
            last_edge_on = any_edge;
            let next_hot = (hot_on, hot_flags, monitors.clone());
            if last_hot_corners.as_ref() != Some(&next_hot) {
                kada_hook::hotcorner::configure(next_hot.0, next_hot.1, next_hot.2.clone());
                last_hot_corners = Some(next_hot);
            }
            let next_wheel = (wheel_on, wheel_flags, monitors);
            if last_wheel_edges.as_ref() != Some(&next_wheel) {
                kada_hook::wheel_edge::configure(next_wheel.0, next_wheel.1, next_wheel.2.clone());
                last_wheel_edges = Some(next_wheel);
            }
            // 托盘「暂停快捷键触发」的勾选对齐：设置页勾选、外部改配置、托盘自身点击都汇总到
            // 这一处比对（点菜单时 CheckMenuItem 已自动翻转，这里把状态钉死到真实配置值）。
            if last_paused != Some(user_paused) {
                if let Some(item) = app.state::<KadaState>().pause_tray.lock().unwrap().as_ref() {
                    let _ = item.set_checked(user_paused);
                }
                last_paused = Some(user_paused);
            }
            // 提权窗口注入的开关（规划 7.2-㉑a）：只在这里同步给钩子层——注入路径每一批都要
            // 读它，放配置锁里现读会拖慢热路径。关掉时钩子层会断开通道，提权进程随之退出。
            if last_elevated != Some(elevated) {
                input::elevated::set_enabled(elevated);
                last_elevated = Some(elevated);
            }
            // 「需要提权、但通道没用上」的提示（开关没开 / 提权进程没在跑 / 通道出错）：
            // 平台层带冷却、取走即报，不会连按刷屏。**失败必须可见**——否则用户看到的仍然
            // 是「按了没反应」，本档就白做了（见 `docs/底层输入能力规划.md` §1 第 5 条）。
            if let Some(notice) = input::elevated::take_notice() {
                report_elevated_notice(&app, notice);
            }
            // 「安全输入」提示（macOS 专属，规划 7.2-㊸）：低频轮询，状态**翻转**才报——
            // 终端输密码这类场景系统会关掉所有事件 tap，快捷键全失效；不提示的话用户看到的
            // 就是「Kada 坏了」，而其实退出那个场景会自己恢复。
            #[cfg(target_os = "macos")]
            {
                secure_tick += 1;
                if secure_tick >= SECURE_INPUT_TICKS {
                    secure_tick = 0;
                    let on = kada_hook::secure_input_enabled();
                    if on != last_secure {
                        last_secure = on;
                        report_secure_input(&app, on);
                    }
                }
            }
        }
    });
}

/// 「安全输入」状态翻转时进消息中心的一条提示（macOS 专属，规划 7.2-㊸）。
///
/// 开与关都说：开 = 「快捷键此刻全失效，不是 Kada 坏了」；关 = 「恢复了」（翻回来也报，
/// 免得用户不知道上一条「失效」是不是还成立）。两条都给下一步：关掉那个场景 / 重启 Kada。
#[cfg(target_os = "macos")]
pub(crate) fn report_secure_input(app: &tauri::AppHandle, on: bool) {
    let state = app.state::<KadaState>();
    let (command, stdout) = if on {
        (
            "系统「安全输入」已开启：快捷键此刻全部失效".to_string(),
            "终端输密码、`sudo` 密码框这类场景会让 macOS 暂时关掉所有事件监听（包括 Kada 的）\
             ——这是系统安全机制，Kada 与任何其它工具都绕不过去。退出那个场景后自动恢复。"
                .to_string(),
        )
    } else {
        (
            "系统「安全输入」已关闭：快捷键恢复".to_string(),
            "如果刚才「按了没反应」，现在应该恢复了；仍不响应时重启一次 Kada。".to_string(),
        )
    };
    commit_result(
        app,
        &state.results,
        &state.unread,
        CommandResult {
            kind: "macos".into(),
            label: "输入层".into(),
            trigger: "安全输入".into(),
            name: String::new(),
            command,
            stdout,
            stderr: String::new(),
            exit_code: None,
            detached: false,
            log_path: None,
            stdout_file: None,
            stderr_file: None,
            // 不弹窗：这条随时可能在用户输密码的当下出现，弹窗既抢焦点又不合时宜。
            show_output: false,
            time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            run_id: None,
        },
    );
}

/// 「需要提权、但通道没用上」时进消息中心的一条提示。
///
/// 三种原因分开说，且都要给出**下一步怎么做**：本档的前提是「用户先知道发生了什么事」
/// （档位 A 立的规矩），到这里再补上「怎么让它通」。安全桌面的边界也在这里如实讲一句——
/// 那是这套东西**也做不到**的部分（见 `docs/底层输入能力规划.md` 档位 B 的边界）。
fn report_elevated_notice(app: &tauri::AppHandle, notice: input::elevated::Notice) {
    let state = app.state::<KadaState>();
    let target = if notice.app.is_empty() {
        "一个提权窗口".to_string()
    } else {
        format!("提权窗口（{}）", notice.app)
    };
    let (headline, detail) = match notice.reason {
        input::elevated::NoticeReason::Disabled => (
            format!("{target}里注入没生效：提权窗口注入还没打开"),
            "前台这个程序是**以管理员身份运行**的，UIPI 不允许 Kada 往里注入（这也是它设计上的\
             用途）。打开 设置 → 提权窗口注入 的「提权窗口也生效」，会弹一次 UAC 授权，之后\
             这类窗口里的文本 / 按键 / 鼠标动作就能生效。"
                .to_string(),
        ),
        input::elevated::NoticeReason::NotRunning => (
            format!("{target}里注入没生效：提权进程没在运行"),
            "开关是开着的，但提权进程没跑（没启动过、或者已经退出）。去 设置 → 提权窗口注入 \
             点「启动提权进程」（会弹一次 UAC）。"
                .to_string(),
        ),
        input::elevated::NoticeReason::Failed => (
            format!("{target}里注入没生效：提权通道出错"),
            "已经回落成普通注入，所以提权窗口里不会有反应。可在 设置 → 提权窗口注入 里重启\
             提权进程；持续失败请把这条消息连同复现步骤一起记下来反馈。"
                .to_string(),
        ),
    };
    commit_result(
        app,
        &state.results,
        &state.unread,
        CommandResult {
            kind: "elevated".into(),
            label: "提权窗口注入".into(),
            trigger: "提权窗口".into(),
            name: String::new(),
            command: headline,
            stdout: detail,
            stderr: String::new(),
            exit_code: None,
            detached: false,
            log_path: None,
            stdout_file: None,
            stderr_file: None,
            // 不弹窗：这条随时可能因为用户正好在提权窗口里按快捷键而出现，弹窗会抢焦点。
            show_output: false,
            time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            run_id: None,
        },
    );
}

/// 配置外部修改监听的巡检线程（见 [`config_watch`] 与规划 7.2-⑩）。
///
/// 每秒看一眼配置文件：被外部改成了另一份**能解析**的内容就采纳（替换内存里那份 + 通知
/// 消息中心 + 广播给界面）；改成解析不动的半截内容就只提示、**谁都不动**（磁盘上那份留给
/// 用户自己修）。这样「手改 JSON / 恢复备份 / 同步客户端落盘」都能生效，且应用内的下一个
/// 保存不会再把外部改动盖掉——内存里那份已经跟着走了。
///
/// 巡检是轮询而非订阅文件系统事件：与钩子看门狗、[`ResetWatch`] 同一取舍——不引入额外依赖
/// 与平台分支，不用管编辑器「先写临时文件再改名」和同步客户端的各种怪癖，代价只是慢一拍
/// （[`config_watch::POLL_MS`]）。读的是一份几 KB 的文件，开销可以忽略。
pub(crate) fn spawn_config_watcher(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(config_watch::POLL_MS));
        let st = app.state::<KadaState>();
        // 判定 + 采纳在锁内一口气做完，通知放到锁外：`cfg_watch` 这把锁与 set_config /
        // import_config 共用，持锁期间不该去动托盘图标、发事件（也不能让写盘等它）。
        let (reloaded, bad) = {
            let current = st.config.read().unwrap().clone();
            let mut watch = st.cfg_watch.lock().unwrap();
            match watch.poll(&st.file, &current) {
                None => (None, None),
                Some(config_watch::Change::Unreadable(err)) => (None, Some(err)),
                Some(config_watch::Change::Reloaded(next)) => {
                    let autostart_changed = next.settings.autostart != current.settings.autostart;
                    let cfg = *next;
                    // 留一份给锁外的通知与广播（配置只有几 KB，且这条路径本来就罕见）。
                    *st.config.write().unwrap() = cfg.clone();
                    (Some((cfg, autostart_changed)), None)
                }
            }
        };
        // 坏内容：内存与磁盘都不动，只提示这一次（同一份内容不重复报，见 [`config_watch`]）。
        if let Some(err) = bad {
            commit_result(
                &app,
                &st.results,
                &st.unread,
                CommandResult {
                    kind: "config".into(),
                    label: "配置".into(),
                    trigger: "外部修改".into(),
                    name: String::new(),
                    command: "配置文件被外部修改，但内容无法解析：已保留当前配置".into(),
                    stdout: format!(
                        "磁盘上的 {} 未被改动（监听不会清理或回写外部内容）。\
                         改好后会自动重新加载；想放弃这次外部改动，可用 config.json.bak 覆盖回去。",
                        st.file.display(),
                    ),
                    stderr: err,
                    exit_code: None,
                    detached: false,
                    log_path: None,
                    stdout_file: None,
                    stderr_file: None,
                    show_output: false,
                    time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                    run_id: None,
                },
            );
            continue;
        }
        // 采纳成功（上面已换进内存）：自启跟着系统同步一次，引擎复位交给定时器线程。
        let Some((cfg, autostart_changed)) = reloaded else {
            continue;
        };
        if autostart_changed {
            sync_autostart(&app, cfg.settings.autostart);
        }
        st.cfg_stale.store(true, Ordering::Relaxed);
        commit_result(
            &app,
            &st.results,
            &st.unread,
            CommandResult {
                kind: "config".into(),
                label: "配置".into(),
                trigger: "外部修改".into(),
                name: String::new(),
                command: "检测到配置文件被外部修改，已重新加载生效".into(),
                stdout: format!(
                    "快捷键 {} 条、改键 {} 条、文本扩展 {} 条、层 {} 个、目录 {} 个。\
                     误改可回滚到 config.json.bak（上次保存时的良好副本）。",
                    cfg.shortcuts.len(),
                    cfg.remaps.len(),
                    cfg.expansions.len(),
                    cfg.layers.len(),
                    cfg.folders.len(),
                ),
                stderr: String::new(),
                exit_code: None,
                detached: false,
                log_path: None,
                stdout_file: None,
                stderr_file: None,
                show_output: false,
                time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                run_id: None,
            },
        );
        let _ = app.emit("config-changed", &cfg);
    });
}

/// 托盘「暂停 / 恢复快捷键触发」：切换 `settings.paused` 并落盘。
///
/// 与设置页的「暂停」勾选是同一个字段（走 [`save_settings_patch`]，会广播 `config-updated`，
/// 设置页随即跟上）；勾选态由状态机定时器线程统一对齐（见 [`spawn_engine_ticker`]）。
pub(crate) fn toggle_pause(app: &tauri::AppHandle) {
    let st = app.state::<KadaState>();
    let paused = !st.config.read().unwrap().settings.paused;
    if let Err(e) = save_settings_patch(app, &st, |s| s.paused = paused) {
        eprintln!("切换暂停失败: {e}");
    }
}

/// 托盘「重载配置」：立即从磁盘再读一份（手改 JSON 后想马上生效，不等监听线程的下一拍）。
///
/// 与 [`spawn_config_watcher`] 同口径——**能解析才采纳、解析不动就别碰磁盘**；区别是这条由
/// 用户主动触发，无论结果如何都给一条可见反馈，免得点了没反应。
pub(crate) fn reload_config_now(app: &tauri::AppHandle) {
    let st = app.state::<KadaState>();
    let result = config_io::read_only(&st.file);
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let command = match result {
        Ok(cfg) => {
            // 与 set_config 同一把锁序（cfg_watch → config.write，巡检线程也按此序）。
            let mut watch = st.cfg_watch.lock().unwrap();
            if *st.config.read().unwrap() == cfg {
                watch.adopt_own_write(&st.file);
                drop(watch);
                "配置已是最新，无需重载".to_string()
            } else {
                let autostart_changed =
                    st.config.read().unwrap().settings.autostart != cfg.settings.autostart;
                *st.config.write().unwrap() = cfg.clone();
                // 与外部改动同一条复位通道：旧配置绑定的按住层 / 待定和弦 / 注入修饰键作废。
                st.cfg_stale.store(true, Ordering::Relaxed);
                watch.adopt_own_write(&st.file);
                drop(watch);
                if autostart_changed {
                    sync_autostart(app, cfg.settings.autostart);
                }
                // 用 config-changed（而非 config-updated）：内存配置真被整份换掉了，界面若有
                // 未保存草稿得按「外部改动」的流程问用户，不能静默覆盖。
                let _ = app.emit("config-changed", &cfg);
                format!(
                    "已从磁盘重载配置：快捷键 {} 条、改键 {} 条、文本扩展 {} 条、层 {} 个。",
                    cfg.shortcuts.len(),
                    cfg.remaps.len(),
                    cfg.expansions.len(),
                    cfg.layers.len(),
                )
            }
        }
        Err(e) => {
            commit_result(
                app,
                &st.results,
                &st.unread,
                CommandResult {
                    kind: "error".into(),
                    label: "配置".into(),
                    trigger: "重载配置".into(),
                    name: String::new(),
                    command: "配置重载失败：磁盘内容无法解析，当前配置保持不变".into(),
                    stdout: String::new(),
                    stderr: e,
                    exit_code: None,
                    detached: false,
                    log_path: None,
                    stdout_file: None,
                    stderr_file: None,
                    show_output: false,
                    time: now.clone(),
                    run_id: None,
                },
            );
            return;
        }
    };
    commit_result(
        app,
        &st.results,
        &st.unread,
        CommandResult {
            kind: "config".into(),
            label: "配置".into(),
            trigger: "重载配置".into(),
            name: String::new(),
            command,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            detached: false,
            log_path: None,
            stdout_file: None,
            stderr_file: None,
            show_output: false,
            time: now,
            run_id: None,
        },
    );
}

/// 执行一串动作（异步跑，避免阻塞钩子回调）；命令类动作的结果进消息中心。
///
/// `timeout_ms` 是命令/脚本的执行超时（0 = 不限时），由调用点从配置读出后传入——调用点
/// 已经持有配置读锁，这里不再进线程里二次加锁（保存配置时写锁会等读锁，多一层没必要）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn fire(
    app: tauri::AppHandle,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
    actions: Vec<Action>,
    trigger: String,
    name: String,
    timeout_ms: u64,
    text_mode: TextInjectMode,
    device: Option<String>,
    origin: Option<(i32, i32)>,
) {
    // 气泡窗口首次懒创建较慢（WebView 冷启动），放后台线程，避免阻塞钩子回调——
    // WH_KEYBOARD_LL 回调超时会被系统摘除，导致后续快捷键/改键/热串全部失效。
    {
        let app = app.clone();
        let name = name.clone();
        let trigger = trigger.clone();
        std::thread::spawn(move || show_toast(&app, &name, &trigger));
    }
    // 先落一条「正在执行」：界面立刻能点进去看（逐步汇报会持续刷新同一张卡）。
    // ponytail: 记录 id 用微秒时间戳——同一微秒起两条链的概率可忽略，省一个全局计数器。
    let id = chrono::Local::now().timestamp_micros() as u64;
    start_run(
        &app,
        &runs,
        &unread,
        RunRecord {
            id,
            trigger: trigger.clone(),
            name: name.clone(),
            time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            start_epoch_ms: chrono::Local::now().timestamp_millis() as u64,
            duration_ms: 0,
            status: "running".into(),
            steps: Vec::new(),
        },
    );
    // 触发设备由**调用方在回调线程上**取好带下来：`current_device` / `current_pointer_device`
    // 都是「最近一次事件」，放到下面 spawn 的线程里读会晚一拍、可能已被别的设备覆盖。
    // 键盘类触发带键盘设备、指针类（鼠标键 / 滚轮 / 手势 / 触摸板）带指针设备——混着用会把
    // 「中键在鼠标 B 上按」判成「键盘 A 按的」（见 7.2-㊷ / 说明书 §3.4）。
    // 触发那一刻的前台窗口（EXT-14「当前窗口」动作的目标）：必须在**调用线程**上抓，
    // 放进下面 spawn 的线程就晚了一拍（动作链还没跑，但前台可能已经变了）。
    // 只在 Windows 抓：macOS / Linux 没实现这个动作族，白付一次前台查询没必要。
    #[cfg(target_os = "windows")]
    let window = input::foreground_window();
    #[cfg(not(target_os = "windows"))]
    let window = None;
    std::thread::spawn(move || {
        // 触发带修饰键的快捷键（如 Ctrl+Alt+T）时修饰键仍物理按住，此时注入按键会被
        // 污染成 Ctrl+Alt+<键>（粘贴 Ctrl+V 变成 Ctrl+Alt+V）。只对真正注入按键的链
        // （Text / Keys，含 If 分支内）等修饰键释放：300ms 还没松就不注入——污染键打进
        // 目标程序轻则无效、重则误触别的快捷键（如 Ctrl+Alt+方向键转屏），跳过并记消息
        // 中心让用户知道为什么没生效。纯命令 / 文件 / 开 URL 的链不受修饰键影响，等
        // 300ms 只是白拖手感，照常直接执行（7.3-㉒ 顺带修「返回值被丢弃」）。
        if injects_keys(&actions) && !input::simulate::wait_modifiers_released(300) {
            commit_result(
                &app,
                &results,
                &unread,
                CommandResult {
                    kind: "error".into(),
                    label: "动作未执行".into(),
                    command: trigger.clone(),
                    trigger: trigger.clone(),
                    name: name.clone(),
                    stdout: String::new(),
                    stderr: "修饰键（Ctrl/Alt/Shift/Meta）在 300ms 内没有松开，为避免注入被污染 \
                             成 Ctrl+Alt+<键>，本次文本/按键动作已跳过——松键再触发即可。"
                        .into(),
                    exit_code: None,
                    detached: false,
                    log_path: None,
                    stdout_file: None,
                    stderr_file: None,
                    show_output: false,
                    time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                    run_id: Some(id),
                },
            );
            finish_run(&app, &runs, &unread, id, "error");
            return;
        }
        let mut vars: Vars = BTreeMap::new();
        let mut last_copied: Option<String> = None;
        let frontmost = input::frontmost_context();
        let mut commit = |result: CommandResult| commit_result(&app, &results, &unread, result);
        let mut on_step = |step: ActionStep| add_step(&app, &runs, &unread, id, step);
        let mut rep = Reporter {
            commit: &mut commit,
            on_step: &mut on_step,
        };
        let opts = RunOptions {
            action_timeout: Duration::from_millis(timeout_ms),
            text_inject_mode: text_mode,
        };
        let t = TriggerCtx {
            trigger: &trigger,
            name: &name,
            frontmost: frontmost.as_ref(),
            device: device.as_deref(),
            window,
            origin,
        };
        let status = run_actions(
            &mut rep,
            &actions,
            t,
            &mut vars,
            &mut last_copied,
            &opts,
            Some(id),
        );
        let mut failed = false;
        update_run(&app, &runs, id, |run| {
            failed = run.steps.iter().any(|s| s.status == "error");
        });
        finish_run(
            &app,
            &runs,
            &unread,
            id,
            match status {
                RunStatus::Aborted => "abort",
                RunStatus::Finished if failed => "error",
                RunStatus::Finished => "ok",
            },
        );
    });
}

/// 这串动作里有没有「往目标程序注入输入」的动作（Text / Keys / Mouse / 临时文本，以及注入
/// 切换键的 `System::SwitchDesktop`，含 If / Parallel 分支内的）。
///
/// 修饰键没松时只有这类动作会被污染（见 [`fire`]），命令 / 文件 / 应用类照常执行；
/// 静态过一遍链足够准，If / Parallel 的嵌套两路都算「可能有」。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) fn injects_keys(actions: &[Action]) -> bool {
    actions.iter().any(|a| match a {
        Action::Text { .. } | Action::Keys { .. } | Action::Mouse { .. } => true,
        // 「临时文本」走剪贴板粘贴，会发出 Ctrl+V / ⌘V；写入 / 读取剪贴板本身不注入。
        Action::Clipboard {
            operation: ClipboardOperation::TempStr { .. },
            ..
        } => true,
        // 算法动作只有勾了「直接注入结果」才走 SendInput；写变量那条不注入。
        Action::Transform { inject, .. } => *inject,
        Action::Group { actions, .. } => injects_keys(actions),
        // 切换虚拟桌面也是注入（`Win+Ctrl+←` 这类）：触发它的修饰键还按着的话，注入的组合键
        // 会被污染成 `Ctrl+Alt+Win+Ctrl+←`——切桌面就成了别的东西。
        #[cfg(feature = "automation")]
        Action::System {
            operation: kada_core::SystemOperation::SwitchDesktop { .. },
            ..
        } => true,
        #[cfg(feature = "automation")]
        Action::If {
            then, otherwise, ..
        } => injects_keys(then) || injects_keys(otherwise),
        #[cfg(feature = "automation")]
        Action::Parallel { actions, .. } => injects_keys(actions),
        _ => false,
    })
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub(crate) fn fire(
    _app: tauri::AppHandle,
    _results: Arc<Mutex<Vec<CommandResult>>>,
    _unread: Arc<AtomicBool>,
    _runs: Arc<Mutex<Vec<RunRecord>>>,
    _actions: Vec<Action>,
    _trigger: String,
    _name: String,
    _timeout_ms: u64,
    _text_mode: TextInjectMode,
    _device: Option<String>,
    _origin: Option<(i32, i32)>,
) {
    // 无钩子即无触发入口，本分支不会运行（macOS 占位）。
}

/// 保存配置的一小处改动（提示框的位置 / 外观 / 可见性）：与 [`set_config`] 同一套口径——
/// 「写盘 + 改内存 + 给监听销账」同持 `cfg_watch` 锁。
///
/// 走它而不是让提示框窗口回传整份配置：那个窗口手上没有（也不该有）用户正在编辑的草稿，
/// 整份回传等于用一个只看得到提示框的视图去覆盖配置。
pub(crate) fn save_settings_patch(
    app: &tauri::AppHandle,
    state: &KadaState,
    patch: impl FnOnce(&mut Settings),
) -> Result<(), String> {
    let mut watch = state.cfg_watch.lock().unwrap();
    let mut cfg = state.config.read().unwrap().clone();
    patch(&mut cfg.settings);
    // 位置 / 外观都是「手改 JSON 也允许」的值：顺手钳一下越界，别存进去一个 scale: 0。
    cfg.settings.hints = std::mem::take(&mut cfg.settings.hints).normalized();
    config_io::save(&state.file, &cfg)?;
    *state.config.write().unwrap() = cfg.clone();
    watch.adopt_own_write(&state.file);
    drop(watch);
    // 广播给别的窗口：主界面的 `cfg.settings.hints` 已经旧了，它下次保存会把旧值写回去
    // （用户在提示框里拖了位置、旋了滚轮，回设置页改个快捷键就被打回原形）。
    let _ = app.emit("config-updated", &cfg);
    push_sync(&cfg);
    Ok(())
}
