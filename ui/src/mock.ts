// 纯浏览器直开（无 Tauri 壳）时的演示兜底：注入假的 __TAURI_INTERNALS__，让 UI 能独立
// 渲染出完整界面（含演示配置），方便前端调试与视觉验收。Tauri WebView 里 __TAURI_INTERNALS__
// 由壳预注入，本模块自动跳过、零影响；纯浏览器下真实 IPC（导出/浏览/录制等）以拒绝收场。

type InvokeArgs = Record<string, unknown>;

const DEMO_CONFIG = {
  folders: [{ id: "f-work", name: "工作", parent: null }],
  layers: [{ id: "l-nav", name: "导航层" }],
  shortcuts: [
    {
      name: "备份到移动硬盘",
      description: "查盘符 → 确认 → 复制",
      folder: null,
      layer: null,
      triggers: ["Ctrl+Alt+B", "Ctrl+Alt+S"],
      enabled: true,
      actions: [
        {
          type: "command",
          shell: "powershell",
          command: "(Get-Volume -FileSystemLabel '我的硬盘').DriveLetter",
          show_output: false,
          var: "drive",
        },
        {
          type: "if",
          condition: { kind: "not_equals", var: "drive", field: "", value: "" },
          then: [{ type: "os", operation: { op: "copy", source: "C:\\工作\\待备份", dest: "{drive}:\\备份\\" } }],
          otherwise: [],
        },
        {
          type: "parallel",
          actions: [
            { type: "command", shell: "cmd", command: "echo 备份 A", show_output: false, var: "" },
            { type: "command", shell: "cmd", command: "echo 备份 B", show_output: false, var: "" },
          ],
        },
        { type: "pause_ms", ms: 500 },
        { type: "keys", keys: ["Ctrl", "S"] },
        { type: "mouse", op: { op: "move", dx: 200, dy: 120 } },
        { type: "mouse", op: { op: "click", button: "left" } },
      ],
    },
    {
      name: "打开工作目录",
      description: null,
      folder: "f-work",
      layer: "l-nav",
      triggers: ["Ctrl+Alt+W", "F9 W"],
      enabled: true,
      actions: [{ type: "os", operation: { op: "open_folder", path: "D:\\工作" } }],
    },
    {
      name: "搜索选中内容",
      description: null,
      folder: null,
      layer: null,
      triggers: ["Ctrl+Alt+S"],
      enabled: true,
      actions: [
        { type: "text", text: "https://example.com/search?q=", mode: "input" },
        { type: "open_url", url: "https://example.com/search" },
      ],
    },
  ],
  remaps: [
    {
      from: "CapsLock",
      to: "",
      tap: "Escape",
      hold: "Ctrl",
      layer: null,
      hold_layer: null,
      lock_layer: null,
      tap_timeout_ms: 200,
      oneshot: null,
      sticky: null,
      tap2: null,
      tap3: null,
      enabled: true,
    },
    {
      // 粘滞的任意键（7.3-⑭）：点一下锁定、再点一下解锁——侧键「按住说话」这类用法。
      from: "F1",
      to: "",
      tap: null,
      hold: null,
      layer: null,
      hold_layer: null,
      lock_layer: null,
      tap_timeout_ms: 200,
      oneshot: null,
      sticky: "MouseBack",
      tap2: null,
      tap3: null,
      enabled: true,
    },
  ],
  expansions: [{ trigger: ";addr", replace: "某市某区某路 88 号", enabled: true }],
  settings: {
    autostart: false,
    paused: false,
    wake_key: null,
    action_timeout_ms: 30000,
    sequence_timeout_ms: 1000,
    chord_timeout_ms: 1000,
    show_status_hud: true,
    text_inject_mode: "clipboard",
    hints: { visible: false, x: null, y: null, scale: 100, opacity: 92 },
  },
};

const DEMO_RESULTS = [
  {
    kind: "command",
    label: "CMD",
    command: "git status --short",
    trigger: "Ctrl+Alt+G",
    name: "Git 状态",
    stdout: "M ui/src/main.ts\nM ui/src/style.css",
    stderr: "",
    exit_code: 0,
    show_output: false,
    time: "2026-09-28 10:00:00",
  },
];

// 演示冲突：与 DEMO_CONFIG 一致的真实冲突（Ctrl+Alt+S 被「备份到移动硬盘」与「搜索选中内容」
// 同时占用；「导航层」有条目却没有切层键指向）。恒返回空会让纯浏览器验收误判「冲突检测坏了」。
const DEMO_CONFLICTS = [
  {
    severity: "error",
    name: "搜索选中内容",
    message: "触发键「Ctrl+Alt+S」被多条快捷键使用（与「备份到移动硬盘」重复）",
  },
  {
    severity: "warn",
    name: "",
    message: "层「导航层」没有任何切层键指向，配在该层的条目不会触发",
  },
  {
    severity: "warn",
    name: "输入密码",
    // 平台能力缺失（`platform: true`）：与上两条分组展示，演示模式也要能看见蓝色分组。
    platform: true,
    message: "「设备是」条件（动作或触发门控）在当前平台取不到设备，恒不成立",
  },
];

// 演示录制结果：点「■ 停止」后注入这段宏（真实录制只有 Tauri 壳内才有）。
const DEMO_RECORDED_ACTIONS = [
  { type: "keys", keys: ["Ctrl", "Shift", "N"] },
  { type: "pause_ms", ms: 300 },
  { type: "text", text: "演示模式录制的文本", mode: "input" },
  { type: "keys", keys: ["Enter"] },
];

export function installBrowserMock() {
  if ("__TAURI_INTERNALS__" in window) return;
  let unread = false;
  let paused = false;
  const results = structuredClone(DEMO_RESULTS);
  const w = window as unknown as {
    __TAURI_INTERNALS__: unknown;
    [key: string]: unknown;
  };
  // 事件监听登记：`plugin:event|listen` 存下回调 id，`emit` 据此把事件推回界面
  // （演示模式没有真正的后端事件源，更新检查/安装的进度必须自己推）。
  const listeners: Record<string, number> = {};
  const emit = (eventName: string, payload: unknown) => {
    const id = listeners[eventName];
    const cb = id == null ? undefined : w[`_${id}`];
    if (typeof cb === "function") cb({ event: eventName, id, payload });
  };
  w.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" }, currentWebview: {} },
    plugins: {},
    transformCallback(cb: unknown) {
      const id = Math.floor(Math.random() * 2 ** 31);
      w[`_${id}`] = cb;
      return id;
    },
    async invoke(cmd: string, args?: InvokeArgs): Promise<unknown> {
      switch (cmd) {
        case "get_config":
          return structuredClone(DEMO_CONFIG);
        case "set_config":
          return [];
        case "get_command_results":
          return structuredClone(results);
        case "get_unread":
          return unread;
        case "mark_results_read":
          unread = false;
          return null;
        case "clear_command_results":
          results.length = 0;
          return null;
        case "get_conflicts":
          // 演示冲突见 DEMO_CONFLICTS（与演示配置一致），不再恒返回空。
          return structuredClone(DEMO_CONFLICTS);
        case "get_update_status":
          return { current: "0.6.0", phase: "idle" };
        case "check_update":
          // 推事件让设置页看到「检查中 → 发现新版本」；否则按钮点了没反应像坏了。
          emit("update-status", { current: "0.6.0", phase: "checking" });
          setTimeout(
            () =>
              emit("update-status", {
                current: "0.6.0",
                phase: "available",
                version: "0.6.1",
                notes: "演示模式：这是一条模拟的更新说明。",
              }),
            600,
          );
          return null;
        case "install_update":
          // 模拟下载进度 → 安装中（真实安装会退出应用，演示模式只推状态）。
          emit("update-status", { current: "0.6.0", phase: "downloading", version: "0.6.1", percent: 0 });
          setTimeout(
            () =>
              emit("update-status", {
                current: "0.6.0",
                phase: "downloading",
                version: "0.6.1",
                percent: 100,
              }),
            800,
          );
          setTimeout(
            () => emit("update-status", { current: "0.6.0", phase: "installing", version: "0.6.1" }),
            1200,
          );
          return null;
        case "set_paused":
          paused = !!args?.paused;
          return null;
        // 停止执行：演示模式没有真的在跑的动作，如实返回 0（前端提示「当前没有正在执行的动作」）。
        case "abort_actions":
          return 0;
        // 编辑页「执行」：演示模式跑不了真动作，只确认前端点了按钮（后端 fire 不在浏览器里）。
        case "run_entry":
          return null;
        // 导入：演示模式的文件对话框恒返回「取消」，走不到这里；留着是为了让 IPC 契约完整
        // （后端 ImportOutcome 的形状）并在将来放开文件选择时不至于崩在「不支持命令」。
        case "import_config":
          return { mode: args?.mode ?? "replace", added: 0, notes: [], backup: null };
        // 导出：对话框现在返回路径（不是取消），导出必须成功收场，否则演示成「导出坏了」。
        case "export_config":
          return null;
        // 导出片段：回一个条数（真实实现写文件，这里只让前端把「已导出 N 项」演出来）。
        case "export_fragment":
          return (args?.picks as unknown[] | undefined)?.length ?? 0;
        case "start_record":
          return null;
        case "stop_record":
          // 演示录制结果：非空，点「■ 停止」能看到录下来的动作进草稿。
          return structuredClone(DEMO_RECORDED_ACTIONS);
        case "get_toast_payload":
          return null;
        // 输入状态指示：演示模式给一份「层 + 三种修饰键」的样例，让 `index.html#hud`
        // 在纯浏览器里也能渲染出来看样式（真实值只有壳内的状态机定时器线程会推）。
        case "get_status_payload":
          return {
            layer: "导航层",
            locked: true,
            mods: [
              { kind: "hold", key: "Ctrl" },
              { kind: "sticky", key: "Shift" },
              { kind: "oneshot", key: "Alt" },
            ],
            summary: "层：导航层（锁定） · 按住 Ctrl · 粘滞 Shift · 单次 Alt",
          };
        // 悬浮指示窗渲染完回报尺寸（演示模式不真的改窗口大小）。
        case "hud_ready":
          return null;
        // 快捷键提示框：演示模式给一份「常显」状态，`index.html#hints` 直开即可看样式
        // （真实位置 / 外观只有壳内的窗口会记）。
        case "get_hints_state":
          return { visible: true, scale: 100, opacity: 92, x: 0, y: 0 };
        case "hints_ready":
          return { visible: true, scale: 100, opacity: 92, x: 0, y: 0 };
        case "hints_move":
        case "hints_commit":
        case "hints_prefs":
        case "hints_set_visible":
          return null;
        // 对话框（plugin:dialog）：文件对话框演示模式返回一个**示例路径**（不再一律取消），
        // 让「浏览 / 导入 / 导出」在纯浏览器里能走完整条流程；`options.directory` 决定返回
        // 目录还是文件路径。
        case "plugin:dialog|open": {
          const options = (args?.options ?? {}) as { directory?: boolean; multiple?: boolean };
          const path = options.directory ? "D:\\工作\\演示文件夹" : "C:\\Users\\demo\\kada-config.json";
          return options.multiple ? [path] : path;
        }
        case "plugin:dialog|save": {
          const options = (args?.options ?? {}) as { defaultPath?: string };
          return `C:\\Users\\demo\\${options.defaultPath ?? "kada-config.json"}`;
        }
        // 确认框要返回**被点击按钮的文案**，不是布尔：`ask()` 拿它跟 okLabel（默认 "Yes"）比、
        // `confirm()` 跟 "Ok" 比，`message()` 直接把它当结果。以前这里返回 true，于是演示模式下
        // 所有确认框都被判成「取消」——删除目录/删除层点了没反应，且看不出原因。
        case "plugin:dialog|message": {
          const buttons = args?.buttons as { ok?: string } | string | undefined;
          if (buttons && typeof buttons === "object") return buttons.ok ?? "Ok";
          return buttons === "YesNo" ? "Yes" : "Ok";
        }
        // 事件监听（plugin:event）：登记回调，供 `emit` 推事件（更新状态等）。
        case "plugin:event|listen": {
          const eventName = String(args?.event ?? "");
          const handler = args?.handler;
          if (eventName && typeof handler === "number") listeners[eventName] = handler;
          return Math.floor(Math.random() * 2 ** 31);
        }
        case "plugin:event|unlisten": {
          const eventName = String(args?.event ?? "");
          if (eventName) delete listeners[eventName];
          return null;
        }
        default:
          throw new Error(`演示模式不支持命令 ${cmd}（请在 Tauri 壳内使用）`);
      }
    },
  };
  console.info("[kada] 浏览器演示模式：已注入模拟 IPC，配置为演示数据。paused =", paused);
}
