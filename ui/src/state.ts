// 跨模块共享的可变状态。U-01 先把被 `options.ts` 读到的 `runningTools` 挪出来；
// 其余状态按 U-02 再逐步搬进这里（原则：模块顶层不互相 import 对方的 `let`）。
import type { ExternalTool } from "./types";

/** 本机正在运行的外部工具（后端探测进程名；动作类型里据此出现对应能力）。 */
export const runningTools: ExternalTool[] = [];
