//! 动作执行引擎：把「触发 → 执行一串动作」的落地逻辑从桌面壳迁出，独立成 crate。
//!
//! 依赖 `kada-core`（动作/配置模型）与 `kada-hook`（文本/按键模拟注入）。
//! 命令类动作的执行结果以 [`CommandResult`] 形式产出，由调用方（桌面壳）通过
//! 回调提交进消息中心；本 crate 不依赖 Tauri，可独立单测。
//!
//! **执行有界**（见规划 7.2-⑦）：命令 / 脚本类动作用 [`RunOptions::action_timeout`] 限时，
//! 超时连同子进程树一起强制终止；用户随时可以用 [`abort::request`] 叫停正在跑的动作链
//! （剩余动作不再执行、正在跑的命令立即被杀）。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

#[cfg(feature = "automation")]
use std::fs;
#[cfg(feature = "automation")]
use std::io::Read;
#[cfg(feature = "automation")]
use std::path::Path;
#[cfg(all(
    feature = "automation",
    any(target_os = "windows", target_os = "linux", target_os = "macos")
))]
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

pub mod abort;

use kada_core::{
    substitute_vars, Action, ClipboardOperation, FrontmostContext, Key, KeysMode, TextInjectMode,
    TextMode, TextValue, Value, Vars,
};

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
use kada_core::{MouseButton, MouseOp};

#[cfg(feature = "automation")]
use kada_core::{
    AppOperation, DesktopDirection, FileObject, OsOperation, Shell, SystemOperation, WindowOperation,
};

#[cfg(target_os = "linux")]
use kada_hook::linux::simulate;
#[cfg(target_os = "macos")]
use kada_hook::macos::simulate;
#[cfg(target_os = "windows")]
use kada_hook::win::simulate;

/// 一次命令类动作（CMD / PowerShell / 关闭程序 / 脚本）的执行结果，进入消息中心。
#[derive(Clone, Debug, Serialize)]
pub struct CommandResult {
    /// 动作种类："cmd" | "powershell" | "close_program" | "script"。
    pub kind: String,
    /// 展示标签："CMD" / "PowerShell" / "关闭程序" / "脚本"。
    pub label: String,
    /// 实际执行的命令文本。
    pub command: String,
    /// 触发它的快捷键（已渲染为 "Ctrl+Alt+C" 形式）。
    pub trigger: String,
    /// 快捷键名称（无名称时为空字符串）。
    pub name: String,
    /// 标准输出（UTF-8）。
    pub stdout: String,
    /// 标准错误（UTF-8）。
    pub stderr: String,
    /// 进程退出码；进程未能启动时为 None。
    pub exit_code: Option<i32>,
    /// 是否弹出结果弹窗（仅影响展示，不影响记录）。
    pub show_output: bool,
    /// 执行时间（本地时区，"YYYY-MM-DD HH:MM:SS"）。
    pub time: String,
    /// 属于哪次执行（[`RunRecord::id`]）。不是动作链产出的（配置消息、热串注入失败等）为 None——
    /// 消息中心据此把「非触发」的普通消息与执行记录分开列。
    pub run_id: Option<u64>,
}

/// 一次动作链跑完后的整体状态（`run_actions` 的返回值，决定执行记录卡片顶部的结论）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    /// 链跑完了（可能中间有失败的动作，见各步 `status`）。
    Finished,
    /// 用户请求中止：剩余动作没执行。
    Aborted,
}

/// 一条动作的执行记录：消息中心「执行记录」里时间线的一个时间块 / 流水线的一个节点。
///
/// 记录**每个**动作（含文本 / 按键 / 延迟这类不产命令结果的），否则「按了没反应」这类问题
/// 在消息中心里是查不到原因的。`parent` / `lane` 是流水线视图的骨架：`parent` 指所属的
/// 条件 / 并行节点，`lane` 是并行成员序号（流水线里各占一条泳道）。
#[derive(Clone, Debug, Serialize)]
pub struct ActionStep {
    /// 本次执行内自增的动作序号（时间线与流水线共用一个 id 空间）。
    pub id: u64,
    /// 所属的「条件判断 / 并行」动作 id；顶层动作为 None。
    pub parent: Option<u64>,
    /// 并行成员序号（0 起）；非并行成员恒 0。
    pub lane: u32,
    /// 动作类型（与 `Action` 的 serde tag 同名，旧版动作已折算到现行类型）。
    pub kind: String,
    /// 动作摘要（用户写的说明优先 + 动作本体渲染）。
    pub title: String,
    /// 条件判断走了哪条分支：`true` = then，`false` = otherwise。
    pub branch: Option<bool>,
    /// 附加数据：条件判断的「条件」本体（交由界面按自己的条件渲染器出摘要）。
    pub detail: Option<serde_json::Value>,
    /// "ok" | "error" | "abort"。
    pub status: String,
    /// 相对本次执行开始的毫秒数（`start_ms` 是开始时点，`end_ms` 是结束时点）。
    pub start_ms: u64,
    pub end_ms: u64,
    /// 命令 / 脚本类动作的标准输出。
    pub stdout: String,
    /// 标准错误（超时 / 被强杀的原因写在前缀里）。
    pub stderr: String,
    /// 进程退出码；未起进程时 None。
    pub exit_code: Option<i32>,
}

/// 一次触发 = 一条执行记录（消息中心里的一条 tab）。
#[derive(Clone, Debug, Serialize)]
pub struct RunRecord {
    /// 执行 id（全局自增）。
    pub id: u64,
    /// 触发它的快捷键（已渲染为 "Ctrl+Alt+K" 形式）。
    pub trigger: String,
    /// 快捷键名称（无名称时为空串）。
    pub name: String,
    /// 开始时间（本地时区，"YYYY-MM-DD HH:MM:SS"）。
    pub time: String,
    /// 开始时刻的 epoch 毫秒（时间线绝对定位用；界面据此还原每一块的真实时刻）。
    pub start_epoch_ms: u64,
    /// 总耗时（毫秒）。还在跑时为已跑掉的时长。
    pub duration_ms: u64,
    /// "running" | "ok" | "error" | "abort"。
    pub status: String,
    /// 逐步记录（`start_ms` / `end_ms` 相对本记录开始）。
    pub steps: Vec<ActionStep>,
}

/// 执行过程中的两个回调（命令结果 + 逐步记录）打包，方便整链与并行成员各自「就地借一份」。
pub struct Reporter<'a> {
    /// 命令类结果 → 消息中心（弹窗 / 红点仍按各自的 `show_output` 走）。
    pub commit: &'a mut dyn FnMut(CommandResult),
    /// 每个动作的执行记录 → 执行记录卡片（时间线 / 流水线）。
    pub on_step: &'a mut dyn FnMut(ActionStep),
}

impl Reporter<'_> {
    /// 记一条命令结果（字段名与方法同名，包一层省得每次写 `(self.commit)(…)`）。
    fn commit(&mut self, r: CommandResult) {
        (self.commit)(r)
    }

    /// 记一条动作执行记录。
    fn on_step(&mut self, s: ActionStep) {
        (self.on_step)(s)
    }
}

/// 一次动作链执行的运行参数（由调用方从配置读出后传入）。
#[derive(Clone, Copy, Debug)]
pub struct RunOptions {
    /// 命令 / 脚本类动作的执行超时；[`Duration::ZERO`] = 不限时。
    ///
    /// 超时不是「性能选项」而是可靠性兜底：`.output()` 会一直等下去，脚本挂住（等输入、
    /// 死循环、弹了个看不见的确认框）就永久占住这条触发的执行线程——后续动作永不执行、
    /// 线程也收不回来。
    pub action_timeout: Duration,
    /// 文本注入方式（读自 `Settings.text_inject_mode`，规划 7.3-㉒）：文本动作与
    /// 「转大小写」按它选「剪贴板粘贴」或「逐字直发」。
    pub text_inject_mode: TextInjectMode,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            action_timeout: Duration::from_millis(kada_core::DEFAULT_ACTION_TIMEOUT_MS),
            text_inject_mode: TextInjectMode::Clipboard,
        }
    }
}

/// 一次触发的现场：谁触发的（触发键 / 名称）与触发瞬间的前台窗口、触发设备。
///
/// 与 [`RunOptions`]（执行边界，读自配置）分开：这些是「这一次触发」的事实，而前台窗口 /
/// 设备类条件是按**触发那一刻**的现场求值的，所以它和超时一样要整条链共享同一份快照。
#[derive(Clone, Copy)]
pub struct TriggerCtx<'a> {
    pub trigger: &'a str,
    pub name: &'a str,
    pub frontmost: Option<&'a FrontmostContext>,
    /// 触发键来自哪台设备（Linux evdev 设备名）；Windows / macOS 恒 `None`。
    pub device: Option<&'a str>,
    /// 触发那一刻的前台窗口句柄（EXT-14「当前窗口」动作的目标）。仅 Windows 抓取，其余
    /// 平台恒 `None`——目标取「触发时」而不是执行时，动作链里先切了前台也不影响它。
    pub window: Option<isize>,
    /// 触发那一刻的鼠标位置（EXT-16「移动到手势起点」）。只有鼠标手势路径填（右键按下点），
    /// 快捷键 / 录制 / 托盘等路径恒 `None`；`MoveToOrigin` 拿不到时执行报错。
    pub origin: Option<(i32, i32)>,
}

/// 一次动作链的执行上下文：整条链共享（触发现场 / 超时 / 中止代数快照）。
struct Ctx<'a> {
    t: TriggerCtx<'a>,
    /// 见 [`RunOptions::action_timeout`]。
    #[cfg_attr(not(feature = "automation"), allow(dead_code))]
    timeout: Duration,
    /// 见 [`RunOptions::text_inject_mode`]（文本动作与「转大小写」用）。
    text_mode: TextInjectMode,
    /// 本次执行开始时记下的中止代数（[`abort::aborted`] 的比对基准）。
    gen: u64,
    /// 本次执行对应的记录 id（`run_actions` 的入参），写进 [`CommandResult::run_id`]，
    /// 让壳层能把同一次触发的结果归到一张卡里。
    run_id: Option<u64>,
    /// 本次执行开始的时刻：逐步记录的毫秒时间以它为零点。
    start: Instant,
    /// 动作序号发号器（并行成员各起线程，靠原子计数保证 id 唯一）。
    step_seq: AtomicU64,
    /// 按住作用域发号器（EXT-16）：一条链 / 一个串行化的并行组各一个作用域，
    /// `Keys press` 登记到当前作用域，作用域结束 / 失败 / 中止时整批抬起。
    scope_seq: AtomicU64,
    /// 链内注入串行锁（规划 7.2-㉞）：注入类动作（文本 / 按键 / 鼠标）持锁执行，保证
    /// 并行分组的成员之间注入不交错、剪贴板不被并发覆盖。单线程链无竞争，取锁零代价。
    inject_lock: Mutex<()>,
}

impl Ctx<'_> {
    /// 距本次执行开始已过的毫秒数（逐步记录的时点）。
    fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// 发一个新动作 id。
    fn next_step_id(&self) -> u64 {
        self.step_seq.fetch_add(1, Ordering::Relaxed)
    }

    /// 发一个新的按住作用域 id。
    fn next_scope(&self) -> u64 {
        self.scope_seq.fetch_add(1, Ordering::Relaxed)
    }
}

/// 递归执行一串动作：遇到「条件判断」动作时按条件求值选择 `then` / `otherwise` 分支继续。
/// 命令类结果通过 [`Reporter::commit`] 产出（由调用方决定如何进消息中心），
/// 每个动作（含文本 / 按键 / 延迟）的执行情况通过 [`Reporter::on_step`] 逐步上报，
/// 由调用方汇成一张执行记录卡（时间线 / 流水线）。
/// `t` 是本次触发的现场（触发键 / 名称 / 前台窗口，供「前台应用/窗口」类条件求值），
/// `vars` 承载本次触发内的变量，`last_copied` 记录最近一次复制/剪切的来源。
/// `opts` 提供超时等执行边界；用户请求中止时停止执行剩余动作并记一条结果。
/// `run_id` 是本次执行的记录 id（写进产出结果，壳层据此归组）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[cfg_attr(not(feature = "automation"), allow(unused_variables))]
pub fn run_actions(
    rep: &mut Reporter<'_>,
    actions: &[Action],
    t: TriggerCtx<'_>,
    vars: &mut Vars,
    last_copied: &mut Option<String>,
    opts: &RunOptions,
    run_id: Option<u64>,
) -> RunStatus {
    // 「正在执行」计数（UI 据此回答「有没有东西可以停」）；整个动作链期间有效。
    let _running = abort::RunGuard::new();
    let ctx = Ctx {
        t,
        timeout: opts.action_timeout,
        text_mode: opts.text_inject_mode,
        gen: abort::generation(),
        run_id,
        start: Instant::now(),
        step_seq: AtomicU64::new(0),
        scope_seq: AtomicU64::new(1),
        inject_lock: Mutex::new(()),
    };
    let scope = ctx.next_scope();
    let _held = HeldScope(scope);
    run_chain(rep, actions, &ctx, vars, last_copied, scope, None, 0);
    if abort::aborted(ctx.gen) {
        RunStatus::Aborted
    } else {
        RunStatus::Finished
    }
}

/// 开一条动作记录：`id` / 标题 / 类型 / 起始时点，执行完再由调用方补结束与结果。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn step_begin(
    action: &Action,
    ctx: &Ctx,
    parent: Option<u64>,
    lane: usize,
    vars: &Vars,
) -> ActionStep {
    let mut step = ActionStep {
        id: ctx.next_step_id(),
        parent,
        lane: lane as u32,
        kind: action_kind(action).into(),
        title: action_summary(action, vars),
        branch: None,
        detail: None,
        status: "ok".into(),
        start_ms: ctx.elapsed_ms(),
        end_ms: 0,
        stdout: String::new(),
        stderr: String::new(),
        exit_code: None,
    };
    step.end_ms = step.start_ms;
    // 条件本体交给界面渲染摘要：条件的展示格式是界面那边的事，不在这里抄一份。
    #[cfg(feature = "automation")]
    if let Action::If { condition, .. } = action {
        step.detail = serde_json::to_value(condition).ok();
    }
    step
}

/// 动作类型名（与 `Action` 的 serde tag 同名，好让界面直接复用它的图标 / 配色表）。
/// 旧版动作（加载时已迁移、但防御性地）折算到现行类型。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn action_kind(action: &Action) -> &'static str {
    match action {
        Action::Text { .. } => "text",
        Action::Keys { .. } => "keys",
        Action::PauseMs { .. } => "pause_ms",
        Action::Mouse { .. } => "mouse",
        Action::Clipboard { .. } => "clipboard",
        Action::Group { .. } => "group",
        #[cfg(feature = "automation")]
        Action::Command { .. } | Action::Cmd { .. } | Action::Powershell { .. } => "command",
        #[cfg(feature = "automation")]
        Action::Launch { .. } | Action::App { .. } | Action::CloseProgram { .. } => "app",
        #[cfg(feature = "automation")]
        Action::OpenFolder { .. } | Action::Os { .. } => "os",
        #[cfg(feature = "automation")]
        Action::System { .. } => "system",
        #[cfg(feature = "automation")]
        Action::OpenUrl { .. } => "open_url",
        #[cfg(feature = "automation")]
        Action::Script { .. } => "script",
        #[cfg(feature = "automation")]
        Action::If { .. } => "if",
        #[cfg(feature = "automation")]
        Action::Parallel { .. } => "parallel",
        #[cfg(feature = "automation")]
        Action::Window { .. } => "window",
    }
}

/// 中止在时间线上的那一条（点「停止执行」之后，链里要有个可见的收尾）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn abort_step(ctx: &Ctx) -> ActionStep {
    let now = ctx.elapsed_ms();
    ActionStep {
        id: ctx.next_step_id(),
        parent: None,
        lane: 0,
        kind: "abort".into(),
        title: "（用户请求停止）".into(),
        branch: None,
        detail: None,
        status: "abort".into(),
        start_ms: now,
        end_ms: now,
        stdout: String::new(),
        stderr: "已停止执行：剩余动作不再执行，正在运行的命令 / 脚本已强制终止。".into(),
        exit_code: None,
    }
}

/// 取链内注入锁。毒化也照样取回：它只是一把「把注入串起来」的空锁，没有需要保护的不变量，
/// 一个成员 panic 不该让后续注入永久取不到锁。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn lock_inject<'c>(ctx: &'c Ctx<'_>) -> std::sync::MutexGuard<'c, ()> {
    ctx.inject_lock.lock().unwrap_or_else(|e| e.into_inner())
}

/// 动作链按住的键（EXT-16）：`(作用域 id, 键)`。全局登记是为了让输入复位（前台切换 /
/// 设备拔出 / 钩子重装 / 配置整份换掉）能从壳层一次性收回——那时已经拿不到某条链的 `Ctx`。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
static HELD_KEYS: Mutex<Vec<(u64, Key)>> = Mutex::new(Vec::new());

/// 按下一个键并登记到作用域。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn hold_key(scope: u64, key: Key) {
    HELD_KEYS.lock().unwrap().push((scope, key));
    simulate::down(key);
}

/// 抬起一个键并从作用域注销（注销最近一次登记，同一键按两次也只抬一次）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn unhold_key(scope: u64, key: Key) {
    {
        let mut held = HELD_KEYS.lock().unwrap();
        if let Some(i) = held.iter().rposition(|(s, k)| *s == scope && *k == key) {
            held.remove(i);
        }
    }
    simulate::up(key);
}

/// 抬起某个作用域里仍按着的键（逆序），并清空登记。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn release_scope(scope: u64) {
    let mut keys: Vec<Key> = Vec::new();
    {
        let mut held = HELD_KEYS.lock().unwrap();
        let mut i = 0;
        // 作用域内的按键很少（通常 1~3 个），O(n²) 的 remove 比额外分配一份映射更省事。
        while i < held.len() {
            if held[i].0 == scope {
                keys.push(held.remove(i).1);
            } else {
                i += 1;
            }
        }
    }
    for k in keys.into_iter().rev() {
        simulate::up(k);
    }
}

/// 输入复位时收回所有仍被动作链按住的键（壳层在 `Engine::reset` 同一处调用）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub fn release_held_keys() {
    let keys: Vec<Key> = {
        let mut held = HELD_KEYS.lock().unwrap();
        held.drain(..).map(|(_, k)| k).collect()
    };
    for k in keys.into_iter().rev() {
        simulate::up(k);
    }
}

/// 没有输入注入后端的平台：没有按住状态可收。
#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub fn release_held_keys() {}

/// 作用域守卫：无论正常收尾、失败提前返回还是 panic，都把该作用域按住的键抬起来。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
struct HeldScope(u64);

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
impl Drop for HeldScope {
    fn drop(&mut self) {
        release_scope(self.0);
    }
}

/// 动作树里有没有 `Keys` 的按下 / 抬起（含嵌套 `If` / `Parallel` / 分组）。
/// 有的话并行组不能真并发：注入锁只保证单步不交错，不保证成员顺序（会出「先松后按」）。
#[cfg(all(
    feature = "automation",
    any(target_os = "windows", target_os = "linux", target_os = "macos")
))]
fn has_key_lifecycle(actions: &[Action]) -> bool {
    actions.iter().any(|a| match a {
        Action::Keys {
            mode: KeysMode::Press | KeysMode::Release,
            ..
        } => true,
        Action::Group { actions, .. } => has_key_lifecycle(actions),
        Action::If {
            then, otherwise, ..
        } => has_key_lifecycle(then) || has_key_lifecycle(otherwise),
        Action::Parallel { actions, .. } => has_key_lifecycle(actions),
        _ => false,
    })
}

/// 动作链的实际执行（`If` 分支递归时复用同一 [`Ctx`]：超时与中止代数整链共享）。
/// `parent` / `lane` 是本层动作在流水线里的位置：所属容器节点 id 与并行成员序号。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[cfg_attr(not(feature = "automation"), allow(unused_variables))]
fn run_chain(
    rep: &mut Reporter<'_>,
    actions: &[Action],
    ctx: &Ctx,
    vars: &mut Vars,
    last_copied: &mut Option<String>,
    scope: u64,
    parent: Option<u64>,
    lane: usize,
) {
    for action in actions {
        // 中止在执行前判一次：正在跑的进程由 `run_with_limits` 的轮询杀掉，这里保证
        // 「剩余动作不再执行」——否则点停止后还会继续注入按键 / 启动程序。
        if abort::aborted(ctx.gen) {
            release_scope(scope);
            rep.commit(abort_result(ctx));
            rep.on_step(abort_step(ctx));
            return;
        }
        let mut step = step_begin(action, ctx, parent, lane, vars);
        if let Action::Group {
            actions: members, ..
        } = action
        {
            let id = step.id;
            run_chain(rep, members, ctx, vars, last_copied, scope, Some(id), 0);
            step.status = if abort::aborted(ctx.gen) {
                "abort".into()
            } else {
                "ok".into()
            };
            step.end_ms = ctx.elapsed_ms();
            rep.on_step(step);
            // 成员链已因中止收尾并记过一条结果，别在外层再记一遍。
            if abort::aborted(ctx.gen) {
                release_scope(scope);
                return;
            }
            continue;
        }
        #[cfg(feature = "automation")]
        if let Action::If {
            condition,
            then,
            otherwise,
            ..
        } = action
        {
            let hit = condition.matches(vars, ctx.t.frontmost, ctx.t.device);
            step.branch = Some(hit);
            let id = step.id;
            run_chain(
                rep,
                if hit { then } else { otherwise },
                ctx,
                vars,
                last_copied,
                scope,
                Some(id),
                0,
            );
            step.status = if abort::aborted(ctx.gen) {
                "abort".into()
            } else {
                "ok".into()
            };
            step.end_ms = ctx.elapsed_ms();
            rep.on_step(step);
            // 分支里已经因中止收尾过（并记了一条结果）：外层别再记一遍。
            if abort::aborted(ctx.gen) {
                release_scope(scope);
                return;
            }
            continue;
        }
        #[cfg(feature = "automation")]
        if let Action::Parallel {
            actions: members, ..
        } = action
        {
            let id = step.id;
            run_parallel(rep, members, ctx, vars, last_copied, id);
            step.status = if abort::aborted(ctx.gen) {
                "abort".into()
            } else {
                "ok".into()
            };
            step.end_ms = ctx.elapsed_ms();
            rep.on_step(step);
            // 并行组被中止：`run_parallel` 已滤掉成员级的中止结果，这里统一补记一条再收尾
            // （若只是 `continue`，下一轮顶部的中止判定会再记一条，变成两条）。
            if abort::aborted(ctx.gen) {
                release_scope(scope);
                rep.commit(abort_result(ctx));
                rep.on_step(abort_step(ctx));
                return;
            }
            continue;
        }
        match run_action(action, ctx, vars, last_copied, scope) {
            Ok(Some(result)) => {
                // 非零退出码与「被超时 / 中止强杀」（原因写在 stderr 前缀里）都算失败。
                let failed = result.exit_code.is_some_and(|c| c != 0)
                    || result.stderr.starts_with("〔咔哒〕");
                step.stdout = result.stdout.clone();
                step.stderr = result.stderr.clone();
                step.exit_code = result.exit_code;
                // 命令结果仍照旧进消息中心（开了「显示输出」还要弹窗），执行记录只是同一件事的另一面。
                rep.commit(result);
                if abort::aborted(ctx.gen) {
                    step.status = "abort".into();
                } else if failed {
                    step.status = "error".into();
                }
            }
            Ok(None) => {}
            Err(e) => {
                // 失败同样进消息中心：此前只写 stderr 日志，而发布版是没有控制台的 GUI 程序，
                // 用户看到的现象就是「按了没反应」——注入失败、路径不存在、程序起不来全都查不到原因。
                eprintln!("动作执行失败: {e}");
                // 失败就把当前作用域里按住的键收回来：不让 `Ctrl` 之类的修饰键粘在系统里，
                // 也不让后续动作误以为自己还处在「按住」状态。
                release_scope(scope);
                let failure = failure_result(action, ctx, vars, &e);
                step.stderr = failure.stderr.clone();
                step.status = "error".into();
                rep.commit(failure);
            }
        }
        if abort::aborted(ctx.gen) {
            step.status = "abort".into();
        }
        step.end_ms = ctx.elapsed_ms();
        rep.on_step(step);
    }
}

/// 执行一个并行分组（规划 7.2-㉞）：成员各起一个线程同时开跑，**等全部结束才返回**。
///
/// 成员拿到 `vars` / `last_copied` 的私有快照，结束后按下标顺序合并回主链（确定性「后者胜」，
/// 组内兄弟读不到彼此的写——判重与顺序语义见需求文档）。成员结果先在本地收集、join 后按下标
/// 顺序提交：`Reporter` 不是 `Sync`，不能跨线程共享；成员级的中止结果在此滤掉，由调用方
/// 统一补记一条（否则 N 个成员各记一条）。注入类成员之间仍由链内注入锁串行（见 [`Ctx::inject_lock`]），
/// 真正并发的是命令 / 脚本 / 文件 / 延迟这类非注入步骤。
///
/// **含按键按下 / 抬起时退化为串行**（EXT-16）：注入锁只保证单步不交错，不保证成员顺序，
/// 「按下左键 → 移动 → 松开左键」拆成三个并行成员可能按 B→C→A 跑。只要任一成员（含嵌套）
/// 用到 `KeysMode::Press / Release`，整组按声明顺序执行，保证 A→B→C。
#[cfg(all(
    feature = "automation",
    any(target_os = "windows", target_os = "linux", target_os = "macos")
))]
fn run_parallel(
    rep: &mut Reporter<'_>,
    members: &[Action],
    ctx: &Ctx,
    vars: &mut Vars,
    last_copied: &mut Option<String>,
    parent: u64,
) {
    let serial = members.iter().any(|m| has_key_lifecycle(std::slice::from_ref(m)));
    // 每个成员：私有 vars / last_copied 快照 + 本地结果表。
    let outs: Vec<(Vars, Option<String>, Vec<CommandResult>, Vec<ActionStep>)> = if serial {
        // 串行组共用一个按住作用域：`Press` 在成员 0、`Release` 在成员 2 也能跨成员保持。
        let gscope = ctx.next_scope();
        let _held = HeldScope(gscope);
        members
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let mut member_vars = vars.clone();
                let mut member_copy = last_copied.clone();
                let mut local: Vec<CommandResult> = Vec::new();
                let mut local_steps: Vec<ActionStep> = Vec::new();
                {
                    let mut collect = |r: CommandResult| local.push(r);
                    let mut collect_step = |s: ActionStep| local_steps.push(s);
                    let mut local_rep = Reporter {
                        commit: &mut collect,
                        on_step: &mut collect_step,
                    };
                    run_chain(
                        &mut local_rep,
                        std::slice::from_ref(m),
                        ctx,
                        &mut member_vars,
                        &mut member_copy,
                        gscope,
                        Some(parent),
                        i,
                    );
                }
                (member_vars, member_copy, local, local_steps)
            })
            .collect()
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = members
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let mut member_vars = vars.clone();
                    let mut member_copy = last_copied.clone();
                    scope.spawn(move || {
                        let mut local: Vec<CommandResult> = Vec::new();
                        let mut local_steps: Vec<ActionStep> = Vec::new();
                        // 真并行的成员各用一个作用域：某个成员失败时只收回它自己按住的键，
                        // 不会误伤外层 / 兄弟成员（含按下动作的组已走上面的串行分支）。
                        let member_scope = ctx.next_scope();
                        let _held = HeldScope(member_scope);
                        {
                            let mut collect = |r: CommandResult| local.push(r);
                            let mut collect_step = |s: ActionStep| local_steps.push(s);
                            let mut local_rep = Reporter {
                                commit: &mut collect,
                                on_step: &mut collect_step,
                            };
                            // 单个成员仍走完整链：成员里的 `If` / 嵌套 `Parallel` / 中止判定照常。
                            run_chain(
                                &mut local_rep,
                                std::slice::from_ref(m),
                                ctx,
                                &mut member_vars,
                                &mut member_copy,
                                member_scope,
                                Some(parent),
                                i,
                            );
                        }
                        (member_vars, member_copy, local, local_steps)
                    })
                })
                .collect();
            // 全部 join（不允许 `scope` 在未 join 的 panic 线程上二次 panic）；成员 panic 时按
            // 无结果 / 无变量回退，不让一个坏成员炸掉整条链。
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_default())
                .collect()
        })
    };
    for (member_vars, member_copy, results, steps) in outs {
        for r in results {
            if r.kind == "abort" {
                continue; // 成员级中止合并成一条，由调用方补记
            }
            rep.commit(r);
        }
        for s in steps {
            if s.status == "abort" && s.kind == "abort" {
                continue; // 同上：中止块不在每条泳道上各画一遍
            }
            rep.on_step(s);
        }
        vars.extend(member_vars);
        if member_copy.is_some() {
            *last_copied = member_copy;
        }
    }
}

/// 用户中止时补记的一条结果：进消息中心，让「点了停止」有可见的回应
/// （否则用户只能靠「它不再继续了」猜）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn abort_result(ctx: &Ctx) -> CommandResult {
    CommandResult {
        kind: "abort".into(),
        label: "中止".into(),
        command: "（用户请求停止）".into(),
        trigger: ctx.t.trigger.into(),
        name: ctx.t.name.into(),
        stdout: String::new(),
        stderr: "已停止执行：剩余动作不再执行，正在运行的命令 / 脚本已强制终止。".into(),
        exit_code: None,
        show_output: false,
        time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        run_id: ctx.run_id,
    }
}

/// 动作失败时进消息中心的一条记录。
///
/// 展示上用「哪种动作失败」当标签（tab 标题是「标签 · 名称」，一眼能看出坏在哪一步），
/// 「命令」一栏放动作摘要（用户写的说明优先，其次是动作本体渲染，如 `删除 D:\x.txt`）——
/// 同一个快捷键挂了多个同类动作时，只给类型是分不清哪一条炸了的。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn failure_result(action: &Action, ctx: &Ctx, vars: &Vars, err: &str) -> CommandResult {
    CommandResult {
        kind: "error".into(),
        label: action_label(action).into(),
        command: action_summary(action, vars),
        trigger: ctx.t.trigger.into(),
        name: ctx.t.name.into(),
        stdout: String::new(),
        stderr: err.to_string(),
        exit_code: None,
        show_output: action_show_output(action),
        time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        run_id: ctx.run_id,
    }
}

/// 失败记录的展示标签（消息中心 tab 上「标签 · 名称」的左半）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn action_label(action: &Action) -> &'static str {
    match action {
        Action::Text {
            mode: TextMode::Input,
            ..
        } => "文本注入失败",
        Action::Text { .. } => "大小写转换失败",
        Action::Keys { .. } => "按键注入失败",
        Action::PauseMs { .. } => "暂停失败",
        Action::Mouse { .. } => "鼠标模拟失败",
        Action::Clipboard { .. } => "剪贴板操作失败",
        Action::Group { .. } => "分组执行失败",
        #[cfg(feature = "automation")]
        Action::Command { .. } | Action::Cmd { .. } | Action::Powershell { .. } => "命令失败",
        #[cfg(feature = "automation")]
        Action::Launch { .. }
        | Action::App {
            operation: AppOperation::Launch { .. },
            ..
        } => "启动程序失败",
        #[cfg(feature = "automation")]
        Action::OpenFolder { .. }
        | Action::Os {
            operation: OsOperation::OpenFolder { .. },
            ..
        } => "打开目录失败",
        #[cfg(feature = "automation")]
        Action::CloseProgram { .. }
        | Action::App {
            operation: AppOperation::Close { .. },
            ..
        } => "关闭程序失败",
        #[cfg(feature = "automation")]
        Action::App {
            operation: AppOperation::Restart { .. },
            ..
        } => "重启程序失败",
        #[cfg(feature = "automation")]
        Action::App {
            operation: AppOperation::Status { .. },
            ..
        } => "查询程序失败",
        #[cfg(feature = "automation")]
        Action::App {
            operation: AppOperation::Maximize { .. },
            ..
        } => "最大化窗口失败",
        #[cfg(feature = "automation")]
        Action::App {
            operation: AppOperation::Minimize { .. },
            ..
        } => "最小化窗口失败",
        #[cfg(feature = "automation")]
        Action::App {
            operation: AppOperation::Activate { .. },
            ..
        } => "窗口置于前台失败",
        #[cfg(feature = "automation")]
        Action::Script { .. } => "脚本失败",
        #[cfg(feature = "automation")]
        Action::OpenUrl { .. } => "打开网址失败",
        #[cfg(feature = "automation")]
        Action::Os { .. } => "文件操作失败",
        #[cfg(feature = "automation")]
        Action::System { .. } => "系统操作失败",
        #[cfg(feature = "automation")]
        Action::If { .. } => "条件判断失败",
        #[cfg(feature = "automation")]
        Action::Parallel { .. } => "并行执行失败",
        #[cfg(feature = "automation")]
        Action::Window { .. } => "窗口操作失败",
    }
}

/// 失败记录是否弹窗：沿用动作自己的 `show_output`——用户给命令 / 脚本开了「显示输出」，
/// 那它连启动都没成功这件事同样该弹出来；其余动作静默记入消息中心 + 未读红点。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn action_show_output(action: &Action) -> bool {
    match action {
        #[cfg(feature = "automation")]
        Action::Command { show_output, .. }
        | Action::Cmd { show_output, .. }
        | Action::Powershell { show_output, .. }
        | Action::Script { show_output, .. } => *show_output,
        _ => false,
    }
}

/// 「命令」一栏的内容：用户写的动作说明优先（它才是用户认得的那句话），
/// 没有说明则渲染动作本体摘要。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn action_summary(action: &Action, vars: &Vars) -> String {
    let body = action_body(action, vars);
    match action_description(action)
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        Some(d) => format!("{d}（{body}）"),
        None => body,
    }
}

/// 动作本体摘要（变量已按本次触发的取值代入，展示的是「实际做了什么」）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn action_body(action: &Action, vars: &Vars) -> String {
    let sub = |s: &str| substitute_vars(s, vars);
    match action {
        Action::Text { text, mode, masked, .. } => match mode {
            // 标记了隐藏的文本连执行记录里也不露出（否则「消息」页一开就看到原文，
            // 界面上的隐藏等于白做）；代价是这条记录看不出注入了什么。
            TextMode::Input => {
                if *masked {
                    "输入文本 ***".into()
                } else {
                    format!("输入文本 {}", preview(&sub(text)))
                }
            }
            TextMode::ToUpper => "把选中/剪贴板文本转为大写".into(),
            TextMode::ToLower => "把选中/剪贴板文本转为小写".into(),
        },
        Action::Keys { keys, mode, .. } => match mode {
            KeysMode::Tap => format!("按下 {}", keys.join("+")),
            KeysMode::Press => format!("按住 {}", keys.join("+")),
            KeysMode::Release => format!("抬起 {}", keys.join("+")),
        },
        Action::PauseMs { ms, .. } => format!("暂停 {ms} 毫秒"),
        Action::Mouse { op, .. } => mouse_body(op),
        Action::Clipboard { operation, .. } => clipboard_body(operation, vars),
        Action::Group { actions, .. } => format!("分组 {} 个动作", actions.len()),
        #[cfg(feature = "automation")]
        Action::Command { command, .. }
        | Action::Cmd { command, .. }
        | Action::Powershell { command, .. } => preview(&sub(command)),
        #[cfg(feature = "automation")]
        Action::Launch { program, args, .. } => {
            format!("启动程序 {}", join_args(&sub(program), args, vars))
        }
        #[cfg(feature = "automation")]
        Action::OpenFolder { path, .. } => format!("打开目录 {}", sub(path)),
        #[cfg(feature = "automation")]
        Action::CloseProgram { program, .. } => format!("关闭程序 {}", sub(program)),
        #[cfg(feature = "automation")]
        Action::Os { operation, .. } => os_body(operation, vars),
        #[cfg(feature = "automation")]
        Action::System { operation, .. } => system_body(operation),
        #[cfg(feature = "automation")]
        Action::App { operation, .. } => app_body(operation, vars),
        #[cfg(feature = "automation")]
        Action::OpenUrl { url, .. } => format!("打开网址 {}", sub(url)),
        #[cfg(feature = "automation")]
        Action::Script {
            path, interpreter, ..
        } => match interpreter {
            Some(i) => format!("运行 {} {}", sub(i), sub(path)),
            None => format!("运行脚本 {}", sub(path)),
        },
        #[cfg(feature = "automation")]
        Action::If { .. } => "条件判断".into(),
        #[cfg(feature = "automation")]
        Action::Parallel { actions, .. } => format!("并行执行 {} 个动作", actions.len()),
        #[cfg(feature = "automation")]
        Action::Window { operation, .. } => window_body(operation),
    }
}

/// 剪贴板动作摘要（消息中心「命令」一栏）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn clipboard_body(op: &ClipboardOperation, vars: &Vars) -> String {
    match op {
        ClipboardOperation::SetClipboard { text } => {
            let t = substitute_vars(text, vars);
            if t.is_empty() {
                "清空剪贴板".into()
            } else {
                format!("写入剪贴板 {}", preview(&t))
            }
        }
        ClipboardOperation::GetClipboard { var } => {
            format!("读取剪贴板 → {}", var.trim())
        }
        ClipboardOperation::TempStr { text } => {
            format!("临时文本 {}", preview(&substitute_vars(text, vars)))
        }
    }
}

/// 当前窗口动作摘要（消息中心「命令」一栏）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn window_body(op: &WindowOperation) -> String {
    match op {
        WindowOperation::MaximizeRestore => "最大化 / 还原当前窗口".into(),
        WindowOperation::Minimize => "最小化当前窗口".into(),
        WindowOperation::ToggleTopmost => "切换当前窗口置顶".into(),
        WindowOperation::Center => "当前窗口居中".into(),
        WindowOperation::Close => "关闭当前窗口".into(),
        WindowOperation::CloseSameApp => "关闭当前窗口的同类窗口".into(),
    }
}

/// 鼠标动作摘要（消息中心「命令」一栏）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn mouse_body(op: &MouseOp) -> String {
    match op {
        MouseOp::Move { dx, dy } => format!("鼠标移动 ({dx}, {dy})"),
        MouseOp::MoveTo { x, y } => format!("鼠标移动到 ({x}, {y})"),
        MouseOp::MoveToOrigin => "鼠标移动到手势起点".into(),
        MouseOp::Click { button } => {
            let b = match button {
                MouseButton::Left => "左键",
                MouseButton::Right => "右键",
                MouseButton::Middle => "中键",
                MouseButton::Back => "侧键·后退",
                MouseButton::Forward => "侧键·前进",
            };
            format!("鼠标点击{b}")
        }
        MouseOp::Scroll { dx, dy } => format!("鼠标滚轮 ({dx}, {dy})"),
    }
}

#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn os_body(op: &OsOperation, vars: &Vars) -> String {
    let sub = |s: &str| substitute_vars(s, vars);
    match op {
        OsOperation::Copy { source, dest } => format!("复制 {} → {}", sub(source), sub(dest)),
        OsOperation::Cut { source, dest } => format!("剪切 {} → {}", sub(source), sub(dest)),
        OsOperation::Paste { dest } => format!("粘贴到 {}", sub(dest)),
        OsOperation::Delete { path } => format!("删除 {}", sub(path)),
        OsOperation::NewFile { path } => format!("新建文件 {}", sub(path)),
        OsOperation::NewFolder { path } => format!("新建目录 {}", sub(path)),
        OsOperation::OpenFolder { path } => format!("打开目录 {}", sub(path)),
        OsOperation::Zip { source, dest } => format!("压缩 {} → {}", sub(source), sub(dest)),
        OsOperation::Unzip { source, dest } => format!("解压 {} → {}", sub(source), sub(dest)),
        OsOperation::GetFileProps { path, var } => {
            format!("读取 {} 的属性写入变量 {var}", sub(path))
        }
    }
}

/// 系统操作摘要（不含变量：系统操作目前不读占位符）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn system_body(op: &SystemOperation) -> String {
    match op {
        SystemOperation::SwitchDesktop { direction } => {
            format!("切换到{}虚拟桌面", direction.label())
        }
    }
}

#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn app_body(op: &AppOperation, vars: &Vars) -> String {
    let sub = |s: &str| substitute_vars(s, vars);
    match op {
        AppOperation::Launch { program, args } => {
            format!("启动程序 {}", join_args(&sub(program), args, vars))
        }
        AppOperation::Close { program } => format!("关闭程序 {}", sub(program)),
        AppOperation::Maximize { program } => format!("最大化窗口 {}", sub(program)),
        AppOperation::Minimize { program } => format!("最小化窗口 {}", sub(program)),
        AppOperation::Activate { program } => format!("放置到前台 {}", sub(program)),
        AppOperation::Status { program, .. } => format!("查询程序 {} 是否在运行", sub(program)),
        AppOperation::Restart { program, args } => {
            format!("重启程序 {}", join_args(&sub(program), args, vars))
        }
    }
}

/// 程序名 + 参数渲染成一行（参数同样代入变量）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn join_args(program: &str, args: &[String], vars: &Vars) -> String {
    let args: Vec<String> = args.iter().map(|a| substitute_vars(a, vars)).collect();
    if args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {}", args.join(" "))
    }
}

/// 取动作上用户写的说明（`Action` 各变体都带可选 `description`）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn action_description(action: &Action) -> Option<&str> {
    match action {
        Action::Text { description, .. }
        | Action::Keys { description, .. }
        | Action::PauseMs { description, .. }
        | Action::Mouse { description, .. }
        | Action::Clipboard { description, .. } => description.as_deref(),
        // 分组没有 `description`：用户填的 `name` 就是这一步的标题。
        Action::Group { name, .. } => {
            let n = name.trim();
            if n.is_empty() {
                None
            } else {
                Some(n)
            }
        }
        #[cfg(feature = "automation")]
        Action::Command { description, .. }
        | Action::Cmd { description, .. }
        | Action::Powershell { description, .. }
        | Action::Launch { description, .. }
        | Action::OpenFolder { description, .. }
        | Action::CloseProgram { description, .. }
        | Action::Os { description, .. }
        | Action::System { description, .. }
        | Action::App { description, .. }
        | Action::OpenUrl { description, .. }
        | Action::Script { description, .. }
        | Action::If { description, .. }
        | Action::Parallel { description, .. }
        | Action::Window { description, .. } => description.as_deref(),
    }
}

/// 摘要里字段长度的上限：这一栏是给用户认「哪一步」的短上下文，
/// 整段替换文本 / 长命令原样铺开会把消息卡片撑得没法看。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
const PREVIEW_MAX_CHARS: usize = 160;

/// 压成单行并截断（换行在卡片里是多行，摘要只需要一眼能认出来）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn preview(s: &str) -> String {
    let one_line: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    match one_line.char_indices().nth(PREVIEW_MAX_CHARS) {
        Some((idx, _)) => format!("{}…", &one_line[..idx]),
        None => one_line,
    }
}

/// 顺序执行一个动作。文本/按键走模拟输入，进程类走 std::process，文件类走 std::fs。
/// 命令类动作（CMD/PowerShell/关闭程序）捕获输出并返回 `Some(CommandResult)`，其余返回 `None`。
/// `vars` 承载本次触发内的变量（`GetFileProps` 写 File、`App::Status` 写 Bool、
/// 命令动作 `var` 非空时写 Text），供后续动作用占位符引用；
/// `last_copied` 记录最近一次复制/剪切的来源，供「粘贴」动作使用。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[cfg_attr(not(feature = "automation"), allow(unused_variables))]
fn run_action(
    action: &Action,
    ctx: &Ctx,
    vars: &mut Vars,
    last_copied: &mut Option<String>,
    scope: u64,
) -> Result<Option<CommandResult>, String> {
    match action {
        Action::Group { .. } => {
            // 分组动作由 run_chain 递归处理（成员按顺序执行），不会到达这里。
            return Err("内部错误：分组动作应在运行器内处理".into());
        }
        Action::Text { text, mode, .. } => {
            // 持链内注入锁：剪贴板 set → 粘贴 → 还原必须原子，否则并行的另一成员会覆盖剪贴板
            // （或读到别人刚写进去的内容）。转大小写路径同理（它内部再调 `inject_text`，
            // 是同一把锁的一次持有，不再取第二次，故无重入死锁）。
            let _inj = lock_inject(ctx);
            match mode {
                TextMode::Input => {
                    let text = substitute_vars(text, vars);
                    inject_text(&text, ctx.text_mode)?;
                }
                TextMode::ToUpper | TextMode::ToLower => {
                    transform_case(matches!(mode, TextMode::ToUpper), ctx.text_mode)?;
                }
            }
        }
        Action::Keys { keys, mode, .. } => {
            // 持链内注入锁：down…up 必须成对，不能被别的成员插进来（插进来目标程序收到的是
            // 一个混合组合键）。
            let _inj = lock_inject(ctx);
            let ks: Vec<Key> = keys
                .iter()
                .map(|k| k.parse::<Key>().map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            match mode {
                KeysMode::Tap => {
                    // 全部按下 → 稍停 → 逆序松开
                    for k in &ks {
                        simulate::down(*k);
                    }
                    std::thread::sleep(Duration::from_millis(30));
                    for k in ks.iter().rev() {
                        simulate::up(*k);
                    }
                }
                KeysMode::Press => {
                    for k in &ks {
                        hold_key(scope, *k);
                    }
                }
                KeysMode::Release => {
                    for k in ks.iter().rev() {
                        unhold_key(scope, *k);
                    }
                }
            }
        }
        Action::Mouse { op, .. } => {
            // 持链内注入锁：点击是「按下 + 抬起」一对，不能被别的成员插进来。
            let _inj = lock_inject(ctx);
            // 「移动到手势起点」在动作层解析：平台注入只认具体坐标，提权协议也不用带 origin。
            let resolved;
            let op = if matches!(op, MouseOp::MoveToOrigin) {
                let (x, y) = ctx.t.origin.ok_or_else(|| {
                    "「移动到手势起点」只适用于鼠标手势触发；快捷键 / 录制等路径没有起点".to_string()
                })?;
                resolved = MouseOp::MoveTo { x, y };
                &resolved
            } else {
                op
            };
            simulate::mouse(op).map_err(|e| e.to_string())?;
        }
        Action::Clipboard { operation, .. } => match operation {
            ClipboardOperation::SetClipboard { text } => {
                let text = substitute_vars(text, vars);
                simulate::set_clipboard_text(&text)
                    .map_err(|e| format!("写入剪贴板失败：{e}"))?;
            }
            ClipboardOperation::GetClipboard { var } => {
                let var = var.trim();
                if var.is_empty() {
                    return Err("变量名不能为空".into());
                }
                let text = simulate::get_clipboard_text()
                    .map_err(|e| format!("读取剪贴板失败：{e}"))?;
                vars.insert(var.to_string(), Value::Text(TextValue { text, exit_code: None }));
            }
            ClipboardOperation::TempStr { text } => {
                // 持链内注入锁：写剪贴板 → 粘贴 → 恢复必须原子，否则并行的另一成员会
                // 覆盖剪贴板（或读到刚写进去的内容）。
                let _inj = lock_inject(ctx);
                let text = substitute_vars(text, vars);
                simulate::type_text(&text)
                    .map_err(|e| format!("临时文本粘贴失败：{e}"))?;
            }
        },
        // 长等待也要能被中止：一次 `PauseMs` 可以是几十秒，睡死在里面的话「停止」要等到
        // 它自然睡醒才生效（用户看到的是「点了停止还在继续」）。切成小片睡，每片查一次。
        Action::PauseMs { ms, .. } => {
            let deadline = Instant::now() + Duration::from_millis(*ms);
            while Instant::now() < deadline {
                if abort::aborted(ctx.gen) {
                    return Ok(None);
                }
                let left = deadline.saturating_duration_since(Instant::now());
                std::thread::sleep(left.min(Duration::from_millis(50)));
            }
        }
        #[cfg(feature = "automation")]
        Action::Command {
            shell,
            command,
            show_output,
            hide_window,
            var,
            ..
        } => {
            let (label, kind, is_powershell) = match shell {
                Shell::Cmd => ("CMD", "cmd", false),
                Shell::Powershell => ("PowerShell", "powershell", true),
            };
            if is_powershell && !cfg!(target_os = "windows") {
                return Err("PowerShell 命令仅 Windows 可用".into());
            }
            let command = substitute_vars(command, vars);
            let out = run_cmd(label, is_powershell, &command, ctx, *hide_window)?;
            if !var.trim().is_empty() {
                vars.insert(
                    var.trim().to_string(),
                    Value::Text(TextValue {
                        text: out.stdout.trim().to_string(),
                        exit_code: out.exit_code,
                    }),
                );
            }
            return Ok(Some(out.into_result(
                kind,
                label,
                command,
                ctx,
                *show_output,
            )));
        }
        #[cfg(feature = "automation")]
        Action::Cmd {
            command,
            show_output,
            var,
            ..
        } => {
            let command = substitute_vars(command, vars);
            // 旧版动作没有 hide_window 字段（加载时会迁移成 Command，见 Config::migrate）。
            let out = run_cmd("CMD", false, &command, ctx, false)?;
            if !var.trim().is_empty() {
                vars.insert(
                    var.trim().to_string(),
                    Value::Text(TextValue {
                        text: out.stdout.trim().to_string(),
                        exit_code: out.exit_code,
                    }),
                );
            }
            return Ok(Some(out.into_result(
                "cmd",
                "CMD",
                command,
                ctx,
                *show_output,
            )));
        }
        #[cfg(feature = "automation")]
        Action::Powershell {
            command,
            show_output,
            var,
            ..
        } => {
            if !cfg!(target_os = "windows") {
                return Err("PowerShell 动作仅 Windows 可用".into());
            }
            let command = substitute_vars(command, vars);
            // 同上：旧版动作无 hide_window。
            let out = run_cmd("PowerShell", true, &command, ctx, false)?;
            if !var.trim().is_empty() {
                vars.insert(
                    var.trim().to_string(),
                    Value::Text(TextValue {
                        text: out.stdout.trim().to_string(),
                        exit_code: out.exit_code,
                    }),
                );
            }
            return Ok(Some(out.into_result(
                "powershell",
                "PowerShell",
                command,
                ctx,
                *show_output,
            )));
        }
        #[cfg(feature = "automation")]
        Action::Launch { program, args, .. } => {
            // 旧版动作（正常流程已在加载时迁移到 App，此处兜底）。
            let program = substitute_vars(program, vars);
            let args: Vec<String> = args.iter().map(|a| substitute_vars(a, vars)).collect();
            launch_program(&program, &args)?;
        }
        #[cfg(feature = "automation")]
        Action::CloseProgram { program, .. } => {
            // 旧版动作（正常流程已在加载时迁移到 App，此处兜底）。
            let program = substitute_vars(program, vars);
            return Ok(Some(close_program(&program, ctx)?));
        }
        #[cfg(feature = "automation")]
        Action::OpenFolder { path, .. } => {
            // 旧版动作（正常流程已在加载时迁移到 Os::OpenFolder，此处兜底）。
            run_os(
                &OsOperation::OpenFolder { path: path.clone() },
                vars,
                last_copied,
            )?;
        }
        #[cfg(feature = "automation")]
        Action::Os { operation, .. } => {
            run_os(operation, vars, last_copied)?;
        }
        #[cfg(feature = "automation")]
        Action::System { operation, .. } => {
            run_system(operation, ctx)?;
        }
        #[cfg(feature = "automation")]
        Action::App { operation, .. } => {
            return run_app(operation, ctx, vars);
        }
        #[cfg(feature = "automation")]
        Action::OpenUrl { url, .. } => {
            let url = substitute_vars(url, vars);
            open_url(&url)?;
        }
        #[cfg(feature = "automation")]
        Action::If { .. } => {
            // 条件判断动作由 run_chain 拦截处理，不会到达这里。
            return Err("内部错误：条件判断动作应在运行器内处理".into());
        }
        #[cfg(feature = "automation")]
        Action::Parallel { .. } => {
            // 并行分组动作由 run_chain 拦截处理，不会到达这里。
            return Err("内部错误：并行执行动作应在运行器内处理".into());
        }
        #[cfg(feature = "automation")]
        Action::Window { operation, .. } => {
            // 目标窗口是**触发时**抓的句柄（`ctx.t.window`），所以链里先切前台也不影响它。
            kada_hook::window_action(ctx.t.window, hook_window_action(*operation))?;
        }
        #[cfg(feature = "automation")]
        Action::Script {
            path,
            interpreter,
            show_output,
            hide_window,
            var,
            ..
        } => {
            let path = substitute_vars(path, vars);
            let out = run_script(interpreter.as_deref(), &path, ctx, vars, *hide_window)?;
            if !var.trim().is_empty() {
                vars.insert(
                    var.trim().to_string(),
                    Value::Text(TextValue {
                        text: out.stdout.trim().to_string(),
                        exit_code: out.exit_code,
                    }),
                );
            }
            let command = match interpreter {
                Some(i) => format!("{i} \"{path}\""),
                None => path.clone(),
            };
            return Ok(Some(out.into_result(
                "script",
                "脚本",
                command,
                ctx,
                *show_output,
            )));
        }
    }
    Ok(None)
}

/// 核心模型的操作 → 钩子层的窗口操作（两份枚举各管一层：core 管配置形状，hook 管实现）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn hook_window_action(op: WindowOperation) -> kada_hook::WindowAction {
    use kada_hook::WindowAction as H;
    match op {
        WindowOperation::MaximizeRestore => H::MaximizeRestore,
        WindowOperation::Minimize => H::Minimize,
        WindowOperation::ToggleTopmost => H::ToggleTopmost,
        WindowOperation::Center => H::Center,
        WindowOperation::Close => H::Close,
        WindowOperation::CloseSameApp => H::CloseSameApp,
    }
}

/// 执行一个文件/目录动作（复制/剪切/粘贴/删除/新建/压缩/取属性）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn run_os(
    op: &OsOperation,
    vars: &mut Vars,
    last_copied: &mut Option<String>,
) -> Result<(), String> {
    match op {
        OsOperation::Copy { source, dest } => {
            let s = substitute_vars(source, vars);
            let d = substitute_vars(dest, vars);
            copy_path(&s, &d)?;
            *last_copied = Some(s);
        }
        OsOperation::Cut { source, dest } => {
            let s = substitute_vars(source, vars);
            let d = substitute_vars(dest, vars);
            move_path(&s, &d)?;
            *last_copied = Some(d);
        }
        OsOperation::Paste { dest } => {
            let d = substitute_vars(dest, vars);
            let s = last_copied
                .clone()
                .ok_or_else(|| "粘贴失败：尚无复制/剪切的来源".to_string())?;
            copy_path(&s, &d)?;
        }
        OsOperation::Delete { path } => {
            let p = substitute_vars(path, vars);
            delete_path(&p)?;
        }
        OsOperation::NewFile { path } => {
            let p = substitute_vars(path, vars);
            new_file(&p)?;
        }
        OsOperation::NewFolder { path } => {
            let p = substitute_vars(path, vars);
            std::fs::create_dir_all(&p).map_err(|e| format!("新建目录失败: {e}"))?;
        }
        OsOperation::OpenFolder { path } => {
            let p = substitute_vars(path, vars);
            if cfg!(target_os = "windows") {
                std::process::Command::new("explorer")
                    .arg(&p)
                    .spawn()
                    .map_err(|e| format!("打开目录失败: {e}"))?;
            } else if cfg!(target_os = "macos") {
                // macOS 的 `open` 一个命令同时管「打开目录」与「打开网址」（见 open_url）。
                std::process::Command::new("open")
                    .arg(&p)
                    .spawn()
                    .map_err(|e| format!("打开目录失败: {e}"))?;
            } else {
                std::process::Command::new("xdg-open")
                    .arg(&p)
                    .spawn()
                    .map_err(|e| format!("打开目录失败: {e}"))?;
            }
        }
        OsOperation::Zip { source, dest } => {
            let s = substitute_vars(source, vars);
            let d = substitute_vars(dest, vars);
            zip_path(&s, &d)?;
        }
        OsOperation::Unzip { source, dest } => {
            let s = substitute_vars(source, vars);
            let d = substitute_vars(dest, vars);
            unzip_path(&s, &d)?;
        }
        OsOperation::GetFileProps { path, var } => {
            let p = substitute_vars(path, vars);
            let obj = file_object(&p)?;
            let name = if var.trim().is_empty() {
                "file".to_string()
            } else {
                var.clone()
            };
            vars.insert(name, Value::File(obj));
        }
    }
    Ok(())
}

/// 执行一个系统操作（[`Action::System`]）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn run_system(op: &SystemOperation, ctx: &Ctx<'_>) -> Result<(), String> {
    match op {
        SystemOperation::SwitchDesktop { direction } => {
            // 持链内注入锁：`Win+Ctrl+←` 这种组合被别的并行成员插进半截，目标系统收到的是
            // 一个混合组合键（切桌面就成了别的东西）。与 `Keys` 同款理由。
            let _inj = lock_inject(ctx);
            switch_desktop(*direction)
        }
    }
}

/// 切换虚拟桌面：注入**系统默认**的切换快捷键（等价于用户手按一遍）。
///
/// 为什么不调系统 API：三平台都没有稳定的公开接口——Windows 的虚拟桌面是未公开的 COM
/// （`IVirtualDesktopManagerInternal`，接口随系统版本变、每版都要重新逆向），Linux 由各桌面
/// 环境自己实现（X11 `_NET_CURRENT_DESKTOP` / Wayland 无通用协议），macOS 的 Space 切换只有
/// 私有 SkyLight 接口。注入默认键位是三平台唯一一致且不挑系统版本的做法，代价是**跟随各平台
/// 的默认键位**——用户若在系统里改过快捷键，得同样改这里（Linux 的 KDE / XFCE 默认键位与
/// GNOME 不同，见下）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn switch_desktop(direction: DesktopDirection) -> Result<(), String> {
    simulate::chord(desktop_keys(direction));
    Ok(())
}

/// 「切换虚拟桌面」在某平台上要注入的键（纯函数，好让单测钉住这张平台映射表）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn desktop_keys(direction: DesktopDirection) -> &'static [Key] {
    use DesktopDirection::*;
    // Windows：`Win+Ctrl+←/→`（Win10 / Win11 默认，Meta 在 Windows 侧映射到 VK_LWIN）。
    // macOS：Mission Control 的「向左/右移动一个空间」默认 `⌃←/→`（系统设置里可改）。
    // Linux：GNOME 默认 `Super+PageUp/PageDown`；KDE / XFCE 默认键位不同，届时用「按键组合」动作自排。
    if cfg!(target_os = "windows") {
        match direction {
            Prev => &[Key::Meta, Key::Control, Key::ArrowLeft],
            Next => &[Key::Meta, Key::Control, Key::ArrowRight],
        }
    } else if cfg!(target_os = "macos") {
        match direction {
            Prev => &[Key::Control, Key::ArrowLeft],
            Next => &[Key::Control, Key::ArrowRight],
        }
    } else {
        match direction {
            Prev => &[Key::Meta, Key::PageUp],
            Next => &[Key::Meta, Key::PageDown],
        }
    }
}

/// 按配置的注入方式发文本（规划 7.3-㉒）：默认剪贴板粘贴；目标程序吞粘贴
/// （游戏 / 终端）时用「逐字直发」绕过剪贴板。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn inject_text(text: &str, mode: TextInjectMode) -> Result<(), String> {
    match mode {
        TextInjectMode::Clipboard => simulate::type_text(text).map_err(|e| e.to_string()),
        TextInjectMode::Unicode => simulate::type_text_unicode(text).map_err(|e| e.to_string()),
    }
}

/// 把当前选中/剪贴板文本转为大写或小写后粘贴回原处：
/// 复制选中（平台组合键）→ 读剪贴板 → 转换 → 粘贴（注入方式同文本动作，见 [`inject_text`]）。
/// 无选中时复制通常不改剪贴板，即退化为「转换剪贴板文本」。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn transform_case(upper: bool, text_mode: TextInjectMode) -> Result<(), String> {
    copy_chord();
    std::thread::sleep(Duration::from_millis(80));
    let clip = simulate::get_clipboard_text().map_err(|e| e.to_string())?;
    let out = if upper {
        clip.to_uppercase()
    } else {
        clip.to_lowercase()
    };
    inject_text(&out, text_mode)?;
    Ok(())
}

/// 「复制选中内容」的组合键：macOS 是 ⌘C，Windows / Linux 是 Ctrl+C。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn copy_chord() {
    if cfg!(target_os = "macos") {
        simulate::chord(&[Key::Meta, Key::C]);
    } else {
        simulate::chord(&[Key::Control, Key::C]);
    }
}

/// 执行一个应用动作（打开/关闭/查询状态/重启/窗口控制）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn run_app(op: &AppOperation, ctx: &Ctx, vars: &mut Vars) -> Result<Option<CommandResult>, String> {
    match op {
        AppOperation::Launch { program, args } => {
            let program = substitute_vars(program, vars);
            let args: Vec<String> = args.iter().map(|a| substitute_vars(a, vars)).collect();
            launch_program(&program, &args)?;
            Ok(None)
        }
        AppOperation::Close { program } => {
            let program = substitute_vars(program, vars);
            Ok(Some(close_program(&program, ctx)?))
        }
        AppOperation::Maximize { program } => {
            let program = substitute_vars(program, vars);
            kada_hook::control_window(&program, kada_hook::WindowOp::Maximize)?;
            Ok(None)
        }
        AppOperation::Minimize { program } => {
            let program = substitute_vars(program, vars);
            kada_hook::control_window(&program, kada_hook::WindowOp::Minimize)?;
            Ok(None)
        }
        AppOperation::Activate { program } => {
            let program = substitute_vars(program, vars);
            kada_hook::control_window(&program, kada_hook::WindowOp::Activate)?;
            Ok(None)
        }
        AppOperation::Status {
            program,
            var,
            retries,
            interval_ms,
        } => {
            let program = substitute_vars(program, vars);
            let image = image_name(&program);
            // 未运行则按 retries/interval_ms 轮询，等程序起来（如「启动后等它就绪」）。
            let mut running = app_running(&image);
            for _ in 0..*retries {
                if running || abort::aborted(ctx.gen) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(*interval_ms));
                running = app_running(&image);
            }
            let name = if var.trim().is_empty() {
                "app".to_string()
            } else {
                var.clone()
            };
            vars.insert(name, Value::Bool(running));
            Ok(None)
        }
        AppOperation::Restart { program, args } => {
            let program = substitute_vars(program, vars);
            let args: Vec<String> = args.iter().map(|a| substitute_vars(a, vars)).collect();
            restart_program(&program, &args, ctx)?;
            Ok(None)
        }
    }
}

/// 启动程序（可选参数）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn launch_program(program: &str, args: &[String]) -> Result<(), String> {
    std::process::Command::new(program)
        .args(args)
        .spawn()
        .map_err(|e| format!("启动程序「{program}」失败: {e}"))?;
    Ok(())
}

/// 用系统默认浏览器打开网址。Windows 走 `explorer`（直接以参数传入，不经 shell，
/// 避免 URL 里的 `&`/`?` 等被 cmd 二次解析）；macOS 走 `open`；Linux 走 `xdg-open`。
///
/// `pub` 且不随 `automation` 门控：桌面壳拦到界面外链（`<a target="_blank">`）时也走这里，
/// 基础版同样要能打开外链——平台分叉只留这一份。
pub fn open_url(url: &str) -> Result<(), String> {
    let res = if cfg!(target_os = "windows") {
        std::process::Command::new("explorer").arg(url).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    res.map(|_| ())
        .map_err(|e| format!("打开网址「{url}」失败: {e}"))
}

/// 关闭程序（结束所有同名进程），返回命令结果供消息中心展示。
///
/// Windows 走 `taskkill`；macOS / Linux 都走 `pkill`（两边都自带这个命令）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn close_program(program: &str, ctx: &Ctx) -> Result<CommandResult, String> {
    let image = image_name(program);
    let cmd = if cfg!(target_os = "windows") {
        format!("taskkill /IM {image} /F /T")
    } else {
        format!("pkill -f {image}")
    };
    // 内部命令（taskkill / pkill），恒不弹黑窗。
    let out = run_cmd("关闭程序", false, &cmd, ctx, true)?;
    Ok(out.into_result("close_program", "关闭程序", cmd, ctx, false))
}

/// 重启程序：先关闭（忽略「未在运行」），再启动。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn restart_program(program: &str, args: &[String], ctx: &Ctx) -> Result<(), String> {
    let _ = close_program(program, ctx);
    launch_program(program, args)?;
    Ok(())
}

/// 判断程序是否正在运行。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn app_running(program: &str) -> bool {
    if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("tasklist");
        c.args(["/FI", &format!("IMAGENAME eq {program}"), "/NH"]);
        no_console_window(&mut c); // 内部查询，恒不弹黑窗
        match c.output() {
            Ok(o) => String::from_utf8_lossy(&o.stdout)
                .to_lowercase()
                .contains(&program.to_lowercase()),
            Err(_) => false,
        }
    } else {
        std::process::Command::new("pgrep")
            .args(["-x", program])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// 取程序名（镜像名）：`program` 为带路径形式时取最后一段文件名，否则原样返回。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn image_name(program: &str) -> String {
    Path::new(program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_string())
}

/// 复制文件或目录（目录递归）。`dest` 为已存在目录时复制到其下，否则视为完整目标路径。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn copy_path(source: &str, dest: &str) -> Result<(), String> {
    let src = Path::new(source);
    let dst = Path::new(dest);
    if !src.exists() {
        return Err(format!("复制失败：源「{source}」不存在"));
    }
    let target = if dst.is_dir() {
        dst.join(src.file_name().ok_or("源路径无效")?)
    } else {
        dst.to_path_buf()
    };
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if src.is_dir() {
        copy_dir_rec(src, &target).map_err(|e| format!("复制目录失败: {e}"))
    } else {
        fs::copy(src, &target).map_err(|e| format!("复制文件失败: {e}"))?;
        Ok(())
    }
}

#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn copy_dir_rec(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_rec(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 移动文件或目录（同盘 `rename`；跨盘回退为复制后删除源）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn move_path(source: &str, dest: &str) -> Result<(), String> {
    let src = Path::new(source);
    if !src.exists() {
        return Err(format!("剪切失败：源「{source}」不存在"));
    }
    let dst = Path::new(dest);
    let target = if dst.is_dir() {
        dst.join(src.file_name().ok_or("源路径无效")?)
    } else {
        dst.to_path_buf()
    };
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::rename(src, &target).is_ok() {
        return Ok(());
    }
    // 跨盘 rename 失败：复制后删除源。
    copy_path(source, dest)?;
    delete_path(source)
}

/// 删除文件或目录（目录递归删除）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn delete_path(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("删除失败：「{path}」不存在"));
    }
    if p.is_dir() {
        fs::remove_dir_all(p).map_err(|e| format!("删除目录失败: {e}"))
    } else {
        fs::remove_file(p).map_err(|e| format!("删除文件失败: {e}"))
    }
}

/// 新建空文件（自动创建父目录；已存在则截断为空）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn new_file(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建父目录失败: {e}"))?;
    }
    fs::write(p, b"").map_err(|e| format!("新建文件失败: {e}"))
}

/// 压缩为 zip：Windows 走 `Compress-Archive`，Linux 走 `zip`。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn zip_path(source: &str, dest: &str) -> Result<(), String> {
    let src = Path::new(source);
    if !src.exists() {
        return Err(format!("压缩失败：源「{source}」不存在"));
    }
    let dest = if dest.to_lowercase().ends_with(".zip") {
        dest.to_string()
    } else {
        format!("{dest}.zip")
    };
    if let Some(parent) = Path::new(&dest).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    }
    let status = if cfg!(target_os = "windows") {
        let full = format!(
            "Compress-Archive -Path '{}' -DestinationPath '{}' -Force",
            source, dest
        );
        let mut c = std::process::Command::new("powershell");
        c.args(["-NoProfile", "-Command", full.as_str()]);
        no_console_window(&mut c); // 内部压缩，恒不弹黑窗
        c.status()
    } else {
        std::process::Command::new("zip")
            .args(["-r", &dest, source])
            .status()
    }
    .map_err(|e| format!("启动压缩命令失败: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("压缩失败（源不存在或系统缺少压缩组件）".into())
    }
}

/// 解压 zip：Windows 走 `Expand-Archive`，Linux 走 `unzip`。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn unzip_path(source: &str, dest: &str) -> Result<(), String> {
    let src = Path::new(source);
    if !src.exists() {
        return Err(format!("解压失败：压缩包「{source}」不存在"));
    }
    fs::create_dir_all(dest).map_err(|e| format!("创建解压目录失败: {e}"))?;
    let status = if cfg!(target_os = "windows") {
        let full = format!(
            "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
            source, dest
        );
        let mut c = std::process::Command::new("powershell");
        c.args(["-NoProfile", "-Command", full.as_str()]);
        no_console_window(&mut c); // 内部解压，恒不弹黑窗
        c.status()
    } else {
        std::process::Command::new("unzip")
            .args(["-o", source, "-d", dest])
            .status()
    }
    .map_err(|e| format!("启动解压命令失败: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("解压失败（压缩包损坏或系统缺少解压组件）".into())
    }
}

/// 读取文件/目录属性，构造 [`FileObject`]。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn file_object(path: &str) -> Result<FileObject, String> {
    let p = Path::new(path);
    let meta = fs::metadata(p).map_err(|e| format!("读取属性失败：{e}"))?;
    let is_dir = meta.is_dir();
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = p
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let size = if is_dir { 0 } else { meta.len() };
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(FileObject {
        name,
        path: path.to_string(),
        dir,
        stem,
        ext,
        size,
        modified,
        is_dir,
    })
}

/// 命令 / 脚本的执行结果与「被强制终止」的原因。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
struct CmdOutput {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    /// `Some(说明)` = 因超时 / 用户中止被强制终止（退出码不再是进程自己的语义，置 None）。
    killed: Option<String>,
}

#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
impl CmdOutput {
    /// 组装进消息中心的命令结果；被强制终止时把原因写进 stderr（用户唯一能看到的地方）。
    fn into_result(
        self,
        kind: &str,
        label: &str,
        command: String,
        ctx: &Ctx,
        show_output: bool,
    ) -> CommandResult {
        let stderr = match &self.killed {
            Some(reason) => {
                let note = format!("〔咔哒〕{reason}，已强制终止该进程及其子进程。");
                if self.stderr.is_empty() {
                    note
                } else {
                    format!("{note}\n{}", self.stderr)
                }
            }
            None => self.stderr,
        };
        CommandResult {
            kind: kind.into(),
            label: label.into(),
            command,
            trigger: ctx.t.trigger.into(),
            name: ctx.t.name.into(),
            stdout: self.stdout,
            stderr,
            exit_code: self.exit_code,
            show_output,
            time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            run_id: ctx.run_id,
        }
    }
}

/// 等待子进程结束时轮询的间隔：既是中止 / 超时的判定粒度，也是 CPU 占用的上界
/// （20ms 一次 `try_wait` 基本无感，用户点「停止」最多 20ms 后进程开始被杀）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
const WAIT_POLL_MS: u64 = 20;

/// 让子进程**不弹控制台窗口**（Windows 专有）。
///
/// 咔哒是 GUI 子系统程序、自己没有控制台（`windows_subsystem = "windows"`），于是每次
/// spawn `cmd` / `powershell` / `tasklist` 这类控制台程序，系统都会**新建一个控制台窗口**——
/// 用户按一次快捷键就闪一下黑窗。`CREATE_NO_WINDOW` 让它干脆不要控制台（输出照旧走我们接管
/// 的管道，捕获结果不受影响）。用户命令 / 脚本按 [`Action::Command::hide_window`] 开关决定，
/// 咔哒自己的内部 helper（taskkill / tasklist / 压缩解压）一律隐藏——那些黑窗没人要看。
///
/// 非 Windows 无此概念（`sh -c` 不会开终端窗口），空实现。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn no_console_window(cmd: &mut std::process::Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        /// `CREATE_NO_WINDOW`：不给子进程建控制台（缺省行为是「GUI 父进程 → 新建黑窗」）。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = cmd;
}

/// 跑一个子进程并**限时**等它结束（超时或用户中止则连同子进程树一起杀掉），捕获 UTF-8 输出。
///
/// 不用 `Command::output()`：它没有「等多久」，脚本挂住就永久占住本条触发的执行线程
/// （后续动作永不执行、线程泄漏）。这里改为手动收尾：先把两个管道交给读线程，再轮询
/// `try_wait` 判超时 / 中止。
///
/// 管道必须边跑边读：子进程输出超过管道缓冲（Windows 约 4~64KB）而没人读时，它写阻塞、
/// 我们等它退出——双方互等，超时判定也就永远等不到（`output()` 内部正是用读线程避开了
/// 这一点，所以这里也不能省）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn run_with_limits(
    cmd: &mut std::process::Command,
    label: &str,
    ctx: &Ctx,
) -> Result<CmdOutput, String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    make_process_group(cmd);
    let mut child = cmd.spawn().map_err(|e| format!("执行 {label} 失败: {e}"))?;

    let stdout = child.stdout.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            buf
        })
    });
    let stderr = child.stderr.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            buf
        })
    });

    let deadline = if ctx.timeout.is_zero() {
        None
    } else {
        Some(Instant::now() + ctx.timeout)
    };
    let killed = loop {
        if abort::aborted(ctx.gen) {
            break Some("已中止".to_string());
        }
        if let Some(d) = deadline {
            if Instant::now() >= d {
                break Some(format!("执行超时（超过 {} 秒）", fmt_secs(ctx.timeout)));
            }
        }
        match child.try_wait() {
            Ok(Some(_)) => break None,
            Ok(None) => std::thread::sleep(Duration::from_millis(WAIT_POLL_MS)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("等待 {label} 结束失败: {e}"));
            }
        }
    };

    if killed.is_some() {
        kill_tree(&mut child);
    }
    let status = child
        .wait()
        .map_err(|e| format!("回收 {label} 进程失败: {e}"))?;
    // 读线程到这里必然已经结束：进程退出 / 被杀 → 管道写端全部关闭 → read_to_end 返回。
    let join = |h: Option<std::thread::JoinHandle<Vec<u8>>>| {
        h.map(|h| h.join().unwrap_or_default()).unwrap_or_default()
    };
    let stdout = String::from_utf8_lossy(&join(stdout)).into_owned();
    let stderr = String::from_utf8_lossy(&join(stderr)).into_owned();
    let exit_code = if killed.is_some() {
        None
    } else {
        status.code()
    };
    Ok(CmdOutput {
        stdout,
        stderr,
        exit_code,
        killed,
    })
}

/// 把超时时长渲染成人话（`90 秒` / `1.5 秒` / `200 毫秒`）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn fmt_secs(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms} 毫秒")
    } else if ms.is_multiple_of(1000) {
        format!("{}", ms / 1000)
    } else {
        format!("{:.1}", ms as f64 / 1000.0)
    }
}

/// 强制终止子进程**及其子进程**。
///
/// `Child::kill` 只杀直接子进程，而 Windows 上我们跑的是 `cmd /C <临时批处理>` /
/// `powershell -Command`、Linux 上是 `sh -c`：真正干活的是它们的孙进程（脚本、被启动的
/// 程序）。只杀直接子进程 = 命令“看起来”结束了但脚本还在跑，而且它占着 stdout 管道，
/// 连读线程都不会结束。`taskkill /T` 才杀得掉整棵树。
#[cfg(all(target_os = "windows", feature = "automation"))]
fn kill_tree(child: &mut Child) {
    let pid = child.id();
    let mut c = std::process::Command::new("taskkill");
    c.args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    no_console_window(&mut c); // 内部杀进程，恒不弹黑窗
    let ok = c
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        // taskkill 也可能失败（进程刚好自己退了）：退回只杀直接子进程，聊胜于无。
        let _ = child.kill();
    }
}

/// unix（Linux / macOS）：让子进程**自成进程组**（`setpgid(0,0)`）。
///
/// 没有这一句，子进程与我们同组，「杀整棵树」就无从下手——`sh -c` 的孙进程既不是
/// 我们的直接子进程，也分不出来哪些是它的。自成一组后 [`kill_tree`] 打的是
/// `kill(-pgid)` = 整组一起收。Windows 不需要：`taskkill /T` 自己遍历进程树。
#[cfg(all(unix, feature = "automation"))]
fn make_process_group(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(all(not(unix), feature = "automation"))]
fn make_process_group(_cmd: &mut std::process::Command) {}

/// 强制终止子进程**及其子进程**（Linux / macOS）。
///
/// `Child::kill` 只发 SIGKILL 给直接子进程，而我们跑的是 `sh -c <命令>`：真正干活的是
/// 它的孙进程（脚本本体、被启动的程序）。只杀直接子进程 = 命令「看起来」结束了但脚本
/// 还在跑，而且它占着 stdout 管道，连读线程都不会结束。子进程在 spawn 时已自成进程组
/// （见 [`make_process_group`]），这里 `kill(-pgid)` 一次打整组；失败（组已不存在 /
/// 刚好都退了）退回只杀直接子进程，聊胜于无。
///
/// 天花板：孙进程若自己 `setsid` 脱离（服务化 / nohup 守护），组里找不到它，不追。
#[cfg(all(unix, feature = "automation"))]
fn kill_tree(child: &mut Child) {
    let pgid = child.id() as i32;
    // SAFETY: 只发一个信号；pgid 是本进程刚 spawn 的子进程组 id（`process_group(0)` 定的），
    // 负号 = 打整组——绝不能是 0，0 会打到我们自己所在的组。
    if unsafe { libc::kill(-pgid, libc::SIGKILL) } != 0 {
        let _ = child.kill();
    }
}

/// 执行 shell 命令并捕获 UTF-8 输出（限时、可中止，见 [`run_with_limits`]）。
/// Windows 下 cmd 前缀 `chcp 65001`、PowerShell 前缀设置输出编码，保证中文不乱码；
/// macOS / Linux 下 CMD 走 `sh -c`（两边都有 `/bin/sh`）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn run_cmd(
    label: &str,
    is_powershell: bool,
    command: &str,
    ctx: &Ctx,
    hide_window: bool,
) -> Result<CmdOutput, String> {
    // Windows 的 CMD 走临时批处理文件（见下），跑完要删掉——被强杀时也得删。
    let mut temp_bat: Option<std::path::PathBuf> = None;
    let mut cmd = if cfg!(target_os = "windows") {
        if is_powershell {
            let full = format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8; {command}");
            let mut c = std::process::Command::new("powershell");
            c.args(["-NoProfile", "-Command", full.as_str()]);
            c
        } else {
            // Windows：命令可能含嵌套引号（如 `start "" /min cmd /c "…"`），直接 `cmd /C "…"`
            // 会被 Rust 的 argv 转义成 `\"`，而 cmd 不认反斜杠转义，导致「xxx 不是内部或外部命令」。
            // 改为写入临时批处理文件再执行，彻底绕开引号二次解析；chcp 单独一行保证中文输出不乱码。
            static SERIAL: AtomicU64 = AtomicU64::new(0);
            let id = SERIAL.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("kada_cmd_{}_{}.bat", std::process::id(), id));
            let content = format!("@echo off\r\nchcp 65001>nul\r\n{command}\r\n");
            std::fs::write(&path, content.as_bytes())
                .map_err(|e| format!("写入临时批处理失败: {e}"))?;
            let mut c = std::process::Command::new("cmd");
            c.arg("/C").arg(&path);
            temp_bat = Some(path);
            c
        }
    } else {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", command]);
        c
    };
    if hide_window {
        no_console_window(&mut cmd);
    }
    let out = run_with_limits(&mut cmd, label, ctx);
    if let Some(path) = temp_bat {
        let _ = std::fs::remove_file(path);
    }
    out
}

/// 执行脚本文件：可选解释器（`None` = 直接执行脚本本身），注入环境变量
/// `KADA_TRIGGER`（触发组合）、`KADA_NAME`（快捷键名）、`KADA_VARS`（变量表 JSON）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn run_script(
    interpreter: Option<&str>,
    path: &str,
    ctx: &Ctx,
    vars: &Vars,
    hide_window: bool,
) -> Result<CmdOutput, String> {
    let mut cmd = match interpreter {
        Some(i) => {
            let mut c = std::process::Command::new(i);
            c.arg(path);
            c
        }
        None => std::process::Command::new(path),
    };
    cmd.env("KADA_TRIGGER", ctx.t.trigger)
        .env("KADA_NAME", ctx.t.name)
        .env("KADA_VARS", vars_to_json(vars));
    if hide_window {
        no_console_window(&mut cmd);
    }
    run_with_limits(&mut cmd, "脚本", ctx)
}

/// 把变量表序列化为 JSON 对象字符串（供脚本环境变量 `KADA_VARS` 使用）。
#[cfg(all(
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
fn vars_to_json(vars: &Vars) -> String {
    let mut map = serde_json::Map::new();
    for (name, value) in vars {
        let v = match value {
            Value::File(f) => serde_json::json!({
                "name": f.name,
                "path": f.path,
                "dir": f.dir,
                "stem": f.stem,
                "ext": f.ext,
                "size": f.size,
                "modified": f.modified,
                "is_dir": f.is_dir,
            }),
            Value::Bool(b) => serde_json::json!(b),
            Value::Text(t) => serde_json::json!({ "text": t.text, "exit_code": t.exit_code }),
        };
        map.insert(name.clone(), v);
    }
    serde_json::Value::Object(map).to_string()
}

#[cfg(all(
    test,
    any(target_os = "windows", target_os = "linux", target_os = "macos"),
    feature = "automation"
))]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 测试用的触发现场（各用例共用同一组固定值，便于断言结果里的触发键 / 名称）。
    fn trigger_ctx() -> TriggerCtx<'static> {
        TriggerCtx {
            trigger: "Ctrl+Alt+T",
            name: "测试宏",
            frontmost: None,
            device: None,
            window: None,
            origin: None,
        }
    }

    fn ctx(timeout_ms: u64) -> Ctx<'static> {
        Ctx {
            t: trigger_ctx(),
            // 0 = 不限时（与配置里的 0 同义），用于只测「中止」的用例。
            timeout: Duration::from_millis(timeout_ms),
            text_mode: TextInjectMode::Clipboard,
            gen: abort::generation(),
            run_id: None,
            start: Instant::now(),
            step_seq: AtomicU64::new(0),
            scope_seq: AtomicU64::new(1),
            inject_lock: Mutex::new(()),
        }
    }

    /// 一个必然挂住的子进程（不写任何东西，只睡）。
    /// Windows 上「孙进程」这一层天然存在（cmd / powershell → 脚本本体），正好覆盖
    /// 「只杀直接子进程不够」的场景。
    fn sleeper(secs: u64) -> std::process::Command {
        if cfg!(target_os = "windows") {
            let mut c = std::process::Command::new("powershell");
            c.args([
                "-NoProfile",
                "-Command",
                &format!("Start-Sleep -Seconds {secs}"),
            ]);
            c
        } else {
            let mut c = std::process::Command::new("sleep");
            c.arg(secs.to_string());
            c
        }
    }

    #[test]
    fn run_cmd_captures_output_of_normal_command() {
        let _g = abort::test_lock();
        let out = run_cmd("CMD", false, "echo kada", &ctx(10_000), false).expect("命令应能执行");
        assert_eq!(out.killed, None, "正常结束不算被打断");
        assert_eq!(out.exit_code, Some(0));
        assert!(
            out.stdout.contains("kada"),
            "stdout 应捕获到输出：{:?}",
            out.stdout
        );
    }

    /// 测试进程本身有没有控制台窗口。CI 的 Windows runner 跑在**没有交互控制台**的会话里，
    /// `GetConsoleWindow()` 对任何进程都返回 0——包括没设 `CREATE_NO_WINDOW` 的子进程，
    /// 那种环境下「不隐藏就该有控制台」的反向对照天然不成立（不是开关失效）。
    #[cfg(target_os = "windows")]
    fn test_process_has_console() -> bool {
        extern "system" {
            fn GetConsoleWindow() -> *mut std::ffi::c_void;
        }
        // SAFETY: 纯查询当前进程的控制台句柄，不改变任何状态。
        unsafe { !GetConsoleWindow().is_null() }
    }

    /// 「隐式执行」要验的是**真的不建控制台**，不是「输出被藏起来」——所以拿真子进程问 Windows
    /// 自己有没有控制台窗口（`GetConsoleWindow()`）。`creation_flags` 设了什么，从 Rust 侧读不回来，
    /// 只有这条能作数：隐藏时子进程没有控制台（0），不隐藏时它有（本进程有控制台，子进程会继承）。
    #[cfg(target_os = "windows")]
    #[test]
    fn hide_window_really_removes_the_console() {
        let _g = abort::test_lock();
        let probe = std::env::temp_dir().join(format!("kada_console_probe_{}.ps1", std::process::id()));
        std::fs::write(
            &probe,
            concat!(
                "Add-Type -Namespace KadaProbe -Name Native -MemberDefinition ",
                "'[DllImport(\"kernel32.dll\")] public static extern System.IntPtr GetConsoleWindow();'\n",
                "[KadaProbe.Native]::GetConsoleWindow().ToInt64()\n",
            ),
        )
        .expect("写探针脚本失败");
        let vars: Vars = BTreeMap::new();
        let run = |hide: bool| {
            run_script(
                Some("powershell"),
                probe.to_str().unwrap(),
                &ctx(60_000),
                &vars,
                hide,
            )
            .expect("探针脚本应能跑完")
        };
        let hidden = run(true);
        let visible = run(false);
        let _ = std::fs::remove_file(&probe);

        assert_eq!(
            hidden.stdout.trim(),
            "0",
            "打开「隐式执行」后子进程不该有控制台窗口（stdout={:?} stderr={:?}）",
            hidden.stdout,
            hidden.stderr
        );
        // 反向对照只在「测试进程自己真有控制台」时成立：无控制台的会话里子进程继承不到，
        // 不设 CREATE_NO_WINDOW 也是 0。跳过它、把观测值打出来，别在 CI 上误报回归。
        if test_process_has_console() {
            assert_ne!(
                visible.stdout.trim(),
                "0",
                "没打开开关时子进程该照旧有控制台（反向对照；stdout={:?} stderr={:?}）",
                visible.stdout,
                visible.stderr
            );
        } else {
            eprintln!(
                "跳过反向对照：测试进程本身没有控制台窗口（visible stdout={:?}）",
                visible.stdout
            );
        }
    }

    #[test]
    fn timeout_kills_hung_process_and_reports_reason() {
        let _g = abort::test_lock();
        let start = Instant::now();
        let out = run_with_limits(&mut sleeper(30), "测试", &ctx(800)).expect("进程应能启动");
        assert!(
            matches!(&out.killed, Some(r) if r.contains("超时")),
            "超时必须标记为被强制终止且说明原因：{:?}",
            out.killed
        );
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "超时后应立即返回，不再等进程自然结束"
        );
        assert_eq!(
            out.exit_code, None,
            "被强杀的进程退出码没有语义，置空由 stderr 说明"
        );
        let note = out.into_result("cmd", "CMD", "sleep".into(), &ctx(800), false);
        assert!(
            note.stderr.contains("强制终止"),
            "原因要写进 stderr 才看得到：{}",
            note.stderr
        );
    }

    #[test]
    fn timeout_kills_whole_process_tree() {
        // 只杀直接子进程的话，`cmd`/`sh` 下面的脚本还活着，醒来照样建标记文件。
        let _g = abort::test_lock();
        let marker = std::env::temp_dir().join(format!("kada_tree_{}.txt", std::process::id()));
        let _ = fs::remove_file(&marker);
        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = std::process::Command::new("powershell");
            c.args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Start-Sleep -Seconds 3; New-Item -Path '{}' -ItemType File | Out-Null",
                    marker.display()
                ),
            ]);
            c
        } else {
            // 标记必须由**孙进程**来建：直接子进程（外层 `sh`）一死就没人 `touch` 了，
            // 那样「没建出文件」区分不了「树杀干净了」还是「只杀了 `sh`」——见 7.1-㊴。
            let mut c = std::process::Command::new("sh");
            c.args([
                "-c",
                &format!("sh -c 'sleep 3; touch \"{}\"' & wait", marker.display()),
            ]);
            c
        };
        let out = run_with_limits(&mut cmd, "测试", &ctx(1_200)).expect("进程应能启动");
        assert!(out.killed.is_some(), "1.2 秒还没结束就该判超时");
        std::thread::sleep(Duration::from_millis(3_500));
        assert!(
            !marker.exists(),
            "超时必须连子进程树一起杀，否则脚本还在后台继续跑"
        );
        let _ = fs::remove_file(&marker);
    }

    #[test]
    fn abort_kills_running_process() {
        let _g = abort::test_lock();
        let gen = abort::generation();
        let requester = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            abort::request();
        });
        let start = Instant::now();
        // 不限时（0）：能结束只可能是因为中止生效。
        let mut cmd = sleeper(30);
        let out = run_with_limits(&mut cmd, "测试", &ctx(0)).expect("进程应能启动");
        requester.join().unwrap();
        assert_eq!(out.killed.as_deref(), Some("已中止"));
        assert!(start.elapsed() < Duration::from_secs(8), "中止后应立即返回");
        assert!(gen < abort::generation(), "中止会推进代数");
    }

    /// 跑一串动作，收齐命令结果与逐步记录（测试统一入口；调用方负责先拿 [`abort::test_lock`]）。
    fn run_all(
        actions: &[Action],
        opts: &RunOptions,
    ) -> (Vec<CommandResult>, Vec<ActionStep>, RunStatus) {
        let mut results = Vec::new();
        let mut steps = Vec::new();
        let mut vars: Vars = BTreeMap::new();
        let mut last_copied = None;
        let mut commit = |r: CommandResult| results.push(r);
        let mut on_step = |s: ActionStep| steps.push(s);
        let mut rep = Reporter {
            commit: &mut commit,
            on_step: &mut on_step,
        };
        let status = run_actions(
            &mut rep,
            actions,
            trigger_ctx(),
            &mut vars,
            &mut last_copied,
            opts,
            Some(7),
        );
        (results, steps, status)
    }

    /// 每个动作都留下一条执行记录，父子关系与并行泳道对得上（时间线 / 流水线的骨架）。
    #[test]
    fn every_action_reports_a_step_with_parent_and_lane() {
        let _g = abort::test_lock();
        let actions = vec![
            Action::PauseMs {
                ms: 10,
                description: None,
            },
            Action::Parallel {
                actions: vec![
                    Action::PauseMs {
                        ms: 20,
                        description: None,
                    },
                    Action::PauseMs {
                        ms: 20,
                        description: None,
                    },
                ],
                description: None,
            },
        ];
        let (results, steps, status) = run_all(&actions, &RunOptions::default());
        assert!(results.is_empty(), "{results:?}");
        assert_eq!(status, RunStatus::Finished);
        // 2 条顶层 + 1 个并行容器 + 2 个成员 = 4。
        assert_eq!(steps.len(), 4, "{steps:?}");
        assert_eq!(steps[0].parent, None, "顶层动作不属于任何容器");
        let par = steps
            .iter()
            .find(|s| s.kind == "parallel")
            .expect("并行容器要有记录");
        let members: Vec<&ActionStep> = steps.iter().filter(|s| s.parent == Some(par.id)).collect();
        assert_eq!(members.len(), 2);
        assert_eq!(
            members.iter().map(|s| s.lane).collect::<Vec<_>>(),
            vec![0, 1]
        );
        for s in &steps {
            assert!(s.end_ms >= s.start_ms, "结束时点不能早于开始时点：{s:?}");
            assert_eq!(s.status, "ok");
        }
        assert_eq!(steps[0].parent, None);
        assert_eq!(
            results.iter().filter(|r| r.run_id == Some(7)).count(),
            results.len()
        );
    }

    /// 分组只是组织结构：成员按顺序跑，执行记录挂在分组这一步下面（嵌套分组再挂一层）。
    #[test]
    fn group_runs_members_in_order_and_nests_steps() {
        let _g = abort::test_lock();
        let actions = vec![Action::Group {
            name: "准备".into(),
            actions: vec![
                Action::PauseMs {
                    ms: 10,
                    description: None,
                },
                Action::Group {
                    name: "收尾".into(),
                    actions: vec![Action::PauseMs {
                        ms: 10,
                        description: None,
                    }],
                },
            ],
        }];
        let (_, steps, status) = run_all(&actions, &RunOptions::default());
        assert_eq!(status, RunStatus::Finished);
        // 外层分组 + 第一个成员 + 内层分组 + 内层成员 = 4。
        assert_eq!(steps.len(), 4, "{steps:?}");
        let outer = steps
            .iter()
            .find(|s| s.kind == "group" && s.parent.is_none())
            .expect("外层分组要有记录");
        assert_eq!(outer.title, "准备（分组 2 个动作）");
        let first = steps
            .iter()
            .find(|s| s.kind == "pause_ms" && s.parent == Some(outer.id))
            .expect("第一个成员挂在外层分组下");
        assert_eq!(first.lane, 0);
        let inner = steps
            .iter()
            .find(|s| s.kind == "group" && s.parent == Some(outer.id))
            .expect("嵌套分组挂在父分组下");
        assert!(steps
            .iter()
            .any(|s| s.kind == "pause_ms" && s.parent == Some(inner.id)));
        for s in &steps {
            assert_eq!(s.status, "ok", "{s:?}");
        }
    }

    #[test]
    fn abort_stops_remaining_actions_and_reports() {
        let _g = abort::test_lock();
        let actions = vec![
            Action::PauseMs {
                ms: 5_000,
                description: None,
            },
            Action::PauseMs {
                ms: 5_000,
                description: None,
            },
        ];
        let requester = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            abort::request();
        });
        let mut results: Vec<CommandResult> = Vec::new();
        let start = Instant::now();
        {
            let mut commit = |r: CommandResult| results.push(r);
            let mut vars: Vars = BTreeMap::new();
            let mut last_copied = None;
            let mut on_step = |_s: ActionStep| {};
            let mut rep = Reporter {
                commit: &mut commit,
                on_step: &mut on_step,
            };
            let status = run_actions(
                &mut rep,
                &actions,
                trigger_ctx(),
                &mut vars,
                &mut last_copied,
                &RunOptions {
                    action_timeout: Duration::ZERO,
                    ..Default::default()
                },
                None,
            );
            assert_eq!(
                status,
                RunStatus::Aborted,
                "用户中止后整条链的结论是 Aborted"
            );
        }
        requester.join().unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "中止要立刻生效，不能等 PauseMs 睡完"
        );
        assert_eq!(results.len(), 1, "只记一条中止结果，别每步都记");
        assert_eq!(results[0].kind, "abort");
        assert_eq!(results[0].name, "测试宏");
        assert_eq!(abort::running(), 0, "执行结束后计数器要归零");
    }

    #[test]
    fn abort_requested_before_run_does_not_stop_it() {
        // 代数语义：上次的中止不能波及之后新开始的执行（否则点一次停止会「毒死」下一次触发）。
        let _g = abort::test_lock();
        abort::request();
        let (results, _, status) = run_all(
            &[Action::PauseMs {
                ms: 10,
                description: None,
            }],
            &RunOptions::default(),
        );
        assert!(results.is_empty(), "新一轮执行不该被上一次的中止影响");
        assert_eq!(status, RunStatus::Finished);
    }

    /// 跑一串动作并收集提交的结果（失败上报用例共用；调用方负责先拿 [`abort::test_lock`]）。
    fn run_collect(actions: &[Action], vars: &mut Vars) -> Vec<CommandResult> {
        let mut results: Vec<CommandResult> = Vec::new();
        {
            let mut commit = |r: CommandResult| results.push(r);
            let mut on_step = |_s: ActionStep| {};
            let mut rep = Reporter {
                commit: &mut commit,
                on_step: &mut on_step,
            };
            let mut last_copied = None;
            run_actions(
                &mut rep,
                actions,
                trigger_ctx(),
                vars,
                &mut last_copied,
                &RunOptions::default(),
                None,
            );
        }
        results
    }

    /// 一个必然失败的删除动作（路径不存在）。
    fn failing_delete(description: Option<&str>) -> Action {
        Action::Os {
            operation: OsOperation::Delete {
                path: "D:\\kada_missing_dir\\nope.txt".into(),
            },
            description: description.map(str::to_string),
        }
    }

    #[test]
    fn failed_action_is_reported_with_type_and_summary() {
        let _g = abort::test_lock();
        let actions = vec![failing_delete(Some("清理临时文件"))];
        let mut vars: Vars = BTreeMap::new();
        let results = run_collect(&actions, &mut vars);

        assert_eq!(
            results.len(),
            1,
            "失败必须产出一条记录（此前只有控制台日志，用户看不到）"
        );
        let r = &results[0];
        assert_eq!(r.kind, "error");
        assert_eq!(r.label, "文件操作失败");
        assert_eq!(r.trigger, "Ctrl+Alt+T");
        assert_eq!(r.name, "测试宏");
        assert!(
            r.stderr.contains("不存在"),
            "失败原因要写进 stderr：{}",
            r.stderr
        );
        assert_eq!(r.exit_code, None, "没跑起来的动作没有退出码");
        assert!(
            !r.show_output,
            "没开「显示输出」的动作失败只记消息中心，不弹窗"
        );
        assert!(!r.time.is_empty());
        assert!(
            r.command.contains("清理临时文件"),
            "用户写的说明要展示：{}",
            r.command
        );
        assert!(
            r.command.contains("删除"),
            "还要看得出是哪一步：{}",
            r.command
        );
    }

    #[test]
    fn failed_action_does_not_stop_remaining_actions() {
        let _g = abort::test_lock();
        let actions = vec![
            Action::Keys {
                keys: vec!["NotAKey".into()],
                mode: KeysMode::Tap,
                description: None,
            },
            failing_delete(None),
        ];
        let mut vars: Vars = BTreeMap::new();
        let results = run_collect(&actions, &mut vars);

        assert_eq!(results.len(), 2, "一步失败不该吞掉后面的动作");
        assert_eq!(results[0].label, "按键注入失败");
        assert!(
            results[0].stderr.contains("NotAKey"),
            "要点出坏在哪个键名：{}",
            results[0].stderr
        );
        assert!(
            results[0].command.contains("NotAKey"),
            "摘要要有键名：{}",
            results[0].command
        );
        assert_eq!(results[1].label, "文件操作失败");
    }

    #[test]
    fn failed_command_start_is_reported_and_honors_show_output() {
        let _g = abort::test_lock();
        // 指向一个不存在的脚本文件：进程根本起不来，走的是 `Err` 而不是「非零退出码」。
        let missing = std::env::temp_dir()
            .join("kada_missing_script_never_exists")
            .display()
            .to_string();
        let actions = vec![Action::Script {
            path: missing,
            interpreter: None,
            show_output: true,
            hide_window: false,
            var: String::new(),
            description: None,
        }];
        let mut vars: Vars = BTreeMap::new();
        let results = run_collect(&actions, &mut vars);

        assert_eq!(
            results.len(),
            1,
            "命令/脚本起不来同样要上报（此前只在控制台留一行）"
        );
        assert_eq!(results[0].label, "脚本失败");
        assert!(
            results[0].stderr.contains("失败"),
            "错误原因进 stderr：{}",
            results[0].stderr
        );
        assert!(
            results[0].show_output,
            "动作开了「显示输出」，连启动失败也该弹出来"
        );
    }

    #[test]
    fn failure_summary_substitutes_variables() {
        let _g = abort::test_lock();
        let missing = "D:\\kada_missing_dir\\nope.txt".to_string();
        let mut vars: Vars = BTreeMap::new();
        vars.insert(
            "target".into(),
            Value::Text(TextValue {
                text: missing.clone(),
                exit_code: None,
            }),
        );
        let actions = vec![Action::Os {
            operation: OsOperation::Delete {
                path: "{target}".into(),
            },
            description: None,
        }];
        let results = run_collect(&actions, &mut vars);

        assert_eq!(results.len(), 1);
        assert!(
            results[0].command.contains("nope.txt"),
            "摘要要展示本次实际生效的取值：{}",
            results[0].command
        );
        assert!(
            !results[0].command.contains("{target}"),
            "占位符不该原样留在给用户看的摘要里：{}",
            results[0].command
        );
    }

    #[test]
    fn long_failure_summary_is_truncated_to_one_line() {
        let _g = abort::test_lock();
        let actions = vec![Action::Text {
            text: format!("第一行\n{}", "长".repeat(500)),
            mode: TextMode::Input,
            masked: false,
            description: None,
        }];
        // 文本注入通常成功，这里只验摘要渲染本身：换行压平 + 截断。
        let summary = super::action_summary(&actions[0], &BTreeMap::new());
        assert!(!summary.contains('\n'), "摘要要压成单行：{summary}");
        assert!(summary.contains('…'), "超长文本要截断：{summary}");
        assert!(
            summary.chars().count() <= PREVIEW_MAX_CHARS + 8,
            "截断后不该还很长：{summary}"
        );
    }

    /// 标记了「隐藏内容」的文本动作，执行记录里的摘要也只能是 `***`——否则触发一次之后
    /// 「消息」页就把原文摊在屏幕上，界面上的隐藏等于白做。
    #[test]
    fn masked_text_action_hides_body_in_summary() {
        let _g = abort::test_lock();
        let hidden = Action::Text {
            text: "hunter2".into(),
            mode: TextMode::Input,
            masked: true,
            description: None,
        };
        let summary = super::action_summary(&hidden, &BTreeMap::new());
        assert!(summary.contains("***"), "隐藏的文本要看得出被隐藏了：{summary}");
        assert!(!summary.contains("hunter2"), "隐藏的文本不该出现在摘要里：{summary}");

        // 没标记隐藏的照旧展示原文（隐藏是显式选择，不是默认）。
        let plain = Action::Text {
            text: "hello".into(),
            mode: TextMode::Input,
            masked: false,
            description: None,
        };
        let summary = super::action_summary(&plain, &BTreeMap::new());
        assert!(summary.contains("hello"), "未隐藏的文本照旧展示：{summary}");
    }

    /// 剪贴板动作三类的摘要：写入要代入变量、空文本说明是清空、读取写出变量名、
    /// 临时文本要能认出是哪一步（执行记录里只看类型分不清）。
    #[test]
    fn clipboard_summary_covers_three_ops() {
        let _g = abort::test_lock();
        let mut vars: Vars = BTreeMap::new();
        vars.insert("who".into(), Value::Text("Kada".into()));

        let set = Action::Clipboard {
            operation: ClipboardOperation::SetClipboard { text: "hi {who}".into() },
            description: None,
        };
        let s = super::action_summary(&set, &vars);
        assert!(s.contains("写入剪贴板 hi Kada"), "{s}");

        let clear = Action::Clipboard {
            operation: ClipboardOperation::SetClipboard { text: String::new() },
            description: None,
        };
        assert!(super::action_summary(&clear, &vars).contains("清空剪贴板"));

        let get = Action::Clipboard {
            operation: ClipboardOperation::GetClipboard { var: "clip".into() },
            description: None,
        };
        assert!(super::action_summary(&get, &vars).contains("读取剪贴板 → clip"));

        let temp = Action::Clipboard {
            operation: ClipboardOperation::TempStr { text: "hi".into() },
            description: None,
        };
        assert!(super::action_summary(&temp, &vars).contains("临时文本 hi"));
    }

    /// 切换虚拟桌面的摘要要写明方向——执行记录里只写「切换虚拟桌面」看不出去了哪一边。
    /// 顺带钉住它归「操作系统」动作（`Action::System`）而不是文件动作。
    #[cfg(feature = "automation")]
    #[test]
    fn switch_desktop_summary_names_direction() {
        let _g = abort::test_lock();
        let body = |direction| {
            let action = Action::System {
                operation: SystemOperation::SwitchDesktop { direction },
                description: None,
            };
            assert_eq!(super::action_kind(&action), "system", "该归操作系统动作");
            super::action_summary(&action, &BTreeMap::new())
        };
        let next = body(DesktopDirection::Next);
        assert!(next.contains("下一个"), "摘要要写方向：{next}");
        let prev = body(DesktopDirection::Prev);
        assert!(prev.contains("上一个"), "摘要要写方向：{prev}");

        // 文件动作仍归 os——两类不许混（用户点名的分类拆分）。
        let file = Action::Os {
            operation: OsOperation::Delete { path: "C:\\tmp\\x".into() },
            description: None,
        };
        assert_eq!(super::action_kind(&file), "os");
    }

    /// 切桌面的平台键位映射：Windows `Win+Ctrl+←/→`、macOS `⌃←/→`、Linux `Super+PageUp/Down`。
    /// 三个平台各跑各自那一支（CI 的 windows / ubuntu / macos 矩阵合起来覆盖全表）。
    #[cfg(feature = "automation")]
    #[test]
    fn desktop_keys_follow_each_platforms_defaults() {
        let next = super::desktop_keys(DesktopDirection::Next);
        let prev = super::desktop_keys(DesktopDirection::Prev);

        if cfg!(target_os = "windows") {
            assert_eq!(next, [Key::Meta, Key::Control, Key::ArrowRight]);
            assert_eq!(prev, [Key::Meta, Key::Control, Key::ArrowLeft]);
        } else if cfg!(target_os = "macos") {
            assert_eq!(next, [Key::Control, Key::ArrowRight], "macOS 是 ⌃←/→，不是 ⌘");
            assert_eq!(prev, [Key::Control, Key::ArrowLeft]);
        } else {
            assert_eq!(next, [Key::Meta, Key::PageDown]);
            assert_eq!(prev, [Key::Meta, Key::PageUp]);
        }

        // 两个方向必须是不同的键——写反了「上一个 / 下一个」就只是互为反向，单测抓不到行为，
        // 但至少别是同一串。
        assert_ne!(next, prev);
    }

    // ---- 并行动作（规划 7.2-㉞） ----

    /// EXT-16：含按键按下 / 抬起（含嵌套）的并行组必须退化为串行，否则顺序可能反过来。
    #[cfg(feature = "automation")]
    #[test]
    fn key_lifecycle_forces_parallel_to_serial() {
        let tap = Action::Keys {
            keys: vec!["Ctrl".into()],
            mode: KeysMode::Tap,
            description: None,
        };
        let press = Action::Keys {
            keys: vec!["Ctrl".into()],
            mode: KeysMode::Press,
            description: None,
        };
        assert!(!has_key_lifecycle(&[tap.clone()]));
        assert!(!has_key_lifecycle(&[Action::Group {
            name: String::new(),
            actions: vec![Action::Parallel {
                actions: vec![tap],
                description: None,
            }],
        }]));
        assert!(has_key_lifecycle(&[Action::Group {
            name: String::new(),
            actions: vec![Action::Parallel {
                actions: vec![press.clone()],
                description: None,
            }],
        }]));
        assert!(has_key_lifecycle(&[Action::If {
            condition: kada_core::Condition::Exists {
                path: ".".into()
            },
            then: vec![press],
            otherwise: vec![],
            description: None,
        }]));
    }

    /// 非注入步骤（延迟 / 命令）在并行组里必须真正重叠：两个 1.5s 延迟应约 1.5s 完成，
    /// 而不是串行的 3s。
    #[test]
    fn parallel_group_overlaps_non_injection_steps() {
        let _g = abort::test_lock();
        let actions = vec![Action::Parallel {
            actions: vec![
                Action::PauseMs {
                    ms: 1_500,
                    description: None,
                },
                Action::PauseMs {
                    ms: 1_500,
                    description: None,
                },
            ],
            description: None,
        }];
        let mut vars: Vars = BTreeMap::new();
        let start = Instant::now();
        let results = run_collect(&actions, &mut vars);
        let elapsed = start.elapsed();
        assert!(results.is_empty(), "延迟动作没有结果产出");
        assert!(
            elapsed < Duration::from_millis(2_600),
            "两个 1.5s 延迟必须重叠（串行要 3s），实际 {elapsed:?}"
        );
    }

    /// 成员结果按**下标顺序**提交（确定性），与成员各自跑完的先后无关。
    #[test]
    fn parallel_member_results_ordered_by_index() {
        let _g = abort::test_lock();
        let actions = vec![Action::Parallel {
            actions: vec![
                failing_delete(None), // 成员 0：几乎立刻失败
                Action::Cmd {
                    command: "echo second".into(),
                    show_output: false,
                    var: "".into(),
                    description: None,
                }, // 成员 1：要起进程，慢一些
            ],
            description: None,
        }];
        let mut vars: Vars = BTreeMap::new();
        let results = run_collect(&actions, &mut vars);
        assert_eq!(results.len(), 2, "{results:?}");
        assert_eq!(results[0].kind, "error", "成员 0 的结果在前");
        assert_eq!(results[1].kind, "cmd", "成员 1 的结果在后");
        assert!(results[1].stdout.contains("second"));
    }

    /// 一步失败不取消兄弟：失败记录照样产出，另一个成员正常完成。
    #[test]
    fn parallel_member_failure_does_not_cancel_siblings() {
        let _g = abort::test_lock();
        let actions = vec![
            Action::Parallel {
                actions: vec![
                    failing_delete(Some("组内失败")),
                    Action::Cmd {
                        command: "echo ok".into(),
                        show_output: false,
                        var: "".into(),
                        description: None,
                    },
                ],
                description: None,
            },
            Action::PauseMs {
                ms: 10,
                description: None,
            }, // 组后动作仍执行
        ];
        let mut vars: Vars = BTreeMap::new();
        let results = run_collect(&actions, &mut vars);
        assert_eq!(results.len(), 2, "{results:?}");
        assert!(results[0].stderr.contains("不存在"));
        assert!(results[0].command.contains("组内失败"));
        assert!(results[1].stdout.contains("ok"), "兄弟成员应照常跑完");
    }

    /// 同名变量按成员下标顺序合并（后者胜），确定性可复现。
    #[test]
    fn parallel_var_writes_merge_by_index() {
        let _g = abort::test_lock();
        let write = |text: &str| Action::Cmd {
            command: format!("echo {text}"),
            show_output: false,
            var: "v".into(),
            description: None,
        };
        let actions = vec![Action::Parallel {
            actions: vec![write("first"), write("second")],
            description: None,
        }];
        let mut vars: Vars = BTreeMap::new();
        let _ = run_collect(&actions, &mut vars);
        match vars.get("v") {
            Some(Value::Text(t)) => assert_eq!(t.text, "second", "后者（下标大）胜"),
            other => panic!("变量 v 应为 Text：{other:?}"),
        }
    }

    /// 并行组被中止：只记**一条**中止结果（不是每个成员一条），且立刻返回。
    #[test]
    fn abort_during_parallel_records_single_abort() {
        let _g = abort::test_lock();
        let actions = vec![Action::Parallel {
            actions: vec![
                Action::PauseMs {
                    ms: 5_000,
                    description: None,
                },
                Action::PauseMs {
                    ms: 5_000,
                    description: None,
                },
            ],
            description: None,
        }];
        let requester = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            abort::request();
        });
        let mut results: Vec<CommandResult> = Vec::new();
        {
            let mut commit = |r: CommandResult| results.push(r);
            let mut on_step = |_s: ActionStep| {};
            let mut rep = Reporter {
                commit: &mut commit,
                on_step: &mut on_step,
            };
            let mut vars: Vars = BTreeMap::new();
            let mut last_copied = None;
            let start = Instant::now();
            run_actions(
                &mut rep,
                &actions,
                trigger_ctx(),
                &mut vars,
                &mut last_copied,
                &RunOptions {
                    action_timeout: Duration::ZERO,
                    ..Default::default()
                },
                None,
            );
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "中止要立刻生效，不能等 5s 延迟睡完"
            );
        }
        requester.join().unwrap();
        let aborts: Vec<_> = results.iter().filter(|r| r.kind == "abort").collect();
        assert_eq!(aborts.len(), 1, "并行组被中止只记一条：{results:?}");
        assert_eq!(abort::running(), 0, "执行结束后计数器要归零");
    }

    /// 嵌套并行 / 成员里的 `If` 都能跑通，不会死锁（注入锁只在动作内取一次）。
    #[test]
    fn nested_parallel_and_if_run_without_deadlock() {
        let _g = abort::test_lock();
        let actions = vec![Action::Parallel {
            actions: vec![
                Action::If {
                    condition: kada_core::Condition::Exists { path: ".".into() },
                    then: vec![Action::PauseMs {
                        ms: 50,
                        description: None,
                    }],
                    otherwise: vec![],
                    description: None,
                },
                Action::Parallel {
                    actions: vec![Action::PauseMs {
                        ms: 50,
                        description: None,
                    }],
                    description: None,
                },
            ],
            description: None,
        }];
        let mut vars: Vars = BTreeMap::new();
        let start = Instant::now();
        let results = run_collect(&actions, &mut vars);
        assert!(results.is_empty(), "{results:?}");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "嵌套并行不应死锁或拖长"
        );
    }

    /// 链内注入锁确实让临界区互斥（这是并行组注入不交错、剪贴板不被覆盖的根据）。
    #[test]
    fn inject_lock_serializes_critical_sections() {
        let ctx = ctx(0);
        let concurrent = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..2 {
                s.spawn(|| {
                    for _ in 0..20 {
                        let _g = lock_inject(&ctx);
                        let now = concurrent.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(2));
                        concurrent.fetch_sub(1, Ordering::SeqCst);
                    }
                });
            }
        });
        assert_eq!(peak.load(Ordering::SeqCst), 1, "注入锁必须让临界区互斥");
    }
}
