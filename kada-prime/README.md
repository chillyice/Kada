# Kada'

Kada' 是一个**独立的 MouseInc 鼠标手势 / 滚轮快捷键试验台**，用来在把能力搬进 Kada 之前先试手感。它不属于 Kada 主工程（自带独立 `[workspace]`），也不读写 Kada 的配置。

当前只做 Windows。

## 打包 / 启动

```powershell
cd kada-prime
cargo build --release
New-Item -ItemType Directory -Force dist
Copy-Item target\release\kada-prime.exe "dist\Kada Prime.exe" -Force
Start-Process "dist\Kada Prime.exe"
```

首次启动会在 exe 同目录生成 `config.json`，日志写到 `kada-prime.log`。单实例：重复启动会提示已在运行。

## 默认能试什么

| 触发 | 动作 |
|---|---|
| 按住右键画 **↑** | 音量 + |
| **↓** | 音量 − |
| **←** | Alt+Left（后退） |
| **→** | Alt+Right（前进） |
| **↓→** | Ctrl+C（复制） |
| **↓→↑→** | 打开记事本 |
| **↑↓** | Win+D（显示桌面） |
| 右键按住 + 滚轮上/下 | Ctrl+Tab / Ctrl+Shift+Tab |
| 屏幕左边缘滚轮 | 音量 + / − |
| 屏幕四角（默认关） | 气泡提示四个角 |

- 画的时候会有一条橙色轨迹（托盘可关「绘制轨迹」）。
- 手势没命中时会把吞掉的右键**补回来**（迟约 10ms，菜单会正常弹出）。
- 未配置的方向串会在轨迹层和日志里显示，例如 `未配置：DRUR`，照抄进 `config.json` 就能用。

## 托盘菜单

**左键或右键单击托盘图标**（右键会被手势钩子吞掉再回放，左键更稳）：

- 启用手势 / 右键+滚轮切换 / 屏幕边缘滚轮 / 触发角 / 绘制轨迹
- 重新加载配置
- 编辑配置（记事本）
- 打开配置所在文件夹
- 退出 Kada'

开关会立即写回 `config.json`。

## 配置格式

```json
{
  "settings": {
    "enabled": true,
    "start_distance": 22,
    "timeout_ms": 1500,
    "min_segment": 16,
    "trace": true,
    "capture_unknown": true
  },
  "gestures": [
    {
      "name": "复制",
      "pattern": "DL",
      "enabled": true,
      "actions": [{ "type": "keys", "keys": "Ctrl+C" }]
    }
  ],
  "wheel": {
    "enabled": true,
    "up": [{ "type": "keys", "keys": "Ctrl+Tab" }],
    "down": [{ "type": "keys", "keys": "Ctrl+Shift+Tab" }]
  },
  "edge": {
    "enabled": true,
    "side": "left",
    "width": 3,
    "up": [{ "type": "volume", "dir": "up" }],
    "down": [{ "type": "volume", "dir": "down" }]
  },
  "corners": {
    "enabled": false,
    "size": 4,
    "top_left": [],
    "top_right": [],
    "bottom_left": [],
    "bottom_right": []
  }
}
```

方向串字母（屏幕坐标，y 向下）：

- `U` 上、`D` 下、`L` 左、`R` 右
- 斜向：`UR` 右上、`DR` 右下、`DL` 左下、`UL` 左上
- 也接受箭头 `↑↓←→` 和 MouseInc 风格单词 `UP` / `DOWN` / `UPLEFT` 等（加载时归一化）

`start_distance` 是起手距离，`min_segment` 是方向量化的一段最短长度；调大两者能减少误触。

### 动作类型

| type | 字段 | 说明 |
|---|---|---|
| `keys` | `keys` | 组合键，如 `Ctrl+Alt+K`、`Alt+Left`、`F9`、`VolumeUp` |
| `run` | `cmd` | ShellExecute 打开程序 / 文件 / 网址 |
| `volume` | `dir`: `up` / `down` / `mute` | 音量 |
| `notify` | `text` | 右下角气泡 |
| `click` | `button`: `left` / `right` / `middle` | 当前光标处点击 |
| `scroll` | `dy` | 滚轮，正上负下 |
| `move` | `dx`, `dy` | 相对移动光标 |
| `text` | `text` | 逐字输入 Unicode |
| `sleep` | `ms` | 停顿 |

一条触发里的多个动作会按顺序执行，间隔 80ms（MouseInc 是 100ms）。

## 已知限制 / 风险

- **全局抢右键**：只要「启用手势」，所有程序的右键按下都会被吞，抬起后再决定执行还是回放。游戏、CAD、远程桌面的右键拖拽会被影响；没有排除列表，只能托盘全局关。
- 没有全屏程序检测、没有按程序匹配（Kada 的 `when` 这里没有搬过来）。
- 只支持方向串，不支持 MouseInc 那种任意形状模板（S / P / 圆圈）。
- 回放和动作注入都是 `SendInput`，提权窗口可能拦（UIPI）；Kada' 没做提权伴随进程。
- 注入事件（带 `LLMHF_INJECTED`）不会被识别，所以不能用脚本合成手势来自测。
- 高精度滚轮 / 触摸板滚轮可能触发较密；边缘滚轮做了 150ms 节流。
- 配置和日志写在 exe 同目录；放进 `Program Files` 会没有写权限。

## 开发自测

正常启动会忽略所有 `SendInput` 合成事件（带 `LLMHF_INJECTED`），所以脚本合成的手势不会误触发。要在本机做端到端自测：

```powershell
$env:KADA_PRIME_SELFTEST='1'
Start-Process "dist\Kada Prime.exe"
Remove-Item Env:KADA_PRIME_SELFTEST
```

自测模式会接受外部合成鼠标事件，但 Kada' 自己回放的右键 / 动作注入带 `dwExtraInfo` 标记，仍然会被忽略，不会形成回环。

## 安全退出

托盘菜单「退出 Kada'」。如果钩子出问题导致右键异常，直接：

```powershell
Stop-Process -Name "Kada Prime" -Force
```

（进程名是 exe 文件名的主干，文件名带空格时用 `Get-Process | Where-Object { $_.Path -like '*Kada Prime*' } | Stop-Process`。）
