//! 卡带（skills/kada-config/SKILL.md）里示例的 JSON 形状是否真能解析。
//! 技能文档是给人/AI 看的事实来源，这里守住它不漂移。

use kada_core::{Action, Config};

fn parse(s: &str) -> Config {
    serde_json::from_str(s).unwrap_or_else(|e| panic!("卡带解析失败: {e}\n{s}"))
}

#[test]
fn skill_minimal_cassette_parses() {
    let cfg = parse(
        r#"{
          "kada_snippet": { "version": 1, "name": "窗口贴左", "description": "Alt+Shift+Left" },
          "shortcuts": [{
            "name": "窗口贴左",
            "triggers": ["Alt+Shift+Left"],
            "actions": [{ "type": "keys", "keys": ["Meta", "Left"] }],
            "enabled": true
          }]
        }"#,
    );
    assert_eq!(cfg.shortcuts.len(), 1);
    assert!(cfg.shortcuts[0].enabled);
    assert!(matches!(
        &cfg.shortcuts[0].actions[0],
        Action::Keys { keys, .. } if keys == &["Meta", "Left"]
    ));
}

#[test]
fn skill_remap_and_layer_parse() {
    let cfg = parse(
        r#"{
          "layers": [{ "id": "nav", "name": "导航" }],
          "remaps": [
            { "from": "CapsLock", "to": "", "tap": "Escape", "hold": "Ctrl", "enabled": true },
            { "from": "Space", "to": "", "tap": "Space", "hold_layer": "nav", "enabled": true }
          ],
          "expansions": [{ "trigger": ";addr", "replace": "北京市朝阳区", "enabled": true }]
        }"#,
    );
    let caps = &cfg.remaps[0];
    assert_eq!(caps.tap.as_deref(), Some("Escape"));
    assert_eq!(caps.hold_key().map(kada_core::key_name), Some("Ctrl"));
    assert!(caps.is_tap_hold(), "tap/hold 应判定为 tap-hold");
    assert_eq!(cfg.remaps[1].hold_layer_id(), Some("nav"));
    assert_eq!(cfg.layers[0].id, "nav");
    assert!(cfg.expansions[0].enabled);
}

/// 技能里反复强调的坑：`enabled` 不写就是 false（导入后静默失效）。#[test]
fn missing_enabled_is_false() {
    let cfg = parse(r#"{ "shortcuts": [{ "triggers": ["F8"], "actions": [] }] }"#);
    assert!(!cfg.shortcuts[0].enabled);
}

#[cfg(feature = "automation")]
#[test]
fn skill_automation_actions_parse() {
    let cfg = parse(
        r#"{
          "shortcuts": [{
            "triggers": ["Ctrl+Alt+G"],
            "actions": [
              { "type": "command", "shell": "powershell", "command": "Get-Date" },
              { "type": "app", "operation": { "op": "launch", "program": "notepad.exe", "args": [] } },
              { "type": "os", "operation": { "op": "open_folder", "path": "D:\\Work" } },
              { "type": "clipboard", "operation": { "op": "set_clipboard", "text": "x" } },
              { "type": "transform", "operation": "base64_encode", "input": "{clipboard}", "inject": true },
              { "type": "system", "operation": { "op": "switch_desktop", "direction": "next" } },
              { "type": "if", "condition": { "kind": "frontmost_app", "app": "code.exe" },
                "then": [{ "type": "text", "text": "hi" }], "otherwise": [] },
              { "type": "app", "operation": { "op": "current_window", "action": { "op": "center" } } }
            ],
            "enabled": true
          }]
        }"#,
    );
    assert_eq!(cfg.shortcuts[0].actions.len(), 8);
}

/// 技能 §2 的目录约定 + §5.1 的「原生动作优先」清单：这些形状必须真能解析，
/// 否则文档就是在教模型写死条目。
#[cfg(feature = "automation")]
#[test]
fn skill_generated_folder_and_native_actions_parse() {
    let cfg = parse(
        r#"{
          "kada_snippet": { "version": 1, "name": "打包迁移" },
          "folders": [{ "id": "c0da0000-0000-4000-8000-000000000001", "name": "咔哒生成结果" }],
          "shortcuts": [{
            "name": "打包迁移",
            "folder": "c0da0000-0000-4000-8000-000000000001",
            "triggers": ["Ctrl+Alt+Shift+Z"],
            "actions": [
              { "type": "group", "name": "1/4 压缩", "actions": [
                { "type": "os", "operation": { "op": "zip", "source": "C:\\proj", "dest": "D:\\proj.zip" } }
              ] },
              { "type": "os", "operation": { "op": "copy", "source": "D:\\a.zip", "dest": "E:\\a.zip" } },
              { "type": "os", "operation": { "op": "unzip", "source": "E:\\a.zip", "dest": "E:\\" } },
              { "type": "os", "operation": { "op": "delete", "path": "D:\\tmp" } },
              { "type": "os", "operation": { "op": "get_file_props", "path": "E:\\a.zip", "var": "file" } },
              { "type": "parallel", "actions": [
                { "type": "pause_ms", "ms": 100 },
                { "type": "disabled", "action": { "type": "text", "text": "x" } }
              ] },
              { "type": "wait_for", "target": { "kind": "file", "path": "E:\\a.zip" }, "timeout_ms": 0 },
              { "type": "ask", "var": "v", "prompt": "版本号？", "default": "1.0.0" },
              { "type": "system", "operation": { "op": "empty_recycle_bin" } },
              { "type": "app", "operation": { "op": "status", "program": "a.exe", "var": "on", "retries": 3, "interval_ms": 500 } },
              { "type": "system", "operation": { "op": "send_to", "target": "a.exe", "payload": { "kind": "text", "text": "hi" } } },
              { "type": "inline_script", "source": "print(1+1);", "var": "sum" }
            ],
            "enabled": true
          }]
        }"#,
    );
    let folder = &cfg.folders[0];
    assert_eq!(folder.name, "咔哒生成结果");
    assert_eq!(cfg.shortcuts[0].folder.as_deref(), Some(folder.id.as_str()));
    assert_eq!(cfg.shortcuts[0].actions.len(), 12);
}

#[cfg(feature = "automation")]
#[test]
fn skill_trigger_syntax_is_accepted() {
    use kada_core::Trigger;
    for t in ["Ctrl+Alt+K", "F9 J K", "F&J", "Ctrl+F&J", "F8", "CapsLock"] {
        Trigger::parse(t).unwrap_or_else(|e| panic!("触发键「{t}」解析失败: {e}"));
    }
}

/// 演示卡带：导航层（按住 Tab）+ CapsLock 单击 Esc/长按 Ctrl + 剪贴板转大写。
#[test]
fn skill_demo_nav_layer_parses() {
    let cfg = parse(
        r#"{
          "kada_snippet": { "version": 1, "name": "导航层" },
          "layers": [{ "id": "nav", "name": "导航" }],
          "remaps": [
            { "from": "CapsLock", "to": "", "tap": "Escape", "hold": "Ctrl", "enabled": true },
            { "from": "Tab", "to": "", "tap": "Tab", "hold_layer": "nav", "enabled": true },
            { "from": "H", "to": "Left", "layer": "nav", "enabled": true },
            { "from": "J", "to": "Down", "layer": "nav", "enabled": true },
            { "from": "K", "to": "Up", "layer": "nav", "enabled": true },
            { "from": "L", "to": "Right", "layer": "nav", "enabled": true }
          ],
          "shortcuts": [{
            "name": "剪贴板转大写",
            "triggers": ["Ctrl+Alt+S"],
            "actions": [{ "type": "text", "mode": "to_upper" }],
            "enabled": true
          }]
        }"#,
    );
    assert_eq!(cfg.layers.len(), 1);
    assert_eq!(
        cfg.remaps
            .iter()
            .filter(|r| r.layer.as_deref() == Some("nav"))
            .count(),
        4
    );
    assert!(cfg.remaps[1].is_tap_hold());
}
