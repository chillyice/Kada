# Kada 文档索引

> 一页地图：每份文档一句话定位 + 谁该读。新成员/新会话从这里出发，不必通读全部。

## 新人阅读顺序

1. 根 `README.md` —— 项目是什么，30 秒
2. `docs/架构设计.md` —— 分层、事件怎么流转、关键机制为什么这样设计
3. `AGENTS.md` —— 项目规则与约定（命名/键模型/钩子机制/里程碑），写代码前必读；AI 会话自动加载
4. `docs/需求设计说明书.md` —— 功能需求唯一活文档（键模型/配置模型/接口/规划），实现功能前读
5. `docs/变更归档.md` —— 按需检索，找历史实现与设计决策

## 仓库内文档

| 文档 | 一句话定位 | 谁该读 |
|------|-----------|--------|
| `README.md`（根） | 公开门面：项目简介、特性、技术栈 | 所有人第一眼 |
| `AGENTS.md`（根） | 项目规则 + 导航索引：命名、键模型、钩子机制、里程碑 | 所有开发者（人与 AI），动手前必读 |
| `docs/架构设计.md` | 分层 / 事件流转 / 关键机制 / 跨平台策略 | 新成员建立全局认知；动架构前重读 |
| `docs/需求设计说明书.md` | 功能需求唯一活文档：键模型、配置模型、钩子、桌面壳、UI、规划事项（**按功能模块组织**，顶部是模块导航表） | 实现任何功能前；查接口/模型 |
| `docs/变量提取与引用指南.md` | 变量怎么提取、怎么用占位符引用（含「移动硬盘盘符不定」实战） | 想用变量串联多个动作、做自动化时 |
| `docs/竞品分析与优化规划.md` | 竞品横向对比 + 差距清单 + 分阶段优化路线图（P0~P3） | 规划下一步功能方向、排优先级时 |
| `docs/触摸板手势可行性调研.md` | 规划 7.2-㊻ 的可行性调研：三平台（Linux libinput / macOS 私有 API / Windows Raw Input）能拿到什么手势事件、怎么接进现有 `Key` 模型、成本与落地顺序 | 评估「触摸板手势作触发源」是否立项 / 怎么落地时 |
| `docs/底层输入能力规划.md` | 输入层下探的分档规划：三平台各站在哪一层、两个真实缺口（厂商 divert 按键 / 提权窗口注入）、四个档位（A 环境诊断 / B 提权伴随进程——**两档均已落地** / C HID++ 解锁侧键——下一个 / D 内核驱动评估后不做）、签名与信任成本、轻盈五条不变量、落地顺序 | 想「再往底层走」时先读它；判断某条限制该不该用驱动 / HID++ 解决时 |
| `docs/人工验证清单.md` | 只能真机 + 真人操作才能确认的验收项：可照做的步骤 + 预期现象 + 判失败（锁屏/唤醒/切窗复位、动作超时与中止、动作失败上报、未保存改动守卫、删除确认与导入合并 / 替换、配置外部修改监听、更新端到端含产物自检）；**顶部有逐条验证状态**（最近一轮 2026-10-10：§1~§3、§5~§8 通过，§4 休眠唤醒未执行，§9~§14 与附录待真人复跑） | 验收相关改动、发版前 |
| `docs/变更归档.md` | 已实现变更归档：按里程碑的文件-改动表、设计决策、冒烟结论 | 检索历史实现时 |

## 代码地图

> 目录与关键文件职责一览；AGENTS.md 只留规则，动代码前按这张图定位。

```
crates/kada-core/           # 零重量纯逻辑：键模型、快捷键解析/匹配、配置模型、冲突检测、片段摘取
  src/lib.rs                # Key/Modifier/RawEvent/Shortcut + parse/format/matches + Config/Action/Trigger/sanitize
  src/merge.rs              # 配置合并（导入的「合并」方式：只增不删、不造新冲突、设置保留本机）
  src/snippet.rs            # 配置片段（分享用）：按「类别+下标」摘出条目 + 被引用的层；片段市场的索引模型/解析，见 AGENTS「铁律」
crates/kada-hook/           # 平台钩子引擎：全局键盘事件监听 + 拦截 + 注入
  src/win.rs                # Windows: WH_KEYBOARD_LL + WH_MOUSE_LL + swallow/Replace 状态机 + key↔VK + 自愈看门狗
  src/win/simulate.rs       # Windows: SendInput 注入 + 剪贴板文本；注入统一拆 Op 走 deliver（前台是提权窗口时交代理，见 架构设计.md §3.23）
  src/win/agent.rs          # Windows: 提权伴随进程（命名管道 + 行协议 + DACL/双向身份校验 + runas 拉起 + 父进程守望）
  src/linux.rs              # Linux: evdev + uinput 钩子 + 键码映射 + simulate
  src/macos.rs              # macOS: CGEventTap 钩子 + key↔虚拟键码 + AX 前台查询 + simulate（需「辅助功能」权限）
  src/lib.rs                # 平台中性入口：platform_input_env / elevated（提权窗口注入，非 Windows 为空实现）
  examples/demo.rs          # M1 冒烟 demo（三平台，真人按键验证）
  examples/mouse_probe.rs   # 鼠标侧键分诊探针（Windows）：自检「按需挂钩子」+ 看侧键事件有没有到得了钩子（分诊厂商软件接管，见 docs/底层输入能力规划.md 档位 C）
  examples/input_env_probe.rs   # 输入层环境诊断探针：印「认出了谁 + HVCI 状态」，真机核对入口（见 人工验证清单.md §30）
  examples/elevated_probe.rs    # 提权通道探针（Windows）：验 DACL / 双向身份校验 / 行协议往返 / 分片 / 坏 op 整批拒 / 断开即退（UAC 那段见 人工验证清单.md §31）
crates/kada-actions/        # 动作执行引擎（automation feature 门控）：run_actions/run_os/run_app/run_cmd/Script + CommandResult
  src/abort.rs              # 中止开关（代数计数器 + 运行计数 RunGuard，细节见 架构设计.md §3.17）
src-tauri/                  # Tauri 2 桌面壳（crate "kada"）
  src/lib.rs                # KadaState/decide/Recorder/消息中心/tauri commands/托盘常驻 + 窗口懒创建
  src/engine.rs             # 输入决策引擎（Tauri 无关、可单测）：tap-hold/层/和弦/键序列/热串状态机 + Inject 通道
  src/config_io.rs          # 配置读写：原子写 + .bak + 损坏留档与自愈；read_only 只读入口供监听
  src/config_watch.rs       # 配置外部修改监听（内容指纹比对），可单测
  src/update.rs             # 软件更新：检查/下载/安装状态机 + 进度节流，Tauri 无关部分可单测
  tauri.conf.json           # 窗口/图标/构建配置 + bundle.createUpdaterArtifacts + plugins.updater（公钥/端点）
ui/                         # Vite + TypeScript 前端（kada-ui）
  src/main.ts               # 配置界面：快捷键（动作流程图编排）/改键/文本扩展/宏录制/消息中心/设置页/片段市场；草稿隔离 + 未保存守卫
  src/mock.ts               # 纯浏览器演示模式：无 Tauri 壳时注入模拟 IPC + 演示配置（含各类事件桩，可直开浏览器调试）
  src/style.css             # 深色 UI 样式
snippets/                   # 片段市场的内置索引托管：*.json 片段 + index.json（脚本生成，客户端只 GET）
scripts/                    # build-snippet-index.mjs（片段→索引，CI 用 --check 查同步）/ verify-release.mjs（发版产物自检）/ linux-package（容器打包）/ ihomy-mirror（更新源每日镜像 + systemd 单元）
.github/workflows/          # ci.yml（三平台构建+测试）；release.yml（打 v* tag 签名发版）
```

## 文档维护规矩

- 新的变更归档**追加到 `docs/变更归档.md` 末尾**，不写回 AGENTS.md（AGENTS.md 只留规则 + 导航，20KB 上限——描述性细节一律外移到本目录，超限会挤掉注入上下文）。
- 功能需求变更同步 `docs/需求设计说明书.md` 的**所属模块小节**（该文档已改按功能模块组织，没有版本时间线；`V0.x` 版本号只写进 `docs/变更归档.md`）。
- 只能在真机由真人操作确认的验收项（锁屏/唤醒、切窗、GUI 点击等）写进 `docs/人工验证清单.md`，**不要**只留在 `变更归档.md` 的「未验证」一句话里——那里写不下步骤，接手的人不知道从哪下手。跑完一轮就把结果回填到该清单顶部的「验证状态」表（**没跑的标 ⬜ 未执行，别继承上一轮的结论**），并在 `变更归档.md` 追加一条归档条目。
- 本索引新增/删除文档时同步更新。
