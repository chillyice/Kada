# Kada 文档索引

> 一页地图：每份文档一句话定位 + 谁该读。新成员/新会话从这里出发，不必通读全部。
> **要定位「改哪儿」→ `docs/代码地图.md`**（症状 → 文件 → grep 令牌 → 规则出处）。

## 新人阅读顺序

1. 根 `README.md` —— 项目是什么，30 秒
2. `AGENTS.md` —— 项目规则与铁律（AI 会话自动加载），写代码前必读
3. `docs/代码地图.md` —— 症状 → 代码位置
4. `docs/架构设计.md` —— 分层、事件怎么流转、关键机制为什么这样设计
5. `docs/需求设计说明书.md` —— 功能需求唯一活文档（键模型/配置模型/接口/规划）
6. `docs/变更归档.md` —— 索引表，按需检索

## 仓库内文档

| 文档 | 一句话定位 | 谁该读 |
|------|-----------|--------|
| `README.md`（根） | 公开门面：项目简介、特性、技术栈 | 所有人第一眼 |
| `AGENTS.md`（根） | 项目规则 + 铁律 + 导航索引：命名、键模型、钩子约束、命令、归档流程 | 所有开发者（人与 AI），动手前必读 |
| `docs/代码地图.md` | **症状 → 文件 → grep 令牌 → 规则出处**的路由表；令牌由 `scripts/check-docs.mjs` 守着（符号改名即红） | 定位代码、动手前 |
| `docs/架构设计.md` | 分层 / 事件流转 / 关键机制 / 跨平台策略 | 新成员建立全局认知；动架构前重读 |
| `docs/需求设计说明书.md` | 功能需求唯一活文档：键模型、配置模型、钩子、桌面壳、UI、规划事项（**按功能模块组织**，顶部是模块导航表） | 实现任何功能前；查接口/模型 |
| `docs/变更归档.md` | 已实现变更的**索引表**（版本 / 主题 / 主要文件 / 决策） | 检索历史实现时 |
| `docs/归档/` | 变更归档**正文四卷**：`01-奠基与核心` / `02-输入层与执行` / `03-界面与发布` / `04-V1x-收官` | 索引表定位到卷号后进去看细节 |
| `docs/变量提取与引用指南.md` | 变量怎么提取、怎么用占位符引用（含「移动硬盘盘符不定」实战） | 想用变量串联多个动作、做自动化时 |
| `docs/竞品分析与优化规划.md` | 竞品横向对比 + 差距清单 + 分阶段优化路线图（P0~P3） | 规划下一步功能方向、排优先级时 |
| `docs/触摸板手势可行性调研.md` | 规划 7.2-㊻ 的可行性调研：三平台（Linux libinput / macOS 私有 API / Windows Raw Input）能拿到什么手势事件、怎么接进现有 `Key` 模型、成本与落地顺序 | 评估「触摸板手势作触发源」是否立项 / 怎么落地时 |
| `docs/触摸板手势开发计划.md` | 触摸板手势的**跨会话进度账本**：现状（已落地 / 已修 / 待办 TP-01~TP-14）、下一步（平台采集、多指离散手势）、已定格的设计决定、已知坑、每次会话收尾清单 | 继续做触摸板相关功能时**先读它**；接手别人的半成品时 |
| `docs/鼠标侧键跨型号调研.md` | 规划 7.2-㊼ 的调研：标准侧键 MB4/MB5 跨型号一致、额外侧键无统一编号，结论是不扩键模型（用厂商软件重映射到 F13–F24 变通）；标准侧键「不出现」属厂商 divert，另见 `底层输入能力规划.md` 档位 C | 评估「支持更多鼠标侧键」时 |
| `docs/鼠标手势与功能扩展规划.md` | 鼠标手势专项（现状 / 分级目标 / 数据模型 / 三平台接线 / 配置全链路 / 体积实测 / 验收 / 待决策 / 试验台）+ 功能扩展候选（触发源 / 行为开关 / 动作库）+ 可扩展项 `EXT-01~EXT-37` + 扩展后效果 + 设置界面落点 + **逐项状态与跨会话进度日志** | 评估「鼠标手势怎么取舍」或「下一步扩展什么」时；开工前定方案 |
| `docs/底层输入能力规划.md` | 输入层下探的分档规划：三平台各站在哪一层、两个真实缺口（厂商 divert 按键 / 提权窗口注入）、四个档位（A 环境诊断 / B 提权伴随进程——**两档均已落地** / C HID++ 解锁侧键——下一个 / D 内核驱动评估后不做）、签名与信任成本、轻盈五条不变量、落地顺序 | 想「再往底层走」时先读它；判断某条限制该不该用驱动 / HID++ 解决时 |
| `docs/人工验证清单.md` | 只能真机 + 真人操作才能确认的验收项：可照做的步骤 + 预期现象 + 判失败（锁屏/唤醒/切窗复位、动作超时与中止、动作失败上报、未保存改动守卫、删除确认与导入合并 / 替换、配置外部修改监听、更新端到端含产物自检）；**顶部有逐条验证状态**（最近一轮 2026-10-10：人工验证清单 §1~§3、§5~§8 通过，§4 休眠唤醒未执行，§9~§14 与附录待真人复跑） | 验收相关改动、发版前 |

## 代码地图

> 目录与关键文件职责一览；**要按症状定位改哪儿，看 `docs/代码地图.md` 的路由表**。

```
crates/kada-core/           # 零重量纯逻辑：键模型、快捷键解析/匹配、配置模型、冲突检测、卡带摘取
  src/lib.rs                # Key/Modifier/RawEvent/Shortcut + parse/format/matches + Config/Action/Trigger/sanitize
  src/gesture.rs            # 鼠标手势 / 触摸板手势模型与识别（方向串量化 / 归一化 / 精确匹配 / 24 点模板，7.1-MG）
  src/merge.rs              # 配置合并（导入的「合并」方式：只增不删、不造新冲突、设置保留本机）
  src/snippet.rs            # 配置卡带（分享用）：按「类别+下标」摘出条目 + 被引用的层；卡带市场的索引模型/解析，见 AGENTS「铁律」
crates/kada-hook/           # 平台钩子引擎：全局键盘事件监听 + 拦截 + 注入
  src/win.rs                # Windows: WH_KEYBOARD_LL + WH_MOUSE_LL + swallow/Replace 状态机 + key↔VK + 自愈看门狗
  src/win/simulate.rs       # Windows: SendInput 注入 + 剪贴板文本；注入统一拆 Op 走 deliver（前台是提权窗口时交代理，见 架构设计.md §3.23）
  src/win/agent.rs          # Windows: 提权伴随进程（命名管道 + 行协议 + DACL/双向身份校验 + runas 拉起 + 父进程守望）
  src/win/hidpp.rs          # Windows: 罗技 HID++ 客户端（枚举/握手/关 divert + 本会话保持，见 说明书 §4.8 / 底层输入能力规划 档位 C）
  src/gesture.rs            # 鼠标手势采集状态机（平台无关）+ 轨迹出口（识别在壳层做，7.1-MG）
  src/trackpad.rs           # 触摸板手势采集状态机（按住触发键 → 记鼠标移动点 → 松键交轨迹，说明书 §5.9）
  src/linux.rs              # Linux: evdev + uinput 钩子 + 键码映射 + simulate
  src/macos.rs              # macOS: CGEventTap 钩子 + key↔虚拟键码 + AX 前台查询 + simulate（需「辅助功能」权限）
  src/lib.rs                # 平台中性入口：platform_input_env / elevated（提权窗口注入）/ hidpp（HID++ 解锁），非 Windows 为空实现
  examples/demo.rs          # M1 冒烟 demo（三平台，真人按键验证）
  examples/mouse_probe.rs   # 鼠标侧键分诊探针（Windows）：自检「按需挂钩子」+ 看侧键事件有没有到得了钩子（分诊厂商软件接管，见 docs/底层输入能力规划.md 档位 C）
  examples/input_env_probe.rs   # 输入层环境诊断探针：印「认出了谁 + HVCI 状态」，真机核对入口（见 人工验证清单.md §30）
  examples/hidpp_probe.rs       # 罗技 HID++ 探针：印设备 / 三个鼠标键的 divert 状态，`--unlock` 实写并回读（见 人工验证清单.md §34）
  examples/elevated_probe.rs    # 提权通道探针（Windows）：验 DACL / 双向身份校验 / 行协议往返 / 分片 / 坏 op 整批拒 / 断开即退（UAC 那段见 人工验证清单.md §31）
crates/kada-actions/        # 动作执行引擎（automation feature 门控）：run_actions/run_os/run_app/run_cmd/Script + CommandResult
  src/abort.rs              # 中止开关（代数计数器 + 运行计数 RunGuard，细节见 架构设计.md §3.17）
kada-prime/                 # Kada' 试验台（独立 crate，自带 [workspace]，Windows only，**不进主 workspace**）：鼠标手势 / 滚轮快捷键原型
  src/main.rs               # 运行时状态机 + 鼠标钩子（右键吞/回放）+ 托盘菜单 + 轨迹浮层 + 配置读写
  src/gesture.rs            # 方向量化 / 归一化 / 精确匹配 + 手势状态机（纯逻辑，带单测）
  src/config.rs             # 配置模型（手势 / 滚轮 / 边缘 / 触发角）+ 原子写 + 默认手势
  src/actions.rs            # 迷你动作执行器（组合键 / 启动 / 音量 / 通知 / 鼠标 …）+ 右键回放
  README.md                 # 用法 / 配置格式 / 动作类型 / 已知限制（方案与取舍见 docs/鼠标手势与功能扩展规划.md）
src-tauri/                  # Tauri 2 桌面壳（crate "kada"）
  src/lib.rs                # KadaState/decide/Recorder/消息中心/tauri commands/托盘常驻 + 窗口懒创建
  src/engine.rs             # 输入决策引擎（Tauri 无关、可单测）：tap-hold/层/和弦/键序列/热串状态机 + Inject 通道
  src/config_io.rs          # 配置读写：原子写 + .bak + 损坏留档与自愈；read_only 只读入口供监听
  src/config_watch.rs       # 配置外部修改监听（内容指纹比对），可单测
  src/update.rs             # 软件更新：检查/下载/安装状态机 + 进度节流，Tauri 无关部分可单测
  tauri.conf.json           # 窗口/图标/构建配置 + bundle.createUpdaterArtifacts + plugins.updater（公钥/端点）
ui/                         # Vite + TypeScript 前端（kada-ui）
  src/main.ts               # 配置界面：快捷键（动作流程图编排）/改键/文本扩展/宏录制/消息中心/设置页/卡带市场；草稿隔离 + 未保存守卫
  src/i18n.ts               # 界面文案：tr()/trt() 查表 + 静态 DOM 整块翻译；语言记 localStorage["kada.locale"]
  src/i18n.en.json          # 英文词典（中文原文 → English，key 与源码逐字一致；check-i18n 守门）
  src/mock.ts               # 纯浏览器演示模式：无 Tauri 壳时注入模拟 IPC + 演示配置（含各类事件桩，可直开浏览器调试）
  src/style.css             # 深色 UI 样式
snippets/                   # 卡带市场的内置索引托管：*.json 卡带 + index.json（脚本生成，客户端只 GET）
scripts/                    # build-snippet-index.mjs（卡带→索引，CI 用 --check 查同步）/ check-docs.mjs（文档一致性）/ check-i18n.mjs（界面文案覆盖）/ i18n-html.mjs（index.html 文案提取）/ verify-release.mjs（发版产物自检）/ linux-package（容器打包）/ ihomy-mirror（更新源每日镜像 + systemd 单元）
.github/workflows/          # ci.yml（三平台构建+测试+文档检查）；release.yml（打 v* tag 签名发版）
```

## 文档维护规矩

- **变更归档**：`docs/变更归档.md` 是**索引表**（每条一行：版本 / 主题 / 主要文件 / 决策），正文在 `docs/归档/` 分卷。新的变更 = 索引表加一行 + 当前卷末尾追加 `#####` 条目（标题带版本号），**不要写回 AGENTS.md**（AGENTS.md 只留规则 + 导航，20KB 上限——描述性细节一律外移到本目录，超限会挤掉注入上下文）。
- **功能需求**变更同步 `docs/需求设计说明书.md` 的**所属模块小节**（该文档已改按功能模块组织，没有版本时间线；`V0.x` / `V1.x` 版本号只写进归档）。
- **代码地图**：新增症状路由行时，grep 令牌必须是**源码里逐字存在的字符串**——`scripts/check-docs.mjs` 会核对，符号改名不更新就会红。
- 只能在真机由真人操作确认的验收项（锁屏/唤醒、切窗、GUI 点击等）写进 `docs/人工验证清单.md`，**不要**只留在归档的「未验证」一句话里——那里写不下步骤，接手的人不知道从哪下手。跑完一轮就把结果回填到该清单顶部的「验证状态」表（**没跑的标 ⬜ 未执行，别继承上一轮的结论**），并在归档追加一条。
- **改完文档跑 `node scripts/check-docs.mjs`**（CI 也跑）：失效路径引用 / 裸 `§` 引用歧义 / 代码地图令牌失效 / AGENTS.md 体积超限。
- 本索引新增/删除文档时同步更新。
