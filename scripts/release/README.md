# 一键发版脚本

`Release-Kada.ps1`：改五处版本号 → 跑文档 / 文案检查 → 提交 + 打 tag + 推送 → 等 Release 工作流（约 40 分钟）→ 发布草稿 → 触发 ihomy 镜像并校验 `latest.json`。

每次运行都会在 `scripts/release/logs/release-<时间戳>.log` 留一份完整 transcript（含每次 GitHub API 重试）。**卡住了先看这份日志的最后几行**——它停在哪一步就是卡在哪一步；控制台 / 消息中心末尾也会给出日志路径。

## 在 Kada 里挂快捷键

**两步动作**（脚本动作**没有参数输入框**，只有「路径 + 解释器」，版本号传不进去；且 `.ps1` 不能当可执行文件直接启动——必须用「命令」动作经 PowerShell 拼出参数）：

| 字段 | 值 |
|---|---|
| 动作 1 | 输入变量（`ask`）：变量 `release_version`，提示「输入新的版本号」 |
| 动作 2 | 命令（`command`，shell = PowerShell）：`& '<仓库>\scripts\release\Release-Kada.ps1' -Version '{release_version}'` |
| 隐藏控制台窗口 | 不勾（进度与脚本自己的 Read-Host 都在新控制台里） |
| 弹窗显示结果 | 不勾（脚本用 `Write-Host` 直接写控制台，捕获到的 stdout 基本是空的；失败看消息中心红点 / 控制台 / 日志文件） |
| 设置 → 动作超时 | `0`（不限时；默认 30 秒会把发版杀掉） |

> 第二动作别写成「运行脚本（Script）」：那样只会执行 `powershell.exe <脚本路径>`，既没有 `-Version`，`.ps1` 直启还会失败。`{release_version}` 由命令动作的变量替换填进去。

建议触发键用 `Ctrl+Alt+Shift+R` 这类不易误触的组合：弹框输入版本号是第二道确认。

## 参数

| 参数 | 作用 |
|---|---|
| `-Version 1.2.2` | 指定版本号；不给就弹输入框（从 Kada 触发时由动作 1 的 `{release_version}` 填） |
| `-Resume` | **收尾用**：跳过 bump / 检查 / commit / tag / push，直接「等这次 tag 的工作流 → 发布草稿 → 同步 ihomy」。不给 `-Version` 时取 `src-tauri/tauri.conf.json` 里的当前版本 |
| `-Manual` | 没有 `GITHUB_TOKEN` 时的旧路径：等工作流后打开 Releases 页，你点 Publish，脚本轮询 `releases/latest` |
| `-DryRun` | 只做前置检查并打印计划，不改文件、不提交 |
| `-NoChecks` | 跳过 `check-docs` / `check-i18n`（CI 仍会跑测试） |
| `-NoPause` | 结束时不等回车（给自动化调用） |

## 自动 / 人工分界

自动：版本号校验、五处 bump、文档与文案检查、commit / tag / push、等三平台工作流、发布草稿（PATCH 里带 `tag_name`，发布后回读核对「已发布 + tag 对得上 + 资产数」）、ihomy 同步与版本校验（最多等 15 分钟，跑完才判定）。

只能人工：

- **决定版本号并输入**（就是那个输入框）。
- **GitHub「Publish release」**：设了 `GITHUB_TOKEN` 就自动；没设脚本会在 **bump 之前**就拒绝（提示 `setx` 后重启 Kada），要人工发布就显式加 `-Manual`。
- **有未提交改动时的取舍**：脚本只检查已跟踪文件，脏了就拒绝（未跟踪文件不管）；提交范围由你决定。
- **CI 失败后的修代码**、**真机安装验收**、**自定义发布说明**。
- **一次性凭据**：GitHub PAT、`~/.ssh` 里能登录 `root@ihomy.top:19068` 的私钥、CI 里的签名密钥。

## PAT

Fine-grained token：Resource owner `chillyice`，仅选 `Kada`，权限 **Contents: Read and write** + **Actions: Read-only**。存进用户环境变量后重启 Kada：

```powershell
setx GITHUB_TOKEN "github_pat_xxx"
```

Token 只在本机环境变量里，不进仓库、不进配置同步。**注意继承时机**：Kada 常在开机后就常驻，`setx` 之后再触发快捷键，子进程拿到的还是旧环境——`setx` 完必须重启 Kada 一次。

## 前置条件

- 仓库根运行（脚本按自身位置定位根目录，从哪调用都行）。
- `git` 已配置推送凭据；`ssh -p 19068 root@ihomy.top` 能免密执行 `systemctl start kada-mirror.service`。
- 发版前把功能改动提交完（脚本拒绝脏的已跟踪文件）。

## 卡住了怎么收尾

脚本在「已 bump / 已 push」之后、发布之前失败（中断、关掉控制台、网络断、CI 超时），**tag 和 commit 都已经在远端，不用重来**：

```powershell
powershell -File scripts\release\Release-Kada.ps1 -Resume -NoPause
```

它从「等工作流」接着跑：工作流已绿就直接发布草稿 + 同步 ihomy；已经发布过就只做校验，不会重复发。先看日志确认停在哪一步，再看情况用 `-Resume`。

## 注意

- **发布草稿必须带 `tag_name`**：对 draft 发 PATCH 时省略 `tag_name`，GitHub 会把 release 改标成 `untagged-<hash>`，于是所有资产 / 更新清单的下载地址全变，已发布也等于发废（脚本已显式带上并回读校验）。
- **版本号只应该往前走**：脚本会 bump 到输入的版本，把 1.2.3 改成 1.2.2 是降级（本项目真发生过一次：1.2.3 的 tag 推过又弃用，本地 tag 还留着）。降级时脚本只警告不拦，但本地 / 远端已存在的 tag 会直接拒绝——重发请换新版本号。
- 脚本只 bump 版本号那 5 个文件；其它改动不会被它提交。
