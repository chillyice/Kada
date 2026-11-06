// Tauri IPC 封装：`invoke` 的薄包装 + 只改 `state` 的刷新函数。
// 只放「发一次请求、拿回数据」这一层；要重绘 / 弹提示的编排（load / save）仍在入口模块，
// 免得本模块反过来 import 入口（ESM 循环依赖会让 `localizeStaticDom()` 晚执行）。
import { invoke } from "@tauri-apps/api/core";
import { state } from "./state";
import type {
  Action,
  ActionBase,
  CommandResult,
  Config,
  Conflict,
  ExternalTool,
  InputDeviceList,
  RunRecord,
  UpdateStatus,
} from "./types";

export function fetchConfig(): Promise<Config> {
  return invoke<Config>("get_config");
}

// 落盘整份配置，返回被后端清洗掉的无效条目说明（空数组 = 全部收下）。
export function saveConfig(config: Config): Promise<string[]> {
  return invoke<string[]>("set_config", { config });
}

export function fetchRunningTools(): Promise<ExternalTool[]> {
  return invoke<ExternalTool[]>("running_tools");
}

export function fetchInputDeviceList(): Promise<InputDeviceList> {
  return invoke<InputDeviceList>("input_devices");
}

export function fetchCommandResults(): Promise<CommandResult[]> {
  return invoke<CommandResult[]>("get_command_results");
}

export function fetchRunRecords(): Promise<RunRecord[]> {
  return invoke<RunRecord[]>("get_run_records");
}

export function fetchUnread(): Promise<boolean> {
  return invoke<boolean>("get_unread");
}

export function fetchUpdateStatus(): Promise<UpdateStatus> {
  return invoke<UpdateStatus>("get_update_status");
}

export function fetchConflicts(config: Config): Promise<Conflict[]> {
  return invoke<Conflict[]>("get_conflicts", { config });
}

export async function refreshRunningTools(): Promise<void> {
  try {
    const tools = await fetchRunningTools();
    state.runningTools.length = 0;
    state.runningTools.push(...tools);
  } catch {
    state.runningTools.length = 0;
  }
}

/**
 * 本机键盘列表（「设备是」下拉）。**按需拉、带缓存**：枚举设备要一个个开句柄（Linux 还要
 * 读 /dev/input），不该每次重绘都来一遍；「重新检测」传 `force` 才真的重扫。
 * 拉失败当作「没有可选项」（`supported` 仍按平台能力报）——手写标识的路一直开着。
 */
export async function refreshInputDevices(force = false): Promise<InputDeviceList> {
  if (!force && state.inputDeviceList) return state.inputDeviceList;
  try {
    state.inputDeviceList = await fetchInputDeviceList();
  } catch {
    state.inputDeviceList = { supported: false, devices: [] };
  }
  return state.inputDeviceList;
}

// 冲突检测用到的配置：编辑时把草稿合并进 cfg，让冲突在录入触发键的当下就实时显示，
// 而不是等保存后才知道。
export function currentConfigForConflicts(): Config {
  if (state.section === "shortcuts" && state.draft) {
    const d = state.draft;
    const shortcuts = state.draftNew
      ? [d, ...state.cfg.shortcuts]
      : state.cfg.shortcuts.map((s, i) => (i === state.selected ? d : s));
    return { ...state.cfg, shortcuts };
  }
  return state.cfg;
}

export async function refreshConflicts() {
  try {
    const next = await fetchConflicts(currentConfigForConflicts());
    // 冲突集合变化时，重新弹出气泡（用户消除过旧冲突、又出现新冲突时能再次被提醒）。
    if (JSON.stringify(next) !== JSON.stringify(state.conflicts)) state.conflictBubbleDismissed = false;
    state.conflicts = next;
  } catch {
    state.conflicts = [];
  }
}

// 保存前把命令 / 脚本新字段的空值归一成缺省（不落一堆空键；后端 serde 缺字段 = 默认值）。
// 递归处理容器动作（禁用 / 分组 / 并行 / 条件判断）里的成员。
export function normalizeCommandScriptFields(actions: Action[]): Action[] {
  return actions.map((a): Action => {
    switch (a.type) {
      case "disabled":
        return {
          type: "disabled",
          action: normalizeCommandScriptFields([a.action])[0] as ActionBase,
        };
      case "group":
        return { ...a, actions: normalizeCommandScriptFields(a.actions) };
      case "parallel":
        return { ...a, actions: normalizeCommandScriptFields(a.actions) };
      case "if":
        return {
          ...a,
          then: normalizeCommandScriptFields(a.then),
          otherwise: normalizeCommandScriptFields(a.otherwise),
        };
      case "command":
      case "script": {
        const out: Record<string, unknown> = { ...a };
        const wd = typeof out.working_dir === "string" ? out.working_dir.trim() : "";
        if (wd) out.working_dir = wd;
        else delete out.working_dir;
        if (out.timeout_ms == null) delete out.timeout_ms;
        if (!Array.isArray(out.ok_exit_codes) || out.ok_exit_codes.length === 0)
          delete out.ok_exit_codes;
        if (a.type === "script") {
          if (!Array.isArray(out.interpreter_args) || out.interpreter_args.length === 0)
            delete out.interpreter_args;
          if (!Array.isArray(out.args) || out.args.length === 0) delete out.args;
        }
        // CS-17：默认的「照常再起一份」不落盘，老配置读进来就是它。
        if (out.if_running === "run" || out.if_running == null) delete out.if_running;
        return out as unknown as Action;
      }
      case "wait_for": {
        const out: Record<string, unknown> = { ...a };
        if (out.timeout_ms == null) delete out.timeout_ms;
        if (out.interval_ms == null || out.interval_ms === 200) delete out.interval_ms;
        return out as unknown as Action;
      }
      default:
        return a;
    }
  });
}
