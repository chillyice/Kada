//! 内置示例配置：设置页「载入示例配置」一键并入（走合并导入，只增不删、不碰本机设置）。
//!
//! 全部条目放进同一个目录、共用同一个层——用户删掉那个目录就整体清走，不用一条条找。
//! 目录 / 层用固定 id：重复载入时合并按判重跳过，不会翻倍。

use crate::{
    Action, Config, Folder, Layer, MouseButton, MouseOp, Remap, ShortcutItem, TextExpansion, TextMode,
};
#[cfg(feature = "automation")]
use crate::{Condition, Shell};

/// 示例目录 id（固定值，保证重复载入可判重）。
const FOLDER: &str = "sample";
/// 示例层 id（被切层键与层内改键引用）。
const LAYER: &str = "sample-layer";

/// 内置示例配置（合法 `Config`，全部 `enabled`，无悬空引用、无自冲突——单测钉死）。
pub fn sample_config() -> Config {
    let folder = Some(FOLDER.to_string());
    let layer = Some(LAYER.to_string());
    let mut cfg = Config {
        folders: vec![Folder {
            id: FOLDER.into(),
            name: "示例（可整目录删除）".into(),
            parent: None,
        }],
        layers: vec![Layer {
            id: LAYER.into(),
            name: "示例层".into(),
            // 层标记（纯展示）：emoji 会当「贴纸」骑在组合键 chip 左下角上，一眼看出这条属于哪层。
            mark: Some("🧭".into()),
            ..Default::default()
        }],
        ..Config::default()
    };

    // ---- 改键：各种形态各来一条 ----
    cfg.remaps.push(Remap {
        name: Some("CapsLock：短按 Esc / 长按 Ctrl".into()),
        description: Some("tap-hold 经典用法：平时当 Esc，按住当 Ctrl 组合键".into()),
        folder: folder.clone(),
        from: "CapsLock".into(),
        to: String::new(),
        tap: Some("Escape".into()),
        hold: Some("Ctrl".into()),
        tap_timeout_ms: 200,
        enabled: true,
        ..Remap::default()
    });
    cfg.remaps.push(Remap {
        name: Some("F1 长按进「示例层」".into()),
        description: Some("切层键（momentary）：按住 F1 期间示例层生效，松开退回基础层".into()),
        folder: folder.clone(),
        from: "F1".into(),
        to: String::new(),
        hold_layer: layer.clone(),
        enabled: true,
        ..Remap::default()
    });
    cfg.remaps.push(Remap {
        name: Some("F2 长按锁定「示例层」".into()),
        description: Some("锁定式切层：长按 F2 切换示例层开 / 关，层保持生效（层内放和弦 / 序列用这种）".into()),
        folder: folder.clone(),
        from: "F2".into(),
        to: String::new(),
        lock_layer: layer.clone(),
        enabled: true,
        ..Remap::default()
    });
    // 示例层内：H/J/K/L → 方向键（只在 F1 按住或 F2 锁定期间生效）。
    for (from, to, label) in [
        ("H", "Left", "左"),
        ("J", "Down", "下"),
        ("K", "Up", "上"),
        ("L", "Right", "右"),
    ] {
        cfg.remaps.push(Remap {
            name: Some(format!("{from} → 方向键（{label}）")),
            description: Some("配在「示例层」里：F1 按住 / F2 锁定后，HJKL 变方向键".into()),
            folder: folder.clone(),
            from: from.into(),
            to: to.into(),
            layer: layer.clone(),
            enabled: true,
            ..Remap::default()
        });
    }
    cfg.remaps.push(Remap {
        name: Some("Insert 连击：双击 Esc / 三击 Delete".into()),
        description: Some("tap-dance 连击：单击不变，双击 Insert 发 Esc，三击发 Delete".into()),
        folder: folder.clone(),
        from: "Insert".into(),
        to: "Insert".into(),
        tap2: Some("Escape".into()),
        tap3: Some("Delete".into()),
        enabled: true,
        ..Remap::default()
    });
    cfg.remaps.push(Remap {
        name: Some("F3 单次 Shift".into()),
        description: Some("oneshot：按一下 F3，紧接着按的那个键自动带上 Shift（松开即用即走）".into()),
        folder: folder.clone(),
        from: "F3".into(),
        to: String::new(),
        oneshot: Some("Shift".into()),
        enabled: true,
        ..Remap::default()
    });
    cfg.remaps.push(Remap {
        name: Some("F4 粘滞 Ctrl".into()),
        description: Some("sticky：按一下 F4 锁定 Ctrl，再按一下解锁（「按住说话」类连续组合键用）".into()),
        folder: folder.clone(),
        from: "F4".into(),
        to: String::new(),
        sticky: Some("Ctrl".into()),
        enabled: true,
        ..Remap::default()
    });

    cfg.remaps.push(Remap {
        name: Some("鼠标侧键前进 → 中键点击".into()),
        description: Some("普通改键 + 鼠标键：把鼠标侧键「前进」改成中键单击（左右键与滚轮不纳入触发，避免误拦日常点击）".into()),
        folder: folder.clone(),
        from: "MouseForward".into(),
        to: "MouseMiddle".into(),
        enabled: true,
        ..Remap::default()
    });

    // ---- 快捷键（基础版动作）----
    let sc = |name: &str, desc: &str, triggers: &[&str], actions: Vec<Action>| ShortcutItem {
        name: Some(name.into()),
        description: Some(desc.into()),
        folder: folder.clone(),
        layer: None,
        triggers: triggers.iter().map(|s| s.to_string()).collect(),
        actions,
        enabled: true,
        ..ShortcutItem::default()
    };
    // 同上的层内版本：条目归属「示例层」，只有该层激活时才参与匹配。
    let sc_in_layer = |name: &str, desc: &str, triggers: &[&str], actions: Vec<Action>| ShortcutItem {
        name: Some(name.into()),
        description: Some(desc.into()),
        folder: folder.clone(),
        layer: layer.clone(),
        triggers: triggers.iter().map(|s| s.to_string()).collect(),
        actions,
        enabled: true,
        ..ShortcutItem::default()
    };
    cfg.shortcuts.push(sc(
        "文本注入演示",
        "触发后在光标处打出一段文本（中文走剪贴板注入，最稳）",
        &["Ctrl+Alt+Space"],
        vec![Action::Text {
            text: "咔哒 Kada：文本注入演示".into(),
            mode: Default::default(),
            masked: false,
            description: None,
        }],
    ));
    cfg.shortcuts.push(sc(
        "鼠标滚轮演示",
        "全键盘操作：这条快捷键向上滚 3 格（还有移动 / 点击）",
        &["Ctrl+Alt+M"],
        vec![Action::Mouse {
            op: MouseOp::Scroll { dx: 0, dy: 3 },
            description: None,
        }],
    ));
    cfg.shortcuts.push(sc(
        "多步动作演示",
        "快捷键可以串一串动作：打字 → 停 0.5 秒 → 再打字（还有按键组合 / 命令 / 鼠标等）",
        &["Ctrl+Alt+P"],
        vec![
            Action::Text { text: "3… ".into(), mode: Default::default(), masked: false, description: None },
            Action::PauseMs { ms: 500, description: None },
            Action::Text { text: "2… ".into(), mode: Default::default(), masked: false, description: None },
            Action::PauseMs { ms: 500, description: None },
            Action::Text { text: "1！".into(), mode: Default::default(), masked: false, description: None },
        ],
    ));
    cfg.shortcuts.push(sc(
        "序列触发演示",
        "先按 F9（引导键），再依次按 J、K——序列适合引导键 + 后续步的长组合",
        &["F9 J K"],
        vec![Action::Text {
            text: "序列触发成功（F9 → J → K）".into(),
            mode: Default::default(),
            masked: false,
            description: None,
        }],
    ));
    cfg.shortcuts.push(sc(
        "和弦触发演示",
        // 成员键必须避开两处：① 序列/组合键用过的键（`F9 J K`、`Ctrl+Alt+O/B/R/S/V/M/P`）与
        // 示例层改键的来源键（H/J/K/L，层内按一下就变方向键）；② 热串触发词里用到的字母
        // （;sig/;addr/;btw/;date/;time/;site 用了 s i g a d r b t w e m 与 u/o 之外的常见字母）
        //——和弦成员键按下即被吞、不进热串缓冲，含它就永远凑不齐（;sig 曾因示例和弦 `I&O` 含 i
        // 而彻底不展开）。
        // 冲突检测器替我们盯着这两条（`sample_has_no_self_conflicts` 会红）。
        "同时按住 Y 和 Z——和弦适合双拇指 / 双指同按的快捷触发",
        &["Y&Z"],
        vec![Action::Text {
            text: "和弦触发成功（Y + Z 同按）".into(),
            mode: Default::default(),
            masked: false,
            description: None,
        }],
    ));

    // ---- 快捷键（基础版动作·续）：动作类型与「层内条目」各留一条样例 ----
    cfg.shortcuts.push(sc(
        "选中文本转大写",
        "文本动作的「转大写」模式：选中一段文字后按它，原地整段变大写",
        &["Ctrl+Alt+Shift+U"],
        vec![Action::Text {
            text: String::new(),
            mode: TextMode::ToUpper,
            masked: false,
            description: None,
        }],
    ));
    cfg.shortcuts.push(sc(
        "选中文本转小写",
        "同上，转小写——两个模式都靠剪贴板走一遍，选中内容会被就地替换",
        &["Ctrl+Alt+Shift+W"],
        vec![Action::Text {
            text: String::new(),
            mode: TextMode::ToLower,
            masked: false,
            description: None,
        }],
    ));
    cfg.shortcuts.push(sc(
        "注入组合键演示",
        "Keys 动作：触发后替你按一次 Ctrl+Shift+S（另存为）——把记不住的长组合收成一个键",
        &["Ctrl+Alt+S"],
        vec![Action::Keys {
            keys: vec!["Ctrl".into(), "Shift".into(), "S".into()],
            description: None,
        }],
    ));
    cfg.shortcuts.push(sc_in_layer(
        "层内快捷键演示",
        "归属「示例层」：只有 F1 按住 / F2 锁定后这条才存在，退回基础层就消失——成套快捷键适合收进层里",
        &["Ctrl+Alt+1"],
        vec![Action::Text {
            text: "层内快捷键生效：现在是「示例层」".into(),
            mode: Default::default(),
            masked: false,
            description: None,
        }],
    ));

    cfg.shortcuts.push(sc(
        "鼠标移动 + 点击演示",
        "Mouse 动作：先相对移动光标 60px、稍停，再左键点一下——鼠标三类动作（移动 / 点击 / 滚轮）都齐了",
        &["Ctrl+Alt+Shift+D"],
        vec![
            Action::Mouse {
                op: MouseOp::Move { dx: 60, dy: 0 },
                description: None,
            },
            Action::PauseMs { ms: 60, description: None },
            Action::Mouse {
                op: MouseOp::Click {
                    button: MouseButton::Left,
                },
                description: None,
            },
        ],
    ));
    cfg.shortcuts.push(sc(
        "鼠标侧键触发演示",
        "鼠标键也能当触发键：按下鼠标侧键「后退」即触发（配置引用鼠标键时才会挂鼠标钩子）",
        &["MouseBack"],
        vec![Action::Text {
            text: "鼠标侧键触发成功（MouseBack）".into(),
            mode: Default::default(),
            masked: false,
            description: None,
        }],
    ));

    // ---- 文本扩展（热串）----
    let ex = |trigger: &str, replace: &str, desc: &str| TextExpansion {
        name: Some(format!("热串 {trigger}")),
        description: Some(desc.into()),
        folder: folder.clone(),
        trigger: trigger.into(),
        replace: replace.into(),
        enabled: true,
    };
    cfg.expansions.push(ex(";sig", "此致\n—— 来自咔哒 Kada", "输入 ;sig 再按空格 / 回车 / Tab 展开多行签名"));
    cfg.expansions.push(ex(";addr", "某市某区某路 88 号", "输入 ;addr 再按空格 / 回车 / Tab 展开地址"));
    cfg.expansions.push(ex(";btw", "顺带一提，", "热串适合高频短语；触发词要避开日常单词，加个前缀最稳"));
    cfg.expansions.push(ex(";date", "{date}", "替换文本里的 {date} / {time} / {clipboard} 是动态片段：展开时才求值，这里插入当天日期"));
    cfg.expansions.push(ex(";time", "{time}", "同上，插入当前时间（时分秒）"));
    cfg.expansions.push(ex(";clip", "{clipboard}", "动态片段三兄弟之一：展开时插入当前剪贴板内容（{date} / {time} / {clipboard}）"));
    cfg.expansions.push(ex(
        ";site",
        "咔哒 Kada 下载页：https://ihomy.top/kada",
        "热串不只能打字，贴长链接 / 常用句子同样省事（触发词别用 H/J/K/L——层内它们被改成了方向键）",
    ));

    // ---- automation 专属（默认开启的完整版才有）----
    #[cfg(feature = "automation")]
    {
        cfg.remaps.push(Remap {
            name: Some("记事本里 F6 → Esc（改键条件门控）".into()),
            description: Some("带 when 条件的改键：只有前台是记事本时 F6 才变 Esc，其它应用里 F6 保持原样".into()),
            folder: folder.clone(),
            from: "F6".into(),
            to: "Escape".into(),
            when: Some(Condition::FrontmostApp {
                app: "notepad".into(),
            }),
            enabled: true,
            ..Remap::default()
        });
        cfg.shortcuts.push(sc(
            "打开 Kada 发布页",
            "OpenUrl 动作：用系统默认浏览器打开网址（示例直接开咔哒自己的下载页）",
            &["Ctrl+Alt+O"],
            vec![Action::OpenUrl {
                url: "https://ihomy.top/kada".into(),
                description: None,
            }],
        ));
        cfg.shortcuts.push(sc(
            "打开项目仓库",
            "OpenUrl 的第二个例子：直接开 GitHub 仓库——看更新、反馈问题都从这里进",
            &["Ctrl+Alt+G"],
            vec![Action::OpenUrl {
                url: "https://github.com/chillyice/Kada".into(),
                description: None,
            }],
        ));
    cfg.shortcuts.push(ShortcutItem {
        name: Some("条件门控演示（记事本限定）".into()),
        description: Some("带 when 条件：前台是记事本时这条才生效，其它应用里完全不存在".into()),
        folder: folder.clone(),
        layer: None,
        when: Some(Condition::FrontmostApp {
            app: "notepad".into(),
        }),
        triggers: vec!["Ctrl+Alt+B".into()],
        actions: vec![Action::Text {
            text: "条件快捷键生效：只在记事本里触发".into(),
            mode: Default::default(),
            masked: false,
            description: None,
        }],
        enabled: true,
    });
    cfg.shortcuts.push(sc(
        "命令输出到消息中心",
        "Command 动作：执行一条命令，输出弹窗展示（也可存成变量给后续动作用）",
        &["Ctrl+Alt+R"],
        vec![Action::Command {
            shell: Shell::Cmd,
            command: "echo 来自 Kada 的命令输出".into(),
            show_output: true,
            var: String::new(),
            description: None,
        }],
    ));
    cfg.shortcuts.push(sc(
        "命令取变量 + 引用",
        "命令动作用 whoami 取当前用户名存进变量 user，下一步文本用 {user} 引用它——变量是把多步动作串成一条流水线的纽带",
        &["Ctrl+Alt+V"],
        vec![
            Action::Command {
                shell: Shell::Cmd,
                command: "whoami".into(),
                show_output: false,
                var: "user".into(),
                description: None,
            },
            Action::Text {
                text: "你好，{user}！".into(),
                mode: Default::default(),
                masked: false,
                description: None,
            },
        ],
    ));
    cfg.shortcuts.push(ShortcutItem {
        name: Some("条件判断演示".into()),
        description: Some("If 动作：前台是记事本时走「满足条件」分支，否则走「否则」分支（与触发条件共用同一套判定）".into()),
        folder: folder.clone(),
        layer: None,
        when: None,
        triggers: vec!["Ctrl+Alt+Shift+F".into()],
        actions: vec![Action::If {
            condition: Condition::FrontmostApp {
                app: "notepad".into(),
            },
            then: vec![Action::Text {
                text: "条件成立：当前前台是记事本".into(),
                mode: Default::default(),
                masked: false,
                description: None,
            }],
            otherwise: vec![Action::Text {
                text: "条件不成立：当前前台不是记事本".into(),
                mode: Default::default(),
                masked: false,
                description: None,
            }],
            description: None,
        }],
        enabled: true,
    });
    cfg.shortcuts.push(sc(
        "并行动作演示",
        "Parallel 动作：两条命令各起一线程同时跑，全部结束才进下一步，两份输出都进消息中心",
        &["Ctrl+Alt+Shift+A"],
        vec![Action::Parallel {
            actions: vec![
                Action::Command {
                    shell: Shell::Cmd,
                    command: "echo parallel-A".into(),
                    show_output: false,
                    var: String::new(),
                    description: None,
                },
                Action::Command {
                    shell: Shell::Cmd,
                    command: "echo parallel-B".into(),
                    show_output: false,
                    var: String::new(),
                    description: None,
                },
            ],
            description: None,
        }],
    ));
}

    cfg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{detect_conflicts, sanitize_config, PlatformCaps};

    /// 示例必须是「合并导入能原样吃下」的合法配置：清洗不丢条目、不产生任何警告
    /// （悬空目录 / 层引用会被清掉，等于样例静默残废——绝不允许）。
    #[test]
    fn sample_survives_sanitize_with_nothing_dropped() {
        let cfg = sample_config();
        let (clean, notes) = sanitize_config(&cfg);
        assert!(notes.is_empty(), "清洗不该报任何问题：{notes:?}");
        assert_eq!(clean.folders, cfg.folders);
        assert_eq!(clean.layers, cfg.layers);
        assert_eq!(clean.shortcuts.len(), cfg.shortcuts.len());
        assert_eq!(clean.remaps.len(), cfg.remaps.len());
        assert_eq!(clean.expansions.len(), cfg.expansions.len());
    }

    /// 样例自己不许制造冲突（重复触发键 / 和弦遮蔽 / 层无切层键指向……）。
    #[test]
    fn sample_has_no_self_conflicts() {
        let conflicts = detect_conflicts(&sample_config(), &PlatformCaps::default());
        assert!(conflicts.is_empty(), "样例自带冲突：{conflicts:?}");
    }

    /// 覆盖面守卫：改键形态 / 触发类型 / 热串数量低于这条线就算「样例缩水了」。
    #[test]
    fn sample_covers_core_features() {
        let cfg = sample_config();
        assert!(cfg.remaps.iter().any(|r| r.tap.is_some() && r.hold.is_some()), "缺 tap-hold");
        assert!(cfg.remaps.iter().any(|r| r.hold_layer.is_some()), "缺 momentary 切层");
        assert!(cfg.remaps.iter().any(|r| r.lock_layer.is_some()), "缺锁定切层");
        assert!(cfg.remaps.iter().any(|r| r.tap2.is_some()), "缺连击");
        assert!(cfg.remaps.iter().any(|r| r.tap3.is_some()), "缺三击");
        assert!(cfg.remaps.iter().any(|r| r.oneshot.is_some()), "缺单次键");
        assert!(cfg.remaps.iter().any(|r| r.sticky.is_some()), "缺粘滞键");
        assert!(cfg.remaps.iter().any(|r| r.layer.is_some()), "缺层内改键");
        // 普通改键（`to` 直给）：其余形态字段全空、只有一个目标键。
        assert!(
            cfg.remaps.iter().any(|r| !r.to.is_empty()
                && r.tap.is_none()
                && r.hold.is_none()
                && r.oneshot.is_none()
                && r.sticky.is_none()
                && r.tap2.is_none()
                && r.tap3.is_none()
                && r.hold_layer.is_none()
                && r.lock_layer.is_none()),
            "缺普通改键（to）"
        );
        assert!(cfg.shortcuts.iter().any(|s| s.triggers.iter().any(|t| t.split(' ').count() > 1)), "缺序列");
        assert!(cfg.shortcuts.iter().any(|s| s.triggers.iter().any(|t| t.contains('&'))), "缺和弦");
        assert!(cfg.expansions.len() >= 5, "热串太少");
        assert!(cfg.shortcuts.iter().all(|s| s.enabled) && cfg.remaps.iter().all(|r| r.enabled), "样例必须全启用");
    }

    /// 动作类型与条目形态的覆盖面（跟着样例一起长：加新动作类型时先想「样例里怎么演示它」）。
    #[test]
    fn sample_covers_action_types_and_shapes() {
        let cfg = sample_config();
        let has_action = |pred: fn(&Action) -> bool| cfg.shortcuts.iter().any(|s| s.actions.iter().any(pred));
        assert!(has_action(|a| matches!(a, Action::Text { .. })), "缺文本动作");
        assert!(has_action(|a| matches!(a, Action::Keys { .. })), "缺注入组合键");
        assert!(has_action(|a| matches!(a, Action::Mouse { .. })), "缺鼠标动作");
        assert!(
            has_action(|a| matches!(a, Action::Mouse { op: MouseOp::Move { .. }, .. })),
            "缺鼠标移动"
        );
        assert!(
            has_action(|a| matches!(a, Action::Mouse { op: MouseOp::Click { .. }, .. })),
            "缺鼠标点击"
        );
        assert!(
            has_action(|a| matches!(a, Action::Mouse { op: MouseOp::Scroll { .. }, .. })),
            "缺鼠标滚轮"
        );
        assert!(has_action(|a| matches!(a, Action::PauseMs { .. })), "缺延迟");
        assert!(
            has_action(|a| matches!(a, Action::Text { mode: TextMode::ToUpper, .. })),
            "缺「转大写」模式"
        );
        assert!(
            has_action(|a| matches!(a, Action::Text { mode: TextMode::ToLower, .. })),
            "缺「转小写」模式"
        );
        // 层内快捷键（不是层内改键）：成套快捷键收进层的用法要在样例里立着。
        assert!(cfg.shortcuts.iter().any(|s| s.layer.is_some()), "缺层内快捷键");
        // 鼠标键作触发键（配置引用鼠标键时才会挂鼠标钩子）。
        assert!(
            cfg.shortcuts.iter().any(|s| s.triggers.iter().any(|t| t.contains("Mouse"))),
            "缺鼠标键触发"
        );
        // 层标记（V0.60）：样例层带个标记，用户一眼看到「层标记」长什么样。
        assert!(cfg.layers.iter().all(|l| l.mark.is_some()), "缺层标记演示");
        // 热串要含动态片段，否则 {date}/{time}/{clipboard} 这条能力在样例里没有落点。
        assert!(cfg.expansions.iter().any(|e| e.replace.contains("{date}")), "缺动态片段 {{date}}");
        assert!(
            cfg.expansions.iter().any(|e| e.replace.contains("{clipboard}")),
            "缺动态片段 {{clipboard}}"
        );
    }

    /// automation 追加的条目也在这份覆盖面里（默认 feature 开；基础版编译时整段消失）。
    #[cfg(feature = "automation")]
    #[test]
    fn sample_covers_automation_features() {
        let cfg = sample_config();
        assert!(cfg.shortcuts.iter().any(|s| matches!(s.actions.first(), Some(Action::OpenUrl { .. }))), "缺 OpenUrl");
        assert!(cfg.shortcuts.iter().any(|s| matches!(s.actions.first(), Some(Action::Command { .. }))), "缺命令");
        assert!(cfg.shortcuts.iter().any(|s| s.when.is_some()), "缺条件门控");
        // 改键侧的触发条件门控（`when`）与快捷键侧是两套，各要有一条样例。
        assert!(cfg.remaps.iter().any(|r| r.when.is_some()), "缺改键条件门控");
        assert!(cfg.shortcuts.iter().any(|s| matches!(s.actions.first(), Some(Action::If { .. }))), "缺条件判断动作");
        assert!(cfg.shortcuts.iter().any(|s| matches!(s.actions.first(), Some(Action::Parallel { .. }))), "缺并行动作");
        // 变量要真的被「写出来又读回去」，否则 {var} 这条链在样例里只是摆设。
        let writes: Vec<&str> = cfg
            .shortcuts
            .iter()
            .flat_map(|s| s.actions.iter())
            .filter_map(|a| match a {
                Action::Command { var, .. } if !var.is_empty() => Some(var.as_str()),
                _ => None,
            })
            .collect();
        assert!(!writes.is_empty(), "缺「命令存变量」");
        assert!(
            cfg.shortcuts
                .iter()
                .flat_map(|s| s.actions.iter())
                .any(|a| matches!(a, Action::Text { text, .. } if writes.iter().any(|v| text.contains(&format!("{{{v}}}"))))),
            "写出的变量没有任何条目引用（{writes:?}）"
        );
    }

    /// 和弦成员键按下即被吞、不进热串缓冲，所以触发词里不能含成员键——含了就永远凑不齐
    /// （示例的和弦曾是 `I&O`，把示例自己的热串 `;sig` 打死了，现象是「;sig 死活不展开」）。
    #[test]
    fn sample_chord_members_stay_out_of_hotstring_triggers() {
        let cfg = sample_config();
        let members: std::collections::BTreeSet<char> = cfg
            .shortcuts
            .iter()
            .flat_map(|s| s.triggers.iter())
            .filter(|t| t.contains('&'))
            .flat_map(|t| t.split('&'))
            .filter_map(|m| m.chars().find(|c| c.is_ascii_alphabetic()))
            .map(|c| c.to_ascii_lowercase())
            .collect();
        for e in &cfg.expansions {
            for c in e.trigger.chars().filter(|c| c.is_ascii_alphabetic()) {
                assert!(
                    !members.contains(&c.to_ascii_lowercase()),
                    "热串 {:?} 含和弦成员键 {c}，成员键会被吞掉、该热串永远不展开",
                    e.trigger
                );
            }
        }
    }

    /// 能原样序列化再解析回来（合并导入走的就是这条路）。
    #[test]
    fn sample_json_roundtrip() {
        let cfg = sample_config();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg);
    }
}
