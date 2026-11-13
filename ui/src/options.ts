// 动作类型预设、子操作表、内容判定与摘要（从 main.ts 抽出，U-01）。
import { tr, trt } from "./i18n";
import { state } from "./state";
import type {
  Action,
  ActionBase,
  AppOperation,
  BrightnessOperation,
  CaptureOperation,
  CaptureOutput,
  ClipboardOperation,
  Condition,
  DesktopDirection,
  DisabledAction,
  MouseButton,
  MouseOp,
  OsOperation,
  SemanticOperation,
  SendPayload,
  SystemOperation,
  TransformOperation,
  WaitTarget,
  WindowOperation,
} from "./types";

// ---- 动作类型预设 ----
// 「设置变量」不是独立类型：它是文本操作的子操作（见 `actionSubtypeSelect`），
// 数据里仍是 `ask` 形态（后端 `Action::Ask`），老配置与卡带照旧能读。
export const ACTION_TYPES: { value: ActionBase["type"]; label: string }[] = [
  { value: "text", label: tr("文本操作") },
  { value: "command", label: tr("执行命令") },
  { value: "script", label: tr("执行脚本") },
  { value: "inline_script", label: tr("内置脚本") },
  { value: "wait_for", label: tr("等待条件") },
  { value: "app", label: tr("应用操作") },
  { value: "open_url", label: tr("打开网址") },
  { value: "os", label: tr("文件/文件夹") },
  { value: "system", label: tr("操作系统") },
  { value: "tool", label: tr("外部工具") },
  { value: "keys", label: tr("按键组合") },
  { value: "mouse", label: tr("鼠标操作") },
  { value: "semantic", label: tr("语义动作（安卓）") },
  { value: "pause_ms", label: tr("延迟") },
  { value: "if", label: tr("条件判断") },
  { value: "parallel", label: tr("并行执行") },
  { value: "group", label: tr("动作分组") },
];

// 已从动作类型下拉移除的旧类型：老配置里还能看见、还能改类型，但不再提供新建入口。
// （`clipboard` / `transform` 是用户要求删除的；`send_to` 并入「操作系统」、`window` 并入「应用」、
//   `brightness` 并入「操作系统」。这三类在加载时已被后端迁移，这里留标签只为执行记录 / 防御性渲染。）
export const REMOVED_TYPE_LABELS: Partial<Record<ActionBase["type"], string>> = {
  clipboard: tr("剪贴板（已移除）"),
  transform: tr("算法（已移除）"),
  capture: tr("截图（已移除）"),
  send_to: tr("后台发送"),
  window: tr("窗口操作"),
  brightness: tr("亮度"),
  // 「设置变量」在类型下拉里归文本操作：类型名就显示「文本操作」，方式由子类型下拉表达。
  ask: tr("文本操作"),
};

// 动作类型图标（收起态展示用）。
export const ACTION_ICONS: Record<ActionBase["type"], string> = {
  text: "✏️",
  ask: "✏️",
  command: "💻",
  script: "📜",
  inline_script: "🧠",
  wait_for: "⏳",
  keys: "⌨️",
  send_to: "📨",
  mouse: "🖱️",
  clipboard: "📋",
  transform: "🔣",
  pause_ms: "⏱️",
  semantic: "📱",
  os: "📁",
  system: "🖥️",
  window: "🪟",
  capture: "📸",
  tool: "🧩",
  brightness: "🔆",
  app: "🚀",
  open_url: "🌐",
  if: "🔀",
  parallel: "⇉",
  group: "▣",
};

export const ACTION_TYPE_LABELS = {
  ...Object.fromEntries(ACTION_TYPES.map((t) => [t.value, t.label])),
  ...REMOVED_TYPE_LABELS,
} as Record<ActionBase["type"], string>;

export function actionTypeLabel(t: ActionBase["type"]): string {
  return ACTION_TYPE_LABELS[t];
}

/** 动作类型下拉的可选项：外部工具没探测到在跑时不给新建入口。 */
export function actionTypeChoices(includeTool = false): typeof ACTION_TYPES {
  let list = ACTION_TYPES;
  if (state.runningTools.length === 0 && !includeTool) {
    list = list.filter((t) => t.value !== "tool");
  }
  return list;
}

/** 禁用标记（`{type:"disabled", action}`）：界面一律作用在被包住的那条动作上。 */
export function isDisabled(a: Action): a is DisabledAction {
  return a.type === "disabled";
}

export function unwrapAction(a: Action): ActionBase {
  let cur: Action = a;
  while (cur.type === "disabled") cur = cur.action;
  return cur;
}

// ---- 变量流向（可视化编排的数据流标签） ----
// 该动作会把结果写进哪个变量（没有则空）。
export function actionVarWrites(a: Action): string[] {
  switch (a.type) {
    case "disabled":
      return actionVarWrites(a.action);
    case "command":
    case "script":
    case "ask":
      return a.var.trim() ? [a.var.trim()] : [];
    case "wait_for":
      return a.var.trim() ? [a.var.trim()] : [];
    case "inline_script": {
      // 除了动作自己的「存为变量」，脚本里 set_var("名", …) 写的变量也要算进数据流。
      const names = [...a.source.matchAll(/set_var\s*\(\s*["']([^"']+)["']/g)]
        .map((m) => m[1])
        .filter((n): n is string => n !== undefined);
      return [...new Set([...(a.var.trim() ? [a.var.trim()] : []), ...names])];
    }
    case "os":
      return a.operation.op === "get_file_props" && a.operation.var.trim()
        ? [a.operation.var.trim()]
        : [];
    case "app":
      return a.operation.op === "status" && a.operation.var.trim()
        ? [a.operation.var.trim()]
        : [];
    case "clipboard":
      return a.operation.op === "get_clipboard" && a.operation.var.trim()
        ? [a.operation.var.trim()]
        : [];
    case "transform":
      return a.var.trim() ? [a.var.trim()] : [];
    // 分组只是容器：数据流标签按成员汇总（一眼看出整组写/读了什么）。
    case "group":
      return [...new Set(a.actions.flatMap(actionVarWrites))];
    default:
      return [];
  }
}

// 文本扩展专用的动态片段不是变量，从引用标签里排除。
export const VAR_DYNAMIC = new Set(["date", "time", "clipboard"]);

// 该动作引用了哪些 {变量}（只扫承载占位符的字符串字段，条件分支里的动作单独算，不并入）。
// 不能扫 JSON 序列化原文——嵌套对象的花括号会被误当成变量引用。
export function actionVarReads(a: Action): string[] {
  const texts: string[] = [];
  switch (a.type) {
    case "disabled":
      return actionVarReads(a.action);
    case "text":
      texts.push(a.text);
      break;
    case "command":
      texts.push(a.command, a.working_dir ?? "");
      break;
    case "script":
      texts.push(
        a.path,
        a.interpreter ?? "",
        a.working_dir ?? "",
        ...(a.interpreter_args ?? []),
        ...(a.args ?? []),
      );
      break;
    case "inline_script":
      // 脚本源码不做 `{变量}` 占位符替换（脚本是代码，花括号是 Rhai 语法）；
      // 变量经 get_var("名") 读，按这个形状反解出数据流标签。
      for (const m of a.source.matchAll(/get_var\s*\(\s*["']([^"']+)["']\s*\)/g)) {
        texts.push(`{${m[1]}}`);
      }
      break;
    case "wait_for":
      texts.push(
        a.target.kind === "file"
          ? a.target.path
          : a.target.kind === "process"
            ? a.target.program
            : a.target.title,
      );
      break;
    case "ask":
      texts.push(a.prompt, a.default ?? "");
      break;
    case "os": {
      const o = a.operation;
      if ("source" in o) texts.push(o.source, o.dest);
      if ("path" in o) texts.push(o.path);
      if ("dest" in o) texts.push(o.dest);
      break;
    }
    case "system":
      if (a.operation.op === "send_to") {
        texts.push(a.operation.target);
        if (a.operation.payload.kind === "text") texts.push(a.operation.payload.text);
      }
      break;
    case "app": {
      const o = a.operation;
      if (o.op !== "current_window") texts.push(o.program);
      if ("args" in o) texts.push(...o.args);
      break;
    }
    case "clipboard":
      if (a.operation.op !== "get_clipboard") texts.push(a.operation.text);
      break;
    case "transform":
      texts.push(a.input);
      break;
    case "open_url":
      texts.push(a.url);
      break;
    case "send_to":
      texts.push(a.target);
      if (a.payload.kind === "text") texts.push(a.payload.text);
      break;
    case "capture":
      if (a.output === "file") texts.push(a.path);
      break;
    case "if": {
      const c = a.condition;
      if ("path" in c) texts.push(c.path);
      if ("value" in c) texts.push(c.value);
      if ("app" in c) texts.push(c.app);
      if ("text" in c) texts.push(c.text);
      break;
    }
    case "group":
      return [...new Set(a.actions.flatMap(actionVarReads))];
    default:
      break;
  }
  const out = new Set<string>();
  for (const t of texts) {
    for (const m of t.matchAll(/\{([^{}\s]+)\}/g)) {
      const name = m[1]?.split(".")[0];
      if (name && !VAR_DYNAMIC.has(name)) out.add(name);
    }
  }
  return [...out];
}

// ---- 条件判断的子条件 ----
export const CONDITION_TYPES: { value: Condition["kind"]; label: string }[] = [
  { value: "exists", label: tr("路径存在") },
  { value: "not_exists", label: tr("路径不存在") },
  { value: "is_file", label: tr("路径是文件") },
  { value: "is_dir", label: tr("路径是文件夹") },
  { value: "modified_within", label: tr("修改时间在…分钟内") },
  { value: "equals", label: tr("变量字段等于") },
  { value: "not_equals", label: tr("变量字段不等于") },
  { value: "frontmost_app", label: tr("前台应用是") },
  { value: "not_frontmost_app", label: tr("前台应用不是") },
  { value: "window_title_contains", label: tr("窗口标题包含") },
  { value: "device_is", label: tr("设备是") },
];

/** 触发侧 `when` 门控可用的条件类型：触发时没有变量，`equals/not_equals` 恒不成立，不列。 */
export const WHEN_CONDITION_TYPES = CONDITION_TYPES.filter(
  (c) => c.value !== "equals" && c.value !== "not_equals",
);

export function newCondition(kind: Condition["kind"]): Condition {
  switch (kind) {
    case "exists":
    case "not_exists":
    case "is_file":
    case "is_dir":
      return { kind, path: "" };
    case "modified_within":
      return { kind, path: "", minutes: 5 };
    case "equals":
    case "not_equals":
      return { kind, var: "", field: "", value: "" };
    case "frontmost_app":
    case "not_frontmost_app":
      return { kind, app: "" };
    case "window_title_contains":
      return { kind, text: "" };
    case "device_is":
      return { kind, id: "" };
  }
  return { kind: "exists", path: "" };
}

// ---- 文件/目录动作的子操作 ----
export const OS_OPS: { value: OsOperation["op"]; label: string }[] = [
  { value: "copy", label: tr("复制（文件/文件夹）") },
  { value: "cut", label: tr("剪切（移动）") },
  { value: "paste", label: tr("粘贴（上次复制/剪切来源）") },
  { value: "delete", label: tr("删除") },
  { value: "new_file", label: tr("新建文件") },
  { value: "new_folder", label: tr("新建文件夹") },
  { value: "open_folder", label: tr("打开文件夹") },
  { value: "open_file", label: tr("打开文件（默认程序）") },
  { value: "reveal", label: tr("定位文件（在资源管理器中选中）") },
  { value: "zip", label: tr("压缩为 zip") },
  { value: "unzip", label: tr("解压 zip") },
  { value: "get_file_props", label: tr("获取文件属性") },
];

// ---- 操作系统动作的子操作（与文件/目录是两类） ----
// 「后台发送」「显示器亮度」原先各自是一种动作类型，入口已并入这里：选了它们，
// 动作类型仍然是「操作系统」，分类不再跳走。
export const SYSTEM_OPS: { value: SystemOperation["op"]; label: string }[] = [
  { value: "switch_desktop", label: tr("切换虚拟桌面") },
  { value: "empty_recycle_bin", label: tr("清空回收站") },
  { value: "monitor_off", label: tr("关闭显示器") },
  { value: "toggle_desktop_icons", label: tr("切换桌面图标") },
  { value: "send_to", label: tr("后台发送（按键 / 文本）") },
  { value: "brightness", label: tr("显示器亮度") },
];

/** 虚拟桌面方向的文案（与 core 的 `DesktopDirection::label` 同款说法）。 */
export const DESKTOP_DIRECTIONS: { value: DesktopDirection; label: string }[] = [
  { value: "prev", label: tr("上一个（左）") },
  { value: "next", label: tr("下一个（右）") },
];

export function newSystemOp(op: SystemOperation["op"]): SystemOperation {
  switch (op) {
    case "switch_desktop":
      return { op, direction: "next" };
    case "empty_recycle_bin":
    case "monitor_off":
    case "toggle_desktop_icons":
      return { op };
    case "send_to":
      return { op, target: "", payload: { kind: "keys", keys: [] } };
    case "brightness":
      return { op, operation: "up", step: 10 };
  }
  return { op: "switch_desktop", direction: "next" };
}

// ---- 当前窗口动作的子操作（EXT-14）----
export const WINDOW_OPS: { value: WindowOperation; label: string }[] = [
  { value: "maximize_restore", label: tr("最大化 / 还原") },
  { value: "minimize", label: tr("最小化") },
  { value: "toggle_topmost", label: tr("置顶切换") },
  { value: "center", label: tr("居中") },
  { value: "close", label: tr("关闭") },
  { value: "close_same_app", label: tr("关闭同类窗口") },
];

// ---- 截图 / 贴图（EXT-20）----
export const CAPTURE_OPS: { value: CaptureOperation; label: string }[] = [
  { value: "full_screen", label: tr("全屏") },
  { value: "region", label: tr("区域（框选）") },
  { value: "window", label: tr("当前窗口") },
];
export const CAPTURE_OUTPUTS: { value: CaptureOutput; label: string }[] = [
  { value: "clipboard", label: tr("写入剪贴板") },
  { value: "file", label: tr("保存为 PNG") },
  { value: "pin", label: tr("贴图（置顶浮窗）") },
];

// ---- 显示器亮度（EXT-22）----
export const BRIGHTNESS_OPS: { value: BrightnessOperation; label: string }[] = [
  { value: "up", label: tr("调高") },
  { value: "down", label: tr("调低") },
  { value: "restore", label: tr("恢复原始值") },
];

// ---- 剪贴板动作的子操作（EXT-15）----
export const CLIPBOARD_OPS: { value: ClipboardOperation["op"]; label: string }[] = [
  { value: "set_clipboard", label: tr("写入剪贴板") },
  { value: "get_clipboard", label: tr("读取剪贴板到变量") },
  { value: "tempstr", label: tr("临时文本（粘贴后恢复）") },
];

export function newClipboardOp(op: ClipboardOperation["op"]): ClipboardOperation {
  switch (op) {
    case "set_clipboard":
      return { op, text: "" };
    case "get_clipboard":
      return { op, var: "clip" };
    case "tempstr":
      return { op, text: "" };
  }
  return { op: "set_clipboard", text: "" };
}

// ---- 算法动作的子操作（EXT-18）----
export const TRANSFORM_OPS: { value: TransformOperation; label: string }[] = [
  { value: "base64_encode", label: tr("base64 编码") },
  { value: "base64_decode", label: tr("base64 解码") },
  { value: "url_encode", label: tr("URL 编码") },
  { value: "url_decode", label: tr("URL 解码") },
  { value: "hash_sha256", label: tr("SHA-256 哈希") },
  { value: "hash_sha512", label: tr("SHA-512 哈希") },
];

export function transformLabel(op: TransformOperation): string {
  return TRANSFORM_OPS.find((o) => o.value === op)?.label ?? op;
}

export function newOsOp(op: OsOperation["op"]): OsOperation {  switch (op) {
    case "copy":
      return { op, source: "", dest: "" };
    case "cut":
      return { op, source: "", dest: "" };
    case "paste":
      return { op, dest: "" };
    case "delete":
      return { op, path: "" };
    case "new_file":
      return { op, path: "" };
    case "new_folder":
      return { op, path: "" };
    case "open_folder":
    case "open_file":
    case "reveal":
      return { op, path: "" };
    case "zip":
    case "unzip":
      return { op, source: "", dest: "" };
    case "get_file_props":
      return { op, path: "", var: "file" };
  }
  return { op: "copy", source: "", dest: "" };
}

// ---- 应用动作的子操作 ----
export const APP_OPS: { value: AppOperation["op"]; label: string }[] = [
  { value: "launch", label: tr("打开（启动）") },
  { value: "close", label: tr("关闭") },
  { value: "maximize", label: tr("最大化") },
  { value: "minimize", label: tr("最小化") },
  { value: "activate", label: tr("放置到前台") },
  { value: "status", label: tr("查询运行状态") },
  { value: "restart", label: tr("重启") },
];

export function newAppOp(op: AppOperation["op"]): AppOperation {
  switch (op) {
    case "launch":
      return { op, program: "", args: [] };
    case "close":
    case "maximize":
    case "minimize":
    case "activate":
      return { op, program: "" };
    case "status":
      return { op, program: "", var: "app", retries: 0, interval_ms: 1000 };
    case "restart":
      return { op, program: "", args: [] };
    // 「当前窗口」系列不做程序名匹配：作用对象是触发那一刻的前台窗口。
    case "current_window":
      return { op, action: "maximize_restore" };
  }
  return { op: "launch", program: "", args: [] };
}

// ---- 鼠标动作的子操作（移动 / 点击 / 滚轮） ----
export const MOUSE_OPS: { value: MouseOp["op"]; label: string }[] = [
  { value: "move", label: tr("移动光标") },
  { value: "move_to", label: tr("移动到绝对坐标") },
  { value: "move_to_origin", label: tr("移动到手势起点") },
  { value: "click", label: tr("点击") },
  { value: "scroll", label: tr("滚轮滚动") },
];

export function newMouseOp(op: MouseOp["op"]): MouseOp {
  switch (op) {
    case "move":
      return { op, dx: 100, dy: 0 };
    case "move_to":
      return { op, x: 0, y: 0 };
    case "move_to_origin":
      return { op };
    case "click":
      return { op, button: "left" };
    case "scroll":
      return { op, dx: 0, dy: -3 };
  }
  return { op: "move", dx: 100, dy: 0 };
}

// ---- 语义动作的子操作（安卓无障碍：复制 / 返回 / 通知栏…） ----
// 值的拼写必须与 `kada_core::SemanticOperation` 的 serde 名逐字相同（snake_case），
// 安卓壳拿它直接查表，名字分叉就会「报告说支持、按下去未知动作」。
export const SEMANTIC_OPS: { value: SemanticOperation; label: string }[] = [
  { value: "copy", label: tr("复制") },
  { value: "cut", label: tr("剪切") },
  { value: "paste", label: tr("粘贴") },
  { value: "select_all", label: tr("全选") },
  { value: "ime_enter", label: tr("提交输入") },
  { value: "scroll_forward", label: tr("向下翻页") },
  { value: "scroll_backward", label: tr("向上翻页") },
  { value: "back", label: tr("返回") },
  { value: "home", label: tr("回主屏") },
  { value: "recents", label: tr("最近任务") },
  { value: "notifications", label: tr("通知栏") },
  { value: "quick_settings", label: tr("控制中心") },
  { value: "lock_screen", label: tr("锁屏") },
  { value: "screenshot", label: tr("截图") },
];

export function semanticLabel(op: SemanticOperation): string {
  return SEMANTIC_OPS.find((o) => o.value === op)?.label ?? op;
}

export function newAction(type: ActionBase["type"]): ActionBase {
  switch (type) {
    case "text":
      return { type: "text", text: "", mode: "input", masked: false };
    case "ask":
      return { type: "ask", var: "", prompt: "", default: "", popup: true };
    case "command":
      return {
        type: "command",
        shell: "cmd",
        command: "",
        working_dir: "",
        timeout_ms: null,
        ok_exit_codes: [],
        show_output: "never",
        var: "",
        wait: true,
        on_error: "continue",
        if_running: "run",
      };
    case "script":
      return {
        type: "script",
        path: "",
        interpreter: null,
        interpreter_args: [],
        args: [],
        working_dir: "",
        timeout_ms: null,
        ok_exit_codes: [],
        show_output: "never",
        var: "",
        wait: true,
        on_error: "continue",
        if_running: "run",
      };
    case "inline_script":
      return { type: "inline_script", source: "", var: "", show_output: false };
    case "wait_for":
      return {
        type: "wait_for",
        target: { kind: "file", path: "" },
        timeout_ms: null,
        interval_ms: 200,
        var: "",
      };
    case "keys":
      return { type: "keys", keys: [], mode: "tap" };
    case "send_to":
      return { type: "send_to", target: "", payload: { kind: "keys", keys: [] } };
    case "mouse":
      return { type: "mouse", op: newMouseOp("move") };
    case "semantic":
      return { type: "semantic", operation: "copy" };
    case "clipboard":
      return { type: "clipboard", operation: { op: "set_clipboard", text: "" } };
    case "transform":
      return { type: "transform", operation: "base64_encode", input: "", var: "result", inject: false };
    case "pause_ms":
      return { type: "pause_ms", ms: 0 };
    case "os":
      return { type: "os", operation: newOsOp("copy") };
    case "system":
      return { type: "system", operation: newSystemOp("switch_desktop") };
    case "window":
      return { type: "window", operation: { op: "maximize_restore" } };
    case "capture":
      return { type: "capture", operation: "full_screen", output: "clipboard", path: "" };
    case "tool": {
      const t = state.runningTools[0];
      return { type: "tool", tool: t?.id ?? "", capability: t?.capabilities[0]?.id ?? "" };
    }
    case "brightness":
      return { type: "brightness", operation: "up", step: 10 };
    case "app":
      return { type: "app", operation: newAppOp("launch") };
    case "open_url":
      return { type: "open_url", url: "" };
    case "if":
      return { type: "if", condition: newCondition("exists"), then: [], otherwise: [] };
    case "parallel":
      return { type: "parallel", actions: [] };
    case "group":
      return { type: "group", name: "", actions: [] };
  }
  return { type: "text", text: "", mode: "input" };
}

export function osHasContent(o: OsOperation): boolean {
  switch (o.op) {
    case "copy":
    case "cut":
    case "zip":
    case "unzip":
      return o.source.trim().length > 0 && o.dest.trim().length > 0;
    case "paste":
      return o.dest.trim().length > 0;
    case "delete":
    case "new_file":
    case "new_folder":
    case "open_folder":
    case "open_file":
    case "reveal":
    case "get_file_props":
      return o.path.trim().length > 0;
  }
  return false;
}

export function condHasContent(c: Condition): boolean {
  switch (c.kind) {
    case "exists":
    case "not_exists":
    case "is_file":
    case "is_dir":
    case "modified_within":
      return c.path.trim().length > 0;
    case "equals":
    case "not_equals":
      return c.var.trim().length > 0;
    case "frontmost_app":
    case "not_frontmost_app":
      return c.app.trim().length > 0;
    case "window_title_contains":
      return c.text.trim().length > 0;
    case "device_is":
      return c.id.trim().length > 0;
  }
  return false;
}

export function appHasContent(o: AppOperation): boolean {
  switch (o.op) {
    case "launch":
    case "close":
    case "maximize":
    case "minimize":
    case "activate":
    case "status":
    case "restart":
      return o.program.trim().length > 0;
    // 当前窗口没有必填项：操作本身有默认值，目标由触发时抓取。
    case "current_window":
      return true;
  }
  return false;
}

export function actionHasContent(a: Action): boolean {
  switch (a.type) {
    // 禁用的动作即使还是空壳也留着：用户是「先禁掉」而不是「删掉」。
    case "disabled":
      return true;
    case "text":
      return a.mode === "input" ? a.text.trim().length > 0 : true;
    case "command":
      return a.command.trim().length > 0;
    case "script":
      return a.path.trim().length > 0;
    case "inline_script":
      return a.source.trim().length > 0;
    case "wait_for":
      return waitTargetHasContent(a.target);
    case "keys":
      return a.keys.length > 0;
    case "send_to":
      return (
        a.target.trim().length > 0 &&
        (a.payload.kind === "keys" ? a.payload.keys.length > 0 : true)
      );
    case "mouse":
      return (
        a.op.op === "click" ||
        a.op.op === "move_to_origin" ||
        a.op.op === "move_to" ||
        a.op.dx !== 0 ||
        a.op.dy !== 0
      );
    // 语义动作只有一个必填的子操作（新建时就有默认值），永远可执行。
    case "semantic":
      return true;
    case "clipboard":
      // 写入空文本 = 清空剪贴板，是完整操作；读取要有变量名，临时文本要有内容。
      switch (a.operation.op) {
        case "set_clipboard":
          return true;
        case "get_clipboard":
          return a.operation.var.trim().length > 0;
        case "tempstr":
          return a.operation.text.trim().length > 0;
      }
    case "pause_ms":
      return a.ms > 0;
    case "transform":
      return a.input.trim().length > 0 && (a.var.trim().length > 0 || !!a.inject);
    case "ask":
      // 不弹框时提示语不显示，只要变量名就能跑（默认值可以留空）。
      return a.var.trim().length > 0 && (a.popup === false || a.prompt.trim().length > 0);
    case "os":
      return osHasContent(a.operation);
    // 虚拟桌面 / 回收站这类没有要填的字段（方向有默认值），新建出来就是可执行的；
    // 并入的「后台发送」要填目标与按键，「亮度」要步长合法。
    case "system":
      switch (a.operation.op) {
        case "send_to":
          return (
            a.operation.target.trim().length > 0 &&
            (a.operation.payload.kind === "keys"
              ? a.operation.payload.keys.length > 0
              : true)
          );
        case "brightness":
          return (
            a.operation.operation === "restore" ||
            (a.operation.step >= 1 && a.operation.step <= 100)
          );
        default:
          return true;
      }
    case "window":
      return true;
    case "capture":
      return a.output !== "file" || a.path.trim().length > 0;
    case "tool":
      return a.tool.trim().length > 0 && a.capability.trim().length > 0;
    case "brightness":
      return a.operation === "restore" || (a.step >= 1 && a.step <= 100);
    case "app":
      return appHasContent(a.operation);
    case "open_url":
      return a.url.trim().length > 0;
    case "if":
      return condHasContent(a.condition) || a.then.length > 0 || a.otherwise.length > 0;
    case "parallel":
      return a.actions.length > 0;
    case "group":
      return a.actions.length > 0;
  }
  return false;
}

export function osSummary(o: OsOperation): string {
  switch (o.op) {
    case "copy":
      return trt`复制：${o.source} → ${o.dest}`;
    case "cut":
      return trt`剪切：${o.source} → ${o.dest}`;
    case "paste":
      return trt`粘贴到：${o.dest}`;
    case "delete":
      return trt`删除：${o.path}`;
    case "new_file":
      return trt`新建文件：${o.path}`;
    case "new_folder":
      return trt`新建文件夹：${o.path}`;
    case "open_folder":
      return trt`打开文件夹：${o.path}`;
    case "open_file":
      return trt`打开文件：${o.path}`;
    case "reveal":
      return trt`定位文件：${o.path}`;
    case "zip":
      return trt`压缩：${o.source} → ${o.dest}`;
    case "unzip":
      return trt`解压：${o.source} → ${o.dest}`;
    case "get_file_props":
      return trt`取属性：${o.path} → ${o.var}`;
  }
  return "";
}

/** 后台发送的摘要（系统操作里的一个子操作）。 */
export function sendToSummary(target: string, payload: SendPayload): string {
  const to = target || tr("未设目标窗口");
  return payload.kind === "keys"
    ? trt`后台发送按键 ${payload.keys.join("+")} → ${to}`
    : trt`后台发送文本 ${payload.text} → ${to}`;
}

/** 亮度摘要（系统操作里的一个子操作）。 */
export function brightnessSummary(operation: BrightnessOperation, step: number): string {
  const op = BRIGHTNESS_OPS.find((o) => o.value === operation)?.label ?? operation;
  return operation === "restore" ? tr("亮度：恢复原始值") : trt`亮度：${op} ${step}`;
}

/** 操作系统动作的摘要。 */
export function systemSummary(o: SystemOperation): string {
  switch (o.op) {
    case "switch_desktop":
      return trt`切换虚拟桌面：${o.direction === "prev" ? tr("上一个") : tr("下一个")}`;
    case "empty_recycle_bin":
      return tr("清空回收站");
    case "monitor_off":
      return tr("关闭显示器");
    case "toggle_desktop_icons":
      return tr("切换桌面图标");
    case "send_to":
      return sendToSummary(o.target, o.payload);
    case "brightness":
      return brightnessSummary(o.operation, o.step);
  }
  return "";
}

/** 当前窗口动作的摘要。 */
export function windowSummary(op: WindowOperation): string {
  switch (op) {
    case "maximize_restore":
      return tr("最大化 / 还原当前窗口");
    case "minimize":
      return tr("最小化当前窗口");
    case "toggle_topmost":
      return tr("切换当前窗口置顶");
    case "center":
      return tr("当前窗口居中");
    case "close":
      return tr("关闭当前窗口");
    case "close_same_app":
      return tr("关闭当前窗口的同类窗口");
  }
  return "";
}

/** 剪贴板动作的摘要。 */
export function clipboardSummary(op: ClipboardOperation): string {
  switch (op.op) {
    case "set_clipboard":
      return op.text ? trt`写入剪贴板：${op.text}` : tr("清空剪贴板");
    case "get_clipboard":
      return trt`读取剪贴板 → ${op.var}`;
    case "tempstr":
      return op.text ? trt`临时文本：${op.text}` : tr("临时文本");
  }
}

export function condSummary(c: Condition): string {  switch (c.kind) {
    case "exists":
      return trt`路径存在：${c.path}`;
    case "not_exists":
      return trt`路径不存在：${c.path}`;
    case "is_file":
      return trt`是文件：${c.path}`;
    case "is_dir":
      return trt`是文件夹：${c.path}`;
    case "modified_within":
      return trt`修改时间 ${c.minutes} 分钟内：${c.path}`;
    case "equals":
      return `${c.var}${c.field ? "." + c.field : ""} == ${c.value}`;
    case "not_equals":
      return `${c.var}${c.field ? "." + c.field : ""} != ${c.value}`;
    case "frontmost_app":
      return trt`前台应用是：${c.app}`;
    case "not_frontmost_app":
      return trt`前台应用不是：${c.app}`;
    case "window_title_contains":
      return trt`窗口标题包含：${c.text}`;
    case "device_is":
      return trt`设备是：${c.id}`;
  }
  return "";
}

export function appSummary(o: AppOperation): string {
  switch (o.op) {
    case "launch":
      return trt`打开：${o.program}`;
    case "close":
      return trt`关闭：${o.program}`;
    case "maximize":
      return trt`最大化：${o.program}`;
    case "minimize":
      return trt`最小化：${o.program}`;
    case "activate":
      return trt`放置到前台：${o.program}`;
    case "status":
      return trt`查询状态：${o.program} → ${o.var}${o.retries ? trt`（重试 ${o.retries} 次 / ${o.interval_ms}ms）` : ""}`;
    case "restart":
      return trt`重启：${o.program}`;
    case "current_window":
      return windowSummary(o.action);
  }
  return "";
}

export const MOUSE_BUTTON_LABELS: Record<MouseButton, string> = {
  left: tr("左键"),
  right: tr("右键"),
  middle: tr("中键"),
  back: tr("侧键·后退"),
  forward: tr("侧键·前进"),
};

export function mouseSummary(op: MouseOp): string {
  switch (op.op) {
    case "move":
      return trt`鼠标移动 (${op.dx}, ${op.dy})`;
    case "move_to":
      return trt`鼠标移动到 (${op.x}, ${op.y})`;
    case "move_to_origin":
      return tr("鼠标移动到手势起点");
    case "click":
      return trt`鼠标点击${MOUSE_BUTTON_LABELS[op.button]}`;
    case "scroll":
      return trt`鼠标滚轮 (${op.dx}, ${op.dy})`;
  }
}

export const WAIT_TARGET_TYPES: { value: WaitTarget["kind"]; label: string }[] = [
  { value: "file", label: tr("文件 / 文件夹出现") },
  { value: "process", label: tr("进程启动") },
  { value: "window", label: tr("窗口出现（仅 Windows）") },
];

export function newWaitTarget(kind: WaitTarget["kind"]): WaitTarget {
  switch (kind) {
    case "file":
      return { kind, path: "" };
    case "process":
      return { kind, program: "" };
    case "window":
      return { kind, title: "" };
  }
}

export function waitTargetHasContent(t: WaitTarget): boolean {
  switch (t.kind) {
    case "file":
      return t.path.trim().length > 0;
    case "process":
      return t.program.trim().length > 0;
    case "window":
      return t.title.trim().length > 0;
  }
}

export function waitTargetSummary(t: WaitTarget): string {
  switch (t.kind) {
    case "file":
      return trt`等路径出现：${t.path || "?"}`;
    case "process":
      return trt`等进程启动：${t.program || "?"}`;
    case "window":
      return trt`等窗口出现：${t.title || "?"}`;
  }
}

export function actionSummary(a: Action): string {
  switch (a.type) {
    case "disabled":
      return trt`已禁用：${actionSummary(a.action)}`;
    case "text":
      if (a.mode === "to_upper") return tr("文本转大写（选中/剪贴板）");
      if (a.mode === "to_lower") return tr("文本转小写（选中/剪贴板）");
      // 隐藏的文本在摘要里也只留 ***：收起态的卡片头部就这一行（列表搜索、卡带预览也用这份），
      // 摘要里照样露出原文等于没隐藏。
      if (a.masked) return tr("输入文本：***");
      return a.text ? trt`输入文本：${a.text}` : tr("输入文本");
    case "command": {
      const v = a.var ? ` → ${a.var}` : "";
      return `${a.shell === "cmd" ? "CMD" : "PowerShell"}：${a.command}${v}`;
    }
    case "script": {
      const v = a.var ? ` → ${a.var}` : "";
      const head = a.interpreter
        ? [a.interpreter, ...(a.interpreter_args ?? [])].join(" ")
        : "";
      const tail = (a.args ?? []).join(" ");
      const target = [head, a.path, tail].filter(Boolean).join(" ");
      return trt`脚本：${target}${v}`;
    }
    case "inline_script": {
      const v = a.var ? ` → ${a.var}` : "";
      const first =
        a.source
          .split("\n")
          .map((l) => l.trim())
          .find(Boolean) ?? "";
      return trt`内置脚本：${first}${v}`;
    }
    case "wait_for": {
      const v = a.var ? ` → ${a.var}` : "";
      return `${waitTargetSummary(a.target)}${v}`;
    }
    case "keys":
      if (a.mode === "press") return trt`按住：${a.keys.join("+")}`;
      if (a.mode === "release") return trt`抬起：${a.keys.join("+")}`;
      return trt`按键：${a.keys.join("+")}`;
    case "send_to": {
      const to = a.target || tr("未设目标窗口");
      return a.payload.kind === "keys"
        ? trt`后台发送按键 ${a.payload.keys.join("+")} → ${to}`
        : trt`后台发送文本 ${a.payload.text} → ${to}`;
    }
    case "mouse":
      return mouseSummary(a.op);
    case "semantic":
      return trt`语义动作：${semanticLabel(a.operation)}（仅安卓）`;
    case "clipboard":
      return clipboardSummary(a.operation);
    case "ask":
      // 不弹框形态的摘要看的是「写进去什么」，不再显示用户根本看不到的提示语。
      return a.popup === false
        ? trt`设置变量 ${a.var || "?"} = ${a.default || tr("（空）")}（不弹框）`
        : trt`设置变量 ${a.var || "?"}：${a.prompt}`;
    case "transform": {
      const sink = [a.var.trim() ? `→ ${a.var.trim()}` : "", a.inject ? tr("并注入") : ""]
        .filter(Boolean)
        .join(" ");
      return `${transformLabel(a.operation)}：${a.input}${sink ? ` ${sink}` : ""}`;
    }
    case "pause_ms":
      return trt`延迟 ${a.ms}ms`;
    case "os":
      return osSummary(a.operation);
    case "system":
      return systemSummary(a.operation);
    case "window":
      return windowSummary(a.operation.op);
    case "capture": {
      const op = CAPTURE_OPS.find((o) => o.value === a.operation)?.label ?? a.operation;
      const out =
        a.output === "file"
          ? a.path || tr("未设保存路径")
          : (CAPTURE_OUTPUTS.find((o) => o.value === a.output)?.label ?? a.output);
      return trt`截图：${op} → ${out}`;
    }
    case "tool": {
      const t = state.runningTools.find((x) => x.id === a.tool);
      const cap = t?.capabilities.find((c) => c.id === a.capability);
      return trt`${t?.name ?? a.tool}：${cap?.name ?? a.capability}（未接入）`;
    }
    case "brightness": {
      const op = BRIGHTNESS_OPS.find((o) => o.value === a.operation)?.label ?? a.operation;
      return a.operation === "restore" ? tr("亮度：恢复原始值") : trt`亮度：${op} ${a.step}`;
    }
    case "app":
      return appSummary(a.operation);
    case "open_url":
      return a.url ? trt`打开网址：${a.url}` : tr("打开网址");
    case "if":
      return trt`如果 ${condSummary(a.condition)}（${a.then.length} 个动作${a.otherwise.length ? trt`，否则 ${a.otherwise.length} 个` : ""}）`;
    case "parallel":
      return trt`并行执行（${a.actions.length} 个动作同时执行）`;
    case "group":
      return trt`动作分组（${a.actions.length} 个动作，按顺序执行）`;
  }
  return "";
}

// 收起态显示的描述：优先用用户填写的描述，否则回退到自动生成的动作摘要。
// 分组没有描述字段，用户填的「分组名称」就是标题。
export function actionDescription(a: Action): string {
  if (a.type === "disabled") return trt`已禁用：${actionDescription(a.action)}`;
  if (a.type === "group") {
    const n = a.name?.trim();
    return n ? trt`${n} · ${a.actions.length} 个动作` : actionSummary(a);
  }
  const d = a.description?.trim();
  return d ? d : actionSummary(a);
}
// ---- 键名选项（改键下拉用），对齐 kada-core 的 `Key::ALL` / key_name ----
// 必须是**显式字面量数组**（不要用 spread/循环拼）：`crates/kada-core` 的测试会按字面量
// 解析这里，跟 `Key::ALL` 逐一比对——Rust 新增按键而这里漏了，测试就会红。
export const KEY_OPTIONS = [
  "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M",
  "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
  "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
  "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
  "F13", "F14", "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
  ",", ".", "/", "\\", ";", "\"", "`", "-", "=", "[", "]",
  "Enter", "Esc", "Tab", "Space", "Backspace", "Delete", "Insert", "CapsLock",
  "Shift", "Ctrl", "Alt", "Meta",
  "Home", "End", "PageUp", "PageDown", "Up", "Down", "Left", "Right",
  "MediaPlayPause", "MediaPrev", "MediaNext", "VolumeMute", "VolumeDown", "VolumeUp",
  "Numpad0", "Numpad1", "Numpad2", "Numpad3", "Numpad4",
  "Numpad5", "Numpad6", "Numpad7", "Numpad8", "Numpad9",
  "NumpadAdd", "NumpadSubtract", "NumpadMultiply", "NumpadDivide",
  "NumpadDecimal", "NumpadEnter", "NumLock",
  "MouseMiddle", "MouseBack", "MouseForward", "WheelUp", "WheelDown",
];
