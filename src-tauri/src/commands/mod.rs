// 命令层（T-04 从 lib.rs 搬出）：按域分组的 `#[tauri::command]`。
// `generate_handler!` 里用 `commands::<域>::<命令>` 的路径形式注册（编译期校验）。

pub(crate) mod config;
