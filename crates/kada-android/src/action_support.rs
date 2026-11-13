//! 动作在安卓上的支持判定（无 root 档）。
//!
//! 单一事实来源：Rust 判定、Kotlin 照着执行——**Kotlin 侧不许再维护第二张支持表**，
//! 否则「报告说支持、执行时跳过」这种不一致迟早出现。
//!
//! 支持口径（S1-a 落地集）：
//! - `open_url`：系统浏览器打开（`ACTION_VIEW`），无需权限；
//! - `text`：写剪贴板 + `ACTION_PASTE` 到当前输入框（规划 §4 方案 ③ 的底座）；
//! - `clipboard`：写剪贴板（读回变量 / 临时粘贴暂不支持）；
//! - `pause_ms`：链内延迟；
//! - `group` / `disabled`：容器（递归判定成员）；
//! - 其它（命令 / 脚本 / 按键注入 / 鼠标 / 应用 / 文件 / 系统 / 条件 / 变量 / 脚本…）
//!   在无 root 档**整块不成立**：不执行、不占执行记录，但要在界面上标出来。

use kada_core::Action;

/// 本机（无 root 档）能执行的动作类型。与 [`kada_core::Action`] 的 serde tag 一一对应。
pub(crate) const SUPPORTED: &[&str] = &[
    "open_url",
    "text",
    "clipboard",
    "pause_ms",
    "group",
    "disabled",
];

/// 动作类型名（serde tag，与配置 JSON 里 `"type"` 一致）。
pub(crate) fn kind(action: &Action) -> String {
    match action {
        Action::Group { .. } => "group".into(),
        Action::Disabled { .. } => "disabled".into(),
        other => serde_json::to_value(other)
            .ok()
            .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
            .unwrap_or_else(|| "unknown".into()),
    }
}

/// 这个动作（含容器里的成员）在本机能不能执行。
pub(crate) fn is_supported(action: &Action) -> bool {
    match action {
        Action::Group { actions, .. } => actions.iter().all(is_supported),
        Action::Disabled { action } => is_supported(action),
        _ => SUPPORTED.contains(&kind(action).as_str()),
    }
}

/// 收集一串动作里**本机不支持**的类型名（去重、保序）。
pub(crate) fn collect(actions: &[Action]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    walk(actions, &mut out);
    out
}

fn walk(actions: &[Action], out: &mut Vec<String>) {
    for action in actions {
        match action {
            Action::Group { actions, .. } => walk(actions, out),
            Action::Disabled { action } => walk(std::slice::from_ref(action), out),
            other => {
                if !is_supported(other) {
                    let k = kind(other);
                    if !out.contains(&k) {
                        out.push(k);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_url_is_supported_and_command_is_not() {
        let ok = Action::OpenUrl {
            url: "https://example.com".into(),
            description: None,
        };
        assert!(is_supported(&ok));
        assert!(collect(std::slice::from_ref(&ok)).is_empty());

        let no = Action::Command {
            shell: kada_core::Shell::Cmd,
            command: "echo hi".into(),
            working_dir: String::new(),
            timeout_ms: None,
            ok_exit_codes: Vec::new(),
            show_output: Default::default(),
            hide_window: false,
            var: String::new(),
            wait: true,
            on_error: Default::default(),
            if_running: Default::default(),
            description: None,
        };
        assert!(!is_supported(&no));
        assert_eq!(collect(&[no]), vec!["command".to_string()]);
    }

    #[test]
    fn group_reports_only_unsupported_members() {
        let group = Action::Group {
            name: "g".into(),
            actions: vec![
                Action::OpenUrl {
                    url: "https://example.com".into(),
                    description: None,
                },
                Action::PauseMs {
                    ms: 10,
                    description: None,
                },
                Action::Keys {
                    keys: vec!["Ctrl+A".into()],
                    mode: Default::default(),
                    description: None,
                },
            ],
        };
        assert_eq!(collect(&[group]), vec!["keys".to_string()]);
    }
}
