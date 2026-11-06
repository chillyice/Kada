// 决策层（T-05 从 lib.rs 搬出）：跨平台事件模型 `Ev` / 单条事件的决定 / 快捷键与改键匹配 /
// 热串注入与双击唤醒 / 前台切换与钩子重装的复位巡检（见 说明书 §4.1 / §5.3）。
//
// 纯判定，不碰窗口与 IO：`decide` 只回答「吞掉 / 改发 / 触发」，真正的执行在 engine_host 的 `fire`。

use crate::*;

/// 跨平台键盘事件：各平台钩子统一喂给决策器/录制器。
#[derive(Clone, Debug)]
pub(crate) enum Ev {
    Down {
        key: Key,
        mods: BTreeSet<Modifier>,
        repeat: bool,
    },
    Up {
        key: Key,
    },
}

impl Ev {
    /// 事件来自哪个键（按下 / 抬起都算）。
    fn key(&self) -> Key {
        match self {
            Ev::Down { key, .. } | Ev::Up { key } => *key,
        }
    }

    /// 是不是**指针**（鼠标 / 触摸板 / 数位板）输入：中键 / 侧键 / 滚轮。
    /// 「设备是」据此在键盘设备与指针设备之间选一条（两条分开记，见 `input` 模块）。
    pub(crate) fn is_pointer_input(&self) -> bool {
        self.key().is_mouse()
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) fn to_ev(ev: &input::KeyEvent) -> Ev {
    match ev {
        input::KeyEvent::Down { key, mods, repeat } => Ev::Down {
            key: *key,
            mods: mods.clone(),
            repeat: *repeat,
        },
        input::KeyEvent::Up { key, .. } => Ev::Up { key: *key },
    }
}

/// 单条事件的决定。
pub(crate) enum Outcome {
    /// 命中快捷键：返回其全部动作（按顺序执行）+ 触发键与名称（供消息中心展示）。
    Shortcut {
        actions: Vec<Action>,
        trigger: String,
        name: String,
    },
    /// 命中改键：改发某个键。
    Replace(Key),
    /// 放行。
    Pass,
}

/// 暂停租约时长（毫秒）：录入捕获最多暂停这么久，超时自动恢复。
///
/// 暂停本来是「录入组合键/序列/和弦期间屏蔽触发，避免自己触发自己」，但前端若被中断
/// （切走窗口、关掉详情、窗口收进托盘）可能来不及解除，布尔量会永久卡在暂停态——表现
/// 为整个应用静默失效（快捷键、改键、文本扩展全不响应），用户完全看不出原因。
/// 改成租约：到期自动失效，前端正常解除仍即时生效。
pub(crate) const PAUSE_LEASE_MS: u64 = 60_000;

/// 状态机定时推进间隔（毫秒）。键序列超时、连击等待窗这类「只能靠时间判定」的等待态，
/// 必须由定时器驱动落地——只靠「下一个事件」懒判定的后果是：单独按一下序列 leader 键
/// （之后不按别的键）回放永远不发生，用户看到的是「这个键按了没反应」。见 [`Engine::tick`]。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) const ENGINE_TICK_MS: u64 = 60;

/// 「安全输入」提示的巡检周期（拍数）：[`ENGINE_TICK_MS`] × 32 ≈ 2 秒——状态翻转后
/// 最多两秒出提示，又不必每拍都进一次系统调用（macOS 专属，见 [`report_secure_input`]）。
#[cfg(target_os = "macos")]
pub(crate) const SECURE_INPUT_TICKS: u32 = 32;

/// 进程启动时刻（暂停租约的时间基准）。
static PROCESS_START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);

/// 进程启动至今的毫秒数。
fn now_ms() -> u64 {
    PROCESS_START.elapsed().as_millis() as u64
}

/// 暂停租约是否生效中。
pub(crate) fn pause_active(until: &AtomicU64) -> bool {
    let t = until.load(Ordering::Relaxed);
    t != 0 && now_ms() < t
}

/// 设置暂停租约（`true` = 暂停 [`PAUSE_LEASE_MS`]，`false` = 立即解除）。
pub(crate) fn set_pause_lease(until: &AtomicU64, paused: bool) {
    let deadline = if paused { now_ms() + PAUSE_LEASE_MS } else { 0 };
    until.store(deadline, Ordering::Relaxed);
}

/// 层语义匹配普通改键（非 tap-hold）：激活层条目优先、基础层条目兜底。
/// `ctx` 是触发侧 `when` 门控上下文：条件不成立的条目与「不存在」等价（跳过、落到下一条）。
fn match_plain_remap(
    cfg: &Config,
    key: Key,
    active_layer: Option<&str>,
    ctx: &TriggerContext,
) -> Option<Key> {
    for r in &cfg.remaps {
        if !r.enabled || r.needs_timing_state() || r.layer.as_deref() != active_layer {
            continue;
        }
        if let (Ok(from), Ok(to)) = (r.from.parse::<Key>(), r.to.parse::<Key>()) {
            if from == key && r.when_matches(ctx) {
                return Some(to);
            }
        }
    }
    if active_layer.is_some() {
        for r in &cfg.remaps {
            if !r.enabled || r.needs_timing_state() || r.layer.is_some() {
                continue;
            }
            if let (Ok(from), Ok(to)) = (r.from.parse::<Key>(), r.to.parse::<Key>()) {
                if from == key && r.when_matches(ctx) {
                    return Some(to);
                }
            }
        }
    }
    None
}

/// 层语义匹配快捷键：激活层条目优先、基础层条目兜底。返回 (动作, 触发键, 名称)。
fn match_shortcut(
    cfg: &Config,
    raw: &RawEvent,
    active_layer: Option<&str>,
    ctx: &TriggerContext,
) -> Option<(Vec<Action>, String, String)> {
    for s in &cfg.shortcuts {
        if !s.enabled || s.layer.as_deref() != active_layer {
            continue;
        }
        for t in &s.triggers {
            if let Ok(sc) = t.parse::<Shortcut>() {
                if matches(raw, &sc) && s.when_matches(ctx) {
                    return Some((
                        s.actions.clone(),
                        t.clone(),
                        s.name.clone().unwrap_or_default(),
                    ));
                }
            }
        }
    }
    if active_layer.is_some() {
        for s in &cfg.shortcuts {
            if !s.enabled || s.layer.is_some() {
                continue;
            }
            for t in &s.triggers {
                if let Ok(sc) = t.parse::<Shortcut>() {
                    if matches(raw, &sc) && s.when_matches(ctx) {
                        return Some((
                            s.actions.clone(),
                            t.clone(),
                            s.name.clone().unwrap_or_default(),
                        ));
                    }
                }
            }
        }
    }
    None
}

pub(crate) fn decide(
    ev: &Ev,
    cfg: &Config,
    active_layer: Option<&str>,
    ctx: &TriggerContext,
) -> Outcome {
    match ev {
        Ev::Down { key, mods, repeat } => {
            // 改键优先于快捷键：先消耗掉原生键，避免改键后键再触发快捷键。
            if let Some(to) = match_plain_remap(cfg, *key, active_layer, ctx) {
                return Outcome::Replace(to);
            }
            if !*repeat {
                let raw = RawEvent {
                    key: *key,
                    mods: mods.clone(),
                    pressed: true,
                };
                if let Some((actions, trigger, name)) = match_shortcut(cfg, &raw, active_layer, ctx)
                {
                    return Outcome::Shortcut {
                        actions,
                        trigger,
                        name,
                    };
                }
            }
            Outcome::Pass
        }
        Ev::Up { .. } => Outcome::Pass,
    }
}

/// 按「文本注入方式」设置发文本（规划 7.3-㉒）：默认剪贴板粘贴；目标程序吞粘贴
/// （游戏 / 终端）时切「逐字直发」。热串扩展与 Text 动作共用同一口径（后者在
/// kada-actions 内按 `RunOptions.text_inject_mode` 分派）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn type_text_by_mode(text: &str, mode: TextInjectMode) -> std::io::Result<()> {
    match mode {
        TextInjectMode::Clipboard => input::simulate::type_text(text),
        TextInjectMode::Unicode => input::simulate::type_text_unicode(text),
    }
}

/// 后台执行一次文本扩展：回删触发词 → 注入替换文本 → 补回后缀键。
///
/// 必须另起线程：注入走剪贴板 + `SendInput`，要上百毫秒，跑在钩子回调里会被系统判超时
/// 摘掉钩子（之后快捷键/改键/文本扩展全部失效）。后缀键在判定命中时已被吞掉，这里补回，
/// 保证「addr␣」展开成「我的地址␣」。注入失败会记进消息中心（没有控制台时不再无声无息）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) fn spawn_hotstring(
    app: tauri::AppHandle,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    hit: HotstringHit,
    text_mode: TextInjectMode,
) {
    std::thread::spawn(move || {
        for _ in 0..hit.backspaces {
            input::simulate::tap(Key::Backspace);
        }
        let expanded = resolve_hotstring(&hit.replace);
        if let Err(e) = type_text_by_mode(&expanded, text_mode) {
            commit_result(
                &app,
                &results,
                &unread,
                CommandResult {
                    kind: "hotstring".into(),
                    label: "文本扩展".into(),
                    command: hit.trigger.clone(),
                    trigger: hit.trigger.clone(),
                    name: String::new(),
                    stdout: String::new(),
                    stderr: format!("注入替换文本失败：{e}（触发词已被回删）"),
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
            return;
        }
        input::simulate::tap(hit.terminator);
    });
}

/// 展开热串替换文本的动态片段：`{date}` / `{time}` / `{clipboard}`。
/// 未知占位符保持原样（不破坏用户输入的字面 `{...}`）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn resolve_hotstring(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                let token = &after[..end];
                let val = match token {
                    "date" => Some(chrono::Local::now().format("%Y-%m-%d").to_string()),
                    "time" => Some(chrono::Local::now().format("%H:%M:%S").to_string()),
                    "clipboard" => input::simulate::get_clipboard_text().ok(),
                    _ => None,
                };
                match val {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push('{');
                        out.push_str(token);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 双击唤醒的判定窗口：两次「唤醒键按下」的间隔在此之内才算双击。
pub(crate) const WAKE_DOUBLE_TAP_MS: u64 = 400;

/// 快速唤醒判定：这一击是否构成「双击唤醒键」。
///
/// 被动检测——只报告命中、不拦截按键：唤醒键（UI 里建议选 Alt）常同时是修饰键，拦截会
/// 破坏 Alt+Tab / Alt+字母 等组合。两次按下（非自动重复）间隔 ≤[`WAKE_DOUBLE_TAP_MS`]
/// 判为双击；其间按下其它键则取消上一次单击计数（视作组合键的一部分）。唤醒键由
/// `Settings::wake_key` 指定，**`None` = 关闭快速唤醒**（是 `Settings` 的默认值）。
///
/// **只判定，不建窗口**：调用方命中后必须把唤窗丢到后台线程（见 `run` 里的接线）——
/// 冷启动时建 WebView 要几百毫秒，跑在钩子回调里会被系统判超时摘掉钩子，之后快捷键 /
/// 改键 / 热串全部静默失效。这里不持有 `AppHandle`，顺带也就能单测了。
pub(crate) fn wake_double_tap(
    cfg: &RwLock<Config>,
    last_tap: &mut Option<Instant>,
    ev: &Ev,
) -> bool {
    let Ev::Down { key, repeat, .. } = ev else {
        return false;
    };
    if *repeat {
        return false;
    }
    {
        let guard = cfg.read().unwrap();
        let Some(wake) = guard.settings.wake_key.as_ref() else {
            return false;
        };
        let Ok(wk) = wake.parse::<Key>() else {
            return false;
        };
        if *key != wk {
            // 按下其它键 → 上一次唤醒键单击不算数（可能是组合键的一部分）。
            last_tap.take();
            return false;
        }
    }
    let now = Instant::now();
    match *last_tap {
        Some(prev) if now.duration_since(prev) <= Duration::from_millis(WAKE_DOUBLE_TAP_MS) => {
            // 计完这一双击就归零：再要唤醒得重新点两下。
            *last_tap = None;
            true
        }
        _ => {
            *last_tap = Some(now);
            false
        }
    }
}

/// 前台窗口观察器：**连续两轮巡检看到同一个新窗口**才认「切换」。
///
/// 为什么要确认一轮：`GetForegroundWindow` 会被瞬态窗口抖一下（我们自己的触发气泡、
/// 右键菜单、UAC 提示、桌面切换瞬间），而那些抖动不是「用户换了工作窗口」。误判的代价
/// 是白复位一次——用户手上正凑的和弦 / 刚按的序列 leader 会被丢掉。多等一个巡检间隔
/// （[`ENGINE_TICK_MS`]，60ms）就能把抖动全滤掉，对「清理残留状态」完全没体感。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[derive(Debug)]
pub(crate) struct FocusTracker {
    /// 已确认的当前窗口。
    confirmed: Option<isize>,
    /// 刚看到、还等下一轮确认的新窗口。
    candidate: Option<isize>,
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
impl FocusTracker {
    pub(crate) fn new(initial: Option<isize>) -> Self {
        Self {
            confirmed: initial,
            candidate: None,
        }
    }

    /// 观察一次当前前台窗口，返回是否是一次**真的**切换。
    pub(crate) fn observe(&mut self, now: Option<isize>) -> bool {
        match now {
            Some(w) if Some(w) == self.confirmed => {
                self.candidate = None;
                false
            }
            Some(w) => {
                if self.candidate == Some(w) {
                    self.confirmed = Some(w);
                    self.candidate = None;
                    true
                } else {
                    // 第一次看到，先记下来等下一轮确认。
                    self.candidate = Some(w);
                    false
                }
            }
            // 取不到句柄（锁屏、无前台窗口）：瞬态读空不作数，也不覆盖候选——
            // 锁屏读空、解锁回到同一个窗口时不该复位。
            None => false,
        }
    }
}

/// 输入状态复位的巡检状态（见规划 7.2-④）：上次看到的钩子重装次数 + 前台窗口观察器。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) struct ResetWatch {
    reinstalls: u64,
    focus: FocusTracker,
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
impl ResetWatch {
    pub(crate) fn new() -> Self {
        Self {
            reinstalls: input::reinstall_count(),
            focus: FocusTracker::new(input::foreground_window()),
        }
    }

    /// 巡检一次，返回「需要复位」的原因（`None` = 一切正常）；基线无论如何都推进。
    ///
    /// 两条触发路径正是 `Engine` 那份时序状态会变脏的场景：
    /// - **钩子已重装**（底座自愈看门狗修好了被系统摘除 / 唤醒 / 解锁后的钩子）：那次断线
    ///   期间的 keyup 全丢了，底座自己复位了 `HELD_KEYS` 等三张表，壳层这份镜像也得复位——
    ///   通道就是底座单调递增的 `reinstall_count`（不用回调：那要在钩子线程的消息循环里跑
    ///   壳层代码，还得多一套跨线程注册；60ms 轮询一次足够，且完全不碰钩子层）。
    /// - **前台窗口切换**（Alt-Tab / 点别的窗口）：待定的和弦、序列、连击、热串缓冲都属于
    ///   上一个窗口，跟着过去会打进新窗口；按住式层也要退出（切层键的抬起可能永远不来）。
    pub(crate) fn poll(&mut self) -> Option<&'static str> {
        let reinstalls = input::reinstall_count();
        let reinstalled = reinstalls != self.reinstalls;
        self.reinstalls = reinstalls;

        let switched = self.focus.observe(input::foreground_window());

        if reinstalled {
            Some("钩子已重装")
        } else if switched {
            Some("前台窗口切换")
        } else {
            None
        }
    }
}
