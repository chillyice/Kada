// 全局可变状态。模块顶层不互相 import 对方的 `let`：所有跨模块读写都走 `state.xxx`
// （对象属性可写，import 进来的对象本身不能重新赋值）。
import { domValue, intIn } from "./dom";
import type {
  ActionDragState,
  AnyEntry,
  CommandResult,
  Config,
  Conflict,
  DragPayload,
  DragState,
  EntryTab,
  ExternalTool,
  Gesture,
  HotCornerRule,
  InputDeviceList,
  ListKind,
  Remap,
  RunRecord,
  Section,
  ShortcutItem,
  TextExpansion,
  TouchpadGesture,
  WheelEdgeRule,
} from "./types";

export const state = {
  cfg: {
    folders: [],
    layers: [],
    shortcuts: [],
    remaps: [],
    expansions: [],
    settings: {
      autostart: false,
      paused: false,
      action_timeout_ms: 30_000,
      sequence_timeout_ms: 1000,
      chord_timeout_ms: 1000,
      show_status_hud: true,
      trigger_toast: true,
      hints: { visible: false, scale: 100, opacity: 92 },
    },
  } as Config,
  section: "shortcuts" as Section,
  selected: null as number | null, // 当前列表中的选中下标
  draftNew: false, // 当前编辑是否为“新增”未保存项
  draft: null as ShortcutItem | null, // 快捷键编辑草稿（隔离，保存才写回 cfg）
  remapDraft: null as Remap | null, // 改键编辑草稿
  expansionDraft: null as TextExpansion | null, // 文本扩展编辑草稿
  gestureDraft: null as Gesture | null, // 鼠标操作（右键手势）编辑草稿
  touchpadDraft: null as TouchpadGesture | null, // 触摸板手势编辑草稿
  hotcornerDraft: null as HotCornerRule | null, // 角动作编辑草稿
  wheeledgeDraft: null as WheelEdgeRule | null, // 边缘滚动编辑草稿
  // 草稿基线：打开编辑页（或上次保存）那一刻的草稿签名。当前签名与之不等 = 有未保存改动。
  draftBaseline: null as string | null,
  // 当前停留的页签（三类条目共用一份：打开条目时回到「动作/操作/展开文本」，打开后手动切换沿用；
  // 类型没有该页签时回落「详情」）。
  entryTab: "work" as EntryTab,
  recording: false,
  mouseRecording: false,
  conflicts: [] as Conflict[],
  /** 本机键盘列表（「设备是」下拉的缓存，`null` = 还没拉过）。 */
  inputDeviceList: null as InputDeviceList | null,
  search: "",
  messages: [] as CommandResult[], // 消息中心的普通消息（配置变更、热串失败等；执行记录另有表）
  runs: [] as RunRecord[], // 执行记录（最新在前）：一次触发一条
  msgIndex: 0, // 当前选中的记录 tab（在 msgEntries() 的合成列表里）
  msgSelId: "", // 当前选中的记录 id（跟着 tab 走，重绘不丢选中）
  msgMode: "pipeline" as "timeline" | "pipeline", // 记录卡片的两种展示方式
  msgSelStep: -1, // 时间线上选中的动作块（详情显示在下方）
  unread: false, // 未读红点
  conflictBubbleDismissed: false, // 快捷键列表里的冲突气泡是否已被用户消除
  msgView: "results" as "results" | "conflicts", // 消息页标签：执行记录 / 冲突
  // 复制/剪切板：存对象引用而非下标（增删/重排后仍指对那条），多选整批进同一份。
  // ponytail: 条目在编辑页「保存」时整条换新对象，这里的旧引用会失效（剪切粘贴到的是旧内容）——
  // 需要跨编辑保真时改成 kind + 下标并按内容重定位。
  clipboard: null as { items: { kind: ListKind; obj: AnyEntry }[]; cut: boolean } | null,
  // 列表筛选（7.3-⑳）：与 search 叠加生效，从搜索栏右侧「筛选」菜单开关。
  listFilter: { enabled: false, conflicted: false },
  // 多选集（Ctrl / Shift 点行，右键菜单作用于它）：存对象引用而非下标；切分区清空。
  picked: new Set<object>(),
  pickAnchor: null as object | null, // Shift 范围选的锚点（上次单击 / Ctrl 点的那条）
  menuAnchor: null as HTMLElement | null, // 当前打开的下拉菜单锚点（再点同一按钮 = 收起）
  editingFolderId: null as string | null, // 正在行内改名的目录 id
  editingLayerId: null as string | null, // 正在行内改名的层 id
  // pointer events 拖拽状态（不依赖 WebView2 的 HTML5 DnD，避免其兼容性抖动）。
  dragPayload: null as DragPayload | null, // 拖拽源
  dragOverEl: null as HTMLElement | null, // 当前高亮目标（用于清 class）
  dragState: null as DragState | null,
  suppressClick: false, // 拖拽结束后抑制一次 click，避免误触「打开详情/折叠」
  actionDrag: null as ActionDragState | null,
  /** 本机正在运行的外部工具（后端探测进程名；动作类型里据此出现对应能力）。 */
  runningTools: [] as ExternalTool[],
};

// ---- 未保存改动守卫的草稿标记 ----
// 编辑草稿是 cfg 的副本（openDetail 时 deepClone / 新建模板），所以「改没改过」没法靠引用比对，
// 只能与基线比对：draftBaseline 记下打开编辑页（或上次保存）那一刻的草稿签名。
// 签名里必须带上「只在保存时才读回草稿」的那些输入框——描述 / 所属层、改键的原键与长按阈值、
// 文本扩展的触发词与替换文本——否则改这些字段脏标记不亮，离开时照样静默丢。

export function draftSignature(): string | null {
  if (state.section === "shortcuts") {
    if (!state.draft) return null;
    return JSON.stringify({
      ...state.draft,
      description: domValue("edit-desc") || null,
      layer: domValue("edit-layer") || null,
    });
  }
  if (state.section === "remaps") {
    if (!state.remapDraft) return null;
    return JSON.stringify({
      ...state.remapDraft,
      description: domValue("edit-desc") || null,
      from: domValue("remap-from"),
      to: domValue("remap-to"),
      layer: domValue("remap-layer") || null,
      tap_timeout_ms: intIn(domValue("remap-timeout"), 50, 200, 2000),
    });
  }
  if (state.section === "expansions") {
    if (!state.expansionDraft) return null;
    return JSON.stringify({
      ...state.expansionDraft,
      description: domValue("edit-desc") || null,
      trigger: domValue("edit-exp-trigger").trim(),
      replace: domValue("edit-exp-replace"),
    });
  }
  if (state.section === "gestures") {
    if (!state.gestureDraft) return null;
    return JSON.stringify({
      ...state.gestureDraft,
      description: domValue("edit-desc") || null,
      pattern: domValue("edit-gesture-pattern").trim().toUpperCase(),
    });
  }
  if (state.section === "touchpads") {
    if (!state.touchpadDraft) return null;
    return JSON.stringify({
      ...state.touchpadDraft,
      description: domValue("edit-desc") || null,
      trigger: domValue("edit-touchpad-trigger") || null,
      pattern: domValue("edit-touchpad-pattern").trim().toUpperCase(),
    });
  }
  if (state.section === "hotcorners")
    return state.hotcornerDraft
      ? JSON.stringify({
          ...state.hotcornerDraft,
          description: domValue("edit-desc") || null,
          layer: domValue("hotcorner-layer") || null,
        })
      : null;
  if (state.section === "wheeledges")
    return state.wheeledgeDraft
      ? JSON.stringify({
          ...state.wheeledgeDraft,
          description: domValue("edit-desc") || null,
          layer: domValue("wheeledge-layer") || null,
        })
      : null;
  return null;
}

// 基线为 null = 没有草稿（或正在填充），一律不算脏——编辑内容清空时签名也是 null，
// 不这么判会把「草稿没了」当成「有改动」。
export function isDraftDirty(): boolean {
  return state.draftBaseline !== null && draftSignature() !== state.draftBaseline;
}
