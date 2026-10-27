# 一键发版脚本

`Release-Kada.ps1`：改五处版本号 → 跑文档 / 文案检查 → 提交 + 打 tag + 推送 → 等 Release 工作流（约 40 分钟）→ 发布草稿 → 触发 ihomy 镜像并校验 `latest.json`。

## 在 Kada 里挂快捷键

| 字段 | 值 |
|---|---|
| 动作类型 | 运行脚本（Script） |
| 脚本路径 | `<仓库>\scripts\release\Release-Kada.ps1` |
| 解释器 | `powershell.exe` |
| 隐藏控制台窗口 | 不勾（输入框与进度在控制台里） |
| 弹窗显示结果 | 勾上 |
| 设置 → 动作超时 | `0`（不限时；默认 30 秒会把发版杀掉） |

建议触发键用 `Ctrl+Alt+Shift+R` 这类不易误触的组合：弹框输入版本号是第二道确认。

## 参数

| 参数 | 作用 |
|---|---|
| `-Version 1.2.2` | 指定版本号；不给就弹输入框 |
| `-DryRun` | 只做前置检查并打印计划，不改文件、不提交 |
| `-NoChecks` | 跳过 `check-docs` / `check-i18n`（CI 仍会跑测试） |
| `-NoPause` | 结束时不等回车（给自动化调用） |

## 自动 / 人工分界

自动：版本号校验、五处 bump、文档与文案检查、commit / tag / push、等三平台工作流、发布草稿（有 token 时）、ihomy 同步与版本校验。

只能人工：

- **决定版本号并输入**（就是那个输入框）。
- **GitHub「Publish release」**：设了 `GITHUB_TOKEN` 就自动；没设脚本会打开 Releases 页等你点，然后每 30 秒探测 `releases/latest`。
- **有未提交改动时的取舍**：脚本只检查已跟踪文件，脏了就拒绝（未跟踪文件不管）；提交范围由你决定。
- **CI 失败后的修代码**、**真机安装验收**、**自定义发布说明**。
- **一次性凭据**：GitHub PAT、`~/.ssh` 里能登录 `root@ihomy.top:19068` 的私钥、CI 里的签名密钥。

## PAT

Fine-grained token：Resource owner `chillyice`，仅选 `Kada`，权限 **Contents: Read and write** + **Actions: Read-only**。存进用户环境变量后重启 Kada：

```powershell
setx GITHUB_TOKEN "github_pat_xxx"
```

Token 只在本机环境变量里，不进仓库、不进配置同步。

## 前置条件

- 仓库根运行（脚本按自身位置定位根目录，从哪调用都行）。
- `git` 已配置推送凭据；`ssh -p 19068 root@ihomy.top` 能免密执行 `systemctl start kada-mirror.service`。
- 发版前把功能改动提交完（脚本拒绝脏的已跟踪文件）。

## 注意

- 推送 tag 后如果手动「停止执行」，可能停在「已 push、未发布」的中间态；恢复办法是手动发布草稿，或删掉本地 / 远端 tag 重来。
- 脚本只 bump 版本号那 5 个文件；其它改动不会被它提交。
