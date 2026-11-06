// kada-core 单元测试（C-01：从 lib.rs 搬出，内容与顺序原样保留）。

use crate::*;

#[test]
fn parse_and_format_roundtrip() {
    let cases = [
        "Ctrl+K",
        "Ctrl+Alt+Shift+Space",
        "A",
        "Alt+F4",
        "Shift+CapsLock", // Shift 作为修饰键 + CapsLock 主键
        "Meta+J",
        "Ctrl+-",
        "Alt+/",
        "Shift", // 裸修饰键（改键场景）
        "Ctrl",
    ];
    for s in cases {
        let st: Shortcut = s.parse().unwrap();
        let back = format_shortcut(&st);
        assert_eq!(back, s, "roundtrip {s} -> {back}");
    }
}

#[test]
fn parse_errors() {
    assert!(matches!("".parse::<Shortcut>(), Err(ParseError::Empty)));
    assert!(matches!("+".parse::<Shortcut>(), Err(ParseError::Empty)));
    assert!("Foo+K".parse::<Shortcut>().is_err());
    assert!("Ctrl+K+J".parse::<Shortcut>().is_err());
}

#[test]
fn wheel_is_a_mouse_trigger_but_only_when_bound() {
    let mut cfg = Config::default();
    assert!(!config_uses_mouse(&cfg), "空配置不该挂鼠标钩子");
    assert!(!config_uses_wheel(&cfg));
    cfg.shortcuts.push(ShortcutItem {
        triggers: vec!["Ctrl+WheelUp".into()],
        enabled: true,
        ..Default::default()
    });
    assert!(config_uses_mouse(&cfg), "滚轮触发键也要挂鼠标钩子");
    assert!(config_uses_wheel(&cfg), "绑了滚轮 → 钩子层逐事件处理");
    // 只作为改键的**目标** / 注入目标不算触发侧：不监听滚轮。
    let mut injected = Config::default();
    injected.remaps.push(Remap {
        from: "F1".into(),
        to: "ScrollUp".into(),
        enabled: true,
        ..Default::default()
    });
    assert!(!config_uses_wheel(&injected));
}

#[test]
fn new_keys_roundtrip_and_aliases() {
    // 新增键：解析 → 渲染回环必须一致。
    let cases = [
        "F13",
        "F24",
        "MediaPlayPause",
        "MediaPrev",
        "MediaNext",
        "VolumeMute",
        "VolumeDown",
        "VolumeUp",
        "NumLock",
        "Numpad0",
        "Numpad9",
        "NumpadAdd",
        "NumpadSubtract",
        "NumpadMultiply",
        "NumpadDivide",
        "NumpadDecimal",
        "NumpadEnter",
        "MouseMiddle",
        "MouseBack",
        "MouseForward",
        "WheelUp",
        "WheelDown",
    ];
    for s in cases {
        let k: Key = s.parse().unwrap();
        assert_eq!(key_name(k), s, "roundtrip {s}");
    }
    // 别名解析到同一键，且主键名渲染为规范名。
    assert_eq!("MB4".parse::<Key>().unwrap(), Key::MouseBack);
    assert_eq!(key_name("XButton2".parse::<Key>().unwrap()), "MouseForward");
    assert_eq!("ScrollUp".parse::<Key>().unwrap(), Key::WheelUp);
    assert_eq!("NumpadPlus".parse::<Key>().unwrap(), Key::NumpadAdd);
    assert_eq!("VolUp".parse::<Key>().unwrap(), Key::VolumeUp);
    // 组合键触发：无修饰的鼠标键与带修饰的媒体键都能解析。
    let m: Shortcut = "Ctrl+VolumeUp".parse().unwrap();
    assert_eq!(format_shortcut(&m), "Ctrl+VolumeUp");
    let bare: Shortcut = "MouseBack".parse().unwrap();
    assert_eq!(format_shortcut(&bare), "MouseBack");
}

#[test]
fn matcher_superset_rule() {
    let ctrl_k: Shortcut = "Ctrl+K".parse().unwrap();
    let ev = |key: Key, mods: &[Modifier], pressed: bool| RawEvent {
        key,
        mods: mods.iter().copied().collect(),
        pressed,
    };
    // Ctrl+K 命中
    assert!(matches(&ev(Key::K, &[Modifier::Ctrl], true), &ctrl_k));
    // Ctrl+Shift+K 也命中（修饰键超集）
    assert!(matches(
        &ev(Key::K, &[Modifier::Ctrl, Modifier::Shift], true),
        &ctrl_k
    ));
    // 缺修饰不命中
    assert!(!matches(&ev(Key::K, &[], true), &ctrl_k));
    assert!(!matches(&ev(Key::K, &[Modifier::Alt], true), &ctrl_k));
    // 主键不同不命中
    assert!(!matches(&ev(Key::J, &[Modifier::Ctrl], true), &ctrl_k));
}

#[cfg(test)]
mod config_tests {
    use crate::*;

    #[test]
    fn config_json_roundtrip() {
        let cfg = Config {
            folders: vec![],
            layers: vec![],
            shortcuts: vec![ShortcutItem {
                name: Some("地址".into()),
                description: Some("常用收货地址".into()),
                triggers: vec!["Ctrl+Alt+K".into(), "Alt+K".into()],
                actions: vec![Action::Text {
                    text: "上海市徐汇区……".into(),
                    mode: TextMode::Input,
                    masked: false,
                    description: None,
                }],
                folder: None,
                layer: None,
                enabled: true,
                ..Default::default()
            }],
            remaps: vec![Remap {
                from: "CapsLock".into(),
                to: "Ctrl".into(),
                enabled: true,
                ..Default::default()
            }],
            expansions: vec![],
            gestures: vec![],
            touchpad_gestures: vec![],
            hot_corners: vec![],
            wheel_edges: vec![],
            settings: Settings::default(),
        };
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg, "json:\n{json}");
    }

    #[test]
    fn config_missing_fields_default() {
        // 手工编辑的配置允许缺字段、带未知字段。
        let json = r#"{
            "shortcuts": [{ "triggers": ["Ctrl+K"], "actions": [{ "type": "text", "text": "hi" }] }],
            "extra_field": 1
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(!cfg.shortcuts[0].enabled);
        assert_eq!(
            cfg.shortcuts[0].actions,
            vec![Action::Text {
                text: "hi".into(),
                mode: TextMode::Input,
                masked: false,
                description: None
            }]
        );
    }

    #[test]
    fn send_to_validates_target_and_keys() {
        let ok = Action::SendTo {
            target: "chrome.exe".into(),
            payload: SendPayload::Text { text: "hi".into() },
            description: None,
        };
        assert!(ok.validate().is_ok());
        // 空目标 / 空按键 / 坏键名都拒绝；空文本是合法（无输出）。
        assert!(Action::SendTo {
            target: "  ".into(),
            payload: SendPayload::Text { text: "hi".into() },
            description: None,
        }
        .validate()
        .is_err());
        assert!(Action::SendTo {
            target: "chrome.exe".into(),
            payload: SendPayload::Keys { keys: vec![] },
            description: None,
        }
        .validate()
        .is_err());
        assert!(Action::SendTo {
            target: "chrome.exe".into(),
            payload: SendPayload::Keys {
                keys: vec!["NotAKey".into()]
            },
            description: None,
        }
        .validate()
        .is_err());
        assert!(Action::SendTo {
            target: "chrome.exe".into(),
            payload: SendPayload::Text {
                text: String::new()
            },
            description: None,
        }
        .validate()
        .is_ok());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn actions_json_roundtrip_and_validate() {
        let actions = vec![
            Action::Text {
                text: "你好".into(),
                mode: TextMode::Input,
                masked: false,
                description: None,
            },
            Action::Cmd {
                command: "echo hi".into(),
                show_output: false,
                var: "".into(),
                description: None,
            },
            Action::Powershell {
                command: "Get-Date".into(),
                show_output: true,
                var: "".into(),
                description: None,
            },
            Action::Launch {
                program: "notepad.exe".into(),
                args: vec!["a.txt".into()],
                description: None,
            },
            Action::OpenFolder {
                path: "C:\\Users".into(),
                description: None,
            },
            Action::Keys {
                keys: vec!["Ctrl".into(), "C".into()],
                mode: KeysMode::Tap,
                description: None,
            },
            Action::SendTo {
                target: "notepad.exe".into(),
                payload: SendPayload::Keys {
                    keys: vec!["Ctrl".into(), "S".into()],
                },
                description: None,
            },
            Action::SendTo {
                target: "记事本".into(),
                payload: SendPayload::Text {
                    text: "hi {name}".into(),
                },
                description: None,
            },
            Action::PauseMs {
                ms: 100,
                description: None,
            },
        ];
        for a in &actions {
            assert!(a.validate().is_ok(), "动作应通过校验: {a:?}");
        }
        let json = serde_json::to_string(&actions).unwrap();
        let back: Vec<Action> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, actions);

        // 坏键名 / 空组合 / 空命令必被拒绝
        assert!(Action::Keys {
            keys: vec!["NotAKey".into()],
            mode: KeysMode::Tap,
            description: None
        }
        .validate()
        .is_err());
        assert!(Action::Keys {
            keys: vec![],
            mode: KeysMode::Tap,
            description: None
        }
        .validate()
        .is_err());
        assert!(Action::Cmd {
            command: "  ".into(),
            show_output: false,
            var: "".into(),
            description: None
        }
        .validate()
        .is_err());
        assert!(Action::Launch {
            program: "".into(),
            args: vec![],
            description: None
        }
        .validate()
        .is_err());

        // EXT-16：默认 tap 不落盘（旧配置形状不变），press / release 正常往返。
        let tap = Action::Keys {
            keys: vec!["Ctrl".into()],
            mode: KeysMode::Tap,
            description: None,
        };
        let json = serde_json::to_string(&tap).unwrap();
        assert!(!json.contains("mode"), "默认 tap 不该写进 JSON：{json}");
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), tap);

        for mode in [KeysMode::Press, KeysMode::Release] {
            let a = Action::Keys {
                keys: vec!["Ctrl".into()],
                mode,
                description: None,
            };
            let json = serde_json::to_string(&a).unwrap();
            assert!(json.contains("\"mode\":\"press\"") || json.contains("\"mode\":\"release\""));
            assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), a);
        }
    }

    /// 鼠标模拟动作（7.1-㊱）：序列化往返、点击缺省左键、全 0 偏移被拒（不依赖 automation）。
    #[test]
    fn mouse_action_roundtrip_and_validate() {
        let actions = vec![
            Action::Mouse {
                op: MouseOp::Move { dx: 10, dy: -5 },
                description: None,
            },
            Action::Mouse {
                op: MouseOp::MoveTo { x: -1920, y: 1080 },
                description: None,
            },
            Action::Mouse {
                op: MouseOp::MoveToOrigin,
                description: None,
            },
            Action::Mouse {
                op: MouseOp::Click {
                    button: MouseButton::Middle,
                },
                description: None,
            },
            Action::Mouse {
                op: MouseOp::Scroll { dx: 0, dy: 3 },
                description: Some("向上滚".into()),
            },
        ];
        for a in &actions {
            assert!(a.validate().is_ok(), "动作应通过校验: {a:?}");
        }
        let json = serde_json::to_string(&actions).unwrap();
        assert!(
            json.contains("\"type\":\"mouse\""),
            "序列化带 type=mouse：{json}"
        );
        assert!(json.contains("\"op\":\"move\""), "序列化带 op=move：{json}");
        let back: Vec<Action> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, actions);

        // 点击缺省左键；移动 / 滚轮偏移全 0 被拒。
        let click: Action =
            serde_json::from_str(r#"{"type":"mouse","op":{"op":"click"}}"#).unwrap();
        assert_eq!(
            click,
            Action::Mouse {
                op: MouseOp::Click {
                    button: MouseButton::Left
                },
                description: None
            }
        );
        assert!(Action::Mouse {
            op: MouseOp::Move { dx: 0, dy: 0 },
            description: None
        }
        .validate()
        .is_err());
        assert!(Action::Mouse {
            op: MouseOp::Scroll { dx: 0, dy: 0 },
            description: None
        }
        .validate()
        .is_err());
    }

    /// 禁用标记（编辑页的「禁用」开关）：外面套一层 `disabled`，内容原样往返，校验照旧落到里面那条。
    #[test]
    fn disabled_action_roundtrip_and_validate() {
        let disabled = Action::Disabled {
            action: Box::new(Action::PauseMs {
                ms: 500,
                description: None,
            }),
        };
        assert!(disabled.validate().is_ok());
        let json = serde_json::to_string(&disabled).unwrap();
        assert!(
            json.contains("\"type\":\"disabled\""),
            "序列化带 type=disabled：{json}"
        );
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), disabled);

        // 打开开关后要能原样跑：里面的非法配置不能藏在禁用壳后面混过去。
        let bad = Action::Disabled {
            action: Box::new(Action::Keys {
                keys: vec![],
                mode: KeysMode::Tap,
                description: None,
            }),
        };
        assert!(bad.validate().is_err());
    }
}

#[cfg(test)]
mod conflict_tests {
    use crate::*;

    fn item(trigger: &str) -> ShortcutItem {
        ShortcutItem {
            triggers: vec![trigger.into()],
            actions: vec![Action::Text {
                text: String::new(),
                mode: TextMode::Input,
                masked: false,
                description: None,
            }],
            enabled: true,
            ..Default::default()
        }
    }

    fn remap(from: &str, to: &str) -> Remap {
        Remap {
            from: from.into(),
            to: to.into(),
            enabled: true,
            ..Default::default()
        }
    }

    fn errors(cfg: &Config) -> Vec<Conflict> {
        detect_conflicts(cfg, &PlatformCaps::default())
            .into_iter()
            .filter(|c| c.severity == Severity::Error)
            .collect()
    }

    fn warns(cfg: &Config) -> Vec<Conflict> {
        detect_conflicts(cfg, &PlatformCaps::default())
            .into_iter()
            .filter(|c| c.severity == Severity::Warn)
            .collect()
    }

    #[test]
    fn duplicate_triggers_are_error() {
        let cfg = Config {
            shortcuts: vec![item("Ctrl+K"), item("Ctrl+K")],
            ..Default::default()
        };
        assert_eq!(errors(&cfg).len(), 1);
    }

    #[test]
    fn duplicate_remap_from_is_error() {
        let cfg = Config {
            remaps: vec![remap("CapsLock", "Ctrl"), remap("CapsLock", "Esc")],
            ..Default::default()
        };
        assert_eq!(errors(&cfg).len(), 1);
    }

    #[test]
    fn remap_from_shadowing_shortcut_is_error() {
        let cfg = Config {
            shortcuts: vec![item("CapsLock")],
            remaps: vec![remap("CapsLock", "Ctrl")],
            ..Default::default()
        };
        assert_eq!(errors(&cfg).len(), 1);
    }

    #[test]
    fn superset_overlap_is_warn_only() {
        let cfg = Config {
            shortcuts: vec![item("Ctrl+K"), item("Ctrl+Shift+K")],
            ..Default::default()
        };
        assert!(errors(&cfg).is_empty());
        assert!(!warns(&cfg).is_empty());
    }

    #[test]
    fn clean_config_has_no_conflicts() {
        let cfg = Config {
            shortcuts: vec![item("Ctrl+K"), item("Alt+J")],
            remaps: vec![remap("CapsLock", "Ctrl")],
            ..Default::default()
        };
        assert!(detect_conflicts(&cfg, &PlatformCaps::default()).is_empty());
    }

    #[test]
    fn sequence_duplicate_is_error() {
        let cfg = Config {
            shortcuts: vec![item("F9 J K"), item("F9 J K")],
            ..Default::default()
        };
        assert_eq!(errors(&cfg).len(), 1);
    }

    #[test]
    fn sequence_leader_shadows_combo_is_error() {
        // 序列「F9 J K」的 leader「F9」会吞掉 F9，使同名组合键「F9」失效。
        let cfg = Config {
            shortcuts: vec![item("F9 J K"), item("F9")],
            ..Default::default()
        };
        let errs = errors(&cfg);
        assert_eq!(errs.len(), 1, "errs: {errs:?}");
        assert!(errs[0].message.contains("吞掉"), "msg: {}", errs[0].message);
    }

    #[test]
    fn sequence_no_superset_warn() {
        // 超集软冲突仅对单组合生效，序列跳过：F9 序列 + F9 组合应只有硬冲突（leader 遮蔽），
        // 不应额外产生超集 Warn。
        let cfg = Config {
            shortcuts: vec![item("F9 J K"), item("F9")],
            ..Default::default()
        };
        assert!(warns(&cfg).is_empty(), "warns: {:?}", warns(&cfg));
    }

    #[test]
    fn chord_duplicate_is_error_unordered() {
        // F&J 与 J&F 等价（无序），判重。
        let cfg = Config {
            shortcuts: vec![item("F&J"), item("J&F")],
            ..Default::default()
        };
        assert_eq!(errors(&cfg).len(), 1);
    }

    #[test]
    fn chord_member_shadows_combo_is_error() {
        // 和弦成员 F 会吞掉 F，使同名组合键 F 失效。
        let cfg = Config {
            shortcuts: vec![item("F&J"), item("F")],
            ..Default::default()
        };
        let errs = errors(&cfg);
        assert_eq!(errs.len(), 1, "errs: {errs:?}");
        assert!(errs[0].message.contains("吞掉"), "msg: {}", errs[0].message);
    }

    #[test]
    fn chord_member_shadows_sequence_leader_is_error() {
        // 和弦成员 F 会吞掉 F，使序列「F G」的 leader 失效。
        let cfg = Config {
            shortcuts: vec![item("F&J"), item("F G")],
            ..Default::default()
        };
        let errs = errors(&cfg);
        assert_eq!(errs.len(), 1, "errs: {errs:?}");
        assert!(errs[0].message.contains("吞掉"), "msg: {}", errs[0].message);
    }

    #[test]
    fn chord_member_as_modifier_does_not_shadow_combo() {
        // 修饰键成员从不被吞（吞掉 Ctrl 会把 Ctrl+Alt+Tab 变成 Ctrl+Tab），
        // 所以它遮蔽不了以该修饰键为「主键」的组合。
        let cfg = Config {
            shortcuts: vec![item("Ctrl+Alt&J"), item("Ctrl")],
            ..Default::default()
        };
        assert!(errors(&cfg).is_empty(), "errs: {:?}", errors(&cfg));
    }

    #[test]
    fn chord_modifier_member_is_not_a_duplicate_of_bare_chord() {
        // F&J 与 Ctrl+F&J 是两条不同的触发键（后者多一个修饰要求），不是重复。
        let cfg = Config {
            shortcuts: vec![item("F&J"), item("Ctrl+F&J")],
            ..Default::default()
        };
        assert!(errors(&cfg).is_empty(), "errs: {:?}", errors(&cfg));
    }

    #[test]
    fn chord_superset_is_warned() {
        // F&J 会先凑齐 → 三成员的 F&J&K 永远轮不到（软冲突：两边都合法，只是有一个不生效）。
        let cfg = Config {
            shortcuts: vec![item("F&J"), item("F&J&K")],
            ..Default::default()
        };
        let ws = warns(&cfg);
        assert_eq!(ws.len(), 1, "warns: {ws:?}");
        assert!(ws[0].message.contains("永远"), "msg: {}", ws[0].message);
        assert!(ws[0].message.contains("F&J&K"), "msg: {}", ws[0].message);
        assert_eq!(ws[0].name, "", "name 指向永远不生效的那条（这里没起名）");

        // 书写顺序反过来照样报：成员更少的那个必然先凑齐，与顺序无关。
        let cfg = Config {
            shortcuts: vec![item("F&J&K"), item("F&J")],
            ..Default::default()
        };
        let ws = warns(&cfg);
        assert_eq!(ws.len(), 1, "warns: {ws:?}");
        assert!(ws[0].message.contains("F&J&K"), "msg: {}", ws[0].message);
    }

    #[test]
    fn chord_same_keys_lighter_mods_wins_by_order() {
        // 成员键相同、只差修饰要求：同一刻凑齐，靠前者先被判到 → 写在后面的那条轮不到。
        let cfg = Config {
            shortcuts: vec![item("F&J"), item("Ctrl+F&J")],
            ..Default::default()
        };
        let ws = warns(&cfg);
        assert_eq!(ws.len(), 1, "warns: {ws:?}");
        assert!(ws[0].message.contains("Ctrl+F&J"), "msg: {}", ws[0].message);

        // 反过来（更严的写在前面）：不按 Ctrl 时更宽松的那条仍能触发 → 两条都可达，不报。
        let cfg = Config {
            shortcuts: vec![item("Ctrl+F&J"), item("F&J")],
            ..Default::default()
        };
        assert!(warns(&cfg).is_empty(), "warns: {:?}", warns(&cfg));
    }

    #[test]
    fn chord_different_mods_do_not_conflict() {
        // 修饰要求不同（互不包含）：各自在修饰键按下时生效，两条都可达。
        let cfg = Config {
            shortcuts: vec![item("Ctrl+F&J"), item("Alt+F&J")],
            ..Default::default()
        };
        assert!(warns(&cfg).is_empty(), "warns: {:?}", warns(&cfg));
    }

    #[test]
    fn chord_superset_only_within_same_layer() {
        // 不同层各自生效，不算遮蔽（与「同层内」的其余冲突判定口径一致）。
        let mut hi = item("F&J");
        hi.layer = Some("L1".into());
        let mut lo = item("F&J&K");
        lo.layer = Some("L2".into());
        let cfg = Config {
            shortcuts: vec![hi, lo],
            ..Default::default()
        };
        assert!(warns(&cfg).is_empty(), "warns: {:?}", warns(&cfg));
    }

    #[test]
    fn superset_overlap_only_within_same_layer() {
        // 用户报过：和弦层里绑 `B`、基础层里绑 `Alt+B`，不该报「B 遮蔽 Alt+B」——层里的
        // 宽组合只在**该层激活期间**先命中，退出层就回到基础层，属于层的局部覆盖，不是配置
        // 写错。同层里同样的两条仍然要报。
        let mut layered = item("B");
        layered.layer = Some("chords".into());
        let cfg = Config {
            shortcuts: vec![layered, item("Alt+B")],
            ..Default::default()
        };
        assert!(warns(&cfg).is_empty(), "warns: {:?}", warns(&cfg));

        let cfg_same_layer = Config {
            shortcuts: vec![item("B"), item("Alt+B")],
            ..Default::default()
        };
        assert_eq!(warns(&cfg_same_layer).len(), 1, "同层仍应报遮蔽");
    }

    #[test]
    fn remap_shadows_shortcut_only_when_layers_interact() {
        // 两个不同的非基础层永远不会同时生效 → 不算冲突。
        let mut r = remap("B", "Ctrl");
        r.layer = Some("L1".into());
        let mut s = item("B");
        s.layer = Some("L2".into());
        let cfg = Config {
            shortcuts: vec![s.clone()],
            remaps: vec![r],
            ..Default::default()
        };
        assert!(errors(&cfg).is_empty(), "errs: {:?}", errors(&cfg));

        // 基础层改键会兜进任何激活层 → 仍然拦截层内的同名快捷键。
        let cfg_base = Config {
            shortcuts: vec![s],
            remaps: vec![remap("B", "Ctrl")],
            ..Default::default()
        };
        assert_eq!(errors(&cfg_base).len(), 1);
    }
}

#[cfg(test)]
mod settings_text_mode_tests {
    use crate::*;

    #[test]
    fn text_inject_mode_parses_tolerantly() {
        let parse = |s: &str| {
            serde_json::from_str::<Settings>(s)
                .unwrap()
                .text_inject_mode
        };
        assert_eq!(
            parse(r#"{"text_inject_mode":"unicode"}"#),
            TextInjectMode::Unicode
        );
        assert_eq!(
            parse(r#"{"text_inject_mode":"clipboard"}"#),
            TextInjectMode::Clipboard
        );
        // 老配置缺字段 = 默认剪贴板（7.3-㉒ 之前一直只有这一种方式）。
        assert_eq!(parse("{}"), TextInjectMode::Clipboard);
        // 手改写错值回落默认，不让整份配置解析失败（加载路径对解析失败是硬处理）。
        assert_eq!(
            parse(r#"{"text_inject_mode":"unicod"}"#),
            TextInjectMode::Clipboard
        );
        // 序列化用小写值，与解析约定一致。
        assert_eq!(
            serde_json::to_string(&TextInjectMode::Unicode).unwrap(),
            r#""unicode""#
        );
        assert_eq!(
            serde_json::to_string(&TextInjectMode::Clipboard).unwrap(),
            r#""clipboard""#
        );
    }
}

#[cfg(test)]
mod os_and_sanitize_tests {
    use crate::*;

    #[cfg(feature = "automation")]
    #[test]
    fn os_action_json_roundtrip_and_validate() {
        let actions = vec![
            Action::Os {
                operation: OsOperation::Copy {
                    source: "C:\\a".into(),
                    dest: "D:\\b".into(),
                },
                description: None,
            },
            Action::Os {
                operation: OsOperation::GetFileProps {
                    path: "C:\\f.txt".into(),
                    var: "f".into(),
                },
                description: None,
            },
        ];
        for a in &actions {
            assert!(a.validate().is_ok(), "动作应通过校验: {a:?}");
        }
        let json = serde_json::to_string(&actions).unwrap();
        let back: Vec<Action> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, actions);

        // 空路径必被拒绝；变量名允许为空（回退默认）。
        assert!(Action::Os {
            operation: OsOperation::Copy {
                source: "".into(),
                dest: "".into()
            },
            description: None,
        }
        .validate()
        .is_err());
        assert!(Action::Os {
            operation: OsOperation::Delete { path: "  ".into() },
            description: None
        }
        .validate()
        .is_err());
        assert!(Action::Os {
            operation: OsOperation::GetFileProps {
                path: "x".into(),
                var: "".into()
            },
            description: None,
        }
        .validate()
        .is_ok());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn substitute_vars_expands_placeholders() {
        let mut vars = Vars::new();
        vars.insert(
            "f".into(),
            Value::File(FileObject {
                name: "a.txt".into(),
                path: "C:\\d\\a.txt".into(),
                dir: "C:\\d".into(),
                stem: "a".into(),
                ext: ".txt".into(),
                size: 12,
                modified: 0,
                is_dir: false,
            }),
        );
        assert_eq!(substitute_vars("{f}", &vars), "C:\\d\\a.txt");
        assert_eq!(substitute_vars("{f.name}", &vars), "a.txt");
        assert_eq!(substitute_vars("{f.ext}", &vars), ".txt");
        assert_eq!(substitute_vars("{f.size}", &vars), "12");
        assert_eq!(substitute_vars("{f.missing}", &vars), "{f.missing}");
        assert_eq!(substitute_vars("{unknown}", &vars), "{unknown}");
        assert_eq!(
            substitute_vars("copy {f} to {f.dir}", &vars),
            "copy C:\\d\\a.txt to C:\\d"
        );
    }

    #[cfg(feature = "automation")]
    #[test]
    fn sanitize_drops_invalid_parts_keeps_valid() {
        let cfg = Config {
            folders: vec![],
            layers: vec![],
            shortcuts: vec![
                ShortcutItem {
                    triggers: vec!["Ctrl+K".into()],
                    actions: vec![Action::Text {
                        text: "ok".into(),
                        mode: TextMode::Input,
                        masked: false,
                        description: None,
                    }],
                    enabled: true,
                    ..Default::default()
                },
                ShortcutItem {
                    triggers: vec!["Bad+Key".into()],
                    actions: vec![Action::Text {
                        text: "bad".into(),
                        mode: TextMode::Input,
                        masked: false,
                        description: None,
                    }],
                    enabled: true,
                    ..Default::default()
                },
                ShortcutItem {
                    triggers: vec!["Ctrl+J".into()],
                    actions: vec![Action::Cmd {
                        command: "  ".into(),
                        show_output: false,
                        var: "".into(),
                        description: None,
                    }],
                    enabled: true,
                    ..Default::default()
                },
            ],
            remaps: vec![
                Remap {
                    from: "CapsLock".into(),
                    to: "Ctrl".into(),
                    enabled: true,
                    ..Default::default()
                },
                Remap {
                    from: "NotAKey".into(),
                    to: "Ctrl".into(),
                    enabled: true,
                    ..Default::default()
                },
            ],
            expansions: vec![],
            gestures: vec![],
            touchpad_gestures: vec![],
            hot_corners: vec![],
            wheel_edges: vec![],
            settings: Settings::default(),
        };
        let (clean, ignored) = sanitize_config(&cfg);

        // 三条快捷键都保留（各自至少还有有效触发键或有效动作），但坏部分被清掉。
        assert_eq!(clean.shortcuts.len(), 3, "ignored: {}", ignored.join("; "));
        assert_eq!(clean.shortcuts[0].triggers, vec!["Ctrl+K".to_string()]);
        assert_eq!(clean.shortcuts[0].actions.len(), 1);
        assert!(clean.shortcuts[1].triggers.is_empty(), "无效触发键应被清空");
        assert_eq!(clean.shortcuts[1].actions.len(), 1);
        assert!(clean.shortcuts[2].actions.is_empty(), "无效动作应被清空");
        assert_eq!(clean.shortcuts[2].triggers, vec!["Ctrl+J".to_string()]);

        assert_eq!(clean.remaps.len(), 1, "无效改键应被丢弃");
        assert!(ignored.iter().any(|m| m.contains("Bad+Key")));
        assert!(ignored.iter().any(|m| m.contains("命令")));
        assert!(ignored.iter().any(|m| m.contains("NotAKey")));
    }

    #[test]
    fn sanitize_wake_key_validates_and_clears_invalid() {
        // 合法唤醒键保留；非法（非单键）清空并提示。
        let ok = Config {
            settings: Settings {
                wake_key: Some("Alt".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&ok);
        assert_eq!(clean.settings.wake_key.as_deref(), Some("Alt"));
        assert!(ignored.is_empty());

        let bad = Config {
            settings: Settings {
                wake_key: Some("Ctrl+Alt".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&bad);
        assert_eq!(clean.settings.wake_key, None);
        assert!(ignored.iter().any(|m| m.contains("唤醒键")));
    }

    #[test]
    fn sanitize_gesture_visual_clamps_and_fixes_colors() {
        let cfg = Config {
            settings: Settings {
                gesture_trail_color: " #ABC ".into(),
                gesture_fail_color: "not-a-color".into(),
                gesture_trail_width: 0,
                gesture_label_font_size: 999,
                gesture_label_offset: -5,
                ..Default::default()
            },
            ..Default::default()
        };
        let (clean, _) = sanitize_config(&cfg);
        assert_eq!(clean.settings.gesture_trail_color, "#aabbcc");
        assert_eq!(
            clean.settings.gesture_fail_color,
            default_gesture_fail_color()
        );
        assert_eq!(clean.settings.gesture_trail_width, 1);
        assert_eq!(clean.settings.gesture_label_font_size, 48);
        assert_eq!(clean.settings.gesture_label_offset, 0);
    }

    #[test]
    fn system_shortcut_list_all_parse() {
        // 清单里的每条都必须能解析成合法 Shortcut，且彼此不重复（防笔误）。
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for (combo, desc) in SYSTEM_SHORTCUTS {
            let sc = combo
                .parse::<Shortcut>()
                .unwrap_or_else(|e| panic!("系统快捷键「{combo}」（{desc}）解析失败: {e}"));
            assert!(seen.insert((sc.mods, sc.key)), "系统快捷键「{combo}」重复");
        }
    }

    #[cfg(feature = "automation")]
    fn sample_vars() -> Vars {
        let mut vars = Vars::new();
        vars.insert(
            "f".into(),
            Value::File(FileObject {
                name: "a.txt".into(),
                path: "C:\\d\\a.txt".into(),
                dir: "C:\\d".into(),
                stem: "a".into(),
                ext: ".txt".into(),
                size: 12,
                modified: 0,
                is_dir: false,
            }),
        );
        vars
    }

    #[cfg(feature = "automation")]
    #[test]
    fn condition_equals_and_field_resolution() {
        let vars = sample_vars();
        assert!(Condition::Equals {
            var: "f".into(),
            field: "ext".into(),
            value: ".txt".into()
        }
        .matches(&vars, None, None));
        assert!(Condition::NotEquals {
            var: "f".into(),
            field: "ext".into(),
            value: ".zip".into()
        }
        .matches(&vars, None, None));
        // field 为空 → 比较完整路径
        assert!(Condition::Equals {
            var: "f".into(),
            field: String::new(),
            value: "C:\\d\\a.txt".into()
        }
        .matches(&vars, None, None));
        // 变量或字段不存在 → 条件不成立
        assert!(!Condition::Equals {
            var: "f".into(),
            field: "nope".into(),
            value: "x".into()
        }
        .matches(&vars, None, None));
        assert!(!Condition::Equals {
            var: "missing".into(),
            field: String::new(),
            value: "x".into()
        }
        .matches(&vars, None, None));
    }

    #[cfg(feature = "automation")]
    #[test]
    fn condition_path_predicates() {
        let mut vars = sample_vars();
        // 当前目录（必然存在、是目录）+ 一个必不存在的路径
        assert!(Condition::Exists { path: ".".into() }.matches(&vars, None, None));
        assert!(Condition::IsDir { path: ".".into() }.matches(&vars, None, None));
        assert!(!Condition::IsFile { path: ".".into() }.matches(&vars, None, None));
        assert!(Condition::NotExists {
            path: "___kada_no_such_path___".into()
        }
        .matches(&vars, None, None));
        // 路径支持变量占位符：用指向真实存在的当前目录的变量验证替换。
        vars.insert(
            "cur".into(),
            Value::File(FileObject {
                name: ".".into(),
                path: ".".into(),
                dir: "".into(),
                stem: ".".into(),
                ext: "".into(),
                size: 0,
                modified: 0,
                is_dir: true,
            }),
        );
        assert!(Condition::Exists {
            path: "{cur}".into()
        }
        .matches(&vars, None, None));
        assert!(Condition::IsDir {
            path: "{cur}".into()
        }
        .matches(&vars, None, None));
    }

    #[cfg(feature = "automation")]
    #[test]
    fn condition_modified_within() {
        let vars = sample_vars();
        // 新建一个临时文件：刚写入的 mtime 必在最近 10 分钟内（避免用目录 mtime，其只在增删文件时更新）。
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp =
            std::env::temp_dir().join(format!("kada_mtime_{}_{}.txt", std::process::id(), nanos));
        std::fs::write(&tmp, b"x").unwrap();
        let tmp_path = tmp.to_string_lossy().into_owned();
        assert!(Condition::ModifiedWithin {
            path: tmp_path.clone(),
            minutes: 10
        }
        .matches(&vars, None, None));
        let _ = std::fs::remove_file(&tmp);

        // 不存在的路径 → 条件不成立
        assert!(!Condition::ModifiedWithin {
            path: "___kada_no_such_path___".into(),
            minutes: 10
        }
        .matches(&vars, None, None));
        // 路径为空 → 校验拒绝
        assert!(Condition::ModifiedWithin {
            path: "  ".into(),
            minutes: 10
        }
        .validate()
        .is_err());
        assert!(Condition::ModifiedWithin {
            path: tmp_path,
            minutes: 10
        }
        .validate()
        .is_ok());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn condition_frontmost_matching() {
        let ctx = |name: &str, title: &str| FrontmostContext {
            process_name: name.to_string(),
            window_title: title.to_string(),
        };
        let chrome = Some(ctx("chrome.exe", "Kada - Google Chrome"));
        let none: Option<FrontmostContext> = None;

        // 子串匹配（不区分大小写）
        assert!(Condition::FrontmostApp {
            app: "chrome".into()
        }
        .matches(&Vars::new(), chrome.as_ref(), None));
        assert!(Condition::FrontmostApp {
            app: "CHROME.EXE".into()
        }
        .matches(&Vars::new(), chrome.as_ref(), None));
        assert!(!Condition::FrontmostApp { app: "code".into() }.matches(
            &Vars::new(),
            chrome.as_ref(),
            None
        ));
        assert!(Condition::NotFrontmostApp { app: "code".into() }.matches(
            &Vars::new(),
            chrome.as_ref(),
            None
        ));
        assert!(!Condition::NotFrontmostApp {
            app: "chrome".into()
        }
        .matches(&Vars::new(), chrome.as_ref(), None));

        // 通配匹配（* / ?）
        assert!(Condition::FrontmostApp {
            app: "chrome.*".into()
        }
        .matches(&Vars::new(), chrome.as_ref(), None));
        assert!(Condition::FrontmostApp {
            app: "chr?me.exe".into()
        }
        .matches(&Vars::new(), chrome.as_ref(), None));
        assert!(!Condition::FrontmostApp {
            app: "*.txt".into()
        }
        .matches(&Vars::new(), chrome.as_ref(), None));

        // 窗口标题子串（不区分大小写）
        assert!(Condition::WindowTitleContains {
            text: "chrome".into()
        }
        .matches(&Vars::new(), chrome.as_ref(), None));
        assert!(Condition::WindowTitleContains {
            text: "CHROME".into()
        }
        .matches(&Vars::new(), chrome.as_ref(), None));
        assert!(!Condition::WindowTitleContains {
            text: "safari".into()
        }
        .matches(&Vars::new(), chrome.as_ref(), None));

        // 无前台上下文 → 前台条件一律不成立
        assert!(!Condition::FrontmostApp {
            app: "chrome".into()
        }
        .matches(&Vars::new(), none.as_ref(), None));
        assert!(!Condition::NotFrontmostApp { app: "code".into() }.matches(
            &Vars::new(),
            none.as_ref(),
            None
        ));
        assert!(
            !Condition::WindowTitleContains { text: "x".into() }.matches(
                &Vars::new(),
                none.as_ref(),
                None
            )
        );
    }

    #[cfg(feature = "automation")]
    #[test]
    fn condition_device_matching() {
        let dev = Some("AT Translated Set 2 keyboard");
        let other = Some("Logitech USB Keyboard");

        // 子串匹配（不区分大小写）
        assert!(Condition::DeviceIs { id: "set 2".into() }.matches(&Vars::new(), None, dev));
        assert!(Condition::DeviceIs {
            id: "LOGITECH".into()
        }
        .matches(&Vars::new(), None, other));
        assert!(!Condition::DeviceIs {
            id: "logitech".into()
        }
        .matches(&Vars::new(), None, dev));

        // 通配匹配（* / ?）
        assert!(Condition::DeviceIs {
            id: "AT Translated*".into()
        }
        .matches(&Vars::new(), None, dev));
        assert!(Condition::DeviceIs {
            id: "*Set ? keyboard".into()
        }
        .matches(&Vars::new(), None, dev));

        // 无设备上下文（macOS / 取不到设备）→ 恒不成立
        assert!(!Condition::DeviceIs {
            id: "keyboard".into()
        }
        .matches(&Vars::new(), None, None));

        // 空设备标识 → 校验拒绝
        assert!(Condition::DeviceIs { id: "  ".into() }.validate().is_err());
        assert!(Condition::DeviceIs {
            id: "keyboard".into()
        }
        .validate()
        .is_ok());
    }

    /// 「设备是」下拉存的值：Windows 设备路径压成稳定片段（实例号换口就变，别存进去）；
    /// Linux 的 evdev 名字原样留。压出来的片段必须还能命中原路径。
    #[test]
    fn device_pattern_keeps_stable_part() {
        let hid = "\\\\?\\HID#VID_046D&PID_C52B&MI_00#7&b3c8a2e&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
        assert_eq!(device_pattern(hid), "VID_046D&PID_C52B");
        assert!(
            match_name(&device_pattern(hid), hid),
            "压出来的片段要能命中原路径"
        );

        let acpi = "\\\\?\\ACPI#MSI0007#4&1cc9388&0#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
        assert_eq!(device_pattern(acpi), "ACPI#MSI0007");
        assert!(match_name(&device_pattern(acpi), acpi));

        // Linux evdev 名字：本来就是稳定标识，原样返回（不含 # 与 vid_）。
        assert_eq!(
            device_pattern("AT Translated Set 2 keyboard"),
            "AT Translated Set 2 keyboard"
        );

        // 其余形态：只去掉接口类 GUID 尾巴。
        assert_eq!(
            device_pattern("\\\\?\\SOMEBUS#dev#{00000000-0000-0000-0000-000000000000}"),
            "\\\\?\\SOMEBUS#dev"
        );
        assert_eq!(device_pattern("  "), "");
    }

    /// 设备大类的排序口径：同一块硬件被枚举成多条（数位板 = 笔 + 鼠标口）时留最具体的，
    /// 界面分组用的 id 则是给前端的稳定串，不是文案。
    #[test]
    fn device_kind_rank_prefers_the_specific_one() {
        assert!(DeviceKind::Tablet.rank() > DeviceKind::Touchpad.rank());
        assert!(DeviceKind::Touchpad.rank() > DeviceKind::Mouse.rank());
        assert!(DeviceKind::Mouse.rank() > DeviceKind::Keyboard.rank());
        assert!(DeviceKind::Keyboard.rank() > DeviceKind::Other.rank());
        assert_eq!(DeviceKind::Touchpad.id(), "touchpad");
        // 类别不参与匹配：同一份配置值能同时命中数位板与鼠标口（它们压出的片段相同）。
        let pen = "\\\\?\\HID#VID_056A&PID_0374&MI_02&Col01#7&1a2b&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let mouse = "\\\\?\\HID#VID_056A&PID_0374&MI_02&Col02#7&1a2b&0&0000#{378a4e42-3a1e-4c5c-bd8b-1b1d0f0d9c39}";
        assert_eq!(device_pattern(pen), device_pattern(mouse));
        // 但「同一块硬件」必须分得开：数位板的两个集合共享前缀，别的设备（别的接口号）不共享
        // ——接收器下共用 VID/PID 的键盘与鼠标就靠这一点不被误合并。
        assert_eq!(device_hardware_key(pen), device_hardware_key(mouse));
        let rcvr_kbd = "\\\\?\\HID#VID_046D&PID_C52B&MI_01&Col01#7&aa&0&0000#{884b96c3}";
        let rcvr_mouse = "\\\\?\\HID#VID_046D&PID_C52B&MI_02&Col01#7&aa&0&0000#{884b96c3}";
        assert_eq!(
            device_pattern(rcvr_kbd),
            device_pattern(rcvr_mouse),
            "同一颗接收器片段相同"
        );
        assert_ne!(
            device_hardware_key(rcvr_kbd),
            device_hardware_key(rcvr_mouse),
            "接口号不同 = 两台设备，别合并"
        );
        // 触摸板的两个集合：父设备实例相同、集合序号不同 → 同一个硬件身份。
        let pad_mouse = r"\\?\HID#CUST0001&Col01#5&135c4e80&0&0000#{378de44c}";
        let pad_touch = r"\\?\HID#CUST0001&Col02#5&135c4e80&0&0001#{4d1e55b2}";
        assert_eq!(
            device_hardware_key(pad_mouse),
            device_hardware_key(pad_touch)
        );
        assert_eq!(
            device_hardware_key(pad_mouse),
            r"\\?\HID#CUST0001#5&135c4e80"
        );
        // 稳定片段把实例号也丢掉（换 USB 口不失效），并且只认到硬件 ID 一段。
        assert_eq!(device_pattern(pad_mouse), r"\\?\HID#CUST0001");
        assert_eq!(device_pattern(pad_touch), r"\\?\HID#CUST0001");
        // evdev 名字没有 # 结构：原样返回（同名就当同一台）。
        assert_eq!(
            device_hardware_key("AT Translated Set 2 keyboard"),
            "AT Translated Set 2 keyboard"
        );
    }

    #[cfg(feature = "automation")]
    #[test]
    fn if_action_validate_recurses() {
        assert!(Action::If {
            condition: Condition::Exists { path: ".".into() },
            then: vec![Action::Text {
                text: "ok".into(),
                mode: TextMode::Input,
                masked: false,
                description: None
            }],
            otherwise: vec![],
            description: None,
        }
        .validate()
        .is_ok());
        // 条件为空 → 拒绝
        assert!(Action::If {
            condition: Condition::Exists { path: "  ".into() },
            then: vec![],
            otherwise: vec![],
            description: None,
        }
        .validate()
        .is_err());
        // 嵌套动作非法 → 递归拒绝
        assert!(Action::If {
            condition: Condition::Exists { path: ".".into() },
            then: vec![Action::Cmd {
                command: "  ".into(),
                show_output: false,
                var: "".into(),
                description: None
            }],
            otherwise: vec![],
            description: None,
        }
        .validate()
        .is_err());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn if_action_json_roundtrip() {
        // 递归嵌套的 If 也能完整往返（校验 serde tag 与 field 默认值）。
        let a = Action::If {
            condition: Condition::Equals {
                var: "f".into(),
                field: "ext".into(),
                value: ".txt".into(),
            },
            then: vec![Action::Text {
                text: "yes".into(),
                mode: TextMode::Input,
                masked: false,
                description: None,
            }],
            otherwise: vec![Action::If {
                condition: Condition::Exists { path: "{f}".into() },
                then: vec![],
                otherwise: vec![Action::PauseMs {
                    ms: 10,
                    description: None,
                }],
                description: None,
            }],
            description: None,
        };
        let json = serde_json::to_string(&a).unwrap();
        let back: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a, "json: {json}");
    }

    #[cfg(feature = "automation")]
    #[test]
    fn parallel_action_validate_recurses() {
        // 合法成员 / 空组都放行（空组等价于什么都不做）。
        assert!(Action::Parallel {
            actions: vec![Action::PauseMs {
                ms: 10,
                description: None
            }],
            description: None,
        }
        .validate()
        .is_ok());
        assert!(Action::Parallel {
            actions: vec![],
            description: None
        }
        .validate()
        .is_ok());
        // 嵌套成员非法 → 递归拒绝。
        assert!(Action::Parallel {
            actions: vec![Action::Keys {
                keys: vec!["NotAKey".into()],
                mode: KeysMode::Tap,
                description: None
            }],
            description: None,
        }
        .validate()
        .is_err());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn parallel_action_json_roundtrip() {
        let a = Action::Parallel {
            actions: vec![
                Action::Cmd {
                    command: "echo a".into(),
                    show_output: false,
                    var: "".into(),
                    description: None,
                },
                Action::Parallel {
                    actions: vec![Action::Text {
                        text: "x".into(),
                        mode: TextMode::Input,
                        masked: false,
                        description: None,
                    }],
                    description: None,
                },
            ],
            description: None,
        };
        let json = serde_json::to_string(&a).unwrap();
        let back: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a, "json: {json}");
    }

    #[test]
    fn group_action_validate_recurses_and_roundtrips() {
        // 合法成员 / 空组都放行（空组等价于什么都不做）。
        assert!(Action::Group {
            name: "清理".into(),
            actions: vec![Action::PauseMs {
                ms: 10,
                description: None
            }],
        }
        .validate()
        .is_ok());
        assert!(Action::Group {
            name: "空".into(),
            actions: vec![]
        }
        .validate()
        .is_ok());
        // 嵌套成员非法 → 递归拒绝（分组可套分组）。
        assert!(Action::Group {
            name: "外层".into(),
            actions: vec![Action::Group {
                name: "内层".into(),
                actions: vec![Action::Keys {
                    keys: vec!["NotAKey".into()],
                    mode: KeysMode::Tap,
                    description: None
                }],
            }],
        }
        .validate()
        .is_err());

        let a = Action::Group {
            name: "备份".into(),
            actions: vec![Action::Group {
                name: "收尾".into(),
                actions: vec![Action::Text {
                    text: "x".into(),
                    mode: TextMode::Input,
                    masked: false,
                    description: None,
                }],
            }],
        };
        let json = serde_json::to_string(&a).unwrap();
        let back: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a, "json: {json}");
        // 老配置 / 手写配置缺字段时照旧可解析。
        let bare: Action = serde_json::from_str(r#"{"type":"group"}"#).unwrap();
        assert_eq!(
            bare,
            Action::Group {
                name: String::new(),
                actions: vec![]
            }
        );
    }

    #[cfg(feature = "automation")]
    #[test]
    fn migrate_recurses_into_parallel() {
        // 组内旧版动作同样被迁移（`Cmd` → `Command(Cmd)`）。
        let migrated = migrate_action(Action::Parallel {
            actions: vec![Action::Cmd {
                command: "echo a".into(),
                show_output: false,
                var: "".into(),
                description: None,
            }],
            description: None,
        });
        match migrated {
            Action::Parallel { actions, .. } => {
                assert!(
                    matches!(
                        actions[0],
                        Action::Command {
                            shell: Shell::Cmd,
                            ..
                        }
                    ),
                    "组内旧版动作应被迁移：{:?}",
                    actions[0]
                );
            }
            other => panic!("应仍是 Parallel：{other:?}"),
        }
    }

    #[cfg(feature = "automation")]
    #[test]
    fn app_action_json_roundtrip_and_validate() {
        let actions = vec![
            Action::App {
                operation: AppOperation::Launch {
                    program: "notepad.exe".into(),
                    args: vec!["a.txt".into()],
                },
                description: None,
            },
            Action::App {
                operation: AppOperation::Status {
                    program: "notepad.exe".into(),
                    var: "running".into(),
                    retries: 3,
                    interval_ms: 500,
                },
                description: None,
            },
            Action::App {
                operation: AppOperation::Restart {
                    program: "notepad.exe".into(),
                    args: vec![],
                },
                description: None,
            },
            Action::App {
                operation: AppOperation::Maximize {
                    program: "notepad.exe".into(),
                },
                description: None,
            },
            Action::App {
                operation: AppOperation::Minimize {
                    program: "notepad.exe".into(),
                },
                description: None,
            },
            Action::App {
                operation: AppOperation::Activate {
                    program: "notepad.exe".into(),
                },
                description: None,
            },
        ];
        for a in &actions {
            assert!(a.validate().is_ok(), "{a:?}");
        }
        let json = serde_json::to_string(&actions).unwrap();
        let back: Vec<Action> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, actions);

        // 程序为空必被拒绝；Status 变量名可空。
        assert!(Action::App {
            operation: AppOperation::Close { program: "".into() },
            description: None
        }
        .validate()
        .is_err());
        assert!(Action::App {
            operation: AppOperation::Maximize {
                program: "  ".into()
            },
            description: None,
        }
        .validate()
        .is_err());
        assert!(Action::App {
            operation: AppOperation::Minimize { program: "".into() },
            description: None,
        }
        .validate()
        .is_err());
        assert!(Action::App {
            operation: AppOperation::Activate { program: "".into() },
            description: None,
        }
        .validate()
        .is_err());
        assert!(Action::App {
            operation: AppOperation::Status {
                program: "x".into(),
                var: "".into(),
                retries: 0,
                interval_ms: 0,
            },
            description: None,
        }
        .validate()
        .is_ok());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn open_url_json_roundtrip_and_validate() {
        let a = Action::OpenUrl {
            url: "https://example.com?a=1&b=2".into(),
            description: None,
        };
        assert!(a.validate().is_ok());
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains(r#""type":"open_url""#), "json: {json}");
        let back: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a);
        // 网址为空必被拒绝。
        assert!(Action::OpenUrl {
            url: "".into(),
            description: None
        }
        .validate()
        .is_err());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn migrate_legacy_launch_and_close_program() {
        let mut cfg = Config {
            shortcuts: vec![ShortcutItem {
                triggers: vec!["Ctrl+K".into()],
                actions: vec![
                    Action::Launch {
                        program: "notepad.exe".into(),
                        args: vec![],
                        description: None,
                    },
                    Action::CloseProgram {
                        program: "notepad.exe".into(),
                        description: None,
                    },
                    Action::If {
                        condition: Condition::Exists { path: ".".into() },
                        then: vec![Action::Launch {
                            program: "x".into(),
                            args: vec![],
                            description: None,
                        }],
                        otherwise: vec![],
                        description: None,
                    },
                ],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        cfg.migrate();
        let acts = &cfg.shortcuts[0].actions;
        assert!(matches!(
            acts[0],
            Action::App {
                operation: AppOperation::Launch { .. },
                description: _
            }
        ));
        assert!(matches!(
            acts[1],
            Action::App {
                operation: AppOperation::Close { .. },
                description: _
            }
        ));
        // 嵌套 If 里的旧动作也迁移
        match &acts[2] {
            Action::If { then, .. } => {
                assert!(matches!(
                    then[0],
                    Action::App {
                        operation: AppOperation::Launch { .. },
                        description: _
                    }
                ));
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[cfg(feature = "automation")]
    #[test]
    fn bool_value_substitution_and_compare() {
        let mut vars = Vars::new();
        vars.insert("running".into(), Value::Bool(true));
        vars.insert("text".into(), Value::Text("hello".into()));
        vars.insert(
            "cmd".into(),
            Value::Text(TextValue {
                text: "E".into(),
                exit_code: Some(0),
            }),
        );
        assert_eq!(substitute_vars("{running}", &vars), "true");
        assert_eq!(substitute_vars("{running.value}", &vars), "true");
        assert_eq!(substitute_vars("{text}", &vars), "hello");
        assert_eq!(substitute_vars("{cmd}", &vars), "E");
        assert_eq!(substitute_vars("{cmd.exit_code}", &vars), "0");
        assert_eq!(
            substitute_vars("{text.exit_code}", &vars),
            "{text.exit_code}"
        );
        // 条件判断能比较布尔变量
        assert!(Condition::Equals {
            var: "running".into(),
            field: "".into(),
            value: "true".into()
        }
        .matches(&vars, None, None));
        assert!(Condition::Equals {
            var: "text".into(),
            field: "".into(),
            value: "hello".into()
        }
        .matches(&vars, None, None));
    }

    /// `sanitize_config` 的往返稳定性（原规划 7.3-⑰ 点名的缺口）：一份**全合法**的配置
    /// 过清洗后必须原样保留（无 ignored）、能 JSON 往返、且清洗幂等（再洗一次仍不变）。
    /// 已有的一堆用例锁的是「坏条目被丢掉」；这条锁另一面——合法配置不被误伤。
    #[test]
    fn sanitize_valid_config_is_unchanged_roundtrip_and_idempotent() {
        let cfg = Config {
            folders: vec![Folder {
                id: "f1".into(),
                name: "工作".into(),
                parent: None,
            }],
            layers: vec![Layer {
                id: "l1".into(),
                name: "导航".into(),
                ..Default::default()
            }],
            shortcuts: vec![ShortcutItem {
                name: Some("测试".into()),
                description: Some("说明".into()),
                folder: Some("f1".into()),
                layer: Some("l1".into()),
                // 组合 / 序列 / 和弦三种触发都合法。
                triggers: vec!["Ctrl+K".into(), "F9 J".into(), "F&J".into()],
                actions: vec![
                    Action::Text {
                        text: "hi".into(),
                        mode: TextMode::Input,
                        masked: false,
                        description: None,
                    },
                    Action::Keys {
                        keys: vec!["Ctrl".into(), "S".into()],
                        mode: KeysMode::Tap,
                        description: None,
                    },
                    Action::PauseMs {
                        ms: 100,
                        description: None,
                    },
                ],
                enabled: true,
                ..Default::default()
            }],
            remaps: vec![
                // tap-hold（短按/长按）与普通改键各一条。
                Remap {
                    from: "CapsLock".into(),
                    tap: Some("Escape".into()),
                    hold: Some("Control".into()),
                    enabled: true,
                    ..Default::default()
                },
                Remap {
                    from: "F1".into(),
                    to: "Home".into(),
                    enabled: true,
                    ..Default::default()
                },
            ],
            expansions: vec![TextExpansion {
                trigger: ";addr".into(),
                replace: "某地".into(),
                enabled: true,
                ..Default::default()
            }],
            gestures: vec![],
            touchpad_gestures: vec![],
            hot_corners: vec![],
            wheel_edges: vec![],
            settings: Settings {
                wake_key: Some("Alt".into()),
                ..Default::default()
            },
        };

        let (clean, ignored) = sanitize_config(&cfg);
        assert!(ignored.is_empty(), "合法配置不该有忽略项：{ignored:?}");
        assert_eq!(clean, cfg, "合法配置过清洗后必须原样保留");

        // JSON 往返（配置是跨平台同步介质，保存 / 加载不能变形）。
        let json = serde_json::to_string(&clean).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back, clean, "JSON 往返后配置应不变");

        // 幂等：清洗结果再洗一次仍不变、也无提示。
        let (again, ignored2) = sanitize_config(&clean);
        assert!(ignored2.is_empty(), "二次清洗不该有新提示：{ignored2:?}");
        assert_eq!(again, clean, "清洗应幂等");
    }

    /// 复制快捷菜单（EXT-13）：配置形状（动作 / 子菜单 / 分割线）、缺省 `enabled = true`
    /// 不落盘、间隔钳值、坏条目在清洗时丢弃。菜单是新配置，锁住形状免得手改 JSON 或
    /// 老版本回写把它打散。
    #[test]
    fn clipboard_menu_shape_migrate_and_sanitize() {
        let json = r#"{
            "layers": [], "shortcuts": [], "remaps": [], "expansions": [], "folders": [],
            "settings": { "clipboard_menu": {
                "enabled": true,
                "double_copy_ms": 9000,
                "items": [
                    { "kind": "action", "label": "纯文本", "action": { "type": "pause_ms", "ms": 1 } },
                    { "kind": "separator" },
                    { "kind": "submenu", "label": "搜索", "items": [
                        { "kind": "action", "label": "网页", "enabled": false, "action": { "type": "pause_ms", "ms": 2 } }
                    ] }
                ]
            } }
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        let menu = &cfg.settings.clipboard_menu;
        assert!(menu.enabled);
        assert_eq!(menu.items.len(), 3);
        // 缺省 enabled = true；显式 false 保留。
        assert!(matches!(
            &menu.items[0],
            MenuEntry::Action { enabled: true, .. }
        ));
        assert!(matches!(&menu.items[2], MenuEntry::Submenu { items, .. }
            if matches!(&items[0], MenuEntry::Action { enabled: false, .. })));

        // 序列化：缺省 true 不落盘（少噪音），但读回来仍是 true。
        let text = serde_json::to_string(menu).unwrap();
        assert!(
            !text.contains("\"label\":\"纯文本\",\"enabled\":true"),
            "条目缺省 enabled 不该落盘: {text}"
        );
        let back: ClipboardMenu = serde_json::from_str(&text).unwrap();
        assert_eq!(&back, menu);

        // migrate 钳间隔。
        let mut cfg = cfg;
        cfg.migrate();
        assert_eq!(cfg.settings.clipboard_menu.double_copy_ms, 2000);

        // 清洗：空标签 / 非法动作 / 空子菜单丢弃，合法条目保留。
        cfg.settings.clipboard_menu.items.push(MenuEntry::Action {
            label: "  ".into(),
            enabled: true,
            action: Action::PauseMs {
                ms: 1,
                description: None,
            },
        });
        cfg.settings.clipboard_menu.items.push(MenuEntry::Action {
            label: "坏动作".into(),
            enabled: true,
            action: Action::Keys {
                keys: vec![],
                mode: KeysMode::Tap,
                description: None,
            },
        });
        cfg.settings.clipboard_menu.items.push(MenuEntry::Submenu {
            label: "空".into(),
            enabled: true,
            items: Vec::new(),
        });
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(
            clean.settings.clipboard_menu.items.len(),
            3,
            "只该留下 3 条合法条目"
        );
        assert_eq!(ignored.len(), 3, "三处坏条目都要有提示：{ignored:?}");
    }

    #[cfg(feature = "automation")]
    #[test]
    fn inline_script_roundtrips_and_validates() {
        let a = Action::InlineScript {
            source: r#"set_var("x", 1)"#.into(),
            var: "out".into(),
            show_output: false,
            description: None,
        };
        let json = serde_json::to_string(&a).unwrap();
        assert!(
            json.contains("\"inline_script\""),
            "serde 标签要稳定：{json}"
        );
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), a);
        assert!(a.validate().is_ok());

        let empty = Action::InlineScript {
            source: "  ".into(),
            var: String::new(),
            show_output: false,
            description: None,
        };
        assert!(empty.validate().unwrap_err().contains("脚本内容"));

        let huge = Action::InlineScript {
            source: "x".repeat(INLINE_SCRIPT_MAX_BYTES + 1),
            var: String::new(),
            show_output: false,
            description: None,
        };
        assert!(huge.validate().unwrap_err().contains("上限"));
    }
}

#[cfg(test)]
mod new_action_and_folder_tests {
    use crate::*;

    #[cfg(feature = "automation")]
    #[test]
    fn text_mode_and_shell_json_roundtrip() {
        let actions = vec![
            Action::Text {
                text: "hi".into(),
                mode: TextMode::Input,
                masked: false,
                description: None,
            },
            Action::Text {
                text: "".into(),
                mode: TextMode::ToUpper,
                masked: false,
                description: None,
            },
            Action::Text {
                text: "".into(),
                mode: TextMode::ToLower,
                masked: false,
                description: None,
            },
            Action::Command {
                shell: Shell::Cmd,
                command: "echo hi".into(),
                working_dir: String::new(),
                timeout_ms: None,
                ok_exit_codes: Vec::new(),
                show_output: false.into(),
                hide_window: false,
                var: "".into(),
                wait: true,
                on_error: Default::default(),
                if_running: Default::default(),
                description: None,
            },
            Action::Command {
                shell: Shell::Powershell,
                command: "Get-Date".into(),
                working_dir: String::new(),
                timeout_ms: None,
                ok_exit_codes: Vec::new(),
                show_output: true.into(),
                hide_window: false,
                var: "".into(),
                wait: true,
                on_error: Default::default(),
                if_running: Default::default(),
                description: None,
            },
        ];
        for a in &actions {
            assert!(a.validate().is_ok(), "应通过校验: {a:?}");
        }
        let json = serde_json::to_string(&actions).unwrap();
        let back: Vec<Action> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, actions, "json: {json}");

        // 旧版 text（无 mode）默认 Input；Command 命令为空被拒绝。
        let legacy: Action = serde_json::from_str(r#"{"type":"text","text":"x"}"#).unwrap();
        assert_eq!(
            legacy,
            Action::Text {
                text: "x".into(),
                mode: TextMode::Input,
                masked: false,
                description: None
            }
        );
        assert!(Action::Command {
            shell: Shell::Cmd,
            command: "  ".into(),
            working_dir: String::new(),
            timeout_ms: None,
            ok_exit_codes: Vec::new(),
            show_output: false.into(),
            hide_window: false,
            var: "".into(),
            wait: true,
            on_error: Default::default(),
            if_running: Default::default(),
            description: None
        }
        .validate()
        .is_err());
    }

    /// `show_output` 三态（CS-11）兼容老布尔；`wait` / `on_error` 缺省保持旧语义且不落盘。
    #[cfg(feature = "automation")]
    #[test]
    fn show_output_tri_state_and_command_defaults_are_compatible() {
        let old_false: ShowOutput = serde_json::from_str("false").unwrap();
        assert_eq!(old_false, ShowOutput::Bool(false));
        assert!(!old_false.shows(false) && !old_false.shows(true));
        let old_true: ShowOutput = serde_json::from_str("true").unwrap();
        assert!(old_true.shows(false) && old_true.shows(true));

        let always: ShowOutput = serde_json::from_str("\"always\"").unwrap();
        assert!(always.shows(false) && always.shows(true));
        let on_error: ShowOutput = serde_json::from_str("\"on_error\"").unwrap();
        assert!(!on_error.shows(false) && on_error.shows(true));
        let never: ShowOutput = serde_json::from_str("\"never\"").unwrap();
        assert!(!never.shows(false) && !never.shows(true));
        assert_eq!(
            serde_json::to_string(&ShowOutput::Mode(ShowOutputMode::OnError)).unwrap(),
            "\"on_error\""
        );

        let a: Action =
            serde_json::from_str(r#"{"type":"command","shell":"cmd","command":"echo a"}"#).unwrap();
        match &a {
            Action::Command {
                show_output,
                wait,
                on_error,
                ..
            } => {
                assert_eq!(*show_output, ShowOutput::Bool(false), "老配置缺省不弹窗");
                assert!(*wait, "缺省要等它跑完");
                assert_eq!(*on_error, OnError::Continue, "缺省失败后继续");
            }
            other => panic!("应解析成 Command：{other:?}"),
        }
        let json = serde_json::to_string(&a).unwrap();
        assert!(!json.contains("\"wait\""), "默认 wait 不落盘：{json}");
        assert!(
            !json.contains("\"on_error\""),
            "默认 on_error 不落盘：{json}"
        );
    }

    /// 「当前窗口」动作（EXT-14）：配置 / 卡带里的形状是
    /// `{"type":"window","operation":{"op":"close"}}`（与 App / System 的 `op` 口径一致）。
    #[cfg(feature = "automation")]
    #[test]
    fn window_action_json_roundtrip_and_validate() {
        let a: Action =
            serde_json::from_str(r#"{"type":"window","operation":{"op":"maximize_restore"}}"#)
                .unwrap();
        assert!(a.validate().is_ok());
        assert!(matches!(
            a,
            Action::Window {
                operation: WindowOperation::MaximizeRestore,
                ..
            }
        ));
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains(r#""op":"maximize_restore""#), "{json}");
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), a);
    }

    /// 入口收口：旧的三类动作在加载时并进所属分类（亮度 / 后台发送 → 系统，窗口 → 应用）。
    #[cfg(feature = "automation")]
    #[test]
    fn migrate_merges_brightness_send_to_and_window_into_categories() {
        let bright = migrate_action(Action::Brightness {
            operation: BrightnessOperation::Down,
            step: 25,
            description: None,
        });
        assert_eq!(
            bright,
            Action::System {
                operation: SystemOperation::Brightness {
                    operation: BrightnessOperation::Down,
                    step: 25,
                },
                description: None,
            }
        );

        let send = migrate_action(
            serde_json::from_str(
                r#"{"type":"send_to","target":"notepad","payload":{"kind":"keys","keys":["Ctrl","S"]}}"#,
            )
            .unwrap(),
        );
        assert!(matches!(
            send,
            Action::System {
                operation: SystemOperation::SendTo { .. },
                ..
            }
        ));

        let win = migrate_action(
            serde_json::from_str(r#"{"type":"window","operation":{"op":"close"}}"#).unwrap(),
        );
        assert_eq!(
            win,
            Action::App {
                operation: AppOperation::CurrentWindow {
                    action: WindowOperation::Close,
                },
                description: None,
            }
        );
    }

    /// 合并后的新形态：`system.send_to` / `system.brightness` / `app.current_window`
    /// 能往返、能校验，必填项与步长越界照样拦住。
    #[cfg(feature = "automation")]
    #[test]
    fn merged_operation_json_roundtrip_and_validate() {
        for json in [
            r#"{"type":"system","operation":{"op":"send_to","target":"notepad","payload":{"kind":"text","text":"hi"}}}"#,
            r#"{"type":"system","operation":{"op":"brightness","operation":"down","step":25}}"#,
            r#"{"type":"app","operation":{"op":"current_window","action":{"op":"close"}}}"#,
        ] {
            let a: Action = serde_json::from_str(json).unwrap();
            assert!(a.validate().is_ok(), "{json}");
            let back = serde_json::to_string(&a).unwrap();
            assert_eq!(serde_json::from_str::<Action>(&back).unwrap(), a, "{back}");
        }

        let bad: Action = serde_json::from_str(
            r#"{"type":"system","operation":{"op":"send_to","target":"","payload":{"kind":"text","text":"x"}}}"#,
        )
        .unwrap();
        assert!(bad.validate().is_err());
        let bad: Action = serde_json::from_str(
            r#"{"type":"system","operation":{"op":"brightness","operation":"up","step":0}}"#,
        )
        .unwrap();
        assert!(bad.validate().is_err());
    }

    /// 平台能力缺口检查要认得合并后的形态（不只是旧类型）。
    #[cfg(feature = "automation")]
    #[test]
    fn platform_helpers_detect_merged_operations() {
        let send = Action::System {
            operation: SystemOperation::SendTo {
                target: "notepad".into(),
                payload: SendPayload::Text { text: "hi".into() },
            },
            description: None,
        };
        assert!(uses_background_input(std::slice::from_ref(&send)));

        let bright = Action::System {
            operation: SystemOperation::Brightness {
                operation: BrightnessOperation::Up,
                step: 10,
            },
            description: None,
        };
        assert!(uses_brightness(std::slice::from_ref(&bright)));

        let win = Action::App {
            operation: AppOperation::CurrentWindow {
                action: WindowOperation::Close,
            },
            description: None,
        };
        assert!(uses_window_action(std::slice::from_ref(&win)));
    }

    /// 旧截图动作还能解析（老配置不能因为功能移除就打不开）。
    #[cfg(feature = "automation")]
    #[test]
    fn legacy_capture_action_still_parses() {
        let a: Action = serde_json::from_str(
            r#"{"type":"capture","operation":"region","output":"file","path":"D:\\a.png"}"#,
        )
        .unwrap();
        assert!(matches!(
            a,
            Action::Capture {
                operation: CaptureOperation::Region,
                output: CaptureOutput::File,
                ..
            }
        ));
        assert!(serde_json::to_string(&a)
            .unwrap()
            .contains(r#""type":"capture""#));
    }

    /// 外部工具占位动作：工具 / 能力都不许为空；目录里的 id 能查到展示名。
    #[cfg(feature = "automation")]
    #[test]
    fn tool_action_validate_and_catalog() {
        let a: Action =
            serde_json::from_str(r#"{"type":"tool","tool":"bit-pouch","capability":"open"}"#)
                .unwrap();
        assert!(matches!(a, Action::Tool { .. }));
        assert!(a.validate().is_ok());
        assert_eq!(external_tool("bit-pouch").unwrap().name, "比特口袋");
        assert_eq!(
            external_tool("bit-pouch").unwrap().capabilities[0].name,
            "打开"
        );

        let empty: Action = serde_json::from_str(r#"{"type":"tool"}"#).unwrap();
        assert!(empty.validate().is_err());
        let unknown: Action =
            serde_json::from_str(r#"{"type":"tool","tool":"x","capability":"y"}"#).unwrap();
        assert!(unknown.validate().is_ok());
        assert!(external_tool("x").is_none());
    }

    /// 亮度（EXT-22）：默认方向 / 步长不落盘；步长限 1~100，恢复不看步长。
    #[cfg(feature = "automation")]
    #[test]
    fn brightness_action_json_roundtrip_and_validate() {
        let a: Action = serde_json::from_str(r#"{"type":"brightness"}"#).unwrap();
        assert!(matches!(
            a,
            Action::Brightness {
                operation: BrightnessOperation::Up,
                step: 10,
                ..
            }
        ));
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            r#"{"type":"brightness"}"#
        );
        let down: Action =
            serde_json::from_str(r#"{"type":"brightness","operation":"down","step":25}"#).unwrap();
        assert_eq!(down.validate(), Ok(()));
        let zero: Action =
            serde_json::from_str(r#"{"type":"brightness","operation":"up","step":0}"#).unwrap();
        assert!(zero.validate().is_err());
        let restore: Action =
            serde_json::from_str(r#"{"type":"brightness","operation":"restore","step":0}"#)
                .unwrap();
        assert_eq!(restore.validate(), Ok(()));
    }

    /// 剪贴板动作（EXT-15）：配置 / 卡带里的形状是
    /// `{"type":"clipboard","operation":{"op":"set_clipboard","text":"..."}}`；
    /// `tempstr` 用短名（不是 `temp_str`），与规划里的写法一致。
    #[test]
    fn clipboard_action_json_roundtrip_and_validate() {
        let a: Action = serde_json::from_str(
            r#"{"type":"clipboard","operation":{"op":"set_clipboard","text":"hi"}}"#,
        )
        .unwrap();
        assert!(a.validate().is_ok());
        assert!(matches!(
            a,
            Action::Clipboard {
                operation: ClipboardOperation::SetClipboard { .. },
                ..
            }
        ));
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains(r#""op":"set_clipboard""#), "{json}");
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), a);

        let temp: Action =
            serde_json::from_str(r#"{"type":"clipboard","operation":{"op":"tempstr","text":"x"}}"#)
                .unwrap();
        assert!(temp.validate().is_ok());
        assert!(matches!(
            temp,
            Action::Clipboard {
                operation: ClipboardOperation::TempStr { .. },
                ..
            }
        ));

        // 空文本清空剪贴板是合法操作；读取剪贴板必须给变量名；临时文本不能为空。
        assert!(Action::Clipboard {
            operation: ClipboardOperation::SetClipboard {
                text: String::new()
            },
            description: None,
        }
        .validate()
        .is_ok());
        assert!(Action::Clipboard {
            operation: ClipboardOperation::GetClipboard { var: " ".into() },
            description: None,
        }
        .validate()
        .is_err());
        assert!(Action::Clipboard {
            operation: ClipboardOperation::TempStr {
                text: String::new()
            },
            description: None,
        }
        .validate()
        .is_err());
    }

    /// 算法动作（EXT-18）：`operation` 是短名（不是对象），默认 `inject = false` 不落盘。
    #[test]
    fn transform_action_json_roundtrip_and_validate() {
        let a: Action = serde_json::from_str(
            r#"{"type":"transform","operation":"base64_encode","input":"{clip}","var":"b64"}"#,
        )
        .unwrap();
        assert!(a.validate().is_ok());
        assert!(matches!(
            a,
            Action::Transform {
                operation: TransformOperation::Base64Encode,
                inject: false,
                ..
            }
        ));
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains(r#""operation":"base64_encode""#), "{json}");
        assert!(!json.contains("inject"), "{json}");
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), a);

        assert!(Action::Transform {
            operation: TransformOperation::UrlDecode,
            input: " ".into(),
            var: "out".into(),
            inject: false,
            description: None,
        }
        .validate()
        .is_err());
        assert!(Action::Transform {
            operation: TransformOperation::HashSha256,
            input: "x".into(),
            var: " ".into(),
            inject: false,
            description: None,
        }
        .validate()
        .is_err());
        assert!(Action::Transform {
            operation: TransformOperation::HashSha256,
            input: "x".into(),
            var: " ".into(),
            inject: true,
            description: None,
        }
        .validate()
        .is_ok());
    }

    /// `masked` 只影响展示，但它是配置字段，得守住三件事：老配置（没这个键）读成「不隐藏」、
    /// 标记了隐藏的能落盘并原样读回、默认值 `false` 不落盘（配置与导出卡带里不留噪音）。
    /// 不断言「注入行为」，因为 `masked` 本就不参与执行（见 `Action::Text` 的字段注释）。
    #[test]
    fn text_action_masked_roundtrip() {
        let old: Action = serde_json::from_str(r#"{"type":"text","text":"pw"}"#).unwrap();
        assert!(matches!(old, Action::Text { masked: false, .. }), "{old:?}");

        let hidden = Action::Text {
            text: "pw".into(),
            mode: TextMode::Input,
            masked: true,
            description: None,
        };
        let json = serde_json::to_string(&hidden).unwrap();
        assert!(json.contains(r#""masked":true"#), "隐藏标记该落盘: {json}");
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), hidden);

        let plain = Action::Text {
            text: "hi".into(),
            mode: TextMode::Input,
            masked: false,
            description: None,
        };
        assert!(
            !serde_json::to_string(&plain).unwrap().contains("masked"),
            "默认值不该落盘"
        );
    }

    /// 「隐式执行」开关（`Command` / `Script` 的 `hide_window`）与 `masked` 同一套口径：
    /// 老配置（没这个键）读成「不隐藏」、打开过的能落盘并原样读回、默认值不落盘。
    #[cfg(feature = "automation")]
    #[test]
    fn command_and_script_hide_window_roundtrip() {
        let old: Action =
            serde_json::from_str(r#"{"type":"command","shell":"cmd","command":"echo hi"}"#)
                .unwrap();
        assert!(
            matches!(
                old,
                Action::Command {
                    hide_window: false,
                    ..
                }
            ),
            "老配置该读成不隐藏: {old:?}"
        );

        let hidden = Action::Command {
            shell: Shell::Cmd,
            command: "echo hi".into(),
            working_dir: String::new(),
            timeout_ms: None,
            ok_exit_codes: Vec::new(),
            show_output: false.into(),
            hide_window: true,
            var: String::new(),
            wait: true,
            on_error: OnError::Continue,
            if_running: IfRunning::Run,
            description: None,
        };
        let json = serde_json::to_string(&hidden).unwrap();
        assert!(json.contains(r#""hide_window":true"#), "开关该落盘: {json}");
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), hidden);

        let plain = Action::Script {
            path: "x.py".into(),
            interpreter: None,
            interpreter_args: Vec::new(),
            args: Vec::new(),
            working_dir: String::new(),
            timeout_ms: None,
            ok_exit_codes: Vec::new(),
            show_output: false.into(),
            hide_window: false,
            var: String::new(),
            wait: true,
            on_error: OnError::Continue,
            if_running: IfRunning::Run,
            description: None,
        };
        assert!(
            !serde_json::to_string(&plain)
                .unwrap()
                .contains("hide_window"),
            "默认值不该落盘"
        );
    }

    /// 命令 / 脚本的新字段（批次一：`working_dir` / `timeout_ms` / `ok_exit_codes` /
    /// `args` / `interpreter_args`）：老配置缺字段读成默认值、默认值不落盘、
    /// 设了值能原样读回（CS-01~CS-05）。
    #[cfg(feature = "automation")]
    #[test]
    fn command_script_new_fields_default_and_roundtrip() {
        // 老配置（写于新字段出现之前）缺字段 = 新默认值。
        let old: Action =
            serde_json::from_str(r#"{"type":"command","shell":"cmd","command":"echo hi"}"#)
                .unwrap();
        assert!(
            matches!(
                old,
                Action::Command { ref working_dir, timeout_ms: None, ref ok_exit_codes, .. }
                    if working_dir.is_empty() && ok_exit_codes.is_empty()
            ),
            "老配置该读成默认值: {old:?}"
        );
        let old_script: Action =
            serde_json::from_str(r#"{"type":"script","path":"x.py","interpreter":"python"}"#)
                .unwrap();
        assert!(
            matches!(
                old_script,
                Action::Script { ref interpreter_args, ref args, ref working_dir, timeout_ms: None, ref ok_exit_codes, .. }
                    if interpreter_args.is_empty() && args.is_empty() && working_dir.is_empty() && ok_exit_codes.is_empty()
            ),
            "老脚本配置该读成默认值: {old_script:?}"
        );

        // 默认值不落盘；`timeout_ms` 只有 Some 才写。
        let plain = Action::Command {
            shell: Shell::Cmd,
            command: "echo hi".into(),
            working_dir: String::new(),
            timeout_ms: None,
            ok_exit_codes: Vec::new(),
            show_output: false.into(),
            hide_window: false,
            var: String::new(),
            wait: true,
            on_error: OnError::Continue,
            if_running: IfRunning::Run,
            description: None,
        };
        let json = serde_json::to_string(&plain).unwrap();
        for key in [
            "working_dir",
            "timeout_ms",
            "ok_exit_codes",
            "show_output",
            "wait",
            "on_error",
            "if_running",
        ] {
            assert!(!json.contains(key), "{key} 默认值不该落盘: {json}");
        }

        // 设了值能原样读回；`timeout_ms: 0`（不限时）也要落盘。
        let full = Action::Script {
            path: "backup.py".into(),
            interpreter: Some("python".into()),
            interpreter_args: vec!["-u".into()],
            args: vec!["--dest".into(), "D:\\backup".into()],
            working_dir: "D:\\work".into(),
            timeout_ms: Some(0),
            ok_exit_codes: vec![0, 3010],
            show_output: false.into(),
            hide_window: false,
            var: String::new(),
            wait: false,
            on_error: OnError::Stop,
            if_running: IfRunning::Queue,
            description: None,
        };
        let json = serde_json::to_string(&full).unwrap();
        assert!(
            json.contains(r#""timeout_ms":0"#),
            "0 = 不限时该落盘: {json}"
        );
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), full);
    }

    /// 等待动作（CS-18）：缺省轮询间隔不落盘、目标按 `kind` 判别；三种目标都能往返。
    #[cfg(feature = "automation")]
    #[test]
    fn wait_for_roundtrip_and_defaults() {
        let file = Action::WaitFor {
            target: WaitTarget::File {
                path: "D:\\out.zip".into(),
            },
            timeout_ms: None,
            interval_ms: default_wait_interval_ms(),
            var: String::new(),
            description: None,
        };
        assert!(file.validate().is_ok());
        let json = serde_json::to_string(&file).unwrap();
        assert!(!json.contains("interval_ms"), "缺省间隔不落盘: {json}");
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), file);

        let process = Action::WaitFor {
            target: WaitTarget::Process {
                program: "notepad.exe".into(),
            },
            timeout_ms: Some(0),
            interval_ms: 500,
            var: "pid".into(),
            description: Some("等记事本".into()),
        };
        assert!(process.validate().is_ok());
        let json = serde_json::to_string(&process).unwrap();
        assert!(json.contains(r#""kind":"process""#), "{json}");
        assert!(json.contains(r#""interval_ms":500"#), "{json}");
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), process);

        let window = Action::WaitFor {
            target: WaitTarget::Window {
                title: "*记事本*".into(),
            },
            timeout_ms: Some(5000),
            interval_ms: default_wait_interval_ms(),
            var: String::new(),
            description: None,
        };
        assert_eq!(
            serde_json::from_str::<Action>(&serde_json::to_string(&window).unwrap()).unwrap(),
            window
        );

        // 空目标 = 无效（保存在清洗路径会丢，这里先拦住）。
        assert!(Action::WaitFor {
            target: WaitTarget::File { path: "  ".into() },
            timeout_ms: None,
            interval_ms: 200,
            var: String::new(),
            description: None,
        }
        .validate()
        .is_err());
    }

    /// `if_running` 默认 `run` 不落盘，显式 `skip` / `queue` 能往返。
    #[cfg(feature = "automation")]
    #[test]
    fn if_running_defaults_and_roundtrip() {
        let skipped = Action::Command {
            shell: Shell::Cmd,
            command: "echo hi".into(),
            working_dir: String::new(),
            timeout_ms: None,
            ok_exit_codes: Vec::new(),
            show_output: false.into(),
            hide_window: false,
            var: String::new(),
            wait: true,
            on_error: OnError::Continue,
            if_running: IfRunning::Skip,
            description: None,
        };
        let json = serde_json::to_string(&skipped).unwrap();
        assert!(json.contains(r#""if_running":"skip""#), "{json}");
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), skipped);
        let old: Action =
            serde_json::from_str(r#"{"type":"command","shell":"cmd","command":"echo hi"}"#)
                .unwrap();
        assert!(
            matches!(
                old,
                Action::Command {
                    if_running: IfRunning::Run,
                    ..
                }
            ),
            "老配置缺 if_running 该是默认 run: {old:?}"
        );
    }

    #[cfg(feature = "automation")]
    #[test]
    fn migrate_cmd_powershell_open_folder() {
        let mut cfg = Config {
            shortcuts: vec![ShortcutItem {
                triggers: vec!["Ctrl+K".into()],
                actions: vec![
                    Action::Cmd {
                        command: "echo hi".into(),
                        show_output: true,
                        var: "out".into(),
                        description: None,
                    },
                    Action::Powershell {
                        command: "Get-Date".into(),
                        show_output: false,
                        var: "".into(),
                        description: None,
                    },
                    Action::OpenFolder {
                        path: "C:\\Users".into(),
                        description: None,
                    },
                ],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        cfg.migrate();
        let acts = &cfg.shortcuts[0].actions;
        assert!(matches!(
            acts[0],
            Action::Command {
                shell: Shell::Cmd,
                ..
            }
        ));
        assert!(matches!(
            acts[1],
            Action::Command {
                shell: Shell::Powershell,
                ..
            }
        ));
        assert!(matches!(
            acts[2],
            Action::Os {
                operation: OsOperation::OpenFolder { .. },
                description: _
            }
        ));
    }

    #[cfg(feature = "automation")]
    #[test]
    fn os_open_folder_new_folder_validate() {
        assert!(Action::Os {
            operation: OsOperation::OpenFolder {
                path: "C:\\".into()
            },
            description: None
        }
        .validate()
        .is_ok());
        assert!(Action::Os {
            operation: OsOperation::NewFolder {
                path: "C:\\x".into()
            },
            description: None
        }
        .validate()
        .is_ok());
        assert!(Action::Os {
            operation: OsOperation::OpenFolder { path: "  ".into() },
            description: None
        }
        .validate()
        .is_err());
        assert!(Action::Os {
            operation: OsOperation::NewFolder { path: "".into() },
            description: None
        }
        .validate()
        .is_err());
    }

    /// 切换虚拟桌面（操作系统动作）：没有必填字段（方向有缺省）；手工只写
    /// `{"op":"switch_desktop"}` 也能读，且方向缺省为「下一个」。
    #[cfg(feature = "automation")]
    #[test]
    fn switch_desktop_validate_and_default_direction() {
        for direction in [DesktopDirection::Prev, DesktopDirection::Next] {
            assert!(Action::System {
                operation: SystemOperation::SwitchDesktop { direction },
                description: None
            }
            .validate()
            .is_ok());
        }

        let bare: Action =
            serde_json::from_str(r#"{"type":"system","operation":{"op":"switch_desktop"}}"#)
                .unwrap();
        assert!(
            matches!(
                bare,
                Action::System {
                    operation: SystemOperation::SwitchDesktop {
                        direction: DesktopDirection::Next
                    },
                    ..
                }
            ),
            "缺省方向该是下一个: {bare:?}"
        );

        let prev = Action::System {
            operation: SystemOperation::SwitchDesktop {
                direction: DesktopDirection::Prev,
            },
            description: None,
        };
        let json = serde_json::to_string(&prev).unwrap();
        assert!(
            json.contains(r#""type":"system""#),
            "类型该是 system: {json}"
        );
        assert!(json.contains(r#""direction":"prev""#), "方向该落盘: {json}");
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), prev);

        // 文件/目录动作里不该再有切桌面这项（两类已分开）。
        let file_only: Action =
            serde_json::from_str(r#"{"type":"os","operation":{"op":"delete","path":"x"}}"#)
                .unwrap();
        assert!(
            matches!(file_only, Action::Os { .. }),
            "文件动作照旧读成 Os"
        );
    }

    #[cfg(feature = "automation")]
    #[test]
    fn script_action_json_roundtrip_and_validate() {
        let actions = vec![
            Action::Script {
                path: "C:\\scripts\\do.ps1".into(),
                interpreter: None,
                interpreter_args: Vec::new(),
                args: Vec::new(),
                working_dir: String::new(),
                timeout_ms: None,
                ok_exit_codes: Vec::new(),
                show_output: false.into(),
                hide_window: false,
                var: "".into(),
                wait: true,
                on_error: OnError::Continue,
                if_running: IfRunning::Run,
                description: None,
            },
            Action::Script {
                path: "backup.py".into(),
                interpreter: Some("python".into()),
                interpreter_args: Vec::new(),
                args: Vec::new(),
                working_dir: String::new(),
                timeout_ms: None,
                ok_exit_codes: Vec::new(),
                show_output: true.into(),
                hide_window: false,
                var: "out".into(),
                wait: true,
                on_error: OnError::Continue,
                if_running: IfRunning::Run,
                description: None,
            },
        ];
        for a in &actions {
            assert!(a.validate().is_ok(), "{a:?}");
        }
        let json = serde_json::to_string(&actions).unwrap();
        let back: Vec<Action> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, actions);

        // 脚本路径为空必被拒绝。
        assert!(Action::Script {
            path: "  ".into(),
            interpreter: None,
            interpreter_args: Vec::new(),
            args: Vec::new(),
            working_dir: String::new(),
            timeout_ms: None,
            ok_exit_codes: Vec::new(),
            show_output: false.into(),
            hide_window: false,
            var: "".into(),
            wait: true,
            on_error: OnError::Continue,
            if_running: IfRunning::Run,
            description: None,
        }
        .validate()
        .is_err());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn ask_action_requires_var_and_prompt() {
        let ok = Action::Ask {
            var: "version".into(),
            prompt: "版本号".into(),
            default: "1.2.3".into(),
            description: None,
        };
        ok.validate().unwrap();
        let json = serde_json::to_string(&ok).unwrap();
        let back: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ok);

        for bad in [
            Action::Ask {
                var: " ".into(),
                prompt: "p".into(),
                default: String::new(),
                description: None,
            },
            Action::Ask {
                var: "v".into(),
                prompt: " ".into(),
                default: String::new(),
                description: None,
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn sanitize_cleans_folders() {
        let cfg = Config {
            folders: vec![
                Folder {
                    id: "a".into(),
                    name: "工作".into(),
                    parent: None,
                },
                Folder {
                    id: "b".into(),
                    name: "子目录".into(),
                    parent: Some("a".into()),
                },
                Folder {
                    id: "a".into(),
                    name: "重复".into(),
                    parent: None,
                }, // 重复 id
                Folder {
                    id: "".into(),
                    name: "空id".into(),
                    parent: None,
                }, // 空 id
                Folder {
                    id: "c".into(),
                    name: "孤儿父".into(),
                    parent: Some("nope".into()),
                }, // 父不存在
                Folder {
                    id: "d".into(),
                    name: "成环".into(),
                    parent: Some("d".into()),
                }, // 自环
            ],
            shortcuts: vec![
                ShortcutItem {
                    triggers: vec!["Ctrl+K".into()],
                    actions: vec![Action::Text {
                        text: "x".into(),
                        mode: TextMode::Input,
                        masked: false,
                        description: None,
                    }],
                    folder: Some("a".into()),
                    enabled: true,
                    ..Default::default()
                },
                ShortcutItem {
                    triggers: vec!["Ctrl+J".into()],
                    actions: vec![Action::Text {
                        text: "y".into(),
                        mode: TextMode::Input,
                        masked: false,
                        description: None,
                    }],
                    folder: Some("missing".into()), // 引用不存在目录
                    enabled: true,
                    ..Default::default()
                },
            ],
            // 改键/文本扩展与快捷键同规则：目录引用也参与校验（断引用清空）。
            remaps: vec![
                Remap {
                    from: "CapsLock".into(),
                    to: "Ctrl".into(),
                    folder: Some("a".into()),
                    enabled: true,
                    ..Default::default()
                },
                Remap {
                    from: "F1".into(),
                    to: "F2".into(),
                    folder: Some("missing".into()),
                    enabled: true,
                    ..Default::default()
                },
            ],
            expansions: vec![TextExpansion {
                trigger: ";a".into(),
                replace: "x".into(),
                folder: Some("missing".into()),
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        // 保留 a、b、c、d（d 的环被断），去掉重复 a 与空 id。
        assert_eq!(
            clean.folders.len(),
            4,
            "folders: {:?} ignored: {:?}",
            clean.folders,
            ignored
        );
        assert_eq!(clean.folders[0].name, "工作");
        assert_eq!(clean.folders[1].parent.as_deref(), Some("a"));
        assert!(clean
            .folders
            .iter()
            .find(|f| f.id == "c")
            .unwrap()
            .parent
            .is_none());
        assert!(clean
            .folders
            .iter()
            .find(|f| f.id == "d")
            .unwrap()
            .parent
            .is_none());
        // 无效目录引用被清空
        assert_eq!(clean.shortcuts[0].folder.as_deref(), Some("a"));
        assert!(clean.shortcuts[1].folder.is_none());
        assert_eq!(clean.remaps[0].folder.as_deref(), Some("a"));
        assert!(clean.remaps[1].folder.is_none());
        assert!(clean.expansions[0].folder.is_none());
    }
}

#[cfg(test)]
mod hotstring_tests {
    use crate::*;

    #[test]
    fn key_to_char_letters_digits_punct() {
        assert_eq!(key_to_char(Key::A, false), Some('a'));
        assert_eq!(key_to_char(Key::A, true), Some('A'));
        assert_eq!(key_to_char(Key::Z, false), Some('z'));
        assert_eq!(key_to_char(Key::Digit1, false), Some('1'));
        assert_eq!(key_to_char(Key::Digit1, true), Some('!'));
        assert_eq!(key_to_char(Key::Semicolon, false), Some(';'));
        assert_eq!(key_to_char(Key::Semicolon, true), Some(':'));
        assert_eq!(key_to_char(Key::Comma, true), Some('<'));
        assert_eq!(key_to_char(Key::Minus, true), Some('_'));
        assert_eq!(key_to_char(Key::Space, false), Some(' '));
        // 非可打印键返回 None
        assert_eq!(key_to_char(Key::Enter, false), None);
        assert_eq!(key_to_char(Key::ArrowUp, false), None);
        assert_eq!(key_to_char(Key::Control, false), None);
        assert_eq!(key_to_char(Key::F13, false), None);
        assert_eq!(key_to_char(Key::MouseBack, false), None);
    }

    #[test]
    fn terminator_detection() {
        assert!(is_hotstring_terminator(Key::Space));
        assert!(is_hotstring_terminator(Key::Enter));
        assert!(is_hotstring_terminator(Key::Tab));
        assert!(!is_hotstring_terminator(Key::A));
        assert!(!is_hotstring_terminator(Key::Backspace));
    }

    #[test]
    fn match_expansion_longest_wins() {
        let exps = vec![
            TextExpansion {
                trigger: "addr".into(),
                replace: "地址".into(),
                enabled: true,
                ..Default::default()
            },
            TextExpansion {
                trigger: "email".into(),
                replace: "邮箱".into(),
                enabled: true,
                ..Default::default()
            },
            TextExpansion {
                trigger: "work-email".into(),
                replace: "工作邮箱".into(),
                enabled: true,
                ..Default::default()
            },
            TextExpansion {
                trigger: "off".into(),
                replace: "关".into(),
                enabled: false,
                ..Default::default()
            }, // 停用
        ];
        assert_eq!(match_expansion(":addr", &exps).unwrap().replace, "地址");
        // 最长匹配优先（work-email 而非 email）
        assert_eq!(
            match_expansion("work-email", &exps).unwrap().replace,
            "工作邮箱"
        );
        // 停用的不命中
        assert_eq!(match_expansion("off", &exps), None);
        // 无命中
        assert_eq!(match_expansion("xyz", &exps), None);
        // 空触发词不参与匹配
        let exps2 = vec![TextExpansion {
            trigger: "".into(),
            replace: "x".into(),
            enabled: true,
            ..Default::default()
        }];
        assert_eq!(match_expansion("", &exps2), None);
    }

    #[test]
    fn expansion_json_roundtrip_and_validate() {
        let exps = vec![
            TextExpansion {
                trigger: ":addr".into(),
                replace: "上海市…".into(),
                enabled: true,
                ..Default::default()
            },
            TextExpansion {
                trigger: ";sig".into(),
                replace: "{date}".into(),
                enabled: false,
                ..Default::default()
            },
        ];
        let json = serde_json::to_string(&exps).unwrap();
        let back: Vec<TextExpansion> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, exps);

        // 空触发词必被拒绝；空替换允许（等价删除触发词）
        assert!(TextExpansion {
            trigger: "  ".into(),
            replace: "x".into(),
            enabled: true,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(TextExpansion {
            trigger: "ok".into(),
            replace: "".into(),
            enabled: true,
            ..Default::default()
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn sanitize_drops_invalid_expansions() {
        let cfg = Config {
            expansions: vec![
                TextExpansion {
                    trigger: "addr".into(),
                    replace: "地址".into(),
                    enabled: true,
                    ..Default::default()
                },
                TextExpansion {
                    trigger: "  ".into(),
                    replace: "坏".into(),
                    enabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(clean.expansions.len(), 1);
        assert!(ignored.iter().any(|m| m.contains("文本扩展")));
    }
}

#[cfg(test)]
mod tap_hold_tests {
    use crate::*;

    #[test]
    fn remap_tap_hold_detection_and_keys() {
        let ordinary = Remap {
            from: "CapsLock".into(),
            to: "Ctrl".into(),
            enabled: true,
            ..Default::default()
        };
        assert!(!ordinary.is_tap_hold());
        assert_eq!(ordinary.tap_key(), None);
        assert_eq!(ordinary.hold_key(), None);
        assert_eq!(ordinary.tap_timeout(), DEFAULT_TAP_TIMEOUT_MS); // 0 归一化为 200

        let tap_hold = Remap {
            from: "CapsLock".into(),
            to: "".into(),
            tap: Some("Esc".into()),
            hold: Some("Ctrl".into()),
            tap_timeout_ms: 0,
            enabled: true,
            ..Default::default()
        };
        assert!(tap_hold.is_tap_hold());
        assert_eq!(tap_hold.tap_key(), Some(Key::Escape));
        assert_eq!(tap_hold.hold_key(), Some(Key::Control));
        assert_eq!(tap_hold.tap_timeout(), 200);

        // 只填 tap、只填 hold 均可；自定义阈值生效
        let tap_only = Remap {
            from: "CapsLock".into(),
            to: "".into(),
            tap: Some("Esc".into()),
            hold: None,
            tap_timeout_ms: 150,
            enabled: true,
            ..Default::default()
        };
        assert!(tap_only.is_tap_hold());
        assert_eq!(tap_only.tap_key(), Some(Key::Escape));
        assert_eq!(tap_only.hold_key(), None);
        assert_eq!(tap_only.tap_timeout(), 150);
    }

    #[test]
    fn remap_json_roundtrip_tap_hold() {
        let r = Remap {
            from: "CapsLock".into(),
            to: "".into(),
            tap: Some("Esc".into()),
            hold: Some("Ctrl".into()),
            tap_timeout_ms: 180,
            enabled: true,
            ..Default::default()
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: Remap = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);

        // 旧配置（无 tap/hold/tap_timeout_ms）反序列化 → 普通改键，阈值默认 200
        let legacy = r#"{"from":"CapsLock","to":"Ctrl","enabled":true}"#;
        let l: Remap = serde_json::from_str(legacy).unwrap();
        assert!(!l.is_tap_hold());
        assert_eq!(l.tap_timeout(), DEFAULT_TAP_TIMEOUT_MS);
    }

    #[test]
    fn sanitize_tap_hold_keeps_valid_drops_invalid() {
        let cfg = Config {
            remaps: vec![
                Remap {
                    from: "CapsLock".into(),
                    to: "".into(),
                    tap: Some("Esc".into()),
                    hold: Some("Ctrl".into()),
                    tap_timeout_ms: 200,
                    enabled: true,
                    ..Default::default()
                },
                // tap 非法、hold 合法 → 清空 tap、保留 hold
                Remap {
                    from: "A".into(),
                    to: "".into(),
                    tap: Some("NotAKey".into()),
                    hold: Some("Shift".into()),
                    tap_timeout_ms: 200,
                    enabled: true,
                    ..Default::default()
                },
                // tap/hold 都非法 → 整条忽略
                Remap {
                    from: "B".into(),
                    to: "".into(),
                    tap: Some("NotAKey".into()),
                    hold: Some("Bad".into()),
                    tap_timeout_ms: 200,
                    enabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(clean.remaps.len(), 2);
        assert!(clean.remaps[0].is_tap_hold());
        assert!(clean.remaps[1].is_tap_hold());
        assert_eq!(clean.remaps[1].tap, None);
        assert_eq!(clean.remaps[1].hold.as_deref(), Some("Shift"));
        assert!(ignored.iter().any(|m| m.contains("均无法解析")));
    }
}

#[cfg(test)]
mod layer_tests {
    use crate::*;

    #[test]
    fn layer_and_hold_layer_detection() {
        let remap = Remap {
            from: "Space".into(),
            to: "".into(),
            hold_layer: Some("symbols".into()),
            ..Default::default()
        };
        assert!(remap.is_tap_hold()); // hold_layer 也走 tap-hold 状态机
        assert_eq!(remap.hold_layer_id(), Some("symbols"));
        assert_eq!(remap.tap_key(), None);
        assert_eq!(remap.hold_key(), None);

        let blank = Remap {
            from: "Space".into(),
            to: "".into(),
            hold_layer: Some("  ".into()),
            ..Default::default()
        };
        assert_eq!(blank.hold_layer_id(), None);
    }

    #[test]
    fn layer_json_roundtrip() {
        let layers = vec![
            Layer {
                id: "base".into(),
                name: "基础".into(),
                ..Default::default()
            },
            Layer {
                id: "symbols".into(),
                name: "符号".into(),
                ..Default::default()
            },
        ];
        let json = serde_json::to_string(&layers).unwrap();
        let back: Vec<Layer> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, layers);
    }

    #[test]
    fn layer_mark_survives_sanitize_and_json_roundtrip() {
        let cfg = Config {
            layers: vec![
                Layer {
                    id: "c".into(),
                    name: "颜色".into(),
                    mark: Some("  #4F9CF9 ".into()),
                },
                Layer {
                    id: "e".into(),
                    name: "图标".into(),
                    mark: Some("🎮".into()),
                },
                Layer {
                    id: "n".into(),
                    name: "空".into(),
                    mark: Some("   ".into()),
                },
            ],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        assert!(ignored.is_empty(), "{ignored:?}");
        assert_eq!(clean.layers[0].mark.as_deref(), Some("#4f9cf9")); // 颜色归一为小写
        assert_eq!(clean.layers[1].mark.as_deref(), Some("🎮")); // emoji 原样保留
        assert_eq!(clean.layers[2].mark, None); // 空白 → 无标记

        // 无标记时字段省略（老配置没有 mark，保存不该凭空多写一个字段）。
        assert!(!serde_json::to_string(&clean.layers[2])
            .unwrap()
            .contains("mark"));

        let back: Config = serde_json::from_str(&serde_json::to_string(&clean).unwrap()).unwrap();
        assert_eq!(back, clean);
    }

    #[test]
    fn layer_without_switch_key_is_reported() {
        // 层里有启用的条目、却没有任何切层键指向它 → 永久不可达，必须在冲突清单里点出来，
        // 否则用户只能看到「层内快捷键配了但不触发」而查不出原因。
        let layered = ShortcutItem {
            layer: Some("symbols".into()),
            triggers: vec!["5&Y".into()],
            actions: vec![],
            enabled: true,
            ..Default::default()
        };
        let base = Config {
            layers: vec![Layer {
                id: "symbols".into(),
                name: "符号".into(),
                ..Default::default()
            }],
            shortcuts: vec![layered],
            ..Default::default()
        };
        let hits = detect_conflicts(&base, &PlatformCaps::default());
        assert!(
            hits.iter().any(|c| c.message.contains("没有切层键")),
            "无切层键的层应被报出"
        );

        // 加一条指向它的「长按进入层」改键后，提示消失。
        let with_key = Config {
            remaps: vec![Remap {
                from: "Space".into(),
                to: "".into(),
                hold_layer: Some("symbols".into()),
                enabled: true,
                ..Default::default()
            }],
            ..base.clone()
        };
        assert!(!detect_conflicts(&with_key, &PlatformCaps::default())
            .iter()
            .any(|c| c.message.contains("没有切层键")));

        // 「长按锁定层」同样是切层键，指向它也算可达（层内放和弦/序列只能用这种）。
        let with_lock = Config {
            remaps: vec![Remap {
                from: "Space".into(),
                to: "".into(),
                lock_layer: Some("symbols".into()),
                enabled: true,
                ..Default::default()
            }],
            ..base.clone()
        };
        assert!(
            !detect_conflicts(&with_lock, &PlatformCaps::default())
                .iter()
                .any(|c| c.message.contains("没有切层键")),
            "长按锁定层也应算可达：{:?}",
            detect_conflicts(&with_lock, &PlatformCaps::default())
        );

        // 层里没有启用的条目时不报（空层不算问题）。
        let empty = Config {
            layers: vec![Layer {
                id: "symbols".into(),
                name: "符号".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(!detect_conflicts(&empty, &PlatformCaps::default())
            .iter()
            .any(|c| c.message.contains("没有切层键")));
    }

    #[test]
    fn lock_layer_detection_and_summary() {
        let remap = Remap {
            from: "Tab".into(),
            to: "".into(),
            lock_layer: Some("symbols".into()),
            ..Default::default()
        };
        assert!(remap.is_tap_hold(), "lock_layer 也走 tap-hold 状态机");
        assert!(remap.needs_timing_state());
        assert_eq!(remap.lock_layer_id(), Some("symbols"));
        assert_eq!(remap.hold_layer_id(), None);
        assert!(
            remap.describe().contains("长按锁定层"),
            "摘要要能看出是锁定式：{}",
            remap.describe()
        );

        let blank = Remap {
            from: "Tab".into(),
            to: "".into(),
            lock_layer: Some("  ".into()),
            ..Default::default()
        };
        assert_eq!(blank.lock_layer_id(), None);
    }

    #[test]
    fn sanitize_keeps_lock_layer_and_drops_blank_refs() {
        let cfg = Config {
            layers: vec![Layer {
                id: "symbols".into(),
                name: "符号".into(),
                ..Default::default()
            }],
            remaps: vec![
                Remap {
                    from: "Tab".into(),
                    to: "".into(),
                    lock_layer: Some("symbols".into()),
                    ..Default::default()
                },
                Remap {
                    from: "A".into(),
                    to: "".into(),
                    lock_layer: Some("nope".into()), // 层不存在 → 清空 → 整条忽略
                    ..Default::default()
                },
                // 同时设了 momentary 与切换式：保留切换式（层内和弦/序列只能靠它）。
                Remap {
                    from: "B".into(),
                    to: "".into(),
                    hold_layer: Some("symbols".into()),
                    lock_layer: Some("symbols".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(clean.remaps.len(), 2);
        assert_eq!(clean.remaps[0].lock_layer.as_deref(), Some("symbols"));
        assert_eq!(clean.remaps[0].hold_layer, None);
        assert_eq!(clean.remaps[1].hold_layer, None);
        assert_eq!(clean.remaps[1].lock_layer.as_deref(), Some("symbols"));
        assert!(ignored.iter().any(|m| m.contains("层不存在")));
        assert!(ignored.iter().any(|m| m.contains("保留长按锁定层")));
    }

    #[test]
    fn sanitize_cleans_layers_and_dangling_refs() {
        let cfg = Config {
            layers: vec![
                Layer {
                    id: "symbols".into(),
                    name: "符号".into(),
                    ..Default::default()
                },
                Layer {
                    id: "symbols".into(),
                    name: "重复".into(),
                    ..Default::default()
                }, // 重复 id → 忽略
                Layer {
                    id: "".into(),
                    name: "坏".into(),
                    ..Default::default()
                }, // 空 id → 忽略
            ],
            shortcuts: vec![
                ShortcutItem {
                    layer: Some("symbols".into()),
                    triggers: vec!["1".into()],
                    actions: vec![Action::Text {
                        text: "F1".into(),
                        mode: TextMode::Input,
                        masked: false,
                        description: None,
                    }],
                    enabled: true,
                    ..Default::default()
                },
                ShortcutItem {
                    layer: Some("nope".into()), // 层不存在 → 清空
                    triggers: vec!["2".into()],
                    actions: vec![],
                    enabled: true,
                    ..Default::default()
                },
            ],
            remaps: vec![
                Remap {
                    from: "Space".into(),
                    to: "".into(),
                    hold_layer: Some("symbols".into()),
                    ..Default::default()
                },
                Remap {
                    from: "A".into(),
                    to: "".into(),
                    hold_layer: Some("nope".into()), // 层不存在 → 清空 → 整条忽略
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(clean.layers.len(), 1);
        assert_eq!(clean.layers[0].id, "symbols");
        assert_eq!(clean.shortcuts.len(), 2);
        assert_eq!(clean.shortcuts[0].layer.as_deref(), Some("symbols"));
        assert_eq!(clean.shortcuts[1].layer, None);
        assert_eq!(clean.remaps.len(), 1);
        assert_eq!(clean.remaps[0].hold_layer.as_deref(), Some("symbols"));
        assert!(ignored.iter().any(|m| m.contains("层")));
    }
}

#[cfg(test)]
mod sequence_tests {
    use crate::*;

    fn ev(key: Key, mods: &[Modifier]) -> RawEvent {
        RawEvent {
            key,
            mods: mods.iter().copied().collect(),
            pressed: true,
        }
    }

    /// 把 "F9 J K" 解析成一串步骤。
    fn seq(s: &str) -> Vec<Shortcut> {
        s.split_whitespace().map(|t| t.parse().unwrap()).collect()
    }

    #[test]
    fn trigger_parse_single_sequence_empty_bad() {
        // 单 token → Combo。
        let t = Trigger::parse("Ctrl+K").unwrap();
        assert!(matches!(t, Trigger::Combo(_)));
        assert!(!t.is_sequence());
        assert_eq!(t.steps().len(), 1);

        // 多 token → Sequence。
        let t = Trigger::parse("F9 J K").unwrap();
        assert!(matches!(t, Trigger::Sequence(ref steps) if steps.len() == 3));
        assert!(t.is_sequence());
        assert_eq!(t.first(), &"F9".parse::<Shortcut>().unwrap());

        // "Space" 单 token 仍是 Combo；"F9 Space" 是序列。
        assert!(matches!(
            Trigger::parse("Space").unwrap(),
            Trigger::Combo(_)
        ));
        assert!(matches!(
            Trigger::parse("F9 Space").unwrap(),
            Trigger::Sequence(ref steps) if steps.len() == 2
        ));

        // 组合键可作为序列步（"F9 Ctrl+K"）。
        assert!(matches!(
            Trigger::parse("F9 Ctrl+K").unwrap(),
            Trigger::Sequence(ref steps) if steps.len() == 2
        ));

        // 空串 / 纯空白 → Empty。
        assert!(matches!(Trigger::parse(""), Err(ParseError::Empty)));
        assert!(matches!(Trigger::parse("   "), Err(ParseError::Empty)));

        // 坏步 → 解析失败（任一坏步都不保留）。
        assert!(Trigger::parse("F9 NotAKey").is_err());
    }

    #[test]
    fn sequence_tracker_start_advance_complete() {
        let seqs = vec![seq("F9 J K")];
        let mut tr = SequenceTracker::new();
        assert!(!tr.is_active());

        // F9 → 建立候选（leader 吞掉）。
        assert_eq!(tr.advance(&ev(Key::F9, &[]), &seqs), SeqAdvance::Advance);
        assert!(tr.is_active());

        // J → 推进。
        assert_eq!(tr.advance(&ev(Key::J, &[]), &seqs), SeqAdvance::Advance);
        assert!(tr.is_active());

        // K → 完成（下标 0），状态清空。
        assert_eq!(tr.advance(&ev(Key::K, &[]), &seqs), SeqAdvance::Complete(0));
        assert!(!tr.is_active());
    }

    #[test]
    fn sequence_tracker_shared_prefix() {
        let seqs = vec![seq("F9 J K"), seq("F9 J L")];
        let mut tr = SequenceTracker::new();
        assert_eq!(tr.advance(&ev(Key::F9, &[]), &seqs), SeqAdvance::Advance);
        assert_eq!(tr.advance(&ev(Key::J, &[]), &seqs), SeqAdvance::Advance);
        assert_eq!(tr.advance(&ev(Key::K, &[]), &seqs), SeqAdvance::Complete(0));

        let mut tr = SequenceTracker::new();
        assert_eq!(tr.advance(&ev(Key::F9, &[]), &seqs), SeqAdvance::Advance);
        assert_eq!(tr.advance(&ev(Key::J, &[]), &seqs), SeqAdvance::Advance);
        assert_eq!(tr.advance(&ev(Key::L, &[]), &seqs), SeqAdvance::Complete(1));
    }

    #[test]
    fn sequence_tracker_chain_break_resets() {
        let seqs = vec![seq("F9 J K")];
        let mut tr = SequenceTracker::new();
        assert_eq!(tr.advance(&ev(Key::F9, &[]), &seqs), SeqAdvance::Advance);
        assert!(tr.is_active());
        // 断链：按下无关键 → NoMatch 并重置。
        assert_eq!(tr.advance(&ev(Key::X, &[]), &seqs), SeqAdvance::NoMatch);
        assert!(!tr.is_active());
    }

    #[test]
    fn sequence_tracker_no_match_when_idle() {
        let seqs = vec![seq("F9 J K")];
        let mut tr = SequenceTracker::new();
        assert_eq!(tr.advance(&ev(Key::A, &[]), &seqs), SeqAdvance::NoMatch);
        assert!(!tr.is_active());
    }

    #[test]
    fn sanitize_keeps_valid_sequence_drops_invalid() {
        let cfg = Config {
            shortcuts: vec![
                ShortcutItem {
                    triggers: vec!["F9 J K".into()],
                    actions: vec![Action::Text {
                        text: "ok".into(),
                        mode: TextMode::Input,
                        masked: false,
                        description: None,
                    }],
                    enabled: true,
                    ..Default::default()
                },
                ShortcutItem {
                    triggers: vec!["F9 BadStep".into()],
                    actions: vec![Action::Text {
                        text: "bad".into(),
                        mode: TextMode::Input,
                        masked: false,
                        description: None,
                    }],
                    enabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(clean.shortcuts.len(), 2, "ignored: {}", ignored.join("; "));
        // 合法序列保留；坏步序列被清空并提示。
        assert_eq!(clean.shortcuts[0].triggers, vec!["F9 J K".to_string()]);
        assert!(clean.shortcuts[1].triggers.is_empty());
        assert!(ignored.iter().any(|m| m.contains("F9 BadStep")));
    }
}

#[cfg(test)]
mod chord_tests {
    use crate::*;

    /// 把 "F&J" 解析成一组成员键（走真正的和弦解析路径，含纯修饰键成员的折叠）。
    fn chord(s: &str) -> Vec<Shortcut> {
        match Trigger::parse(s).unwrap() {
            Trigger::Chord(members) => members,
            _ => panic!("不是和弦: {s}"),
        }
    }

    /// 不带修饰键按下一个成员键（多数用例的默认情形）。
    fn press(tr: &mut ChordTracker, key: Key, chords: &[Vec<Shortcut>]) -> ChordAdvance {
        tr.press(key, &BTreeSet::new(), chords)
    }

    #[test]
    fn chord_tracker_await_then_complete() {
        let chords = vec![chord("F&J")];
        let mut tr = ChordTracker::new();
        assert!(!tr.is_active());
        // F 是成员 → Await（吞掉），未凑齐。
        assert_eq!(press(&mut tr, Key::F, &chords), ChordAdvance::Await);
        assert!(tr.is_active());
        // J 凑齐 → Complete(0)，状态清空。
        assert_eq!(press(&mut tr, Key::J, &chords), ChordAdvance::Complete(0));
        assert!(!tr.is_active());
    }

    #[test]
    fn chord_tracker_unordered() {
        // 先按 J 再按 F 也命中（无序）。
        let chords = vec![chord("F&J")];
        let mut tr = ChordTracker::new();
        assert_eq!(press(&mut tr, Key::J, &chords), ChordAdvance::Await);
        assert_eq!(press(&mut tr, Key::F, &chords), ChordAdvance::Complete(0));
    }

    #[test]
    fn chord_tracker_three_members() {
        let chords = vec![chord("D&F&J")];
        let mut tr = ChordTracker::new();
        assert_eq!(press(&mut tr, Key::D, &chords), ChordAdvance::Await);
        assert_eq!(press(&mut tr, Key::F, &chords), ChordAdvance::Await);
        assert_eq!(press(&mut tr, Key::J, &chords), ChordAdvance::Complete(0));
    }

    #[test]
    fn chord_tracker_non_member_keeps_held() {
        // 非成员键不影响按住集合：成员仍按住，后续补齐成员照样命中。
        let chords = vec![chord("F&J")];
        let mut tr = ChordTracker::new();
        assert_eq!(press(&mut tr, Key::F, &chords), ChordAdvance::Await);
        assert!(tr.is_active());
        assert_eq!(press(&mut tr, Key::X, &chords), ChordAdvance::NoMatch);
        assert!(tr.is_active(), "非成员键不打断已按住的成员");
        assert_eq!(press(&mut tr, Key::J, &chords), ChordAdvance::Complete(0));
    }

    #[test]
    fn chord_tracker_release_removes_member() {
        // 成员抬起即移出集合：抬手后单独按其它成员不构成「同时按住」（不会误触发）。
        let chords = vec![chord("F&J")];
        let mut tr = ChordTracker::new();
        assert_eq!(press(&mut tr, Key::F, &chords), ChordAdvance::Await);
        assert!(tr.release(Key::F), "F 原本在按住集合里");
        assert!(!tr.is_active());
        assert!(!tr.release(Key::F), "已抬起的键再抬起返回 false");
        assert_eq!(press(&mut tr, Key::J, &chords), ChordAdvance::Await);
        assert_eq!(press(&mut tr, Key::F, &chords), ChordAdvance::Complete(0));
    }

    #[test]
    fn chord_tracker_shared_member() {
        let chords = vec![chord("F&J"), chord("F&K")];
        let mut tr = ChordTracker::new();
        assert_eq!(press(&mut tr, Key::F, &chords), ChordAdvance::Await);
        assert_eq!(press(&mut tr, Key::K, &chords), ChordAdvance::Complete(1));

        let mut tr = ChordTracker::new();
        assert_eq!(press(&mut tr, Key::F, &chords), ChordAdvance::Await);
        assert_eq!(press(&mut tr, Key::J, &chords), ChordAdvance::Complete(0));
    }

    #[test]
    fn chord_tracker_no_match_when_idle() {
        let chords = vec![chord("F&J")];
        let mut tr = ChordTracker::new();
        assert_eq!(press(&mut tr, Key::A, &chords), ChordAdvance::NoMatch);
        assert!(!tr.is_active());
    }

    #[test]
    fn chord_tracker_member_mods_must_be_held_when_completing() {
        // Ctrl+F&J：Ctrl 是「要求按住」——F、J 都按着但 Ctrl 没按就不算凑齐。
        let chords = vec![chord("Ctrl+F&J")];
        let ctrl = BTreeSet::from([Modifier::Ctrl]);
        let mut tr = ChordTracker::new();
        assert_eq!(
            tr.press(Key::F, &BTreeSet::new(), &chords),
            ChordAdvance::Await
        );
        assert_eq!(
            tr.press(Key::J, &BTreeSet::new(), &chords),
            ChordAdvance::Await,
            "缺 Ctrl 时不能凑齐（等待窗到点会把它当普通按键回放）"
        );
        assert!(tr.is_active());
        // 补上 Ctrl 后重新按一次 J（F 还按着）→ 凑齐。
        assert_eq!(tr.press(Key::J, &ctrl, &chords), ChordAdvance::Complete(0));
    }

    #[test]
    fn chord_tracker_modifier_only_member_requires_mods_down() {
        // Ctrl+Alt&J：前一个成员是纯修饰键要求，只按时不参与按住集合、也不吞键。
        let chords = vec![chord("Ctrl+Alt&J")];
        let mut tr = ChordTracker::new();
        assert_eq!(
            tr.press(Key::J, &BTreeSet::from([Modifier::Ctrl]), &chords),
            ChordAdvance::Await,
            "只有 Ctrl 还差 Alt"
        );
        assert_eq!(
            tr.press(
                Key::J,
                &BTreeSet::from([Modifier::Ctrl, Modifier::Alt]),
                &chords
            ),
            ChordAdvance::Complete(0)
        );
        assert!(!tr.held().contains(&Key::Alt), "修饰键成员不进按住集合");
    }

    #[test]
    fn chord_tracker_modifier_key_is_never_a_member() {
        // 修饰键成员不做「按住集合」的登记（壳层会先行放行，这里是兜底的那道）。
        let chords = vec![chord("Ctrl+Alt&J")];
        let mut tr = ChordTracker::new();
        assert_eq!(
            tr.press(Key::Alt, &BTreeSet::from([Modifier::Alt]), &chords),
            ChordAdvance::NoMatch
        );
        assert!(!tr.is_active());
    }

    #[test]
    fn sanitize_keeps_valid_chord_drops_invalid() {
        let cfg = Config {
            shortcuts: vec![
                ShortcutItem {
                    triggers: vec!["Ctrl+F&J".into()],
                    actions: vec![Action::Text {
                        text: "ok".into(),
                        mode: TextMode::Input,
                        masked: false,
                        description: None,
                    }],
                    enabled: true,
                    ..Default::default()
                },
                ShortcutItem {
                    triggers: vec!["F&F".into()],
                    actions: vec![Action::Text {
                        text: "bad".into(),
                        mode: TextMode::Input,
                        masked: false,
                        description: None,
                    }],
                    enabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(clean.shortcuts.len(), 2, "ignored: {}", ignored.join("; "));
        // 合法和弦（成员带修饰）保留；坏和弦（成员重复）被清空并提示。
        assert_eq!(clean.shortcuts[0].triggers, vec!["Ctrl+F&J".to_string()]);
        assert!(clean.shortcuts[1].triggers.is_empty());
        assert!(
            ignored.iter().any(|m| m.contains("F&F")),
            "ignored: {ignored:?}"
        );
    }
    #[test]
    fn trigger_parse_chord() {
        // F&J → Chord（2 成员，无序）。
        let t = Trigger::parse("F&J").unwrap();
        assert!(matches!(t, Trigger::Chord(ref m) if m.len() == 2));
        assert!(t.is_chord());
        assert!(!t.is_sequence());
        assert_eq!(t.steps().len(), 2);
        assert_eq!(t.first(), &"F".parse::<Shortcut>().unwrap());

        // 三成员和弦。
        assert!(matches!(Trigger::parse("D&F&J").unwrap(), Trigger::Chord(ref m) if m.len() == 3));

        // 单键不含 & 仍是 Combo。
        assert!(matches!(Trigger::parse("F").unwrap(), Trigger::Combo(_)));

        // 坏输入：空成员。
        assert!(matches!(
            Trigger::parse("F&"),
            Err(ParseError::ChordInvalid(_))
        ));
        assert!(matches!(
            Trigger::parse("F&&J"),
            Err(ParseError::ChordInvalid(_))
        ));
        // 成员重复（同一个键 + 同一个修饰要求按两次不算更「同时」）。
        assert!(matches!(
            Trigger::parse("F&F"),
            Err(ParseError::ChordInvalid(_))
        ));
        assert!(matches!(
            Trigger::parse("Ctrl+F&Ctrl+F"),
            Err(ParseError::ChordInvalid(_))
        ));
        // 全是修饰键成员：没有可吞的键，也就没有「同时按下」的判定时机。
        assert!(matches!(
            Trigger::parse("Ctrl&Alt"),
            Err(ParseError::ChordInvalid(_))
        ));
        // 一个成员里两个主键（`Ctrl+K+L`）无法表达，报未知按键而不是含糊的和弦无效。
        assert!(Trigger::parse("Ctrl+K+L&J").is_err());
    }

    #[test]
    fn trigger_parse_chord_with_modifier_members() {
        // 成员可带修饰：Ctrl+F&J = 按住 Ctrl 的同时把 F、J 一起按住。
        let t = Trigger::parse("Ctrl+F&J").unwrap();
        let Trigger::Chord(members) = &t else {
            panic!("应为和弦")
        };
        assert_eq!(members[0].key, Key::F);
        assert_eq!(members[0].mods, BTreeSet::from([Modifier::Ctrl]));
        assert_eq!(members[1].key, Key::J);
        assert!(members[1].mods.is_empty());
        assert_eq!(format_shortcut(t.first()), "Ctrl+F", "显示要能原样渲染回去");

        // 纯修饰键成员（`Ctrl+Alt`）：折叠成「要求 Ctrl 按住 + 成员键 Alt」，显示不变。
        let t = Trigger::parse("Ctrl+Alt&J").unwrap();
        let Trigger::Chord(members) = &t else {
            panic!("应为和弦")
        };
        assert_eq!(members[0].key, Key::Alt);
        assert_eq!(members[0].mods, BTreeSet::from([Modifier::Ctrl]));
        assert_eq!(format_shortcut(t.first()), "Ctrl+Alt");
        assert_eq!(members[1].key, Key::J);

        // 单个修饰键成员本来就是合法裸键（无需折叠）。
        let t = Trigger::parse("F&Shift").unwrap();
        let Trigger::Chord(members) = &t else {
            panic!("应为和弦")
        };
        assert_eq!(members[1].key, Key::Shift);
        assert!(members[1].mods.is_empty());
    }
}

/// 设置里的两个等待窗（序列 / 和弦）都是「缺字段拿默认值、0 = 不限时是有效取值」，
/// 两类取值都别被 serde 缺省或清洗改掉。
#[cfg(test)]
mod settings_timeout_tests {
    use crate::*;

    #[test]
    fn defaults_match_the_two_constants() {
        let d = Settings::default();
        assert_eq!(d.sequence_timeout_ms, DEFAULT_SEQUENCE_TIMEOUT_MS);
        assert_eq!(d.chord_timeout_ms, DEFAULT_CHORD_TIMEOUT_MS);
    }

    #[test]
    fn old_config_without_timeout_fields_gets_defaults() {
        // 老配置（写于这两个字段出现之前）应拿到 1000ms，而不是 `#[serde(default)]` 的 0
        // （0 在这里 = 不限时，等于悄悄把等待窗取消）。
        let old: Config =
            serde_json::from_str(r#"{"settings":{"autostart":true,"action_timeout_ms":5000}}"#)
                .unwrap();
        assert_eq!(
            old.settings.sequence_timeout_ms,
            DEFAULT_SEQUENCE_TIMEOUT_MS
        );
        assert_eq!(old.settings.chord_timeout_ms, DEFAULT_CHORD_TIMEOUT_MS);
        assert_eq!(old.settings.action_timeout_ms, 5000, "已有的字段照旧");
    }

    /// 全屏暂停（EXT-05）必须**显式勾选**：老配置缺字段时默认关，勾上后原样往返。
    #[test]
    fn ignore_fullscreen_is_opt_in_and_roundtrips() {
        let old: Config = serde_json::from_str(r#"{"settings":{"autostart":true}}"#).unwrap();
        assert!(!old.settings.ignore_fullscreen);
        let cfg = Config {
            settings: Settings {
                ignore_fullscreen: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains(r#""ignore_fullscreen":true"#), "{json}");
        let back: Config = serde_json::from_str(&json).unwrap();
        assert!(back.settings.ignore_fullscreen);
    }

    #[test]
    fn zero_means_unlimited_and_survives_roundtrip() {
        let cfg = Config {
            settings: Settings {
                sequence_timeout_ms: 0,
                chord_timeout_ms: 2500,
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.settings, cfg.settings);
        assert_eq!(
            back.settings.sequence_timeout_ms, 0,
            "0（不限时）不能被缺省值盖回 1000"
        );
    }
}

/// 软件更新源：老配置必须落在 ihomy 镜像上（国内默认能连通的源），切换后要能原样往返。
#[cfg(test)]
mod update_source_tests {
    use crate::*;

    #[test]
    fn old_config_defaults_to_ihomy_mirror() {
        let old: Config = serde_json::from_str(r#"{"settings":{"autostart":true}}"#).unwrap();
        assert_eq!(old.settings.update_source, UpdateSource::Ihomy);
    }

    /// 自动更新必须**显式勾选**：老配置缺字段时默认关（自动下载 + 空闲安装会重启应用，
    /// 不能给已有用户静默打开），勾上后要能原样往返。
    #[test]
    fn auto_update_is_opt_in_and_roundtrips() {
        let old: Config = serde_json::from_str(r#"{"settings":{"autostart":true}}"#).unwrap();
        assert!(!old.settings.auto_update);
        let cfg = Config {
            settings: Settings {
                auto_update: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains(r#""auto_update":true"#), "{json}");
        let back: Config = serde_json::from_str(&json).unwrap();
        assert!(back.settings.auto_update);
    }

    #[test]
    fn source_roundtrips_with_kebab_case_values() {
        let cfg = Config {
            settings: Settings {
                update_source: UpdateSource::Github,
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains(r#""update_source":"github""#), "{json}");
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.settings, cfg.settings);
    }

    /// 两个地址都是编译期常量：写错一个字符客户端会永远「检查更新失败」，这里先兜住。
    #[test]
    fn endpoints_are_https_and_distinct() {
        for source in [UpdateSource::Ihomy, UpdateSource::Github] {
            assert!(
                source.endpoint().starts_with("https://"),
                "{}",
                source.endpoint()
            );
            assert!(
                source.endpoint().ends_with("latest.json"),
                "{}",
                source.endpoint()
            );
        }
        assert_ne!(
            UpdateSource::Ihomy.endpoint(),
            UpdateSource::Github.endpoint()
        );
    }
}

#[cfg(test)]
mod hints_window_tests {
    use crate::*;

    /// 老配置（写于提示框出现之前）缺 `hints`：默认**不显示**（常挂屏幕的浮层不请自来更烦人），
    /// 但字号 / 透明度要有正经初值，别落到 0 变成看不见的一条。
    #[test]
    fn old_config_without_hints_gets_defaults() {
        let old: Config = serde_json::from_str(r#"{"settings":{"autostart":true}}"#).unwrap();
        assert!(!old.settings.hints.visible);
        assert_eq!(old.settings.hints.scale, HintsWindow::DEFAULT_SCALE);
        assert_eq!(old.settings.hints.opacity, HintsWindow::DEFAULT_OPACITY);
        assert_eq!(old.settings.hints.x, None);
    }

    /// 位置 / 外观要能原样往返：重启后提示框得回到用户拖到的地方、保持他调好的大小与透明度。
    #[test]
    fn hints_roundtrip_keeps_position_and_look() {
        let cfg = Config {
            settings: Settings {
                hints: HintsWindow {
                    visible: true,
                    x: Some(-1200),
                    y: Some(48),
                    scale: 140,
                    opacity: 60,
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let back: Config = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back.settings, cfg.settings);
    }

    /// 没拖过的窗口不必往 JSON 里写 `null`：`skip_serializing_if` 让「没位置」就是个缺字段，
    /// 手改配置时也少两行噪音。
    #[test]
    fn unplaced_hints_omit_coordinates() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!json.contains("\"x\""), "缺省位置不该落进 JSON：{json}");
        assert!(json.contains("\"hints\""));
    }

    /// 归一化：手改 JSON / 别的机器同步过来的越界值必须被钳住，且 `migrate` 会顺手调用它
    /// （加载路径只 `migrate()` 不 `sanitize_config()`，这里是唯一的兜底）。
    #[test]
    fn migrate_clamps_out_of_range_look() {
        let mut cfg = Config {
            settings: Settings {
                hints: HintsWindow {
                    visible: true,
                    scale: 0,
                    opacity: 250,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        cfg.migrate();
        assert_eq!(
            cfg.settings.hints.scale,
            HintsWindow::SCALE_RANGE.0,
            "缩成 0 等于把窗口弄没"
        );
        assert_eq!(cfg.settings.hints.opacity, HintsWindow::OPACITY_RANGE.1);
        assert!(cfg.settings.hints.visible, "钳值不该顺手改可见性");
    }
}

#[cfg(test)]
mod advanced_modifier_tests {
    use crate::*;

    fn remap(fields: impl FnOnce(&mut Remap)) -> Remap {
        let mut r = Remap {
            from: "CapsLock".into(),
            to: "Ctrl".into(),
            enabled: true,
            ..Default::default()
        };
        fields(&mut r);
        r
    }

    #[test]
    fn oneshot_sticky_detection_and_needs_timing() {
        let plain = remap(|_| {});
        assert!(!plain.is_oneshot());
        assert!(!plain.is_sticky());
        assert!(!plain.needs_timing_state());

        let oneshot = remap(|r| {
            r.to = String::new();
            r.oneshot = Some("Ctrl".into());
        });
        assert!(oneshot.is_oneshot());
        assert!(!oneshot.is_sticky());
        assert!(oneshot.needs_timing_state());
        assert_eq!(oneshot.oneshot_key(), Some(Key::Control));

        let sticky = remap(|r| {
            r.to = String::new();
            r.sticky = Some("Shift".into());
        });
        assert!(sticky.is_sticky());
        assert!(!sticky.is_oneshot());
        assert!(sticky.needs_timing_state());
        assert_eq!(sticky.sticky_key(), Some(Key::Shift));
    }

    #[test]
    fn multi_tap_detection_and_output_fallback() {
        // 只有 tap：单击/双击/三击都回落 tap。
        let tap_only = remap(|r| {
            r.to = String::new();
            r.tap = Some("Esc".into());
        });
        assert!(!tap_only.is_multi_tap());
        assert_eq!(tap_only.tap_output(1), Some(Key::Escape));
        assert_eq!(tap_only.tap_output(2), Some(Key::Escape));
        assert_eq!(tap_only.tap_output(3), Some(Key::Escape));

        // tap + tap2：双击 tap2、三击回落 tap2。
        let tap2 = remap(|r| {
            r.to = String::new();
            r.tap = Some("Esc".into());
            r.tap2 = Some("Backspace".into());
        });
        assert!(tap2.is_multi_tap());
        assert_eq!(tap2.tap_output(1), Some(Key::Escape));
        assert_eq!(tap2.tap_output(2), Some(Key::Backspace));
        assert_eq!(tap2.tap_output(3), Some(Key::Backspace));

        // tap + tap2 + tap3：三击 tap3。
        let tap3 = remap(|r| {
            r.to = String::new();
            r.tap = Some("Esc".into());
            r.tap2 = Some("Backspace".into());
            r.tap3 = Some("Delete".into());
        });
        assert!(tap3.is_multi_tap());
        assert_eq!(tap3.tap_output(1), Some(Key::Escape));
        assert_eq!(tap3.tap_output(2), Some(Key::Backspace));
        assert_eq!(tap3.tap_output(3), Some(Key::Delete));
    }

    #[test]
    fn tap_outputs_returns_fallback_array() {
        let r = remap(|r| {
            r.to = String::new();
            r.tap = Some("Esc".into());
            r.tap2 = Some("Backspace".into());
        });
        assert_eq!(
            r.tap_outputs(),
            [
                Some(Key::Escape),
                Some(Key::Backspace),
                Some(Key::Backspace)
            ]
        );
        // 只有 tap：三击都回落 tap。
        let t = remap(|r| {
            r.to = String::new();
            r.tap = Some("Esc".into());
        });
        assert_eq!(
            t.tap_outputs(),
            [Some(Key::Escape), Some(Key::Escape), Some(Key::Escape)]
        );
    }

    #[test]
    fn describe_covers_all_modes() {
        assert_eq!(remap(|_| {}).describe(), "Ctrl");
        assert_eq!(
            remap(|r| {
                r.oneshot = Some("Ctrl".into());
                r.to = String::new();
            })
            .describe(),
            "单击 Ctrl"
        );
        assert_eq!(
            remap(|r| {
                r.sticky = Some("Shift".into());
                r.to = String::new();
            })
            .describe(),
            "粘滞 Shift"
        );
        assert_eq!(
            remap(|r| {
                r.to = String::new();
                r.tap = Some("Esc".into());
                r.hold = Some("Ctrl".into());
            })
            .describe(),
            "单击 Esc · 长按 Ctrl"
        );
        assert_eq!(
            remap(|r| {
                r.to = String::new();
                r.tap = Some("Esc".into());
                r.tap2 = Some("Backspace".into());
            })
            .describe(),
            "单击 Esc · 双击 Backspace"
        );
    }

    #[test]
    fn remap_json_roundtrip_new_fields() {
        let r = remap(|r| {
            r.to = String::new();
            r.tap = Some("Esc".into());
            r.tap2 = Some("Backspace".into());
            r.tap3 = Some("Delete".into());
            r.oneshot = Some("Ctrl".into());
            r.sticky = Some("Shift".into());
        });
        let json = serde_json::to_string(&r).unwrap();
        let back: Remap = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
        // 旧配置（无新字段）反序列化 → 新字段全为 None。
        let legacy = r#"{"from":"CapsLock","to":"Ctrl","enabled":true}"#;
        let l: Remap = serde_json::from_str(legacy).unwrap();
        assert!(l.oneshot.is_none() && l.sticky.is_none() && l.tap2.is_none() && l.tap3.is_none());
    }

    #[test]
    fn sanitize_oneshot_sticky_normalizes_and_drops_invalid() {
        let cfg = Config {
            remaps: vec![
                // 粘滞 + 单次同时设 → 保留粘滞、清掉单次与 tap-hold 字段。
                Remap {
                    from: "CapsLock".into(),
                    to: "".into(),
                    sticky: Some("Shift".into()),
                    oneshot: Some("Ctrl".into()),
                    tap: Some("Esc".into()),
                    enabled: true,
                    ..Default::default()
                },
                // 单次修饰合法 → 保留；普通 to 清空。
                Remap {
                    from: "A".into(),
                    to: "B".into(),
                    oneshot: Some("Alt".into()),
                    enabled: true,
                    ..Default::default()
                },
                // 坏键名 → 清空后无形态 → 整条忽略。
                Remap {
                    from: "B".into(),
                    to: "".into(),
                    oneshot: Some("NotAKey".into()),
                    enabled: true,
                    ..Default::default()
                },
                // 坏 tap2 → 清空；tap 仍保留为 tap-hold。
                Remap {
                    from: "C".into(),
                    to: "".into(),
                    tap: Some("Esc".into()),
                    tap2: Some("Bad".into()),
                    enabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(
            clean.remaps.len(),
            3,
            "remaps: {:?}\nignored: {:?}",
            clean.remaps,
            ignored
        );

        // 第 0 条：粘滞 Shift，其它清空。
        assert_eq!(clean.remaps[0].sticky.as_deref(), Some("Shift"));
        assert!(clean.remaps[0].oneshot.is_none());
        assert!(clean.remaps[0].tap.is_none());
        // 第 1 条：单次 Alt，to 清空。
        assert_eq!(clean.remaps[1].oneshot.as_deref(), Some("Alt"));
        assert_eq!(clean.remaps[1].to, "");
        // 第 2 条：坏 tap2 清空，保留 tap。
        assert_eq!(clean.remaps[2].tap.as_deref(), Some("Esc"));
        assert!(clean.remaps[2].tap2.is_none());
        // 提示信息覆盖坏键名与归一化。
        assert!(ignored.iter().any(|m| m.contains("NotAKey")));
        assert!(ignored.iter().any(|m| m.contains("Bad")));
        assert!(ignored.iter().any(|m| m.contains("保留粘滞")));
    }
}
#[cfg(test)]
mod platform_caps_tests {
    use crate::*;

    /// Linux 口径的 caps：前台条件不可用（Wayland / X11 都未接前台查询）。
    /// 不门控：`SendTo`（后台发送）在基础版也存在，它的平台冲突测试要用。
    /// 触摸板手势已接（被动旁听 evdev），不再标 false。
    fn linux_caps() -> PlatformCaps {
        PlatformCaps {
            frontmost_conditions: false,
            window_actions: false,
            mouse_absolute: false,
            background_input: false,
            shell_actions: false,
            hot_corners: false,
            wheel_edges: false,
            ..Default::default()
        }
    }

    /// macOS 口径的 caps：F21~F24 无对应键码，监听与注入都不行（媒体键已随 7.1-㊹ 支持）。
    fn macos_caps() -> PlatformCaps {
        let mut keys = BTreeSet::new();
        for k in [Key::F21, Key::F22, Key::F23, Key::F24] {
            keys.insert(k);
        }
        PlatformCaps {
            listen_unsupported: keys.clone(),
            inject_unsupported: keys,
            frontmost_conditions: true,
            device_conditions: false,
            window_actions: false,
            mouse_absolute: true,
            background_input: false,
            shell_actions: false,
            hot_corners: false,
            wheel_edges: false,
            brightness: false,
            ..Default::default()
        }
    }

    fn shortcut(name: &str, trigger: &str, actions: Vec<Action>) -> ShortcutItem {
        ShortcutItem {
            name: Some(name.into()),
            triggers: vec![trigger.into()],
            actions,
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn full_caps_zero_false_positives() {
        // 全支持（Windows）：平台扫描不产出任何条目。
        let cfg = Config {
            shortcuts: vec![shortcut("s", "Ctrl+VolumeUp", vec![])],
            ..Default::default()
        };
        assert!(detect_conflicts(&cfg, &PlatformCaps::default()).is_empty());
    }

    /// `platform` 分组标记：平台能力缺失为 true、规则类冲突（重复触发键）为 false。
    #[cfg(feature = "automation")]
    #[test]
    fn platform_flag_separates_caps_gap_from_rule_conflicts() {
        let cfg = Config {
            shortcuts: vec![
                shortcut(
                    "a",
                    "Ctrl+K",
                    vec![Action::If {
                        condition: Condition::FrontmostApp { app: "x".into() },
                        then: vec![],
                        otherwise: vec![],
                        description: None,
                    }],
                ),
                shortcut("b", "Ctrl+K", vec![]),
            ],
            ..Default::default()
        };
        let hits = detect_conflicts(&cfg, &linux_caps());
        assert_eq!(hits.len(), 2, "{hits:?}");
        let caps: Vec<&Conflict> = hits.iter().filter(|c| c.platform).collect();
        let rule: Vec<&Conflict> = hits.iter().filter(|c| !c.platform).collect();
        assert_eq!(caps.len(), 1, "平台能力缺失应恰好 1 条：{hits:?}");
        assert!(caps[0].message.contains("前台"));
        assert_eq!(rule.len(), 1, "规则类冲突应恰好 1 条：{hits:?}");
        assert_eq!(rule[0].severity, Severity::Error, "{rule:?}");
    }

    #[cfg(feature = "automation")]
    #[test]
    fn frontmost_condition_flagged_on_linux() {
        let cfg = Config {
            shortcuts: vec![shortcut(
                "s",
                "Ctrl+K",
                vec![Action::If {
                    condition: Condition::FrontmostApp {
                        app: "firefox".into(),
                    },
                    then: vec![Action::Text {
                        text: "x".into(),
                        mode: TextMode::default(),
                        masked: false,
                        description: None,
                    }],
                    otherwise: vec![],
                    description: None,
                }],
            )],
            ..Default::default()
        };
        let hits = detect_conflicts(&cfg, &linux_caps());
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].message.contains("前台"));
        assert!(hits[0].message.contains("恒不成立"));
        // Windows 口径下同配置零提示。
        assert!(detect_conflicts(&cfg, &PlatformCaps::default()).is_empty());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn device_condition_flagged_when_unsupported() {
        let cfg = Config {
            shortcuts: vec![shortcut(
                "s",
                "Ctrl+K",
                vec![Action::If {
                    condition: Condition::DeviceIs {
                        id: "keyboard".into(),
                    },
                    then: vec![Action::Text {
                        text: "x".into(),
                        mode: TextMode::default(),
                        masked: false,
                        description: None,
                    }],
                    otherwise: vec![],
                    description: None,
                }],
            )],
            ..Default::default()
        };
        // macOS 不支持设备条件 → 报一条「设备」警告。
        let hits = detect_conflicts(&cfg, &macos_caps());
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].message.contains("设备"));
        assert!(hits[0].message.contains("恒不成立"));
        // 支持设备条件的平台（Linux / Windows，本例用默认）零提示。
        assert!(detect_conflicts(&cfg, &PlatformCaps::default()).is_empty());
    }

    /// 「当前窗口」动作在缺这套能力的平台上标软冲突（快捷键与手势两条路径都要覆盖）；
    /// 支持的平台上零提示。
    #[cfg(feature = "automation")]
    #[test]
    fn window_action_flagged_where_unsupported() {
        let action = Action::Window {
            operation: WindowOperation::Close,
            description: None,
        };
        let cfg = Config {
            shortcuts: vec![shortcut("w", "Ctrl+K", vec![action.clone()])],
            gestures: vec![crate::gesture::Gesture {
                pattern: "DR".into(),
                actions: vec![action],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        // Windows 口径：零提示。
        assert!(detect_conflicts(&cfg, &PlatformCaps::default()).is_empty());
        for caps in [macos_caps(), linux_caps()] {
            let hits = detect_conflicts(&cfg, &caps);
            assert_eq!(hits.len(), 2, "快捷键与手势各一条：{hits:?}");
            assert!(hits.iter().all(|c| c.platform));
            assert!(hits.iter().all(|c| c.severity == Severity::Warn));
            assert!(hits.iter().all(|c| c.message.contains("当前窗口")));
        }
    }

    /// 鼠标绝对移动在 Linux 标软冲突（快捷键与手势都扫）；Windows / macOS 零提示。
    #[cfg(feature = "automation")]
    #[test]
    fn absolute_mouse_flagged_on_linux_only() {
        let action = Action::Mouse {
            op: MouseOp::MoveToOrigin,
            description: None,
        };
        let cfg = Config {
            shortcuts: vec![shortcut("m", "Ctrl+K", vec![action.clone()])],
            gestures: vec![crate::gesture::Gesture {
                pattern: "DR".into(),
                actions: vec![action],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(detect_conflicts(&cfg, &PlatformCaps::default()).is_empty());
        assert!(detect_conflicts(&cfg, &macos_caps()).is_empty());
        let hits = detect_conflicts(&cfg, &linux_caps());
        assert_eq!(hits.len(), 2, "快捷键与手势各一条：{hits:?}");
        assert!(hits
            .iter()
            .all(|c| c.platform && c.severity == Severity::Warn));
        assert!(hits.iter().all(|c| c.message.contains("鼠标绝对移动")));
    }

    #[test]
    fn background_input_flagged_where_unsupported() {
        let action = Action::SendTo {
            target: "notepad.exe".into(),
            payload: SendPayload::Keys {
                keys: vec!["Ctrl".into(), "S".into()],
            },
            description: None,
        };
        let cfg = Config {
            shortcuts: vec![shortcut("s", "Ctrl+K", vec![action.clone()])],
            gestures: vec![crate::gesture::Gesture {
                pattern: "DR".into(),
                actions: vec![action],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(detect_conflicts(&cfg, &PlatformCaps::default()).is_empty());
        for caps in [macos_caps(), linux_caps()] {
            let hits = detect_conflicts(&cfg, &caps);
            assert_eq!(hits.len(), 2, "快捷键与手势各一条：{hits:?}");
            assert!(hits
                .iter()
                .all(|c| c.platform && c.severity == Severity::Warn));
            assert!(hits.iter().all(|c| c.message.contains("后台发送")));
        }
    }

    #[cfg(feature = "automation")]
    #[test]
    fn shell_actions_flagged_where_unsupported() {
        let action = Action::System {
            operation: SystemOperation::EmptyRecycleBin,
            description: None,
        };
        let cfg = Config {
            shortcuts: vec![shortcut("s", "Ctrl+K", vec![action.clone()])],
            gestures: vec![crate::gesture::Gesture {
                pattern: "DR".into(),
                actions: vec![action],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(detect_conflicts(&cfg, &PlatformCaps::default()).is_empty());
        for caps in [macos_caps(), linux_caps()] {
            let hits = detect_conflicts(&cfg, &caps);
            assert_eq!(hits.len(), 2, "快捷键与手势各一条：{hits:?}");
            assert!(hits
                .iter()
                .all(|c| c.platform && c.severity == Severity::Warn));
            assert!(hits.iter().all(|c| c.message.contains("清空回收站")));
        }
    }

    #[cfg(feature = "automation")]
    #[test]
    fn shell_actions_validate_paths() {
        assert!(Action::Os {
            operation: OsOperation::Reveal {
                path: "C:\\tmp\\a.txt".into()
            },
            description: None,
        }
        .validate()
        .is_ok());
        assert!(Action::Os {
            operation: OsOperation::Reveal { path: "  ".into() },
            description: None,
        }
        .validate()
        .is_err());
        for operation in [
            SystemOperation::EmptyRecycleBin,
            SystemOperation::MonitorOff,
            SystemOperation::ToggleDesktopIcons,
        ] {
            assert!(Action::System {
                operation,
                description: None
            }
            .validate()
            .is_ok());
        }
    }

    #[cfg(feature = "automation")]
    #[test]
    fn nested_if_frontmost_also_flagged() {
        let cfg = Config {
            shortcuts: vec![shortcut(
                "s",
                "F9",
                vec![Action::If {
                    condition: Condition::Exists {
                        path: "/tmp".into(),
                    },
                    then: vec![Action::If {
                        condition: Condition::WindowTitleContains {
                            text: "编辑".into(),
                        },
                        then: vec![],
                        otherwise: vec![],
                        description: None,
                    }],
                    otherwise: vec![],
                    description: None,
                }],
            )],
            ..Default::default()
        };
        let hits = detect_conflicts(&cfg, &linux_caps());
        assert!(hits.iter().any(|c| c.message.contains("前台")), "{hits:?}");
    }

    #[cfg(feature = "automation")]
    #[test]
    fn parallel_group_is_scanned() {
        // 并行组内的动作同样参与平台扫描（㉞）：注入不了的键进「注入不了」提示，
        // 嵌套的前台条件进「恒不成立」提示。
        let cfg = Config {
            shortcuts: vec![
                shortcut(
                    "注入",
                    "F9",
                    vec![Action::Parallel {
                        actions: vec![Action::Keys {
                            keys: vec!["F21".into()],
                            mode: KeysMode::Tap,
                            description: None,
                        }],
                        description: None,
                    }],
                ),
                shortcut(
                    "前台",
                    "F10",
                    vec![Action::Parallel {
                        actions: vec![Action::If {
                            condition: Condition::WindowTitleContains {
                                text: "编辑".into(),
                            },
                            then: vec![],
                            otherwise: vec![],
                            description: None,
                        }],
                        description: None,
                    }],
                ),
            ],
            ..Default::default()
        };
        let mac = detect_conflicts(&cfg, &macos_caps());
        assert!(
            mac.iter()
                .any(|c| c.message.contains("注入不了") && c.name == "注入"),
            "并行组内的注入不了键要报出来：{mac:?}"
        );
        let lin = detect_conflicts(&cfg, &linux_caps());
        assert!(
            lin.iter()
                .any(|c| c.message.contains("前台") && c.name == "前台"),
            "并行组内嵌套的前台条件要报出来：{lin:?}"
        );
    }

    #[test]
    fn unsupported_trigger_key_flagged() {
        let cfg = Config {
            shortcuts: vec![shortcut("s", "Ctrl+F21", vec![])],
            ..Default::default()
        };
        let hits = detect_conflicts(&cfg, &macos_caps());
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].message.contains("F21"));
        assert!(hits[0].message.contains("监听不到"));
        // 序列 leader 与和弦成员同样要报。
        let cfg2 = Config {
            shortcuts: vec![
                shortcut("seq", "F9 F21 K", vec![]),
                shortcut("chord", "F22&J", vec![]),
            ],
            ..Default::default()
        };
        let hits2 = detect_conflicts(&cfg2, &macos_caps());
        assert!(
            hits2.iter().any(|c| c.message.contains("「F9 F21 K」")),
            "{hits2:?}"
        );
        assert!(
            hits2.iter().any(|c| c.message.contains("「F22&J」")),
            "{hits2:?}"
        );
    }

    #[test]
    fn disabled_entries_not_flagged() {
        let mut s = shortcut("s", "Ctrl+VolumeUp", vec![]);
        s.enabled = false;
        let cfg = Config {
            shortcuts: vec![s],
            ..Default::default()
        };
        assert!(detect_conflicts(&cfg, &macos_caps()).is_empty());
    }

    #[test]
    fn remap_sides_flagged() {
        let cfg = Config {
            remaps: vec![
                Remap {
                    from: "F21".into(),
                    to: "A".into(),
                    enabled: true,
                    ..Default::default()
                },
                Remap {
                    from: "B".into(),
                    to: "F24".into(),
                    enabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let hits = detect_conflicts(&cfg, &macos_caps());
        assert!(
            hits.iter()
                .any(|c| c.message.contains("改键来源") && c.message.contains("F21")),
            "{hits:?}"
        );
        assert!(
            hits.iter()
                .any(|c| c.message.contains("目标键") && c.message.contains("F24")),
            "{hits:?}"
        );
    }

    #[test]
    fn keys_action_inject_flagged() {
        let cfg = Config {
            shortcuts: vec![shortcut(
                "s",
                "Ctrl+K",
                vec![Action::Keys {
                    keys: vec!["F21".into(), "Enter".into()],
                    mode: KeysMode::Tap,
                    description: None,
                }],
            )],
            ..Default::default()
        };
        let hits = detect_conflicts(&cfg, &macos_caps());
        assert!(!hits.is_empty(), "{hits:?}");
        assert!(hits
            .iter()
            .any(|c| c.message.contains("F21") && c.message.contains("注入不了")));
    }

    #[test]
    fn listen_and_inject_reported_separately() {
        // 同一条快捷键：触发键监听不到 + 动作注入不了，两条提示分开报。
        let cfg = Config {
            shortcuts: vec![shortcut(
                "s",
                "F22",
                vec![Action::Keys {
                    keys: vec!["F23".into()],
                    mode: KeysMode::Tap,
                    description: None,
                }],
            )],
            ..Default::default()
        };
        let hits = detect_conflicts(&cfg, &macos_caps());
        assert!(
            hits.iter().any(|c| c.message.contains("监听不到")),
            "{hits:?}"
        );
        assert!(
            hits.iter().any(|c| c.message.contains("注入不了")),
            "{hits:?}"
        );
    }

    // ---- 触发侧 when 门控（7.2-㊶） ----

    #[cfg(feature = "automation")]
    #[test]
    fn when_matches_gates_by_frontmost_and_device() {
        use Condition::*;
        let chrome = FrontmostContext {
            process_name: "chrome.exe".into(),
            window_title: "Google".into(),
        };
        let mut s = ShortcutItem {
            when: Some(FrontmostApp {
                app: "chrome".into(),
            }),
            ..Default::default()
        };
        assert!(s.when_matches(&TriggerContext::fixed(Some(&chrome), None)));
        assert!(
            !s.when_matches(&TriggerContext::empty()),
            "无前台上下文时恒不成立"
        );
        // 无条件条目恒真。
        s.when = None;
        assert!(s.when_matches(&TriggerContext::empty()));

        let r = Remap {
            from: "A".into(),
            to: "B".into(),
            when: Some(DeviceIs {
                id: "keyboard".into(),
            }),
            ..Default::default()
        };
        assert!(r.when_matches(&TriggerContext::fixed(None, Some("AT keyboard"))));
        assert!(!r.when_matches(&TriggerContext::fixed(None, None)));
    }

    #[test]
    fn trigger_context_checks_global_exclude_list() {
        let chrome = FrontmostContext {
            process_name: "Photoshop.exe".into(),
            window_title: "PS".into(),
        };
        let ctx = TriggerContext::fixed(Some(&chrome), None);
        assert!(ctx.app_excluded(&["photoshop.exe".into()]));
        assert!(ctx.app_excluded(&["*shop*".into()]));
        assert!(!ctx.app_excluded(&["notepad.exe".into()]));
        assert!(!ctx.app_excluded(&[]), "空列表不排除");
        assert!(!TriggerContext::empty().app_excluded(&["photoshop.exe".into()]));
    }

    #[cfg(feature = "automation")]
    #[test]
    fn same_trigger_different_when_is_not_a_conflict() {
        use Condition::*;
        let hard = |cfg: &Config| {
            detect_conflicts(cfg, &PlatformCaps::default())
                .into_iter()
                .filter(|c| c.severity == Severity::Error)
                .count()
        };
        let mut a = shortcut("chrome 用", "Ctrl+K", vec![]);
        a.when = Some(FrontmostApp {
            app: "chrome".into(),
        });
        let mut b = shortcut("vscode 用", "Ctrl+K", vec![]);
        b.when = Some(FrontmostApp { app: "code".into() });
        let cfg = Config {
            shortcuts: vec![a.clone(), b],
            ..Default::default()
        };
        assert_eq!(hard(&cfg), 0, "同触发键 + 不同 when 是按条件分流，不算重复");

        // 同触发键 + 相同 when（或都无 when）仍是硬冲突。
        let dup = Config {
            shortcuts: vec![a.clone(), a],
            ..Default::default()
        };
        assert_eq!(hard(&dup), 1);
    }

    #[cfg(feature = "automation")]
    #[test]
    fn same_remap_from_different_when_is_not_a_conflict() {
        use Condition::*;
        let hard = |cfg: &Config| {
            detect_conflicts(cfg, &PlatformCaps::default())
                .into_iter()
                .filter(|c| c.severity == Severity::Error)
                .count()
        };
        let a = Remap {
            from: "CapsLock".into(),
            to: "Escape".into(),
            when: Some(FrontmostApp { app: "code".into() }),
            enabled: true,
            ..Default::default()
        };
        let b = Remap {
            from: "CapsLock".into(),
            to: "Ctrl".into(),
            enabled: true,
            ..Default::default()
        };
        assert_eq!(
            hard(&Config {
                remaps: vec![a, b.clone()],
                ..Default::default()
            }),
            0
        );
        let dup = Config {
            remaps: vec![b.clone(), b],
            ..Default::default()
        };
        assert_eq!(hard(&dup), 1);
    }

    #[cfg(feature = "automation")]
    #[test]
    fn sanitize_drops_invalid_when() {
        use Condition::*;
        let mut s = shortcut("s", "Ctrl+K", vec![]);
        s.when = Some(FrontmostApp { app: "   ".into() }); // 空进程名 → 非法
        let (out, ignored) = sanitize_config(&Config {
            shortcuts: vec![s],
            ..Default::default()
        });
        assert!(out.shortcuts[0].when.is_none(), "非法 when 被清掉");
        assert!(
            ignored.iter().any(|m| m.contains("触发条件")),
            "{ignored:?}"
        );

        let mut r = Remap {
            from: "A".into(),
            to: "B".into(),
            enabled: true,
            ..Default::default()
        };
        r.when = Some(DeviceIs { id: "".into() });
        let (out, ignored) = sanitize_config(&Config {
            remaps: vec![r],
            ..Default::default()
        });
        assert!(out.remaps[0].when.is_none());
        assert!(
            ignored.iter().any(|m| m.contains("触发条件")),
            "{ignored:?}"
        );
    }

    #[cfg(feature = "automation")]
    #[test]
    fn when_frontmost_condition_flagged_on_linux() {
        use Condition::*;
        let mut s = shortcut("s", "Ctrl+K", vec![]);
        s.when = Some(FrontmostApp {
            app: "chrome".into(),
        });
        let cfg = Config {
            shortcuts: vec![s],
            ..Default::default()
        };
        let hits = detect_conflicts(&cfg, &linux_caps());
        assert!(
            hits.iter()
                .any(|c| c.message.contains("前台") && c.message.contains("恒不成立")),
            "{hits:?}"
        );
        assert!(detect_conflicts(&cfg, &PlatformCaps::default()).is_empty());
    }
}

#[cfg(test)]
mod input_env_tests {
    use crate::*;

    fn mouse_cfg() -> Config {
        Config {
            shortcuts: vec![ShortcutItem {
                name: Some("后退".into()),
                triggers: vec!["MouseForward".into()],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// 没探测到接管者 → 一条都不报（「不影响未装接管软件的机器」）。
    #[test]
    fn empty_env_reports_nothing() {
        let cfg = mouse_cfg();
        assert!(diagnose_input_env(&cfg, &InputEnv::default()).is_empty());
    }

    /// 探到了接管者、但配置根本没用鼠标键 → 不报：那东西不影响任何一条快捷键，报了是噪音。
    #[test]
    fn diverter_ignored_when_config_has_no_mouse_key() {
        let cfg = Config {
            shortcuts: vec![ShortcutItem {
                name: Some("终端".into()),
                triggers: vec!["Ctrl+Alt+T".into()],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let env = InputEnv {
            diverters: vec!["logi-options-plus".into()],
            hvci: None,
        };
        assert!(diagnose_input_env(&cfg, &env).is_empty());
        // 改键来源是鼠标键同样算「用到鼠标键」（`from` 侧要监听）。
        let mut r = Remap {
            from: "MouseBack".into(),
            to: "Escape".into(),
            enabled: true,
            ..Default::default()
        };
        r.enabled = true;
        let cfg = Config {
            remaps: vec![r],
            ..Default::default()
        };
        assert_eq!(diagnose_input_env(&cfg, &env).len(), 1);
    }

    /// 探到接管者 + 配置用到鼠标键 → 一条，带展示名与处置办法，且归在「平台能力缺失」口径
    /// （`platform: true` + `Warn`），不阻止保存。
    #[test]
    fn diverter_reported_with_advice() {
        let env = InputEnv {
            diverters: vec!["logi-options-plus".into()],
            hvci: None,
        };
        let hits = diagnose_input_env(&mouse_cfg(), &env);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].severity, Severity::Warn);
        assert!(hits[0].platform);
        assert!(
            hits[0].message.contains("Logi Options+"),
            "{}",
            hits[0].message
        );
        // 处置办法要真的在里面（能照做），不是只有一句「可能有问题」。
        assert!(hits[0].message.contains("无操作"), "{}", hits[0].message);
    }

    /// 认不出的 id 丢弃、重复 id 只报一条（探测结果来自平台层，这里别被它带偏）。
    #[test]
    fn unknown_and_duplicate_ids_are_normalized() {
        let env = InputEnv {
            diverters: vec![
                "不存在的接管者".into(),
                "logi-ghub".into(),
                "logi-ghub".into(),
            ],
            hvci: None,
        };
        let hits = diagnose_input_env(&mouse_cfg(), &env);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].message.contains("G HUB"), "{}", hits[0].message);
    }

    /// 用户点过「忽略」的那一条不再进冲突列表；别的接管者照报（粒度是单条，不是总开关）。
    #[test]
    fn ignored_diverter_is_not_reported() {
        let env = InputEnv {
            diverters: vec!["logi-options-plus".into(), "logi-ghub".into()],
            hvci: None,
        };
        let mut cfg = mouse_cfg();
        cfg.settings.input_env_ignored = vec!["logi-options-plus".into()];
        let hits = diagnose_input_env(&cfg, &env);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].message.contains("G HUB"), "{}", hits[0].message);
        // 全忽略 = 一条都不报，但探测结果本身没被改写（设置页卡片还要显示它们）。
        cfg.settings.input_env_ignored = vec!["logi-options-plus".into(), "logi-ghub".into()];
        assert!(diagnose_input_env(&cfg, &env).is_empty());
        // 认不出的 id 留在忽略表里无副作用。
        cfg.settings.input_env_ignored = vec!["以前装过的软件".into()];
        assert_eq!(diagnose_input_env(&cfg, &env).len(), 2);
    }

    /// HVCI 只作环境信息：即便开着也不产出冲突（它拦内核驱动，不影响用户态钩子）。
    #[test]
    fn hvci_is_info_not_conflict() {
        let env = InputEnv {
            diverters: vec![],
            hvci: Some(true),
        };
        assert!(diagnose_input_env(&mouse_cfg(), &env).is_empty());
    }

    /// 目录本身的体检：id 唯一、匹配片段非空且都是小写（探测侧按小写进程名比）。
    #[test]
    fn catalog_is_well_formed() {
        let mut ids: Vec<&str> = DIVERTERS.iter().map(|d| d.id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "接管者目录里有重复 id");
        for d in DIVERTERS {
            assert!(!d.processes.is_empty(), "{} 没有匹配片段", d.id);
            assert!(!d.advice.is_empty(), "{} 没有处置办法", d.id);
            for p in d.processes {
                assert_eq!(*p, p.to_lowercase(), "{} 的片段 {p} 必须是小写", d.id);
            }
        }
    }
}

#[cfg(test)]
mod hot_corner_tests {
    use crate::*;

    fn text(value: &str) -> Action {
        Action::Text {
            text: value.into(),
            mode: TextMode::Input,
            masked: false,
            description: None,
        }
    }

    /// 老配置缺字段：根数组为空，照常读。
    #[test]
    fn old_config_without_hot_corners_gets_defaults() {
        let old: Config = serde_json::from_str(r#"{"settings":{"autostart":true}}"#).unwrap();
        assert!(old.hot_corners.is_empty());
        assert!(old.wheel_edges.is_empty());
    }

    /// 旧版写在 settings 里的四角 / 四边配置：migrate 转成根数组，旧键清空，幂等。
    #[test]
    fn legacy_settings_regions_migrate_to_root_rules() {
        let mut cfg = Config {
            settings: Settings {
                hot_corners: HotCorners {
                    enabled: true,
                    top_left: HotCorner {
                        enabled: true,
                        name: "调度台".into(),
                        actions: vec![text("a")],
                    },
                    bottom_right: HotCorner {
                        enabled: true,
                        name: "".into(),
                        actions: vec![text("b")],
                    },
                    ..Default::default()
                },
                wheel_edges: WheelEdges {
                    enabled: true,
                    left: WheelEdge {
                        enabled: true,
                        name: "音量".into(),
                        actions: vec![text("c")],
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        cfg.migrate();
        assert_eq!(cfg.hot_corners.len(), 2);
        assert_eq!(cfg.hot_corners[0].spot, HotCornerSpot::TopLeft);
        assert_eq!(cfg.hot_corners[0].label(), "调度台");
        assert!(cfg.hot_corners[1].enabled);
        assert_eq!(cfg.wheel_edges.len(), 1);
        assert_eq!(cfg.wheel_edges[0].edge, WheelEdgeSpot::Left);
        assert!(!cfg.settings.hot_corners.enabled);
        assert!(!cfg.settings.wheel_edges.enabled);

        // 幂等：再迁移一次不会重复生成。
        cfg.migrate();
        assert_eq!(cfg.hot_corners.len(), 2);
        assert_eq!(cfg.wheel_edges.len(), 1);
    }

    /// 同一位置允许多条规则，JSON 往返保持顺序与内容。
    #[test]
    fn hot_corner_rules_roundtrip_including_multiple_per_spot() {
        let cfg = Config {
            hot_corners: vec![
                HotCornerRule {
                    spot: HotCornerSpot::TopLeft,
                    name: Some("调度台".into()),
                    description: Some("打开调度台".into()),
                    layer: None,
                    dwell_ms: 0,
                    #[cfg(feature = "automation")]
                    when: None,
                    actions: vec![text("a")],
                    enabled: true,
                    folder: None,
                    order: 0,
                },
                HotCornerRule {
                    spot: HotCornerSpot::TopLeft,
                    name: None,
                    description: None,
                    layer: None,
                    dwell_ms: 0,
                    #[cfg(feature = "automation")]
                    when: None,
                    actions: vec![text("b")],
                    enabled: true,
                    folder: None,
                    order: 0,
                },
            ],
            wheel_edges: vec![WheelEdgeRule {
                edge: WheelEdgeSpot::Left,
                name: Some("音量".into()),
                description: None,
                layer: None,
                #[cfg(feature = "automation")]
                when: None,
                actions: vec![text("c")],
                enabled: true,
                folder: None,
                order: 0,
            }],
            ..Default::default()
        };
        let back: Config = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back.hot_corners, cfg.hot_corners);
        assert_eq!(back.hot_corners[1].label(), "左上角");
        assert_eq!(back.wheel_edges, cfg.wheel_edges);
        assert_eq!(back.wheel_edges[0].label(), "音量");
    }

    /// 启用了至少一条 → 要装鼠标钩子；`config_uses_mouse` 不受影响（不误报厂商接管）。
    #[test]
    fn hot_corners_need_mouse_hook_but_not_uses_mouse() {
        let mut cfg = Config::default();
        cfg.settings.experimental = true;
        cfg.hot_corners.push(HotCornerRule {
            spot: HotCornerSpot::TopRight,
            name: None,
            description: None,
            layer: None,
            dwell_ms: 0,
            #[cfg(feature = "automation")]
            when: None,
            actions: vec![text("a")],
            enabled: true,
            folder: None,
            order: 0,
        });
        assert!(config_needs_mouse_hook(&cfg));
        assert!(!config_uses_mouse(&cfg));
        // 总闸关掉 → 与没有这个功能一致。
        cfg.settings.experimental = false;
        assert!(!config_needs_mouse_hook(&cfg));
    }

    /// 非 Windows 平台：配了触发角给一条平台 Warn；没配就零噪音。
    #[test]
    fn hot_corners_flagged_where_unsupported() {
        let mut cfg = Config::default();
        assert!(detect_conflicts(
            &cfg,
            &PlatformCaps {
                hot_corners: false,
                ..Default::default()
            }
        )
        .is_empty());
        cfg.hot_corners.push(HotCornerRule {
            spot: HotCornerSpot::BottomLeft,
            name: None,
            description: None,
            layer: None,
            dwell_ms: 0,
            #[cfg(feature = "automation")]
            when: None,
            actions: vec![text("a")],
            enabled: true,
            folder: None,
            order: 0,
        });
        let hits = detect_conflicts(
            &cfg,
            &PlatformCaps {
                hot_corners: false,
                ..Default::default()
            },
        );
        assert_eq!(hits.len(), 1);
        assert!(hits[0].platform);
        assert!(hits[0].message.contains("触发角"));
        assert!(detect_conflicts(&cfg, &PlatformCaps::default()).is_empty());
    }

    /// 清洗只摘掉坏动作，位置与名字保留（名字去空白，空名变 None）。
    #[test]
    fn sanitize_drops_bad_hot_corner_actions() {
        let mut cfg = Config::default();
        cfg.hot_corners.push(HotCornerRule {
            spot: HotCornerSpot::TopLeft,
            name: Some("  左上  ".into()),
            description: None,
            layer: None,
            dwell_ms: 0,
            #[cfg(feature = "automation")]
            when: None,
            actions: vec![
                text("ok"),
                Action::Mouse {
                    op: MouseOp::Move { dx: 0, dy: 0 },
                    description: None,
                },
            ],
            enabled: true,
            folder: None,
            order: 0,
        });
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(clean.hot_corners[0].name.as_deref(), Some("左上"));
        assert_eq!(clean.hot_corners[0].actions.len(), 1);
        assert!(ignored.iter().any(|m| m.contains("触发角")));
    }

    /// 防误触停留：超上限的值在清洗时钳回 3000；0 = 缺省，不写进 JSON（老配置 / 卡带没这个键）。
    #[test]
    fn hot_corner_dwell_is_clamped_and_omitted_when_zero() {
        let mut cfg = Config::default();
        cfg.hot_corners.push(HotCornerRule {
            spot: HotCornerSpot::BottomRight,
            name: None,
            description: None,
            layer: None,
            dwell_ms: 10_000,
            #[cfg(feature = "automation")]
            when: None,
            actions: vec![text("a")],
            enabled: true,
            folder: None,
            order: 0,
        });
        let (mut clean, _) = sanitize_config(&cfg);
        assert_eq!(clean.hot_corners[0].dwell_ms, HOT_CORNER_DWELL_MAX_MS);
        let json = serde_json::to_value(&clean.hot_corners[0]).unwrap();
        assert_eq!(json["dwell_ms"], HOT_CORNER_DWELL_MAX_MS);

        clean.hot_corners[0].dwell_ms = 0;
        let json = serde_json::to_value(&clean.hot_corners[0]).unwrap();
        assert!(json.get("dwell_ms").is_none(), "0 = 缺省，不写进 JSON");
        let back: HotCornerRule = serde_json::from_value(json).unwrap();
        assert_eq!(back.dwell_ms, 0, "缺这个键的老配置照常读成 0");
    }

    /// 边缘滚动启用 → 要装鼠标钩子，但 `config_uses_mouse` 不受影响（不误报厂商接管）。
    #[test]
    fn wheel_edges_need_mouse_hook_but_not_uses_mouse() {
        let mut cfg = Config::default();
        cfg.settings.experimental = true;
        cfg.wheel_edges.push(WheelEdgeRule {
            edge: WheelEdgeSpot::Right,
            name: None,
            description: None,
            layer: None,
            #[cfg(feature = "automation")]
            when: None,
            actions: vec![text("a")],
            enabled: true,
            folder: None,
            order: 0,
        });
        assert!(config_needs_mouse_hook(&cfg));
        assert!(!config_uses_mouse(&cfg));
        // 总闸关掉 → 不装。
        cfg.settings.experimental = false;
        assert!(!config_needs_mouse_hook(&cfg));
    }

    /// 触摸板手势是**独立开关**：`experimental` 关着也照装鼠标钩子；自己的开关关掉就不装。
    #[test]
    fn touchpad_gesture_uses_its_own_switch() {
        let mut cfg = Config::default();
        cfg.touchpad_gestures.push(TouchpadGesture {
            trigger: Some(Key::CapsLock),
            pattern: "D".into(),
            enabled: true,
            ..Default::default()
        });
        cfg.settings.touchpad = true;
        assert!(touchpad_gesture_enabled(&cfg));
        assert!(config_needs_mouse_hook(&cfg));
        // 鼠标操作总闸关着不影响触摸板。
        cfg.settings.experimental = false;
        assert!(config_needs_mouse_hook(&cfg));
        // 触摸板开关关掉 → 不装（鼠标那枚开着也一样，这里没有鼠标类条目）。
        cfg.settings.touchpad = false;
        assert!(!config_needs_mouse_hook(&cfg));
    }

    /// 触摸板自己的两个参数：开关 / 起手距离原样往返，排除名单按 `clean_list` 清洗。
    #[test]
    fn touchpad_settings_roundtrip_and_clean_exclude() {
        let mut cfg = Config::default();
        cfg.settings.touchpad = true;
        cfg.settings.touchpad_start_distance = 30;
        cfg.settings.touchpad_exclude = vec![" a.exe ".into(), "  ".into()];
        let (out, _) = sanitize_config(&cfg);
        assert!(out.settings.touchpad);
        assert_eq!(out.settings.touchpad_start_distance, 30);
        assert_eq!(out.settings.touchpad_exclude, vec!["a.exe".to_string()]);
        let back: Config = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert!(back.settings.touchpad);
        assert_eq!(back.settings.touchpad_start_distance, 30);
    }

    /// 纯形状条目：方向串留空 + 有模板 = 保留；两者都空 = 整条丢掉（永远匹配不上）。
    #[test]
    fn shape_only_touchpad_gesture_kept_by_sanitize() {
        let mut cfg = Config::default();
        cfg.touchpad_gestures.push(TouchpadGesture {
            trigger: Some(Key::CapsLock),
            pattern: String::new(),
            template: Some(pad_template(0)),
            actions: vec![text("shape")],
            enabled: true,
            ..Default::default()
        });
        cfg.touchpad_gestures.push(TouchpadGesture {
            trigger: Some(Key::CapsLock),
            pattern: "  ".into(),
            template: None,
            actions: vec![text("nothing")],
            enabled: true,
            ..Default::default()
        });
        let (out, ignored) = sanitize_config(&cfg);
        assert_eq!(out.touchpad_gestures.len(), 1, "ignored: {ignored:?}");
        assert!(out.touchpad_gestures[0].pattern.is_empty());
        assert!(out.touchpad_gestures[0].template.is_some());
        assert!(ignored
            .iter()
            .any(|m| m.contains("方向串为空且没有形状模板")));
        // 纯形状条目也要能开采集 / 认触发键，否则它配了也永远不会被采集。
        assert!(touchpad_gesture_enabled(&out));
        assert!(is_touchpad_trigger(&out, Key::CapsLock));
    }

    /// 纯形状条目的判重看模板：模板不同 = 两条都留；模板相同 = 后一条被丢。
    #[test]
    fn shape_only_touchpad_gesture_dedup_by_template() {
        let mut cfg = Config::default();
        for shift in [0, 1] {
            cfg.touchpad_gestures.push(TouchpadGesture {
                trigger: Some(Key::CapsLock),
                pattern: String::new(),
                template: Some(pad_template(shift)),
                actions: vec![text("x")],
                enabled: true,
                ..Default::default()
            });
        }
        let (out, ignored) = sanitize_config(&cfg);
        assert_eq!(out.touchpad_gestures.len(), 2, "ignored: {ignored:?}");
        // 模板完全相同的第三条 → 判重丢掉。
        cfg.touchpad_gestures.push(TouchpadGesture {
            trigger: Some(Key::CapsLock),
            pattern: String::new(),
            template: Some(pad_template(0)),
            actions: vec![text("dup")],
            enabled: true,
            ..Default::default()
        });
        let (out, ignored) = sanitize_config(&cfg);
        assert_eq!(out.touchpad_gestures.len(), 2, "ignored: {ignored:?}");
        assert!(ignored.iter().any(|m| m.contains("形状模板完全相同")));
    }

    /// 没接触摸板采集的平台给一条平台 Warn（不拦保存）。
    #[test]
    fn touchpad_flagged_where_unsupported() {
        let mut cfg = Config::default();
        cfg.touchpad_gestures.push(TouchpadGesture {
            trigger: Some(Key::CapsLock),
            pattern: "R".into(),
            template: None,
            actions: vec![text("a")],
            enabled: true,
            ..Default::default()
        });
        let hits = detect_conflicts(
            &cfg,
            &PlatformCaps {
                touchpad_gestures: false,
                ..Default::default()
            },
        );
        assert!(hits
            .iter()
            .any(|c| c.platform && c.message.contains("触摸板手势在当前平台用不了")));
        assert!(detect_conflicts(&cfg, &PlatformCaps::default())
            .iter()
            .all(|c| !c.message.contains("触摸板手势在当前平台用不了")));
    }

    /// 合法 24 点模板（坐标都在 `0..=1000`）；`shift` 用来造「不同形状」。
    fn pad_template(shift: i16) -> Vec<[i16; 2]> {
        (0..crate::gesture::TEMPLATE_POINTS as i16)
            .map(|i| [((i * 40) % 1000) + shift, (i * 20) % 1000])
            .collect()
    }

    /// 非 Windows 平台给一条平台 Warn，不拦保存。
    #[test]
    fn wheel_edges_flagged_where_unsupported() {
        let mut cfg = Config::default();
        assert!(detect_conflicts(
            &cfg,
            &PlatformCaps {
                wheel_edges: false,
                ..Default::default()
            }
        )
        .is_empty());
        cfg.wheel_edges.push(WheelEdgeRule {
            edge: WheelEdgeSpot::Bottom,
            name: None,
            description: None,
            layer: None,
            #[cfg(feature = "automation")]
            when: None,
            actions: vec![text("a")],
            enabled: true,
            folder: None,
            order: 0,
        });
        let hits = detect_conflicts(
            &cfg,
            &PlatformCaps {
                wheel_edges: false,
                ..Default::default()
            },
        );
        assert_eq!(hits.len(), 1);
        assert!(hits[0].platform);
        assert!(hits[0].message.contains("边缘滚动"));
        assert!(detect_conflicts(&cfg, &PlatformCaps::default()).is_empty());
    }

    /// 清洗只摘掉坏动作，边名去空白。
    #[test]
    fn sanitize_drops_bad_wheel_edge_actions() {
        let mut cfg = Config::default();
        cfg.wheel_edges.push(WheelEdgeRule {
            edge: WheelEdgeSpot::Left,
            name: Some("  桌面  ".into()),
            description: None,
            layer: None,
            #[cfg(feature = "automation")]
            when: None,
            actions: vec![
                text("ok"),
                Action::Mouse {
                    op: MouseOp::Move { dx: 0, dy: 0 },
                    description: None,
                },
            ],
            enabled: true,
            folder: None,
            order: 0,
        });
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(clean.wheel_edges[0].name.as_deref(), Some("桌面"));
        assert_eq!(clean.wheel_edges[0].actions.len(), 1);
        assert!(ignored.iter().any(|m| m.contains("边缘滚动")));
    }
}
