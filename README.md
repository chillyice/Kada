# 咔哒 Kada

轻快跨平台快捷键工具。全局键盘快捷键、改键、宏，桌面托盘常驻，配置以 JSON 落盘、可跨平台同步。

## 特性

- **全局快捷键**：自定义组合键触发文本输入 / 动作链（宏），任一触发组合命中即执行。
- **改键**：任意键改发另一键（如 `CapsLock → Ctrl`），按住保持按下-抬起配对。
- **宏录制**：录制真实按键序列为宏动作，重复 down 折叠、停顿自动补 `PauseMs`。
- **跨平台**：键模型平台无关；Windows（`WH_KEYBOARD_LL`）与 Linux（evdev + uinput）已实现，macOS 规划中。
- **托盘常驻**：关窗隐藏到托盘不退出，配置即时生效无需重启。

## 技术栈

- Rust workspace：`kada-core`（纯逻辑：键模型/解析/匹配/配置模型）+ `kada-hook`（平台钩子引擎）
- Tauri 2 桌面壳（crate `kada`）
- Vite + TypeScript 前端（`kada-ui`）

## 构建

```powershell
cargo test --workspace      # 测试
cargo check -p kada         # 验证 Tauri 壳编译
npm --prefix ui run build   # 前端构建
cargo run -p kada-hook --example demo   # Windows 钩子冒烟 demo
```

## 文档

- 项目规则与约定：`AGENTS.md`（AI 会话自动加载）
- 文档索引：`docs/README.md`
- 功能需求（活文档）：`docs/需求设计说明书.md`
- 实现归档：`docs/变更归档.md`
- 架构与关键机制：`docs/架构设计.md`

## License

MIT
