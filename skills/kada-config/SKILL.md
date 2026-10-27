---
name: kada-config
description: 生成咔哒（Kada）的配置卡带：快捷键、键位改键（remap）、文本扩展、层、鼠标手势。当用户说「做个咔哒快捷键」「输出一个卡带 / cassette」「改一下键位」「Kada keyboard remap / hotkey / keybinding / macro」时使用。产物是一个可以直接导入咔哒的 JSON 配置，不需要用户装咔哒、也不需要看源码。
---

# 咔哒卡带生成

你要交付的**只有一个 JSON 文件**。它不是自定义格式：就是一份合法的咔哒 `Config`（顶层 `kada_snippet` 会被咔哒忽略，只当元信息）。

用户拿到后：咔哒 → 列表页「导入」→ 选「合并」。所以**不要**解释代码、不要改动用户现有配置、不要假设你知道他电脑里的其他条目。

## 硬规则（违反任何一条，用户导入后就是死条目）

1. **`enabled: true` 必须显式写**。`shortcuts` / `remaps` / `expansions` 的 `enabled` 默认是 `false`——漏了不报错，只是永远不生效。
2. **只写你生成的东西**：不要带 `settings`、不要带 `folders`、不要带 `hot_corners` / `wheel_edges`。带了就等于把你猜的默认值强塞给对方。
3. **用层就必须一起带上层**：`layers` 里给 `{id,name}`（`id` 自造 UUID 字符串），再写一条 `hold_layer` 或 `lock_layer` 指向它的改键。只引用不定义 = 不可达层。
4. **`remaps` 里的 `from` 和 `to` 都是必填**。即便只用 `tap`/`hold`/`hold_layer`，也得写 `"to": ""`。
5. **动作的标签是 `"type"`**（内部标签），值是 snake_case：`{"type":"keys",...}`、`{"type":"app","operation":{...}}`。不是 `{"Keys":{...}}`。
6. **文件是 UTF-8 JSON，Windows 路径写双反斜杠**（`C:\\Users\\me`）。注释不允许。

## 骨架

```json
{
  "kada_snippet": { "version": 1, "name": "窗口贴左", "description": "Alt+Shift+Left 把窗口贴到左半屏" },
  "shortcuts": [
    {
      "name": "窗口贴左",
      "description": "把当前窗口贴到左半屏",
      "triggers": ["Alt+Shift+Left"],
      "actions": [{ "type": "keys", "keys": ["Meta", "Left"] }],
      "enabled": true
    }
  ]
}
```

`Config` 顶层字段（都是数组，缺省即空）：`shortcuts` / `remaps` / `expansions` / `layers` / `folders` / `gestures` / `hot_corners` / `wheel_edges` / `settings`。

## 键名表

写进 `triggers` / `keys` / `from` / `to` 的键名，只认这一套（大小写敏感）：

```
A–Z  0–9  F1–F24
Enter Escape Tab Space Backspace Delete Insert CapsLock
Shift Control Alt Meta            ← 写 "Ctrl" 也行，等价 Control
Home End PageUp PageDown Up Down Left Right
, . / \ ; " ` - = [ ]             ← 逗号句点等直接用符号
MediaPlayPause MediaPrev MediaNext VolumeMute VolumeDown VolumeUp
Numpad0–9 NumpadAdd NumpadSubtract NumpadMultiply NumpadDivide NumpadDecimal NumpadEnter NumLock
MouseMiddle MouseBack MouseForward WheelUp WheelDown
```

## 触发键语法

- **组合键**：`Ctrl+Alt+K`、`Ctrl+Shift+S`。修饰键顺序随意。
- **序列（leader）**：用空格分隔多步，`F9 J K` = 依次按 F9 → J → K。
- **和弦**：`&` 表示同时按住，`F&J`、`Ctrl+F&J`。至少一个非修饰键成员。
- 一个 `shortcuts` 条目可以写多个 `triggers`，任一命中即触发。
- **单个键也可以**：`F8`、`CapsLock`。
- 不要用 `Ctrl+Alt+Delete`、`Win+L` 这类系统保留组合。

## 动作（`actions` 数组，按顺序执行）

不写 `mode` 的值都是默认值，可以省略。

| 目的 | JSON |
|---|---|
| 按键组合 | `{"type":"keys","keys":["Ctrl","S"]}` |
| 按住不放 | `{"type":"keys","keys":["Shift"],"mode":"press"}`，之后用 `"mode":"release"` 抬起 |
| 输入文本 | `{"type":"text","text":"你好"}` |
| 选中文本转大小写 | `{"type":"text","mode":"to_upper"}`（`to_upper`/`to_lower`，不需要 `text`） |
| 等待 | `{"type":"pause_ms","ms":150}` |
| 执行命令 | `{"type":"command","shell":"cmd","command":"start notepad"}`（`shell` 也可 `powershell`；`show_output` 默认 false） |
| 打开程序 | `{"type":"app","operation":{"op":"launch","program":"C:\\...\\Code.exe","args":[]}}` |
| 打开网址 | `{"type":"open_url","url":"https://example.com"}` |
| 打开文件夹/文件 | `{"type":"os","operation":{"op":"open_folder","path":"D:\\Work"}}`（`open_file` / `reveal` 同理） |
| 文件操作 | `{"type":"os","operation":{"op":"copy","source":"a.txt","dest":"b.txt"}}`（`cut`/`paste`/`delete`/`new_folder`/`zip`/`unzip`） |
| 剪贴板 | `{"type":"clipboard","operation":{"op":"set_clipboard","text":"x"}}`（`get_clipboard` 要 `var`；`tempstr` = 粘贴后还原） |
| 算法 | `{"type":"transform","operation":"base64_encode","input":"{clipboard}","inject":true}`（`base64_decode`/`url_encode`/`url_decode`/`hash_sha256`/`hash_sha512`；不 `inject` 就要给 `var`） |
| 条件 | `{"type":"if","condition":{"kind":"frontmost_app","app":"code.exe"},"then":[ ... ],"otherwise":[ ... ]}` |
| 分组 | `{"type":"group","name":"准备","actions":[ ... ]}` |
| 并行 | `{"type":"parallel","actions":[ ... ]}` |
| 当前窗口 | `{"type":"app","operation":{"op":"current_window","action":{"op":"maximize_restore"}}}`（`minimize`/`toggle_topmost`/`center`/`close`/`close_same_app`，仅 Windows） |
| 系统 | `{"type":"system","operation":{"op":"switch_desktop","direction":"next"}}`（`empty_recycle_bin`/`monitor_off`/`toggle_desktop_icons`/`brightness`） |
| 跑脚本 | `{"type":"script","path":"C:\\x.ps1","interpreter":"powershell"}` |

条件 `kind`：`exists` / `not_exists` / `is_file` / `is_dir`（`path`）、`equals` / `not_equals`（`var`,`value`）、`modified_within`（`path`,`minutes`）、`frontmost_app` / `not_frontmost_app`（`app`）、`window_title_contains`（`text`）、`device_is`（`id`）。

变量：命令/脚本动作的 `var` 接住输出，后续动作的字符串里用 `{变量名}` 引用。

## 改键（`remaps`）

```json
{ "from": "CapsLock", "to": "Ctrl", "enabled": true }
```

四种形态**互斥**，按 `sticky > oneshot > tap-hold > 普通 to` 的优先级生效：

| 形态 | 字段 | 说明 |
|---|---|---|
| 普通改键 | `to` | 按 `from` 就发 `to` |
| tap-hold | `tap` / `hold` / `tap_timeout_ms` | 短按发 `tap`、长按发 `hold`，默认阈值 200ms |
| tap-dance | `tap` + `tap2` / `tap3` | 单击/双击/三击各自输出，缺省回落到上一级 |
| 进层 | `hold_layer` | 按住 `from` 期间该层生效，松开即退 |
| 锁层 | `lock_layer` | 长按 `from` 切换该层开关，层保持生效 |
| 单次键 | `oneshot` | 按一下 `from`，替你把 `oneshot` 键按住到下一个键按完 |
| 粘滞键 | `sticky` | 按一下锁定 `sticky` 键，再按一下解锁 |

例：CapsLock 单击 Esc、长按 Ctrl、按住空格进导航层——

```json
"remaps": [
  { "from": "CapsLock", "to": "", "tap": "Escape", "hold": "Ctrl", "enabled": true },
  { "from": "Space", "to": "", "hold_layer": "<层 id>", "tap": "Space", "enabled": true }
]
```

`layers`: `[{"id":"nav-layer","name":"导航"}]`——`id` 只是字符串，用可读名字就行，不要和现有层重名。

## 文本扩展（`expansions`）

输入触发词 + 空格/回车/Tab 自动展开：

```json
{ "trigger": ";addr", "replace": "北京市朝阳区…", "enabled": true }
```

`replace` 支持 `{date}`、`{time}`、`{clipboard}`。触发词不能为空。

## 交付

产物叫 `<名字>.json`，正文只给这一个文件/代码块，然后一句话：

> 保存成 `xxx.json`，在咔哒里「导入 → 合并」即可。合并只增不删，不会动你现有的配置。

如果用户说**没装咔哒**：照样生成，补一句「这是咔哒的配置卡带，装好后导入就能用，也可以发给别人导入」。

## 自检（交付前逐条过）

- [ ] 每条 `shortcuts` / `remaps` / `expansions` 都有 `"enabled": true`
- [ ] 每个动作都有 `"type"`，值全小写 snake_case
- [ ] 用到的层 id 都在 `layers` 里定义过，且有 `hold_layer` / `lock_layer` 指过去
- [ ] `remaps` 每条都有 `from` 和 `to`（哪怕 `to` 是空串）
- [ ] 所有键名都在上面的表里，拼写大小写一致
- [ ] 没有 `settings` / `folders`，没有注释，路径是双反斜杠
- [ ] JSON 能被解析（顶层是对象，数组用逗号分隔，无尾逗号）
