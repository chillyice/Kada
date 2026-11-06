import { invoke } from "@tauri-apps/api/core";
import { open as openFile, save as saveFile } from "@tauri-apps/plugin-dialog";
import { applyLocale, savedLocale, tr, trt } from "./i18n";
import { el, onClickAsync, toast } from "./dom";
import { state } from "./state";
import { DEFAULT_SNIPPET_INDEX_URL, bindMarket, resetMarketCache } from "./market";
import { actionFields, actionSubtypeSelect, convertedAction } from "./form";
import { actionTypeChoices, newAction, unwrapAction } from "./options";
import {
  THEME_KEY,
  applyTheme,
  loadTheme,
  render,
  load,
  save,
} from "./main";
import type {
  ActionBase,
  ElevatedStatus,
  HidppStatusView,
  HidppUnlockView,
  InputEnvReport,
  MenuEntry,
  UpdateStatus,
} from "./types";

// 设置页 + 软件更新 + 更新对话框（U-03 从 main.ts 搬出）。
// 「设置」是一个一级页：外观 / 通用行为 / 输入与响应 / 配置与数据 / 软件更新五张卡片。
// 这里只管这一页的填充（syncSettings）、事件绑定（bindSettings）与它自己的弹窗（导入方式、
// 导入结果、更新说明 / 下载询问）。左侧列表与编辑页的编排仍在入口。

// ---- 设置 ----
/** 超时（毫秒）→ 界面上的秒数（配置里存毫秒，界面按秒录入；0 = 不限）。 */
function timeoutSeconds(ms: number): number {
  return Math.round((Number(ms) || 0) / 1000);
}

/** 等待窗（毫秒）→ 输入框文本（界面按毫秒录入；0 = 不限）。缺字段的老配置按默认 1000 显示。 */
function timeoutMs(ms: number | undefined): string {
  const n = Number(ms);
  return String(ms === undefined || !Number.isFinite(n) || n < 0 ? 1000 : Math.floor(n));
}

export function syncSettings() {
  (document.getElementById("set-autostart") as HTMLInputElement).checked = state.cfg.settings.autostart;
  (document.getElementById("set-paused") as HTMLInputElement).checked = state.cfg.settings.paused;
  // 缺字段按「关」显示，与后端 serde default 一致。
  (document.getElementById("set-ignore-fullscreen") as HTMLInputElement).checked =
    state.cfg.settings.ignore_fullscreen === true;
  (document.getElementById("set-wake-key") as HTMLSelectElement).value = state.cfg.settings.wake_key ?? "";
  (document.getElementById("set-action-timeout") as HTMLInputElement).value = String(
    timeoutSeconds(state.cfg.settings.action_timeout_ms),
  );
  (document.getElementById("set-sequence-timeout") as HTMLInputElement).value = timeoutMs(
    state.cfg.settings.sequence_timeout_ms,
  );
  (document.getElementById("set-chord-timeout") as HTMLInputElement).value = timeoutMs(
    state.cfg.settings.chord_timeout_ms,
  );
  // 缺字段的老配置（写在该设置出现之前）按「开」显示，与后端 `default_true` 一致。
  (document.getElementById("set-status-hud") as HTMLInputElement).checked =
    state.cfg.settings.show_status_hud !== false;
  // 提示框默认「关」（后端 `HintsWindow::default` 也是 false）：老配置缺 `hints` 时显示为未勾选。
  (document.getElementById("set-hints") as HTMLInputElement).checked =
    state.cfg.settings.hints?.visible === true;
  // 触发提醒缺字段按「开」（与后端 `default_true` 一致）。
  (document.getElementById("set-trigger-toast") as HTMLInputElement).checked =
    state.cfg.settings.trigger_toast !== false;
  // 缺字段的老配置按默认「剪贴板粘贴」显示，与后端 serde default 一致。
  (document.getElementById("set-text-inject") as HTMLSelectElement).value =
    state.cfg.settings.text_inject_mode ?? "clipboard";
  // 索引地址：只显示自定义值；内置默认 / 空串都显示为空（后端把空串当默认）。
  // 老配置没有这个字段时同样显示空 = 用内置默认，别把内置 URL 摆给用户看。
  const snippetIndexValue = state.cfg.settings.snippet_index_url ?? "";
  (document.getElementById("set-snippet-index") as HTMLInputElement).value =
    snippetIndexValue === DEFAULT_SNIPPET_INDEX_URL ? "" : snippetIndexValue;
  // 同步存档目录：未设置时给一句引导语，设置后显示完整路径。
  document.getElementById("sync-dir")!.textContent =
    state.cfg.settings.sync_dir?.trim() || tr("未设置 —— 选一个文件夹（如 OneDrive）即可自动存一份");
  // 输入层环境诊断：缺字段的老配置按「开」显示，与后端 `default_true` 一致；
  // 结论每次进设置页重算（探测在后端，有 3 秒缓存，不构成负担）。
  (document.getElementById("set-input-env") as HTMLInputElement).checked =
    state.cfg.settings.input_env_diag !== false;
  void renderInputEnvReport();
  // 鼠标手势的实验开关（规划 7.2-MG）：缺字段按「关」显示，与后端 serde default 一致
  // （新功能默认关——它带「全局抢右键」的副作用，替用户打开不合适）。
  (document.getElementById("set-experimental") as HTMLInputElement).checked =
    state.cfg.settings.experimental === true;
  // 触摸板手势是**独立分类**：自己的开关与参数，跟上面那枚无关（缺字段 = 关 / 默认值）。
  (document.getElementById("set-touchpad") as HTMLInputElement).checked =
    state.cfg.settings.touchpad === true;
  (document.getElementById("set-touchpad-start-distance") as HTMLInputElement).value = String(
    state.cfg.settings.touchpad_start_distance ?? 22,
  );
  (document.getElementById("set-touchpad-exclude") as HTMLTextAreaElement).value = (
    state.cfg.settings.touchpad_exclude ?? []
  ).join("\n");
  // 缺字段按「开」显示（后端 serde default 就是 true）。
  (document.getElementById("set-touchpad-restore-cursor") as HTMLInputElement).checked =
    state.cfg.settings.touchpad_restore_cursor !== false;
  (document.getElementById("set-touchpad-overlay") as HTMLInputElement).checked =
    state.cfg.settings.touchpad_overlay !== false;
  (document.getElementById("set-gesture-screenshot") as HTMLInputElement).checked =
    state.cfg.settings.gesture_screenshot !== false;
  (document.getElementById("set-gesture-exclude") as HTMLTextAreaElement).value =
    (state.cfg.settings.gesture_exclude ?? []).join("\n");
  (document.getElementById("set-exclude-apps") as HTMLTextAreaElement).value =
    (state.cfg.settings.exclude_apps ?? []).join("\n");
  // 手势参数（EXT-01）：缺字段按后端 serde default 显示，避免老配置显示成 0。
  (document.getElementById("set-gesture-start-distance") as HTMLInputElement).value = String(
    state.cfg.settings.gesture_start_distance ?? 22,
  );
  (document.getElementById("set-gesture-timeout") as HTMLInputElement).value = String(
    state.cfg.settings.gesture_timeout_ms ?? 1500,
  );
  (document.getElementById("set-gesture-min-segment") as HTMLInputElement).value = String(
    state.cfg.settings.gesture_min_segment ?? 16,
  );
  // 手势视觉（EXT-02）：颜色用取色器，数值同样回落后端默认。
  (document.getElementById("set-gesture-trail-color") as HTMLInputElement).value =
    state.cfg.settings.gesture_trail_color ?? "#4c9aff";
  (document.getElementById("set-gesture-trail-width") as HTMLInputElement).value = String(
    state.cfg.settings.gesture_trail_width ?? 5,
  );
  (document.getElementById("set-gesture-label-font-size") as HTMLInputElement).value = String(
    state.cfg.settings.gesture_label_font_size ?? 14,
  );
  (document.getElementById("set-gesture-label-offset") as HTMLInputElement).value = String(
    state.cfg.settings.gesture_label_offset ?? 16,
  );
  (document.getElementById("set-gesture-fail-color") as HTMLInputElement).value =
    state.cfg.settings.gesture_fail_color ?? "#ff5f56";
  // 提权窗口注入：缺字段的老配置按「关」显示（后端 serde default 就是 false）；状态每次都
  // 问一遍后端（提权进程可能已经退出，这个值会变）。
  (document.getElementById("set-elevated") as HTMLInputElement).checked =
    state.cfg.settings.elevated_inject === true;
  void renderElevatedReport();
  // 软件更新源：缺字段按默认 ihomy 显示（后端 serde default 一致）。
  (document.getElementById("set-update-source") as HTMLSelectElement).value =
    state.cfg.settings.update_source ?? "ihomy";
  (document.getElementById("set-auto-update") as HTMLInputElement).checked =
    state.cfg.settings.auto_update === true;
  // 复制快捷菜单（双击 Ctrl+C）已删除，设置卡不再渲染。
  // 旧配置里残留的条目仍能只读渲染（没有宿主节点时函数直接返回），不再提供编辑入口。
  renderClipboardMenuEditor();
  syncMouseOpts();
  syncTouchpadOpts();
}

/** 鼠标操作的参数设置块只在总开关打开时显示（关闭时配置仍保留）。 */
export function syncMouseOpts() {
  document
    .getElementById("mouse-opts")
    ?.classList.toggle("hidden", state.cfg.settings.experimental !== true);
  syncGestureVisual();
}

/** 触摸板手势的参数块只在自己的开关打开时显示（与「鼠标操作」那枚互不影响）。 */
export function syncTouchpadOpts() {
  document
    .getElementById("touchpad-opts")
    ?.classList.toggle("hidden", state.cfg.settings.touchpad !== true);
  syncGestureVisual();
}

/** 轨迹浮层是两边共用的一块窗：任一边开着就显示，两边都关着才整张卡片收起。 */
function syncGestureVisual() {
  document
    .getElementById("gesture-visual-card")
    ?.classList.toggle(
      "hidden",
      state.cfg.settings.experimental !== true && state.cfg.settings.touchpad !== true,
    );
}

/**
 * 重画「提权窗口注入」卡片的结论区（后端 `elevated_status`，规划 7.2-㉑a）。
 *
 * 三种结论分开说（见 [`ElevatedStatus`]）：本平台不支持 / 开关没开 / 开关开着但提权进程
 * 没连上。按钮跟着状态走——没起来时是「启动提权进程」（点了会弹 UAC），连上后是「断开」。
 */
async function renderElevatedReport() {
  const box = document.getElementById("elevated-report")!;
  const run = document.getElementById("elevated-toggle-run") as HTMLButtonElement;
  let s: ElevatedStatus;
  try {
    s = await invoke<ElevatedStatus>("elevated_status");
  } catch (e) {
    box.textContent = trt`状态读取失败：${e}`;
    run.hidden = true;
    return;
  }
  box.replaceChildren();
  const line = (text: string) => box.append(el("div", "env-line", text));
  if (!s.supported) {
    run.hidden = true;
    line(tr("当前平台不支持（这是 Windows 专属能力：它的对手是 UIPI）。"));
    return;
  }
  const enabled = state.cfg.settings.elevated_inject === true;
  run.hidden = false;
  run.textContent = s.connected ? tr("断开提权进程") : tr("启动提权进程");
  if (!enabled) {
    // 开关关着时按钮没有意义（就算拉起来，注入也不会走它）——禁用而不是隐藏，
    // 让「先开开关」这件事看得见。
    run.disabled = true;
    run.title = tr("先打开上面的「提权窗口也生效」");
    line(tr("未启用。打开上面的开关后点「启动提权进程」（会弹一次 UAC 授权）。"));
    return;
  }
  run.disabled = false;
  run.title = "";
  if (s.connected) {
    line(tr("已连接：提权进程在运行，提权窗口里的注入会交给它。"));
  } else {
    line(tr("开关已打开，但提权进程没在运行——点右上角「启动提权进程」拉起它。"));
  }
  line(tr("它平时不运行：关掉开关、或退出 Kada，它就跟着退出，不会留在系统里。"));
}

/**
 * 重画「输入层环境」卡片的结论区（后端 `input_env_report`，规划 7.2-㊽）。
 *
 * 三种结论必须分开说，不能含糊成一句「没问题」：本平台没做这项探测 / 已关掉探测 /
 * 探测了 N 类但都没发现；只有第一种是能力缺口，后两种才是真的「干净」。
 */
async function renderInputEnvReport() {
  const box = document.getElementById("input-env-report")!;
  let r: InputEnvReport;
  try {
    r = await invoke<InputEnvReport>("input_env_report", { config: state.cfg });
  } catch (e) {
    box.textContent = trt`检测失败：${e}`;
    return;
  }
  // 罗技 HID++ 解锁只在 Windows 有实现（档位 C）；其余平台整块不显示，
  // 免得点了才发现「本平台不支持」。
  document.getElementById("hidpp-box")!.hidden = !r.hidpp_supported;
  box.replaceChildren();
  const line = (text: string) => box.append(el("div", "env-line", text));
  if (!r.supported) {
    line(tr("当前平台暂不支持这项检测。"));
    return;
  }
  if (state.cfg.settings.input_env_diag === false) {
    line(tr("已关闭。开启后 Kada 才会检测会接管按键的厂商软件。"));
    return;
  }
  if (r.diverters.length === 0) {
    line(trt`未检测到会接管按键的厂商软件（已查 ${r.checked} 类）。`);
  } else {
    line(trt`检测到 ${r.diverters.length} 个可能接管按键的软件：`);
    for (const d of r.diverters) {
      const item = el("div", d.ignored ? "env-hit ignored" : "env-hit");
      const head = el("div", "env-hit-head");
      head.append(el("div", "env-hit-name", d.ignored ? trt`${d.name}（已忽略提示）` : d.name));
      head.append(inputEnvIgnoreButton(d));
      item.append(head);
      item.append(el("div", "env-hit-advice", d.advice));
      box.append(item);
    }
  }
  // 内存完整性：只报状态、不当问题说——它拦的是内核驱动，不影响 Kada 现在的用户态钩子。
  if (r.hvci === true) line(tr("系统「内存完整性」已开启（不影响 Kada 当前的工作方式，仅作记录）。"));
  else if (r.hvci === false) line(tr("系统「内存完整性」未开启。"));
  else line(tr("系统「内存完整性」状态读不到（该功能未配置，或系统未提供）。"));
}

/**
 * 命中项上的「忽略 / 恢复提示」按钮（规划 7.2-㊽ 余项②）。
 *
 * 粒度是**单个软件**，不是总开关：某个接管者在你这台机器上确实不影响用时，掐掉这一条
 * 就行，不必把整条诊断链关掉（后者连别的接管者也不报了）。
 */
function inputEnvIgnoreButton(d: InputEnvReport["diverters"][number]): HTMLButtonElement {
  const btn = el("button", "ghost env-hit-ignore", d.ignored ? tr("恢复提示") : tr("忽略")) as HTMLButtonElement;
  btn.title = d.ignored
    ? tr("恢复这条提示：它重新出现在冲突检测里")
    : tr("不再为这个软件提示（它仍在下方列表里，可随时恢复）");
  btn.addEventListener("click", () => void toggleInputEnvIgnore(d.id, !d.ignored));
  return btn;
}

/** 记住 / 撤销对某个接管者的忽略，落盘后重画结论区（冲突检测也随配置一起更新）。 */
async function toggleInputEnvIgnore(id: string, ignore: boolean) {
  const rest = (state.cfg.settings.input_env_ignored ?? []).filter((x) => x !== id);
  state.cfg.settings.input_env_ignored = ignore ? [...rest, id] : rest;
  await save();
  await renderInputEnvReport();
}

/**
 * 只读扫描罗技 HID++ 设备（`docs/底层输入能力规划.md` 档位 C）。
 *
 * 只在用户点按钮时跑：枚举要开 HID 集合、每个槽位还要等超时，不能拖慢设置页首屏。
 */
async function renderHidppStatus() {
  const box = document.getElementById("hidpp-report")!;
  box.textContent = tr("正在扫描罗技设备…");
  let s: HidppStatusView;
  try {
    s = await invoke<HidppStatusView>("hidpp_status");
  } catch (e) {
    box.textContent = trt`扫描失败：${e}`;
    return;
  }
  box.replaceChildren();
  const line = (text: string) => box.append(el("div", "env-line", text));
  if (!s.supported) {
    line(tr("当前平台不支持（只有 Windows 能直接解锁罗技侧键）。"));
    return;
  }
  if (s.error) line(s.error);
  if (s.devices.length === 0) {
    line(tr("没有找到可用的罗技 HID++ 设备（没接罗技设备，或控制接口被独占）。"));
  }
  for (const d of s.devices) {
    const item = el("div", "env-hit");
    item.append(el("div", "env-hit-name", d.name));
    item.append(
      el(
        "div",
        "env-hit-advice",
        trt`HID++ 索引 ${d.index === 255 ? tr("直连") : d.index} · 控件 ${d.control_count} 个 · ${d.channel} 报文`,
      ),
    );
    for (const c of d.controls) {
      const state = !c.present
        ? tr("设备上没有")
        : c.diverted
          ? c.persistent
            ? tr("被厂商软件接管（持久）")
            : tr("被厂商软件接管")
          : tr("标准报文");
      item.append(el("div", "env-hit-advice", `${c.label}：${state}`));
    }
    box.append(item);
  }
  if (s.keeper_active) line(tr("本次运行期间保持解锁中（设备重连后会自动再关）。"));
  if (s.keeper_error) line(trt`保持解锁的最近一次错误：${s.keeper_error}`);
}

/**
 * 解锁罗技侧键：把中键 / 后退 / 前进里处于 divert 的关掉，让它们回到标准报文。
 *
 * 后端同时会往消息中心推一条结论（用户点完按钮多半会离开设置页，结论不能只留在这里）。
 */
async function unlockHidpp() {
  const box = document.getElementById("hidpp-report")!;
  const btn = document.getElementById("hidpp-unlock") as HTMLButtonElement;
  btn.disabled = true;
  box.textContent = tr("正在解锁（读取并写入罗技控制接口）…");
  let r: HidppUnlockView;
  try {
    r = await invoke<HidppUnlockView>("hidpp_unlock");
  } catch (e) {
    box.textContent = trt`解锁失败：${e}`;
    btn.disabled = false;
    return;
  }
  btn.disabled = false;
  box.replaceChildren();
  const line = (text: string) => box.append(el("div", "env-line", text));
  if (r.error) {
    line(trt`解锁失败：${r.error}`);
    line(tr("可以在 Options+ / G HUB 里把该按键设为「无操作 / 默认功能」，或直接退出它。"));
    return;
  }
  if (r.devices.length === 0) {
    line(tr("没有找到罗技设备。"));
    return;
  }
  for (const d of r.devices) {
    const item = el("div", "env-hit");
    item.append(el("div", "env-hit-name", d.name));
    if (d.cleared.length) item.append(el("div", "env-hit-advice", trt`已解锁：${d.cleared.join("、")}`));
    if (d.already.length) item.append(el("div", "env-hit-advice", trt`本来就是标准报文：${d.already.join("、")}`));
    if (d.missing.length) item.append(el("div", "env-hit-advice", trt`设备上没有：${d.missing.join("、")}`));
    for (const f of d.failed) item.append(el("div", "env-hit-advice", trt`失败：${f}`));
    box.append(item);
  }
  line(
    r.keeping
      ? tr("已生效，本次运行期间会保持解锁（设备重连或厂商软件重启后自动再关）。")
      : tr("没有需要解锁的键：钩子已经能收到这些按键。"),
  );
}

export function bindSettings() {
  // 主题不进配置（见 THEME_KEY 处注释），所以自己读写 localStorage + 直接打 data-theme。
  const theme = document.getElementById("set-theme") as HTMLSelectElement;
  theme.value = loadTheme();
  theme.addEventListener("change", () => {
    localStorage.setItem(THEME_KEY, theme.value);
    applyTheme(theme.value);
  });
  // 界面语言：只记本机（localStorage），切完整页 reload——静态文案与已渲染的界面一起换掉。
  const lang = document.getElementById("set-lang") as HTMLSelectElement;
  lang.value = savedLocale() ?? "system";
  lang.addEventListener("change", () => {
    applyLocale(lang.value === "system" ? null : (lang.value as "zh" | "en"));
    location.reload();
  });
  const autostart = document.getElementById("set-autostart") as HTMLInputElement;
  const paused = document.getElementById("set-paused") as HTMLInputElement;
  const wakeKey = document.getElementById("set-wake-key") as HTMLSelectElement;
  const actionTimeout = document.getElementById("set-action-timeout") as HTMLInputElement;
  const sequenceTimeout = document.getElementById("set-sequence-timeout") as HTMLInputElement;
  const chordTimeout = document.getElementById("set-chord-timeout") as HTMLInputElement;
  const statusHud = document.getElementById("set-status-hud") as HTMLInputElement;
  const hints = document.getElementById("set-hints") as HTMLInputElement;
  autostart.addEventListener("change", () => {
    state.cfg.settings.autostart = autostart.checked;
    void save();
  });
  paused.addEventListener("change", () => {
    state.cfg.settings.paused = paused.checked;
    void save();
  });
  // 全屏程序里暂停（EXT-05）：只是行为开关，配置照常保存。
  const ignoreFullscreen = document.getElementById("set-ignore-fullscreen") as HTMLInputElement;
  ignoreFullscreen.addEventListener("change", () => {
    state.cfg.settings.ignore_fullscreen = ignoreFullscreen.checked;
    void save();
  });
  wakeKey.addEventListener("change", () => {
    state.cfg.settings.wake_key = wakeKey.value || null;
    void save();
  });
  actionTimeout.addEventListener("change", () => {
    // 负值 / 非数字按 0（不限时）处理：后端只认 u64，宁可放开也不静默变成 1 秒把人坑了。
    const secs = Math.max(0, Math.floor(Number(actionTimeout.value) || 0));
    actionTimeout.value = String(secs);
    state.cfg.settings.action_timeout_ms = secs * 1000;
    void save();
  });
  // 两个等待窗按毫秒录入（默认 1000）：负数 / 非数字同样按 0（不限时）处理。
  sequenceTimeout.addEventListener("change", () => {
    const ms = Math.max(0, Math.floor(Number(sequenceTimeout.value) || 0));
    sequenceTimeout.value = String(ms);
    state.cfg.settings.sequence_timeout_ms = ms;
    void save();
  });
  chordTimeout.addEventListener("change", () => {
    const ms = Math.max(0, Math.floor(Number(chordTimeout.value) || 0));
    chordTimeout.value = String(ms);
    state.cfg.settings.chord_timeout_ms = ms;
    void save();
  });
  statusHud.addEventListener("change", () => {
    state.cfg.settings.show_status_hud = statusHud.checked;
    void save();
  });
  const triggerToast = document.getElementById("set-trigger-toast") as HTMLInputElement;
  triggerToast.addEventListener("change", () => {
    state.cfg.settings.trigger_toast = triggerToast.checked;
    void save();
  });
  // 提示框的开关：改动随整份配置落盘，后端 `set_config` 比对到 `visible` 变了就把窗口
  // 建出来 / 收起来，并把托盘菜单的勾对齐（见 `apply_hints_visible`）。
  hints.addEventListener("change", () => {
    state.cfg.settings.hints = { ...(state.cfg.settings.hints ?? { scale: 100, opacity: 92 }), visible: hints.checked };
    void save();
  });
  const textInject = document.getElementById("set-text-inject") as HTMLSelectElement;
  textInject.addEventListener("change", () => {
    state.cfg.settings.text_inject_mode = textInject.value as "clipboard" | "unicode";
    void save();
  });
  // 索引地址：留空 = 内置默认（后端把空串也当默认，别让人以为索引被关掉了）。
  const snippetIndex = document.getElementById("set-snippet-index") as HTMLInputElement;
  snippetIndex.addEventListener("change", () => {
    state.cfg.settings.snippet_index_url = snippetIndex.value.trim();
    // 换了索引地址，下次进市场页必须重新拉，别把旧索引继续挂在列表里。
    resetMarketCache();
    void save();
  });
  // 输入层环境诊断（规划 7.2-㊽）：关掉即「整条链路静默」——后端连探测都不做
  // （`get_conflicts` / `input_env_report` 都先看这个开关），所以改完要重画结论区。
  const inputEnv = document.getElementById("set-input-env") as HTMLInputElement;
  inputEnv.addEventListener("change", () => {
    state.cfg.settings.input_env_diag = inputEnv.checked;
    void save();
    void renderInputEnvReport();
  });
  // 鼠标操作总开关（规划 7.2-MG）：关掉后鼠标类功能一律不可用——**配置保留**
  // （只是入口隐藏 + 行为不生效），所以这里只改开关，不动 cfg.gestures。
  const experimental = document.getElementById("set-experimental") as HTMLInputElement;
  experimental.addEventListener("change", () => {
    state.cfg.settings.experimental = experimental.checked;
    syncMouseOpts();
    // 改完重画列表：快捷键页里鼠标类条目的显隐由它决定。
    void save().then(() => render());
  });
  // 触摸板手势总开关：与「鼠标操作」完全分开——只影响触摸板条目与采集，
  // 关掉时配置保留（入口隐藏 + 不采集，触发键照常输出）。
  const touchpad = document.getElementById("set-touchpad") as HTMLInputElement;
  touchpad.addEventListener("change", () => {
    state.cfg.settings.touchpad = touchpad.checked;
    syncTouchpadOpts();
    void save().then(() => render());
  });
  // 触摸板手势的起手距离（像素）：这一笔至少移动这么远才算数，钳在 1~50。
  const touchpadStart = document.getElementById(
    "set-touchpad-start-distance",
  ) as HTMLInputElement;
  touchpadStart.addEventListener("change", () => {
    const v = Math.min(50, Math.max(1, Math.floor(Number(touchpadStart.value) || 22)));
    touchpadStart.value = String(v);
    state.cfg.settings.touchpad_start_distance = v;
    void save();
  });
  // 触摸板手势的排除程序：每行一个，与鼠标手势的 `gesture_exclude` 各自一份名单。
  const touchpadExclude = document.getElementById(
    "set-touchpad-exclude",
  ) as HTMLTextAreaElement;
  touchpadExclude.addEventListener("change", () => {
    state.cfg.settings.touchpad_exclude = touchpadExclude.value
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);
    void save();
  });
  // 松手后光标回起笔点：关掉 = 光标留在这一笔走到的位置（有人还要接着用指针）。
  const touchpadRestore = document.getElementById(
    "set-touchpad-restore-cursor",
  ) as HTMLInputElement;
  touchpadRestore.addEventListener("change", () => {
    state.cfg.settings.touchpad_restore_cursor = touchpadRestore.checked;
    void save();
  });
  // 采集时显示轨迹浮层：只影响触摸板这条来源（右键手势没有单独开关）。
  const touchpadOverlay = document.getElementById("set-touchpad-overlay") as HTMLInputElement;
  touchpadOverlay.addEventListener("change", () => {
    state.cfg.settings.touchpad_overlay = touchpadOverlay.checked;
    void save();
  });
  const gestureScreenshot = document.getElementById(
    "set-gesture-screenshot",
  ) as HTMLInputElement;
  gestureScreenshot.addEventListener("change", () => {
    state.cfg.settings.gesture_screenshot = gestureScreenshot.checked;
    void save();
  });
  // 手势排除程序：每行一个（后端清洗会 trim 并丢空行），失焦时落盘。
  const gestureExclude = document.getElementById("set-gesture-exclude") as HTMLTextAreaElement;
  gestureExclude.addEventListener("change", () => {
    state.cfg.settings.gesture_exclude = gestureExclude.value
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);
    void save();
  });
  // 全局排除程序（EXT-04）：命中的程序里所有触发都不生效，与手势专用列表独立。
  const excludeApps = document.getElementById("set-exclude-apps") as HTMLTextAreaElement;
  excludeApps.addEventListener("change", () => {
    state.cfg.settings.exclude_apps = excludeApps.value
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);
    void save();
  });
  // 手势参数（EXT-01）：三个数都是「手改 JSON 也允许」的值，这里按界面建议范围钳一下，
  // 免得存进去一个 0 把识别弄坏。失焦 / 回车时落盘。
  const gestureStart = document.getElementById("set-gesture-start-distance") as HTMLInputElement;
  gestureStart.addEventListener("change", () => {
    const v = Math.min(50, Math.max(1, Math.floor(Number(gestureStart.value) || 22)));
    gestureStart.value = String(v);
    state.cfg.settings.gesture_start_distance = v;
    void save();
  });
  const gestureTimeout = document.getElementById("set-gesture-timeout") as HTMLInputElement;
  gestureTimeout.addEventListener("change", () => {
    const v = Math.min(3000, Math.max(500, Math.floor(Number(gestureTimeout.value) || 1500)));
    gestureTimeout.value = String(v);
    state.cfg.settings.gesture_timeout_ms = v;
    void save();
  });
  const gestureMinSegment = document.getElementById("set-gesture-min-segment") as HTMLInputElement;
  gestureMinSegment.addEventListener("change", () => {
    const v = Math.min(40, Math.max(4, Math.floor(Number(gestureMinSegment.value) || 16)));
    gestureMinSegment.value = String(v);
    state.cfg.settings.gesture_min_segment = v;
    void save();
  });
  // 手势视觉（EXT-02）：颜色是 `#rrggbb`，数值范围与后端 sanitize 一致。
  const gestureTrailColor = document.getElementById("set-gesture-trail-color") as HTMLInputElement;
  gestureTrailColor.addEventListener("change", () => {
    state.cfg.settings.gesture_trail_color = gestureTrailColor.value;
    void save();
  });
  const gestureTrailWidth = document.getElementById("set-gesture-trail-width") as HTMLInputElement;
  gestureTrailWidth.addEventListener("change", () => {
    const v = Math.min(20, Math.max(1, Math.floor(Number(gestureTrailWidth.value) || 5)));
    gestureTrailWidth.value = String(v);
    state.cfg.settings.gesture_trail_width = v;
    void save();
  });
  const gestureLabelFont = document.getElementById(
    "set-gesture-label-font-size",
  ) as HTMLInputElement;
  gestureLabelFont.addEventListener("change", () => {
    const v = Math.min(48, Math.max(8, Math.floor(Number(gestureLabelFont.value) || 14)));
    gestureLabelFont.value = String(v);
    state.cfg.settings.gesture_label_font_size = v;
    void save();
  });
  const gestureLabelOffset = document.getElementById(
    "set-gesture-label-offset",
  ) as HTMLInputElement;
  gestureLabelOffset.addEventListener("change", () => {
    const v = Math.min(200, Math.max(0, Math.floor(Number(gestureLabelOffset.value) || 16)));
    gestureLabelOffset.value = String(v);
    state.cfg.settings.gesture_label_offset = v;
    void save();
  });
  const gestureFailColor = document.getElementById("set-gesture-fail-color") as HTMLInputElement;
  gestureFailColor.addEventListener("change", () => {
    state.cfg.settings.gesture_fail_color = gestureFailColor.value;
    void save();
  });
  // 复制快捷菜单（双击 Ctrl+C）已删除：不再有设置项与条目编辑器。
  // 「重新检测」：结论本身在后端有 3 秒缓存，这里绕开不了它，但连点几次足以跨过缓存
  // ——退出厂商软件后想让结论立刻翻篇，这是最省事的手动刷新入口。
  document
    .getElementById("input-env-refresh")!
    .addEventListener("click", () => void renderInputEnvReport());
  // 罗技 HID++ 解锁（档位 C）：扫描是只读的，解锁会写设备（可随时被厂商软件改回）。
  document.getElementById("hidpp-scan")!.addEventListener("click", () => void renderHidppStatus());
  document.getElementById("hidpp-unlock")!.addEventListener("click", () => void unlockHidpp());
  // 提权窗口注入（规划 7.2-㉑a）：**开关只写配置**，拉起 / 断开是按钮的动作——开关本身
  // 不弹 UAC（用户点「启动」才弹），关掉时后端会顺带断开通道，提权进程随之退出。
  const elevated = document.getElementById("set-elevated") as HTMLInputElement;
  elevated.addEventListener("change", () => {
    state.cfg.settings.elevated_inject = elevated.checked;
    void save();
    void renderElevatedReport();
  });
  // 启动 / 断开：这一下会弹 UAC（用户在等授权，界面上要说清楚），且后端在等提权进程把通道
  // 建起来（最长 30 秒）。所以按键就地禁用 + 写一句进度，别让人以为界面卡死了。
  const elevatedRun = document.getElementById("elevated-toggle-run") as HTMLButtonElement;
  onClickAsync(elevatedRun, async () => {
    const box = document.getElementById("elevated-report")!;
    const wasConnected = elevatedRun.textContent?.includes(tr("断开")) === true;
    elevatedRun.disabled = true;
    box.textContent = wasConnected ? tr("正在断开…") : tr("正在请求管理员授权（会弹 UAC，请在弹出的窗口里确认）…");
    try {
      if (wasConnected) await invoke("elevated_stop");
      else await invoke<string>("elevated_start");
    } catch (e) {
      box.textContent = trt`${wasConnected ? tr("断开") : tr("启动")}失败：${e}`;
      elevatedRun.disabled = false;
      return;
    }
    await renderElevatedReport();
  });
  document.getElementById("export-config")!.addEventListener("click", () => void exportConfig());
  document.getElementById("sync-choose")!.addEventListener("click", () => void chooseSyncDir());
  document.getElementById("sync-clear")!.addEventListener("click", () => {
    state.cfg.settings.sync_dir = null;
    void save();
  });
  document.getElementById("sync-now")!.addEventListener("click", () => void syncNow());
  document.getElementById("sync-restore")!.addEventListener("click", () => void syncRestore());
  document.getElementById("import-config")!.addEventListener("click", () => void importConfig());
  document.getElementById("load-sample")!.addEventListener("click", () => void loadSampleConfig());
  bindMarket();
  // 更新源：写进配置后下一次检查（含手动「检查更新」）立即生效，后端每次检查都重读设置。
  const updateSource = document.getElementById("set-update-source") as HTMLSelectElement;
  updateSource.addEventListener("change", () => {
    state.cfg.settings.update_source = updateSource.value as "ihomy" | "github";
    void save();
  });
  document.getElementById("check-update")!.addEventListener("click", () => {
    void invoke("check_update");
  });
  const autoUpdate = document.getElementById("set-auto-update") as HTMLInputElement;
  autoUpdate.addEventListener("change", () => {
    state.cfg.settings.auto_update = autoUpdate.checked;
    void save();
    // 已经发现新版本时勾上就立刻开始下载：否则要等下一次检查（可能一天后）才动，
    // 用户会以为开关没生效。
    if (autoUpdate.checked && updateStatus?.phase === "available") {
      void invoke("download_update");
    }
  });
  document.getElementById("download-update")!.addEventListener("click", () => {
    void invoke("download_update");
  });
  document.getElementById("install-update")!.addEventListener("click", () => {
    void invoke("install_update");
  });
  // 导入方式：遮罩 / Esc 一律按「取消」——绝不替用户选一种导入方式。
  document.getElementById("import-merge")!.addEventListener("click", () => resolveImportMode("merge"));
  document.getElementById("import-replace")!.addEventListener("click", () =>
    resolveImportMode("replace"),
  );
  document.getElementById("import-cancel")!.addEventListener("click", () => resolveImportMode(null));
  document.getElementById("import-modal")!.querySelector(".modal-mask")!
    .addEventListener("click", () => resolveImportMode(null));
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && importModePending) {
      e.preventDefault();
      resolveImportMode(null);
    }
  });

  // 导入结果框：纯展示，点哪都关。
  document.getElementById("import-result-ok")!.addEventListener("click", hideImportResult);
  document.getElementById("import-result-close")!.addEventListener("click", hideImportResult);
  document.getElementById("import-result-modal")!.querySelector(".modal-mask")!
    .addEventListener("click", hideImportResult);

}

async function exportConfig() {
  const path = await saveFile({
    defaultPath: "kada-config.json",
    filters: [{ name: "JSON", extensions: ["json"] }],
  });
  if (!path) return;
  try {
    await invoke("export_config", { path });
    toast(tr("已导出到 ") + path);
  } catch (e) {
    toast(trt`导出失败: ${e}`);
  }
}

// 选同步存档目录：以本机探测到的 OneDrive 目录作默认位置（后端 `default_sync_dir`），
// 选定后写进配置并保存——保存本身就会立刻推一份过去（后端 `push_sync`，规划 7.2-㉝）。
async function chooseSyncDir() {
  const def = (await invoke<string | null>("default_sync_dir").catch(() => null)) ?? undefined;
  const dir = await openFile({ directory: true, defaultPath: def });
  if (!dir || Array.isArray(dir)) return;
  state.cfg.settings.sync_dir = dir;
  await save();
}

// 手动推送：把当前配置立刻写一份到同步存档目录，成功给明确反馈（与自动推送的静默失败区分）。
async function syncNow() {
  try {
    const path = await invoke<string>("sync_now");
    toast(tr("已同步到 ") + path);
  } catch (e) {
    toast(trt`同步失败: ${e}`);
  }
}

// 从存档恢复：后端给存档文件路径，走既有的「导入配置」流程（合并 / 替换 + 结果弹窗）。
async function syncRestore() {
  let path: string;
  try {
    path = await invoke<string>("sync_import_path");
  } catch (e) {
    toast(trt`无法读取同步存档: ${e}`);
    return;
  }
  const mode = await askImportMode(fileName(path));
  if (!mode) return;
  try {
    const r = await invoke<ImportOutcome>("import_config", { path, mode });
    await load();
    showImportResult(r);
  } catch (e) {
    toast(trt`恢复失败: ${e}`);
  }
}

// 导入结果（后端 ImportOutcome）：只回「忽略了几处」的话，用户查不出是哪条没进来，
// 所以明细逐条列出。
type ImportMode = "merge" | "replace";
export type ImportOutcome = {
  mode: ImportMode;
  added: number;
  notes: string[];
  backup: string | null;
};

let importModePending: ((m: ImportMode | null) => void) | null = null;

// 导入方式选择。导入是唯一能一次改掉整份配置的入口，必须先明确选，不能点一下就算数。
function askImportMode(file: string): Promise<ImportMode | null> {
  // 已经有一个框等着时，后来的请求按「取消」处理（同未保存守卫：否则先到的 promise 永远挂着）。
  if (importModePending) return Promise.resolve(null);
  document.getElementById("import-file")!.textContent = trt`将导入 ${file}，请选择导入方式：`;
  document.getElementById("import-modal")!.classList.remove("hidden");
  return new Promise((resolve) => (importModePending = resolve));
}

function resolveImportMode(mode: ImportMode | null) {
  const pending = importModePending;
  importModePending = null;
  document.getElementById("import-modal")!.classList.add("hidden");
  pending?.(mode);
}

async function importConfig() {
  const path = await openFile({
    filters: [{ name: "JSON", extensions: ["json"] }],
  });
  if (!path || Array.isArray(path)) return;
  const mode = await askImportMode(fileName(path));
  if (!mode) return;
  try {
    const r = await invoke<ImportOutcome>("import_config", { path, mode });
    await load();
    showImportResult(r);
  } catch (e) {
    toast(trt`导入失败: ${e}`);
  }
}

// 载入内置示例配置：后端走合并语义（只增不删、不碰设置），结果复用导入结果弹窗展示。
async function loadSampleConfig() {
  try {
    const r = await invoke<ImportOutcome>("load_sample_config");
    await load();
    showImportResult(r);
  } catch (e) {
    toast(trt`载入示例失败: ${e}`);
  }
}

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

export function showImportResult(r: ImportOutcome) {
  const skipped = r.notes.length;
  document.getElementById("import-result-text")!.textContent =
    r.mode === "merge"
      ? trt`已合并：新增 ${r.added} 条${skipped ? trt`，跳过 / 忽略 ${skipped} 条` : ""}。`
      : trt`已替换为导入的配置：共 ${r.added} 条${skipped ? trt`，忽略 ${skipped} 条` : ""}。`;
  const note = document.getElementById("import-result-note")!;
  note.classList.toggle("hidden", !r.backup);
  if (r.backup) note.textContent = trt`导入前的配置已留档：${r.backup}（覆盖回去即可回滚）`;
  const list = document.getElementById("import-result-list")!;
  list.replaceChildren(...r.notes.map((n) => el("li", undefined, n)));
  document.getElementById("import-result-modal")!.classList.remove("hidden");
}

function hideImportResult() {
  document.getElementById("import-result-modal")!.classList.add("hidden");
}

// ---- 软件更新 ----
// 状态源在后端（src-tauri/src/update.rs）：检查/下载/安装各阶段由 Rust 推进并 emit
// `update-status`，前端只渲染，不自己维护「正在检查」这类瞬时状态（否则窗口重开就丢）。
// 下载与安装是两步：`available` 给「下载」，下载完进 `ready` 才给「安装」。
let updateStatus: UpdateStatus | null = null;

/** 入口把新状态交进来（load() 初始化 / update-status 事件）；渲染仍由 syncUpdate() 负责。 */
export function setUpdateStatus(s: UpdateStatus) {
  updateStatus = s;
}

export function syncUpdate() {
  const line = document.getElementById("update-status")!;
  const actions = document.getElementById("update-actions")!;
  const downloadBtn = document.getElementById("download-update")!;
  const installBtn = document.getElementById("install-update")!;
  const checkBtn = document.getElementById("check-update") as HTMLButtonElement;
  if (!updateStatus) {
    line.textContent = tr("正在读取版本…");
    actions.classList.add("hidden");
    return;
  }
  const current = `v${updateStatus.current}`;
  const portable = updateStatus.kind === "portable";
  const busy =
    updateStatus.phase === "checking" ||
    updateStatus.phase === "downloading" ||
    updateStatus.phase === "installing";
  checkBtn.disabled = busy;
  // 下载与安装是两步：`available` 只给「下载」；`ready`（已下载）才给「安装」。
  const canDownload = updateStatus.phase === "available";
  const canInstall = updateStatus.phase === "ready";
  actions.classList.toggle("hidden", !canDownload && !canInstall);
  downloadBtn.classList.toggle("hidden", !canDownload);
  installBtn.classList.toggle("hidden", !canInstall);

  switch (updateStatus.phase) {
    case "idle":
      line.textContent = trt`当前版本 ${current}`;
      break;
    case "checking":
      line.textContent = trt`当前版本 ${current} · 正在检查更新…`;
      break;
    case "up-to-date":
      line.textContent = trt`当前版本 ${current}，已是最新版本`;
      break;
    case "available":
      line.textContent = portable
        ? trt`发现新版本 v${updateStatus.version}（当前 ${current}），点「下载」获取更新包`
        : trt`发现新版本 v${updateStatus.version}（当前 ${current}），点「下载」获取安装包`;
      break;
    case "downloading": {
      const pct =
        updateStatus.percent === null || updateStatus.percent === undefined
          ? ""
          : ` ${Math.round(updateStatus.percent)}%`;
      line.textContent = trt`正在下载 v${updateStatus.version}…${pct}`;
      break;
    }
    case "ready": {
      // 安装会退出并重启：安装版交给安装器静默装完重启，免安装版原位替换 exe 后重启。
      const how = portable
        ? tr("点「安装」将替换 kada.exe 并重启应用")
        : tr("点「安装」将退出应用、由安装器静默安装后重启");
      const auto = state.cfg.settings.auto_update
        ? portable
          ? tr("；已开启自动更新，也会在长时间空闲或下次启动时自动安装")
          : tr("；已开启自动更新，也会在长时间空闲时自动安装")
        : "";
      line.textContent = trt`已下载 v${updateStatus.version}，${how}${auto}`;
      break;
    }
    case "installing":
      line.textContent = portable
        ? trt`正在替换程序到 v${updateStatus.version}，应用即将重启…`
        : trt`正在安装 v${updateStatus.version}，应用即将退出…`;
      break;
    case "error":
      line.textContent = trt`更新失败：${updateStatus.message}`;
      break;
  }
}

// 点「查看更新日志」：把清单里的更新说明（`notes`）用同一个自绘模态框展示。
// 说明只跟着发现 / 已下载两个阶段来（下载时已存进 `pending.json`，重启后仍在）；
// 没有说明就如实说「暂无」，不另造 changelog 数据源。
function openUpdateNotes() {
  const s = updateStatus;
  if (!s || (s.phase !== "available" && s.phase !== "ready")) return;
  const notes = (s.notes ?? "").trim();
  openUpdateDialog({
    tone: "info",
    title: trt`更新日志 v${s.version}`,
    text: notes || tr("暂无更新说明"),
    action: null,
  });
}

// ---- 更新对话框（自绘）----
// 壳层（`src-tauri/src/update.rs` 的 `show_dialog`）手动检查 / 下载 / 安装时的反馈都推
// `update-dialog` 过来，这里用与主界面同一套模态框渲染，不再弹灰白系统框。
// 托盘「检查更新」会先唤起主窗口，那条事件可能赶在页面加载完之前——挂载时再用
// `take_update_dialog` 补取一次（后端存一份、取走即清）。
export type UpdateDialogPayload = {
  tone: "info" | "error";
  title: string;
  text: string;
  action?: "download" | null;
};

/** 主按钮要做什么；`null` = 纯说明（只有一个「知道了」）。 */
let updateDialogAction: "download" | null = null;

export function openUpdateDialog(d: UpdateDialogPayload) {
  const modal = document.getElementById("update-modal")!;
  modal.classList.toggle("is-error", d.tone === "error");
  document.getElementById("update-title")!.textContent = d.title;
  document.getElementById("update-text")!.textContent = d.text;
  const download = d.action === "download";
  const ok = document.getElementById("update-ok") as HTMLButtonElement;
  const cancel = document.getElementById("update-cancel") as HTMLButtonElement;
  ok.textContent = download ? tr("下载") : tr("知道了");
  ok.className = "primary";
  cancel.textContent = tr("稍后");
  cancel.classList.toggle("hidden", !download);
  updateDialogAction = download ? "download" : null;
  modal.classList.remove("hidden");
  // 先摘 `hidden` 再 focus：`display:none` 的子树里 focus() 是空操作。
  ok.focus();
}

/** `confirmed` = 点了主按钮（说明框的「知道了」也算）；只有下载询问框才真的去下载。 */
function closeUpdateDialog(confirmed: boolean) {
  document.getElementById("update-modal")!.classList.add("hidden");
  const action = updateDialogAction;
  updateDialogAction = null;
  if (confirmed && action === "download") void invoke("download_update");
}

export function bindUpdateDialog() {
  document.getElementById("update-ok")!.addEventListener("click", () => closeUpdateDialog(true));
  document.getElementById("update-cancel")!.addEventListener("click", () => closeUpdateDialog(false));
  document.getElementById("update-close")!.addEventListener("click", () => closeUpdateDialog(false));
  document.getElementById("update-notes")!.addEventListener("click", () => openUpdateNotes());
  document.getElementById("update-modal")!.querySelector(".modal-mask")!.addEventListener("click", () =>
    closeUpdateDialog(false),
  );
  // 与其它确认框同口径：Esc = 取消（下载询问框 = 不下载，纯说明框只是关掉）。
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (document.getElementById("update-modal")!.classList.contains("hidden")) return;
    e.preventDefault();
    closeUpdateDialog(false);
  });
}

// ---- 复制快捷菜单（EXT-13，已删除；保留只读编辑器函数供旧配置兜底）----
// 条目 = 一个 Action（可用「分组」串多步），编辑器内嵌在设置页；菜单浮窗只负责展示与点击，
// 动作执行始终在壳层（与快捷键同一条 fire 链路）。
export function renderClipboardMenuEditor() {
  const host = document.getElementById("clipboard-menu-editor");
  if (!host) return;
  host.replaceChildren();
  const rerender = () => renderClipboardMenuEditor();
  // 只读展示时不要顺手往 cfg 里塞一个默认字段（没碰过菜单的用户不该因此多一份配置）。
  const menu = state.cfg.settings.clipboard_menu ?? { enabled: false, double_copy_ms: 500, items: [] };
  host.append(menuItemsEditor(menu.items, 1, rerender));
}

function menuItemsEditor(items: MenuEntry[], depth: number, rerender: () => void): HTMLElement {
  const list = el("div", "menu-editor-list");
  if (!items.length) {
    list.append(el("div", "unit-hint", tr("还没有条目：点下面的「＋ 动作」开始添加。")));
  }
  items.forEach((_, i) => list.append(menuEntryEditor(items, i, depth, rerender)));
  return list;
}

function moveMenuEntry(items: MenuEntry[], from: number, to: number) {
  if (to < 0 || to >= items.length || from === to) return;
  const moved = items.splice(from, 1)[0];
  if (!moved) return;
  items.splice(to, 0, moved);
}

function menuEntryEditor(
  items: MenuEntry[],
  i: number,
  depth: number,
  rerender: () => void,
): HTMLElement {
  const item = items[i];
  const box = el("div", "menu-editor-entry");
  // 调用方按 items 的下标渲染，正常不会越界；越界时给个空壳，别让整块菜单崩掉。
  if (!item) return box;
  // 动作字段的改动在失焦时落盘（与设置页其它字段同一口径）。
  box.addEventListener("change", () => void save());

  const head = el("div", "menu-editor-head");
  if (item.kind === "separator") {
    head.append(el("span", "menu-editor-kind", tr("分割线")));
  } else {
    const label = el("input", "action-input menu-editor-label") as HTMLInputElement;
    label.type = "text";
    label.value = item.label;
    label.placeholder = tr("菜单上显示的文字");
    label.addEventListener("input", () => {
      item.label = label.value;
    });
    head.append(label);

    const on = el("label", "action-check") as HTMLLabelElement;
    const cb = el("input", "toggle") as HTMLInputElement;
    cb.type = "checkbox";
    cb.checked = item.enabled !== false;
    cb.title = tr("取消勾选 = 菜单里灰掉不可点（配置保留）");
    cb.addEventListener("change", () => {
      item.enabled = cb.checked;
    });
    on.append(cb, el("span", undefined, tr("启用")));
    head.append(on);
  }

  const up = el("button", "ghost menu-editor-btn", "↑") as HTMLButtonElement;
  up.title = tr("上移");
  up.disabled = i === 0;
  up.addEventListener("click", () => {
    moveMenuEntry(items, i, i - 1);
    rerender();
    void save();
  });
  const down = el("button", "ghost menu-editor-btn", "↓") as HTMLButtonElement;
  down.title = tr("下移");
  down.disabled = i === items.length - 1;
  down.addEventListener("click", () => {
    moveMenuEntry(items, i, i + 1);
    rerender();
    void save();
  });
  const del = el("button", "ghost menu-editor-btn danger", "×") as HTMLButtonElement;
  del.title = tr("删除这一条");
  del.addEventListener("click", () => {
    items.splice(i, 1);
    rerender();
    void save();
  });
  head.append(up, down, del);
  box.append(head);

  if (item.kind === "action") {
    const action = unwrapAction(item.action);
    const typeRow = el("div", "action-type-row");
    const typeSel = el("select", "action-type-name") as HTMLSelectElement;
    typeSel.title = tr("点击切换动作类型");
    typeSel.setAttribute("aria-label", tr("动作类型"));
    for (const t of actionTypeChoices(action.type === "tool")) {
      const o = el("option") as HTMLOptionElement;
      o.value = t.value;
      o.textContent = t.label;
      typeSel.append(o);
    }
    // 「设置变量」归文本操作：类型名显示「文本操作」，方式看子类型下拉。
    typeSel.value = action.type === "ask" ? "text" : action.type;
    typeSel.addEventListener("change", () => {
      item.action = convertedAction(action, typeSel.value as ActionBase["type"]);
      rerender();
      void save();
    });
    typeRow.append(typeSel);
    const sub = actionSubtypeSelect(
      action,
      () => {
        rerender();
        void save();
      },
      (next) => {
        item.action = next;
      },
    );
    if (sub) typeRow.append(sub);
    box.append(typeRow);
    box.append(
      actionFields(action, () => {
        rerender();
        void save();
      }),
    );
  } else if (item.kind === "submenu") {
    box.append(el("div", "menu-editor-sub", tr("子菜单")));
    box.append(menuItemsEditor(item.items, depth + 1, rerender));
    // 子菜单最多再嵌两层（顶层算第 1 层，与后端 `MAX_DEPTH` 一致）。
    if (depth < 3) {
      const adds = el("div", "set-actions menu-editor-adds");
      for (const [text, mk] of [
        [tr("＋ 动作"), () => ({ kind: "action", label: tr("新条目"), enabled: true, action: newAction("text") })],
        [
          tr("＋ 子菜单"),
          () => ({
            kind: "submenu",
            label: tr("新子菜单"),
            enabled: true,
            items: [{ kind: "action", label: tr("新条目"), enabled: true, action: newAction("text") }],
          }),
        ],
        [tr("＋ 分割线"), () => ({ kind: "separator" })],
      ] as [string, () => MenuEntry][]) {
        const b = el("button", "ghost", text) as HTMLButtonElement;
        b.addEventListener("click", () => {
          item.items.push(mk());
          rerender();
          void save();
        });
        adds.append(b);
      }
      box.append(adds);
    }
  }
  return box;
}
