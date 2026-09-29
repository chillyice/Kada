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
      triggers: ["Ctrl+Alt+B"],
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
        { type: "pause_ms", ms: 500 },
        { type: "keys", keys: ["Ctrl", "S"] },
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

export function installBrowserMock() {
  if ("__TAURI_INTERNALS__" in window) return;
  let unread = false;
  let paused = false;
  const results = structuredClone(DEMO_RESULTS);
  const w = window as unknown as {
    __TAURI_INTERNALS__: unknown;
    [key: string]: unknown;
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
          return args?.config ? [] : [];
        case "get_update_status":
          return { current: "0.6.0", phase: "idle" };
        case "check_update":
        case "install_update":
          return null;
        case "set_paused":
          paused = !!args?.paused;
          return null;
        // 停止执行：演示模式没有真的在跑的动作，如实返回 0（前端提示「当前没有正在执行的动作」）。
        case "abort_actions":
          return 0;
        // 导入：演示模式的文件对话框恒返回「取消」，走不到这里；留着是为了让 IPC 契约完整
        // （后端 ImportOutcome 的形状）并在将来放开文件选择时不至于崩在「不支持命令」。
        case "import_config":
          return { mode: args?.mode ?? "replace", added: 0, notes: [], backup: null };
        case "start_record":
          return null;
        case "stop_record":
          return [];
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
        // 对话框（plugin:dialog）：文件对话框演示模式一律返回空（用户取消）。
        case "plugin:dialog|open":
        case "plugin:dialog|save":
          return null;
        // 确认框要返回**被点击按钮的文案**，不是布尔：`ask()` 拿它跟 okLabel（默认 "Yes"）比、
        // `confirm()` 跟 "Ok" 比，`message()` 直接把它当结果。以前这里返回 true，于是演示模式下
        // 所有确认框都被判成「取消」——删除目录/删除层点了没反应，且看不出原因。
        case "plugin:dialog|message": {
          const buttons = args?.buttons as { ok?: string } | string | undefined;
          if (buttons && typeof buttons === "object") return buttons.ok ?? "Ok";
          return buttons === "YesNo" ? "Yes" : "Ok";
        }
        // 事件监听（plugin:event）：演示模式下没有事件源，登记即返回句柄。
        case "plugin:event|listen":
          return Math.floor(Math.random() * 2 ** 31);
        case "plugin:event|unlisten":
          return null;
        default:
          throw new Error(`演示模式不支持命令 ${cmd}（请在 Tauri 壳内使用）`);
      }
    },
  };
  console.info("[kada] 浏览器演示模式：已注入模拟 IPC，配置为演示数据。paused =", paused);
}
