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
  settings: { autostart: false, paused: false, wake_key: null },
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
        case "start_record":
          return null;
        case "stop_record":
          return [];
        case "get_toast_payload":
          return null;
        // 对话框（plugin:dialog）：演示模式一律返回空（用户取消）。
        case "plugin:dialog|open":
        case "plugin:dialog|save":
          return null;
        case "plugin:dialog|ask":
        case "plugin:dialog|message":
        case "plugin:dialog|confirm":
          return true;
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
