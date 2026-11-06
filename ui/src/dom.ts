// 无状态小工具：建节点 / 异步点击 / 取值与格式化。
// 这里只放「不依赖任何业务状态」的函数——读 `state` 或改配置的一律不进本文件。
import { trt } from "./i18n";

export function el(tag: string, cls?: string, text?: string): HTMLElement {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text !== undefined) n.textContent = text;
  return n;
}

// 异步点击处理器：`addEventListener` 的回调必须返回 void，直接塞 async 函数会把 Promise
// 丢在没人处理的地方（钩不到失败）。这里显式 void 掉；处理器内部仍要自己 try/catch 提示。
export function onClickAsync(target: HTMLElement, fn: () => Promise<void>): void {
  target.addEventListener("click", () => void fn());
}

// 深拷贝（配置是纯 JSON 数据，用结构化克隆隔离编辑草稿，避免误写回原配置）。
export function deepClone<T>(x: T): T {
  return structuredClone(x);
}

// 圆圈问号：hover / 键盘聚焦显示说明（用于解释「会写入变量」的动作，如取文件属性/查询应用状态）。
export function helpIcon(tip: string): HTMLElement {
  const s = el("span", "help", "?");
  s.setAttribute("data-tip", tip);
  s.tabIndex = 0;
  return s;
}

// 主窗口右上角的保存提示（不是透明浮窗，不涉及 focusable）。
export function toast(msg: string) {
  const node = document.getElementById("save-status")!;
  node.textContent = msg;
  setTimeout(() => (node.textContent = ""), 2000);
}

// 按 id 读输入框当前值（控件不在 DOM 里时当空串——渲染未铺开就取签名不会炸）。
export function domValue(id: string): string {
  const node = document.getElementById(id) as
    | HTMLInputElement
    | HTMLSelectElement
    | HTMLTextAreaElement
    | null;
  return node ? node.value : "";
}

// 数值框取值：解析失败回退默认值、越界收回到 [min, max]（input 的 min/max 属性就是口径）。
// 此前 `parseInt(x, 10) || 0` 对负数照单全收（-50ms 这种值一路落盘，后端 u64 反序列化直接报错）。
export function intIn(v: string, min: number, fallback: number, max = Number.MAX_SAFE_INTEGER): number {
  const n = parseInt(v, 10);
  if (!Number.isFinite(n)) return fallback;
  return Math.min(max, Math.max(min, n));
}

export function fmtMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)}s`;
  const m = Math.floor(ms / 60_000);
  return trt`${m}分${Math.round((ms % 60_000) / 1000)}秒`;
}
