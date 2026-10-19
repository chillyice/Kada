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
      name: "切到下一个桌面",
      description: null,
      folder: null,
      layer: null,
      triggers: ["Ctrl+Alt+D"],
      enabled: true,
      actions: [{ type: "system", operation: { op: "switch_desktop", direction: "next" } }],
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
    kind: "config",
    label: "配置",
    command: "配置文件已被重新加载",
    trigger: "外部修改",
    name: "",
    stdout: "",
    stderr: "",
    exit_code: null,
    show_output: false,
    time: "2026-09-28 10:00:00",
    run_id: null,
  },
];

// 演示执行记录：一条带「条件判断 + 并行 + 失败动作」的链，让时间线（并排的泳道）与
// 流水线（分支泳道、点块展开）在纯浏览器里也能验收。字段与后端 RunRecord / ActionStep 一致。
const DEMO_RUNS = [
  {
    id: 1759046400000001,
    trigger: "Ctrl+Alt+G",
    name: "Git 提交",
    time: "2026-09-28 10:01:02",
    start_epoch_ms: 1759046462000,
    duration_ms: 820,
    status: "error",
    steps: [
      { id: 0, parent: null, lane: 0, kind: "if", title: "条件判断（仓库有改动）", branch: true, detail: { kind: "exists", path: "D:\\work\\repo\\.git" }, status: "ok", start_ms: 0, end_ms: 6, stdout: "", stderr: "", exit_code: null },
      { id: 1, parent: null, lane: 0, kind: "parallel", title: "并行执行 3 个动作", branch: null, detail: null, status: "ok", start_ms: 6, end_ms: 820, stdout: "", stderr: "", exit_code: null },
      { id: 2, parent: 1, lane: 0, kind: "command", title: "git add -A", branch: null, detail: null, status: "ok", start_ms: 8, end_ms: 240, stdout: "", stderr: "", exit_code: 0 },
      { id: 3, parent: 1, lane: 1, kind: "command", title: "npm run build", branch: null, detail: null, status: "ok", start_ms: 12, end_ms: 820, stdout: "built in 1.2s\n", stderr: "", exit_code: 0 },
      { id: 4, parent: 1, lane: 2, kind: "os", title: "删除 D:\\work\\repo\\dist\\stale", branch: null, detail: null, status: "error", start_ms: 20, end_ms: 60, stdout: "", stderr: "删除失败：「D:\\work\\repo\\dist\\stale」不存在", exit_code: null },
      { id: 5, parent: null, lane: 0, kind: "text", title: "已提交", branch: null, detail: null, status: "ok", start_ms: 821, end_ms: 890, stdout: "", stderr: "", exit_code: null },
    ],
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

// 演示鼠标录制结果（「● 录制鼠标操作」停止后注入）：移动到某处点击 / 滚轮。
const DEMO_RECORDED_MOUSE_ACTIONS = [
  { type: "mouse", op: { op: "move", dx: 120, dy: -40 } },
  { type: "mouse", op: { op: "click", button: "left" } },
  { type: "mouse", op: { op: "move", dx: -30, dy: 0 } },
  { type: "mouse", op: { op: "scroll", dx: 0, dy: -3 } },
  { type: "mouse", op: { op: "move", dx: 0, dy: 60 } },
  { type: "mouse", op: { op: "click", button: "back" } },
];

export function installBrowserMock() {
  if ("__TAURI_INTERNALS__" in window) return;
  let unread = false;
  let paused = false;
  const results = structuredClone(DEMO_RESULTS);
  const runs = structuredClone(DEMO_RUNS);
  // 提权窗口注入开关的演示态：`set_config` 存进来的设置里带上它，卡片状态跟着走
  // （浏览器里没有真提权进程，`connected` 恒 false）。
  let demoElevatedEnabled = false;
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
          demoElevatedEnabled =
            (args as { config?: { settings?: { elevated_inject?: boolean } } })?.config?.settings
              ?.elevated_inject === true;
          return [];
        case "get_command_results":
          return structuredClone(results);
        case "get_run_records":
          return structuredClone(runs);
        case "get_unread":
          return unread;
        case "mark_results_read":
          unread = false;
          return null;
        case "clear_command_results":
          results.length = 0;
          runs.length = 0;
          return null;
        case "get_conflicts":
          // 演示冲突见 DEMO_CONFLICTS（与演示配置一致），不再恒返回空。
          return structuredClone(DEMO_CONFLICTS);
        case "input_env_report":
          // 演示数据（与 DEMO_CONFLICTS 同一个理由）：浏览器里根本没有进程表可查，但设置页
          // 那张卡片的「检测到接管者」渲染路径得能在纯浏览器里验收，所以给一条假的命中项。
          return {
            supported: true,
            diverters: [
              {
                id: "logi-options-plus",
                name: "Logi Options+",
                advice:
                  "在 Options+ 里把该按键改成「无操作 / 默认功能」，它就会回到标准报文本；不想改设置就直接退出 Options+。",
              },
            ],
            checked: 8,
            hvci: true,
          };
        case "elevated_status":
          // 演示数据：按「Windows + 跟随设置开关」的样子给，设置页那张卡片的启用/未启用
          // 两态渲染路径才验得到（浏览器里没有真提权进程，start 也不会真的弹 UAC）。
          return { supported: true, enabled: demoElevatedEnabled, connected: false };
        case "elevated_start":
          return "演示模式：不会真的拉起提权进程";
        case "elevated_stop":
          return null;
        case "get_update_status":
          return { current: "0.6.0", kind: "installed", phase: "idle" };
        case "check_update":
          // 推事件让设置页看到「检查中 → 发现新版本」；否则按钮点了没反应像坏了。
          emit("update-status", { current: "0.6.0", kind: "installed", phase: "checking" });
          setTimeout(
            () =>
              emit("update-status", {
                current: "0.6.0",
                kind: "installed",
                phase: "available",
                version: "0.6.1",
                notes: "演示模式：这是一条模拟的更新说明。",
              }),
            600,
          );
          return null;
        case "download_update":
          // 下载与安装是两步：先走到 `ready`，演示模式不自动装。
          emit("update-status", {
            current: "0.6.0",
            kind: "installed",
            phase: "downloading",
            version: "0.6.1",
            percent: 0,
          });
          setTimeout(
            () =>
              emit("update-status", {
                current: "0.6.0",
                kind: "installed",
                phase: "downloading",
                version: "0.6.1",
                percent: 100,
              }),
            800,
          );
          setTimeout(
            () =>
              emit("update-status", {
                current: "0.6.0",
                kind: "installed",
                phase: "ready",
                version: "0.6.1",
              }),
            1200,
          );
          return null;
        case "install_update":
          // 真实安装会退出应用，演示模式只推「安装中」。
          emit("update-status", { current: "0.6.0", kind: "installed", phase: "installing", version: "0.6.1" });
          return null;
        case "set_paused":
          paused = !!args?.paused;
          return null;
        // 停止执行：演示模式没有真的在跑的动作，如实返回 0（前端提示「当前没有正在执行的动作」）。
        case "abort_actions":
          return 0;
// 编辑页「执行」：演示模式跑不了真动作，但把这次执行演出来——逐步推一条执行记录，
// 界面的时间线 / 流水线就能在纯浏览器里验收（真跑一遍带并行与失败成员的链）。
case "run_entry":
          demoRun();
          return null;
        // 导入：演示模式的文件对话框恒返回「取消」，走不到这里；留着是为了让 IPC 契约完整
        // （后端 ImportOutcome 的形状）并在将来放开文件选择时不至于崩在「不支持命令」。
        case "import_config":
          return { mode: args?.mode ?? "replace", added: 0, notes: [], backup: null };
        // 载入示例配置：演示模式不真改配置，回一个「已合并」的形状让结果弹窗能演出来。
        case "load_sample_config":
          return { mode: "merge", added: 0, notes: [], backup: null };
        // 导出：对话框现在返回路径（不是取消），导出必须成功收场，否则演示成「导出坏了」。
        case "export_config":
          return null;
        // 同步存档（规划 7.2-㉝）：演示模式没有真实 OneDrive 目录——探测恒返回「没找到」，
        // 推送 / 取路径返回一个假路径，好让「设置目录 → 立即同步 / 从存档恢复」的界面流程走得通。
        case "default_sync_dir":
          return null;
        case "sync_now":
          return "D:\\OneDrive\\kada-config.json";
        case "sync_import_path":
          return "D:\\OneDrive\\kada-config.json";
        // 导出卡带：回一个条数（真实实现写文件，这里只让前端把「已导出 N 项」演出来）。
        case "export_fragment":
          return (args?.picks as unknown[] | undefined)?.length ?? 0;
        // 卡带市场：演示模式没有网络，给一份样例索引 + 样例卡带，好让市场列表与安装预览能验收。
        // 卡带里特意放一条命令动作——安装预览的「会执行东西」警示就是给它看的。
        case "fetch_snippet_index":
          return [
            {
              name: "Vim 光标移动",
              description: "HJKL 移到行首 / 行尾 / 上 / 下",
              author: "Kada",
              tags: ["vim", "改键"],
              url: "https://demo.invalid/vim.json",
            },
            {
              name: "常用中文短语",
              description: "写周报 / 提 PR / 甩锅 三件套",
              author: "Kada",
              tags: ["文本扩展"],
              url: "https://demo.invalid/phrases.json",
            },
          ];
        case "fetch_snippet":
          return args?.url?.toString().includes("phrases")
            ? {
                text: JSON.stringify({ demo: true }),
                preview: {
                  shortcuts: [],
                  remaps: [],
                  expansions: [
                    { trigger: ";pr", replace: "PR 已提交，麻烦 review 一下 🙏", folder: null, enabled: true },
                    { trigger: ";ok", replace: "好的，收到", folder: null, enabled: true },
                  ],
                },
              }
            : {
                text: JSON.stringify({ demo: true }),
                preview: {
                  shortcuts: [
                    {
                      name: "打开终端",
                      folder: null,
                      layer: null,
                      triggers: ["Ctrl+Alt+T"],
                      enabled: true,
                      actions: [
                        { type: "command", shell: "cmd", command: "start wt", show_output: false, var: "" },
                      ],
                    },
                  ],
                  remaps: [
                    { from: "H", to: "Home", tap: null, hold: null, layer: null, hold_layer: null, folder: null },
                    { from: "L", to: "End", tap: null, hold: null, layer: null, hold_layer: null, folder: null },
                  ],
                  expansions: [],
                },
              };
        case "install_snippet":
          return { mode: "merge", added: 3, notes: [], backup: null };
        case "start_record":
          return null;
        case "stop_record":
          // 演示录制结果：非空，点「■ 停止」能看到录下来的动作进草稿。
          return structuredClone(DEMO_RECORDED_ACTIONS);
        case "start_mouse_record":
          return null;
        case "stop_mouse_record":
          // 演示鼠标录制结果：移动 + 点击 / 滚轮。
          return structuredClone(DEMO_RECORDED_MOUSE_ACTIONS);
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
  // 演示「手动执行」：开一条记录 → 逐步推动作块 → 收尾（与后端 fire 的推送节奏一致）。
  const demoRun = () => {
    const now = new Date();
    const pad = (n: number) => String(n).padStart(2, "0");
    const run: (typeof DEMO_RUNS)[number] = {
      id: now.getTime() * 1000,
      trigger: "手动执行",
      name: "演示宏",
      time: `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())} ${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`,
      start_epoch_ms: now.getTime(),
      duration_ms: 0,
      status: "running",
      steps: [],
    };
    const push = (run: (typeof DEMO_RUNS)[number]) => {
      const i = runs.findIndex((r) => r.id === run.id);
      if (i >= 0) runs[i] = run;
      else runs.unshift(run);
      emit("run-update", structuredClone(run));
    };
    push(run);
    const demo = DEMO_RUNS[0];
    demo.steps.forEach((s, i) => {
      setTimeout(() => {
        run.duration_ms = s.end_ms;
        // 收尾状态到最后一步才落，中途一直是 running——与后端 fire 的节奏一致。
        if (i === demo.steps.length - 1) run.status = demo.status;
        run.steps.push(structuredClone(s));
        push(run);
      }, 300 * (i + 1));
    });
  };
  console.info("[kada] 浏览器演示模式：已注入模拟 IPC，配置为演示数据。paused =", paused);
}
