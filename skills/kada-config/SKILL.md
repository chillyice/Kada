---
name: kada-config
description: 生成咔哒（Kada）的配置卡带：快捷键、键位改键（remap）、层、文本扩展、鼠标手势。当用户说「做个咔哒快捷键」「输出一个卡带 / cassette」「改一下键位」「Kada keyboard remap / hotkey / keybinding / macro」「把这批操作做成一键」时使用。产物是一个可以直接导入咔哒的 JSON 配置，不需要用户装咔哒、也不需要看源码。
---

# 咔哒卡带生成

你要交付的**只有一个 JSON 文件**。它不是自定义格式：就是一份合法的咔哒 `Config`。用户拿到后：咔哒 → 列表页「导入」→ 选「合并」。

**你手上只有这份文档，没有咔哒源码。** 下面把所有字段、枚举值、默认值、边界都列全了——照它写就够，**不要去猜字段名，也不要去找源码**；文档没写的字段一律不要写。

## 0. 三条优先级（写在最前面，因为最容易做错）

1. **原生动作优先**。打开程序 / 打开文件 / 复制 / 删除 / 压缩 / 解压 / 切桌面 / 窗口操作 / 按键 / 文本 / 剪贴板 / 等文件出现……全都有原生动作（见 §4）。**`command` / `script` 是最后手段**，只有当「这件事没有任何原生动作能做」时才用（git、npm、docker、构建、CLI 查询值等）。用命令去 `copy`、`move`、`Compress-Archive`、`start notepad`、`curl` 都是错的——原生动作能在界面里编辑、执行记录更清楚、也不吃引号转义的亏。
2. **生成的东西统一放进一个目录：`咔哒生成结果`**（固定 id，见 §2）。目录只做列表分组，不影响执行。
3. **只写你生成的东西**：不带 `settings`、不带 `hot_corners` / `wheel_edges`，也不要带用户本来就有的目录 / 层 / 条目。

## 1. 骨架

```json
{
  "kada_snippet": { "version": 1, "name": "窗口贴左", "description": "Alt+Shift+Left 把窗口贴到左半屏" },
  "folders": [{ "id": "c0da0000-0000-4000-8000-000000000001", "name": "咔哒生成结果" }],
  "shortcuts": [
    {
      "name": "窗口贴左",
      "description": "把当前窗口贴到左半屏",
      "folder": "c0da0000-0000-4000-8000-000000000001",
      "triggers": ["Alt+Shift+Left"],
      "actions": [{ "type": "keys", "keys": ["Meta", "Left"] }],
      "enabled": true
    }
  ]
}
```

- `kada_snippet` 只是元信息，咔哒会忽略；`version` 恒为 `1`。
- 顶层字段（都是数组，缺省即空）：`folders` / `layers` / `shortcuts` / `remaps` / `expansions` / `gestures`。
- 文件是 **UTF-8 JSON**，Windows 路径写双反斜杠（`C:\\Users\\me`），**不允许注释和尾逗号**。

## 2. 目录约定（硬规则）

- 用上面那个**固定 id**：`c0da0000-0000-4000-8000-000000000001`，名字写 `咔哒生成结果`。
- 每条 `shortcuts` / `remaps` / `expansions` / `gestures` 都写 `"folder": "c0da0000-0000-4000-8000-000000000001"`。
- 为什么固定 id：导入「合并」按 **id** 认目录。id 固定 → 重复导入不会长出一堆同名目录；用户改过名字也不会被覆盖（合并只加不改）。
- 用户明确要求「放到我自己某个目录下」时，才改用他给的目录 id（没有 id 就只能先建一个）。

## 3. 触发键语法

- **组合键**：`Ctrl+Alt+K`、`Ctrl+Shift+S`（修饰键 `Ctrl`/`Alt`/`Shift`/`Meta`，顺序随意）。
- **序列（leader）**：空格分隔多步，`F9 J K` = 依次按 F9 → J → K。
- **和弦**：`&` 表示同时按住，`F&J`、`Ctrl+F&J`；至少一个非修饰键。
- 一条条目可写多个 `triggers`，任一命中即触发。
- 单键也行：`F8`、`CapsLock`。
- **不要用系统保留组合**：`Ctrl+Alt+Delete`、`Win+L` 之类。
- 导入「合并」按触发键判重（有交集就跳过），所以随手选一个不常见的组合更稳。

## 4. 键名表（写错 = 整条失效）

```
A–Z  0–9  F1–F24
Enter Escape Tab Space Backspace Delete Insert CapsLock
Shift Control Alt Meta            ← 写 "Ctrl" 等价 Control
Home End PageUp PageDown Up Down Left Right
, . / \ ; " ` - = [ ]
MediaPlayPause MediaPrev MediaNext VolumeMute VolumeDown VolumeUp
Numpad0–9 NumpadAdd NumpadSubtract NumpadMultiply NumpadDivide NumpadDecimal NumpadEnter NumLock
MouseMiddle MouseBack MouseForward WheelUp WheelDown
```

## 5. 动作（`actions` 数组，按顺序执行）

每个动作都是一个带 `"type"` 的对象；`type` 全是小写 snake_case。除特别说明，都可以带 `"description": "..."`（纯展示）。

### 5.1 不用命令就能做完的事（优先用这些）

| 想做什么 | 用这个动作 |
|---|---|
| 输入文本 / 选中文本转大小写 | `{"type":"text","text":"你好"}`、`{"type":"text","mode":"to_upper"}` |
| 按键组合 / 按住再松开 | `{"type":"keys","keys":["Ctrl","S"]}`、`{"type":"keys","keys":["Shift"],"mode":"press"}` |
| 等一会儿 | `{"type":"pause_ms","ms":300}` |
| 打开程序（带参数） | `{"type":"app","operation":{"op":"launch","program":"C:\\x.exe","args":["--flag"]}}` |
| 关闭程序 / 最大化 / 最小化 / 唤起 / 重启 / 查是否在跑 | `{"type":"app","operation":{"op":"close","program":"x.exe"}}`（`maximize`/`minimize`/`activate`/`restart`/`status`） |
| 操作**当前窗口**（最大化还原 / 最小化 / 置顶 / 居中 / 关闭） | `{"type":"app","operation":{"op":"current_window","action":{"op":"toggle_topmost"}}}` |
| 打开网址 | `{"type":"open_url","url":"https://example.com"}` |
| 打开文件夹 / 打开文件 / 在资源管理器里定位 | `{"type":"os","operation":{"op":"open_folder","path":"D:\\Work"}}` |
| 复制 / 移动 / 删除 / 新建 | `{"type":"os","operation":{"op":"copy","source":"a","dest":"b"}}`（`cut`/`delete`/`new_folder`/`new_file`） |
| **压缩 / 解压 zip** | `{"type":"os","operation":{"op":"zip","source":"C:\\proj","dest":"D:\\proj.zip"}}`、`{"op":"unzip",...}` |
| 剪贴板读 / 写 / 临时粘贴 | `{"type":"clipboard","operation":{"op":"set_clipboard","text":"x"}}`（`get_clipboard` 要 `var`；`tempstr`） |
| 编码 / 哈希 | 先 `{"type":"clipboard","operation":{"op":"get_clipboard","var":"c"}}`，再 `{"type":"transform","operation":"base64_encode","input":"{c}","inject":true}` |
| 条件分支 | `{"type":"if","condition":{...},"then":[...],"otherwise":[...]}` |
| 等文件 / 进程 / 窗口出现 | `{"type":"wait_for","target":{"kind":"file","path":"D:\\done.txt"},"timeout_ms":30000}` |
| 切虚拟桌面 / 清空回收站 / 关显示器 / 切桌面图标 / 亮度 | `{"type":"system","operation":{"op":"switch_desktop","direction":"next"}}` |
| 让用户先填个值 | `{"type":"ask","var":"v","prompt":"版本号？","default":"1.0.0"}` |
| 几件事同时做 | `{"type":"parallel","actions":[...]}` |
| 分组（纯展示，执行等价于摊平） | `{"type":"group","name":"准备","actions":[...]}` |
| 临时禁用某步（内容保留） | `{"type":"disabled","action":{ ... }}` |

### 5.2 完整形状

```json
{"type":"text","text":"abc","mode":"input"}
{"type":"text","mode":"to_lower"}
{"type":"keys","keys":["Ctrl","Alt","K"],"mode":"tap"}
{"type":"pause_ms","ms":200}
{"type":"mouse","op":{"op":"click","button":"right"}}
{"type":"mouse","op":{"op":"move","dx":40,"dy":-20}}
{"type":"clipboard","operation":{"op":"get_clipboard","var":"clip"}}
{"type":"transform","operation":"url_encode","input":"{clip}","var":"enc"}
{"type":"ask","var":"name","prompt":"叫什么？","default":""}
{"type":"group","name":"准备","actions":[]}
{"type":"parallel","actions":[]}
{"type":"disabled","action":{"type":"pause_ms","ms":100}}
{"type":"os","operation":{"op":"get_file_props","path":"D:\\a.txt","var":"file"}}
{"type":"app","operation":{"op":"status","program":"x.exe","var":"running","retries":3,"interval_ms":500}}
{"type":"system","operation":{"op":"empty_recycle_bin"}}
{"type":"system","operation":{"op":"send_to","target":"notepad.exe","payload":{"kind":"text","text":"hi"}}}
{"type":"wait_for","target":{"kind":"process","program":"x.exe"},"timeout_ms":0}
{"type":"inline_script","source":"print(1+1);","var":"sum"}
{"type":"script","path":"C:\\tools\\a.ps1","interpreter":"powershell","args":["x"],"working_dir":"C:\\tools"}
```

要点：

- `text.mode`：`input`（默认）/ `to_upper` / `to_lower`；`masked: true` 只影响界面显示（密码类文本用）。
- `keys.mode`：`tap`（默认按下再抬起）/ `press` / `release`。
- `mouse.op`：`move{dx,dy}`、`move_to{x,y}`、`move_to_origin`、`click{button: left|right|middle}`、`scroll{dx,dy}`。
- `os` 的 op 全集：`copy` / `cut` / `paste` / `delete` / `new_file` / `new_folder` / `open_folder` / `open_file` / `reveal` / `zip` / `unzip` / `get_file_props`。都只有 `source` / `dest` / `path` / `var` 这几个字段。
- **`zip` 的行为（重要）**：`source` 是目录时，**目录自身会作为 zip 的根**；`dest` 不以 `.zip` 结尾会自动补上。所以「解压回父目录」得到的就是 `<目录名>\`，不会多一层。
- `transform.operation`：`base64_encode` / `base64_decode` / `url_encode` / `url_decode` / `hash_sha256` / `hash_sha512`。不写 `inject` 就必须写 `var`。
- `inline_script` 是**沙箱内的 Rhai 脚本**：无文件 / 网络 / 进程能力，只能 `get_var` / `set_var` / `print` / `trigger` / `shortcut_name`。别拿它当 PowerShell 用。

### 5.3 命令 / 脚本（最后手段）

只有原生动作做不到时才用，典型场景：git / npm / docker / 构建 / 启停服务 / 需要从环境里查一个值。

```json
{
  "type": "command",
  "shell": "powershell",
  "command": "(Get-Volume | Where-Object {$_.FileSystemLabel -ieq \"chillyice\"}).DriveLetter",
  "var": "drive_letter",
  "hide_window": true,
  "timeout_ms": 0,
  "show_output": "on_error"
}
```

| 字段 | 说明 |
|---|---|
| `shell` | `cmd` 或 `powershell`（`powershell` 仅 Windows；Linux 上 `cmd` 等价 `sh -c`） |
| `command` | 命令正文；支持 `{变量}` 占位符 |
| `working_dir` | 工作目录，空 = 继承咔哒进程目录；支持 `~` 和 `{变量}` |
| `timeout_ms` | 本步超时，缺省跟全局设置，`0` = 不限时 |
| `ok_exit_codes` | 算成功的退出码数组，空 = 只要 0（`robocopy` 的 1、安装器 3010 靠它） |
| `show_output` | `false`（默认）/ `true`（= `"always"`）/ `"on_error"` / `"never"` |
| `hide_window` | `true` = 不弹黑窗（后台命令都该开） |
| `var` | 非空时把标准输出（去首尾空白）写进变量 |
| `on_error` | `continue`（默认）/ `stop`（本步失败停整条链） |
| `if_running` | `run`（默认）/ `skip` / `queue`（同一条还在跑时怎么办） |
| `wait` | 默认 `true`；`false` = 起了就不管、不捕获输出 |

同族还有 `script`（跑一个脚本文件：`path` / `interpreter` / `interpreter_args` / `args` / `working_dir` / `timeout_ms` / `ok_exit_codes` / `show_output` / `hide_window` / `var` / `on_error` / `if_running`）和 `inline_script`（沙箱 Rhai，见上）。

**用命令时的三条要求**：`hide_window: true`；耗时不确定就写 `timeout_ms: 0`；只在真拿不到原生方案时用，并在 `description` 里写清「为什么必须用命令」。

## 6. 条件（`if` 与触发侧 `when` 共用）

```json
{"kind":"exists","path":"D:\\a.txt"}
{"kind":"not_exists","path":"D:\\a.txt"}
{"kind":"is_file","path":"D:\\a.txt"}
{"kind":"is_dir","path":"D:\\dir"}
{"kind":"equals","var":"clip","value":"yes"}
{"kind":"not_equals","var":"clip","value":"no"}
{"kind":"modified_within","path":"D:\\a.txt","minutes":30}
{"kind":"frontmost_app","app":"code.exe"}
{"kind":"not_frontmost_app","app":"code.exe"}
{"kind":"window_title_contains","text":"保存"}
{"kind":"device_is","id":"HHKB"}
```

- `equals` / `not_equals` 的 `var` 必须是前面动作写出来的变量；比较的是**完整路径**（`field` 留空），不写 `field` 就行。
- 前台类条件在 Linux/Wayland、取不到窗口时**恒不成立**。
- `when` 写在不成立时 = 该条**不存在**（落到同触发键的下一条，不吞键）。
- 变量只存在于**一次触发链**内；跨链不保留。
- 动作字符串里的 `{...}` **只认变量**（`{变量}` / `{变量.字段}`，未知的原样保留）；`{date}` / `{time}` / `{clipboard}` **只在文本扩展的 `replace` 里**才展开，别写进动作里——要剪贴板就先 `clipboard.get_clipboard` 存成变量。

## 7. 改键（`remaps`）

```json
{ "from": "CapsLock", "to": "Ctrl", "enabled": true, "folder": "c0da0000-0000-4000-8000-000000000001" }
```

`from` 和 `to` **都必填**（哪怕只用 `tap`/`hold`，也要写 `"to": ""`）。四种形态互斥，优先级 `sticky > oneshot > tap-hold > 普通 to`：

| 形态 | 字段 |
|---|---|
| 普通改键 | `to` |
| tap-hold | `tap` / `hold` / `tap_timeout_ms`（默认 200） |
| tap-dance | `tap` + `tap2` / `tap3` |
| 按住进层 | `hold_layer`（松开即退） |
| 长按锁层 | `lock_layer`（再长按切换；与 `hold_layer` 互斥） |
| 单次键 | `oneshot`（替你把该键按住到下一个键按完） |
| 粘滞键 | `sticky`（按一下锁、再按一下解） |

例：CapsLock 单击 Esc、长按 Ctrl；空格按住进导航层、单击仍是空格——

```json
"remaps": [
  { "from": "CapsLock", "to": "", "tap": "Escape", "hold": "Ctrl", "enabled": true },
  { "from": "Space", "to": "", "tap": "Space", "hold_layer": "<层 id>", "enabled": true }
]
```

**用层就必须一起带上层定义**：`"layers": [{"id":"nav-layer","name":"导航"}]`，id 用可读串就行，别和用户已有的重名。

## 8. 文本扩展（`expansions`）

输入触发词 + 空格 / 回车 / Tab 自动展开：

```json
{ "trigger": ";addr", "replace": "北京市朝阳区…", "enabled": true, "folder": "c0da0000-0000-4000-8000-000000000001" }
```

`replace` 支持 `{date}` / `{time}` / `{clipboard}`。触发词不能为空。

## 9. 鼠标手势（`gestures`）

按住右键画轨迹、松手按方向串匹配：

```json
{ "name": "关闭窗口", "pattern": "D,R", "actions": [{ "type": "keys", "keys": ["Alt","F4"] }], "enabled": true, "folder": "c0da0000-0000-4000-8000-000000000001" }
```

方向串用 `U`/`D`/`L`/`R` 与斜向 `UR`/`DR`/`DL`/`UL`，多段用逗号分隔（`D,R,U,L`）。手势需要用户在设置里打开「实验性功能」才生效。

## 10. 交付

产物叫 `<名字>.json`，正文只给这一个文件/代码块，然后一句话：

> 保存成 `xxx.json`，在咔哒里「导入 → 合并」即可。合并只增不删，不会动你现有的配置。

用户说**没装咔哒**：照样生成，补一句「这是咔哒的配置卡带，装好后导入就能用，也可以发给别人导入」。

**可选：直接落盘**（只有用户明确要求「帮我装进去」时才做，先备份）：

- Windows：`%APPDATA%\com.kada\config.json`
- macOS：`~/Library/Application Support/com.kada/config.json`
- Linux：`~/.local/share/com.kada/config.json`
- 写盘前把原文件另存 `config.json.pre-skill-<时间戳>`；写临时文件再 `rename` 覆盖（原子替换），不要直接截断原文件。
- 咔哒开着也能写：它每秒比对一次文件内容，能解析就自动采纳（约 1 秒生效）。
- 顺序：读现有 JSON → 按 id 并入 `folders`（`咔哒生成结果`）→ 追加条目（触发键 / `from` / `trigger` 撞了就跳过）→ 原子写 → 重新解析一遍自检。

## 11. 自检（交付前逐条过）

- [ ] 每条 `shortcuts` / `remaps` / `expansions` / `gestures` 都有 `"enabled": true`
- [ ] 每条都带 `"folder": "c0da0000-0000-4000-8000-000000000001"`，且 `folders` 里有这个 id
- [ ] **没有用 `command` 做原生动作能做的事**（§5.1 全覆盖到了吗？）；用了命令的，`hide_window: true`
- [ ] 每个动作都有 `"type"`，值全小写 snake_case；字段名和 §5 完全一致（不要自创字段）
- [ ] 用到的层 id 都在 `layers` 里定义过，且有 `hold_layer` / `lock_layer` 指过去
- [ ] `remaps` 每条都有 `from` 和 `to`（哪怕 `to` 是空串）
- [ ] 所有键名都在 §4 表里，拼写大小写一致
- [ ] 没有 `settings` / `hot_corners` / `wheel_edges`，没有注释，路径是双反斜杠
- [ ] JSON 解析得动（顶层是对象、无尾逗号）；有 shell 就用 `node -e "JSON.parse(require('fs').readFileSync('x.json','utf8'))"` 过一遍
