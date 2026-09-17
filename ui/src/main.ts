import { invoke } from "@tauri-apps/api/core";

type TextAction = { type: "text"; text: string };
type Step =
  | { type: "text"; text: string }
  | { type: "tap"; key: string }
  | { type: "keys"; keys: string[] }
  | { type: "down"; key: string }
  | { type: "up"; key: string }
  | { type: "pause_ms"; ms: number };
type SeqAction = { type: "sequence"; steps: Step[] };
type Action = TextAction | SeqAction;
type ShortcutItem = {
  name?: string | null;
  triggers: string[];
  action: Action;
  enabled: boolean;
};
type Remap = { from: string; to: string; enabled: boolean };
type Config = { shortcuts: ShortcutItem[]; remaps: Remap[] };

// ---- 状态 ----
let cfg: Config = { shortcuts: [], remaps: [] };
let editing: number | null = null; // 编辑中的 shortcuts 下标
let recording = false;

// ---- 键名选项（改键下拉用），对齐 kada-core 的 key_name ----
const KEY_OPTIONS = [
  ..."ABCDEFGHIJKLMNOPQRSTUVWXYZ".split(""),
  ..."0123456789".split(""),
  ...Array.from({ length: 12 }, (_, i) => `F${i + 1}`),
  ",", ".", "/", "\\", ";", '"', "`", "-", "=", "[", "]",
  "Enter", "Esc", "Tab", "Space", "Backspace", "Delete", "Insert", "CapsLock",
  "Shift", "Ctrl", "Alt", "Meta",
  "Home", "End", "PageUp", "PageDown", "Up", "Down", "Left", "Right",
];

// ---- IPC ----
async function load() {
  cfg = await invoke<Config>("get_config");
  render();
}

async function save() {
  cfg.shortcuts = cfg.shortcuts.filter(
    (s) =>
      s.triggers.length > 0 ||
      !!s.name ||
      (s.action.type === "text" ? s.action.text.length > 0 : s.action.steps.length > 0),
  );
  try {
    await invoke("set_config", { config: cfg });
    toast("已保存");
  } catch (e) {
    toast(`保存失败: ${e}`);
  }
  render();
}

function toast(msg: string) {
  const el = document.getElementById("save-status")!;
  el.textContent = msg;
  setTimeout(() => (el.textContent = ""), 2000);
}

// ---- 组合键录入 ----
const CODE_TABLE: Record<string, string> = {
  Enter: "Enter", Escape: "Esc", Tab: "Tab", Space: "Space",
  Backspace: "Backspace", Delete: "Delete", Insert: "Insert", CapsLock: "CapsLock",
  Home: "Home", End: "End", PageUp: "PageUp", PageDown: "PageDown",
  ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right",
};
const CODE_PUNCT: Record<string, string> = {
  Comma: ",", Period: ".", Slash: "/", Backslash: "\\", Semicolon: ";",
  Quote: '"', Backquote: "`", Minus: "-", Equal: "=",
  BracketLeft: "[", BracketRight: "]",
};

function codeToKey(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F(1[0-2]|[1-9])$/.test(code)) return code;
  return CODE_PUNCT[code] ?? CODE_TABLE[code] ?? null;
}

function comboFromEvent(e: KeyboardEvent): string | null {
  const main = codeToKey(e.code);
  if (!main) return null;
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Meta");
  parts.push(main);
  return parts.join("+");
}

function startCapture(onCommit: (combo: string) => void, btn: HTMLButtonElement) {
  btn.disabled = true;
  btn.textContent = "请按组合键…（Esc 取消）";
  void invoke("set_paused", { paused: true });
  const handler = (e: KeyboardEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.code === "Escape") return finish();
    const combo = comboFromEvent(e);
    if (combo) {
      onCommit(combo);
      finish();
    }
  };
  const finish = () => {
    window.removeEventListener("keydown", handler, true);
    void invoke("set_paused", { paused: false });
    btn.disabled = false;
    btn.textContent = "录入组合键";
  };
  window.addEventListener("keydown", handler, true);
}

// ---- 渲染 ----
function el(tag: string, cls?: string, text?: string): HTMLElement {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text !== undefined) n.textContent = text;
  return n;
}

function render() {
  renderShortcuts();
  renderRemaps();
}

function renderShortcuts() {
  const list = document.getElementById("shortcut-list")!;
  list.replaceChildren();
  cfg.shortcuts.forEach((s, i) => {
    const li = el("li", "row");
    const on = el("input", "toggle") as HTMLInputElement;
    on.type = "checkbox";
    on.checked = s.enabled;
    on.addEventListener("change", () => {
      s.enabled = on.checked;
      void save();
    });

    const info = el("div", "row-info");
    const desc =
      s.action.type === "sequence"
        ? `宏 ${s.action.steps.length} 步`
        : s.action.text;
    info.append(
      el("span", "triggers", s.triggers.join("  ")),
      el("span", "desc", desc || "（文本）"),
    );

    const editBtn = el("button", undefined, "编辑");
    editBtn.addEventListener("click", () => openEditor(i));
    const delBtn = el("button", "danger", "删除");
    delBtn.addEventListener("click", () => {
      cfg.shortcuts.splice(i, 1);
      if (editing === i) editing = null;
      void save();
    });

    li.append(on, info, editBtn, delBtn);
    list.append(li);
  });
}

function openEditor(i: number) {
  editing = i;
  const s = cfg.shortcuts[editing];
  (document.getElementById("edit-name") as HTMLInputElement).value = s.name ?? "";
  renderTriggers(s);
  const typeSel = document.getElementById("edit-action-type") as HTMLSelectElement;
  if (s.action.type === "sequence") {
    typeSel.value = "sequence";
    (document.getElementById("edit-text") as HTMLTextAreaElement).value = "";
    (document.getElementById("edit-steps") as HTMLTextAreaElement).value = stepsToJson(s.action.steps);
  } else {
    typeSel.value = "text";
    (document.getElementById("edit-text") as HTMLTextAreaElement).value = s.action.text;
    (document.getElementById("edit-steps") as HTMLTextAreaElement).value = "";
  }
  syncActionFields();
  document.getElementById("shortcut-editor")!.classList.remove("hidden");
}

function syncActionFields() {
  const isSeq =
    (document.getElementById("edit-action-type") as HTMLSelectElement).value === "sequence";
  document.getElementById("edit-action-fields")!.classList.toggle("hidden", isSeq);
  document.getElementById("edit-sequence-fields")!.classList.toggle("hidden", !isSeq);
}

function stepsToJson(steps: Step[]): string {
  return JSON.stringify(steps, null, 2);
}

function renderTriggers(s: ShortcutItem) {
  const wrap = document.getElementById("edit-triggers")!;
  wrap.replaceChildren();
  s.triggers.forEach((t, i) => {
    const chip = el("span", "chip", t);
    const x = el("button", "chip-x", "×");
    x.addEventListener("click", () => {
      s.triggers.splice(i, 1);
      renderTriggers(s);
    });
    chip.append(x);
    wrap.append(chip);
  });
}

function renderRemaps() {
  const list = document.getElementById("remap-list")!;
  list.replaceChildren();
  cfg.remaps.forEach((r, i) => {
    const li = el("li", "row");
    const on = el("input", "toggle") as HTMLInputElement;
    on.type = "checkbox";
    on.checked = r.enabled;
    on.addEventListener("change", () => {
      r.enabled = on.checked;
      void save();
    });

    const pick = (val: string, set: (v: string) => void) => {
      const sel = document.createElement("select");
      for (const k of KEY_OPTIONS) {
        const o = document.createElement("option");
        o.value = k;
        o.textContent = k;
        sel.append(o);
      }
      sel.value = val;
      sel.addEventListener("change", () => {
        set(sel.value);
        void save();
      });
      return sel;
    };

    const arrow = el("span", "arrow", "→");
    const delBtn = el("button", "danger", "删除");
    delBtn.addEventListener("click", () => {
      cfg.remaps.splice(i, 1);
      void save();
    });

    li.append(on, pick(r.from, (v) => (r.from = v)), arrow, pick(r.to, (v) => (r.to = v)), delBtn);
    list.append(li);
  });
}

// ---- 事件绑定 ----
function bind() {
  document.querySelectorAll<HTMLButtonElement>(".tab").forEach((b) => {
    b.addEventListener("click", () => {
      document.querySelectorAll(".tab").forEach((t) => t.classList.remove("active"));
      b.classList.add("active");
      const target = b.dataset.tab;
      document.getElementById("panel-shortcuts")!.classList.toggle("hidden", target !== "shortcuts");
      document.getElementById("panel-remaps")!.classList.toggle("hidden", target !== "remaps");
    });
  });

  document.getElementById("add-shortcut")!.addEventListener("click", () => {
    cfg.shortcuts.unshift({ name: "", triggers: [], action: { type: "text", text: "" }, enabled: true });
    renderShortcuts();
    openEditor(0);
  });

  document.getElementById("edit-capture")!.addEventListener("click", (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    if (editing === null || !cfg.shortcuts[editing]) return;
    const s = cfg.shortcuts[editing];
    startCapture((combo) => {
      if (!s.triggers.includes(combo)) s.triggers.push(combo);
      renderTriggers(s);
    }, btn);
  });

  (document.getElementById("edit-action-type") as HTMLSelectElement).addEventListener(
    "change",
    syncActionFields,
  );

  document.getElementById("action-record")!.addEventListener("click", async (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    if (editing === null) return;
    if (!recording) {
      try {
        await invoke("start_record");
      } catch (err) {
        toast(`开始录制失败: ${err}`);
        return;
      }
      recording = true;
      btn.classList.add("recording");
      btn.textContent = "■ 录制中（点此停止）";
      const stepsEl = document.getElementById("edit-steps") as HTMLTextAreaElement;
      stepsEl.value = "";
      stepsEl.placeholder = "录制中……切到目标窗口操作，再回来点停止";
      stepsEl.disabled = true;
    } else {
      recording = false;
      const steps = await invoke<Step[]>("stop_record");
      btn.classList.remove("recording");
      btn.textContent = "● 开始录制";
      const stepsEl = document.getElementById("edit-steps") as HTMLTextAreaElement;
      stepsEl.disabled = false;
      stepsEl.value = stepsToJson(steps);
      stepsEl.placeholder = '宏步骤 JSON，例：[{"type":"text","text":"hi"}]';
      // 新录制的宏直接作为本快捷键动作，并切到宏类型
      const s = cfg.shortcuts[editing];
      s.action = { type: "sequence", steps };
      (document.getElementById("edit-action-type") as HTMLSelectElement).value = "sequence";
      syncActionFields();
      void save();
    }
  });

  document.getElementById("save-shortcut")!.addEventListener("click", () => {
    if (editing === null) return;
    void stopRecIfAny();
    const s = cfg.shortcuts[editing];
    s.name = (document.getElementById("edit-name") as HTMLInputElement).value || null;
    const typeSel = document.getElementById("edit-action-type") as HTMLSelectElement;
    if (typeSel.value === "sequence") {
      const raw = (document.getElementById("edit-steps") as HTMLTextAreaElement).value.trim();
      try {
        s.action = { type: "sequence", steps: JSON.parse(raw || "[]") as Step[] };
      } catch {
        toast("宏步骤 JSON 无效");
        return;
      }
    } else {
      s.action = {
        type: "text",
        text: (document.getElementById("edit-text") as HTMLTextAreaElement).value,
      };
    }
    editing = null;
    document.getElementById("shortcut-editor")!.classList.add("hidden");
    void save();
  });

  document.getElementById("cancel-shortcut")!.addEventListener("click", () => {
    void stopRecIfAny();
    editing = null;
    document.getElementById("shortcut-editor")!.classList.add("hidden");
    renderShortcuts();
  });

  document.getElementById("add-remap")!.addEventListener("click", () => {
    cfg.remaps.push({ from: "CapsLock", to: "Ctrl", enabled: true });
    renderRemaps();
    void save();
  });
}

async function stopRecIfAny() {
  if (recording) {
    recording = false;
    const btn = document.getElementById("action-record") as HTMLButtonElement;
    btn.classList.remove("recording");
    btn.textContent = "● 开始录制";
    await invoke("stop_record").catch(() => {});
  }
}

bind();
void load();