// 配置 / 动作迁移（C-04 从 lib.rs 搬出）：旧版动作归一化与设置越界值钳制。
// 加载路径只跑 migrate、不跑清洗（清洗只在保存 / 导入路径，见 说明书 §5.6）。

use crate::*;

/// 把旧版动作迁移为新版（幂等）：`Launch` → `App::Launch`、`CloseProgram` → `App::Close`、
/// `Cmd` → `Command(Cmd)`、`Powershell` → `Command(Powershell)`、`OpenFolder` → `Os::OpenFolder`，
/// 递归处理 `If` 与 `Parallel` 的嵌套动作。
pub fn migrate_action(a: Action) -> Action {
    match a {
        #[cfg(feature = "automation")]
        Action::Launch {
            program,
            args,
            description,
        } => Action::App {
            operation: AppOperation::Launch { program, args },
            description,
        },
        #[cfg(feature = "automation")]
        Action::CloseProgram {
            program,
            description,
        } => Action::App {
            operation: AppOperation::Close { program },
            description,
        },
        #[cfg(feature = "automation")]
        Action::Cmd {
            command,
            show_output,
            var,
            description,
        } => Action::Command {
            shell: Shell::Cmd,
            command,
            working_dir: String::new(),
            timeout_ms: None,
            ok_exit_codes: Vec::new(),
            show_output: show_output.into(),
            hide_window: false,
            var,
            wait: true,
            on_error: OnError::Continue,
            if_running: IfRunning::Run,
            description,
        },
        #[cfg(feature = "automation")]
        Action::Powershell {
            command,
            show_output,
            var,
            description,
        } => Action::Command {
            shell: Shell::Powershell,
            command,
            working_dir: String::new(),
            timeout_ms: None,
            ok_exit_codes: Vec::new(),
            show_output: show_output.into(),
            hide_window: false,
            var,
            wait: true,
            on_error: OnError::Continue,
            if_running: IfRunning::Run,
            description,
        },
        #[cfg(feature = "automation")]
        Action::OpenFolder { path, description } => Action::Os {
            operation: OsOperation::OpenFolder { path },
            description,
        },
        // 入口收口：动作类型下拉里不再单列这三类，旧配置加载时并进所属分类。
        #[cfg(feature = "automation")]
        Action::SendTo {
            target,
            payload,
            description,
        } => Action::System {
            operation: SystemOperation::SendTo { target, payload },
            description,
        },
        #[cfg(feature = "automation")]
        Action::Window {
            operation,
            description,
        } => Action::App {
            operation: AppOperation::CurrentWindow { action: operation },
            description,
        },
        #[cfg(feature = "automation")]
        Action::Brightness {
            operation,
            step,
            description,
        } => Action::System {
            operation: SystemOperation::Brightness { operation, step },
            description,
        },
        #[cfg(feature = "automation")]
        Action::If {
            condition,
            then,
            otherwise,
            description,
        } => Action::If {
            condition,
            then: then.into_iter().map(migrate_action).collect(),
            otherwise: otherwise.into_iter().map(migrate_action).collect(),
            description,
        },
        #[cfg(feature = "automation")]
        Action::Parallel {
            actions,
            description,
        } => Action::Parallel {
            actions: actions.into_iter().map(migrate_action).collect(),
            description,
        },
        Action::Group { name, actions } => Action::Group {
            name,
            actions: actions.into_iter().map(migrate_action).collect(),
        },
        Action::Cycle {
            actions,
            description,
        } => Action::Cycle {
            actions: actions.into_iter().map(migrate_action).collect(),
            description,
        },
        other => other,
    }
}

impl Config {
    /// 原地归一化：把旧版动作迁移为 `App` 动作，并把设置的越界值钳回合法区间。
    pub fn migrate(&mut self) {
        for s in &mut self.shortcuts {
            s.actions = std::mem::take(&mut s.actions)
                .into_iter()
                .map(migrate_action)
                .collect();
        }
        // 触摸板手势已从产品中移除：老配置里混在 `gestures[]`、带 `trigger` 的条目
        // **整条丢弃**（幂等）。不能留在右键手势里——那会让原本的触摸板笔迹变成
        // 「右键画这个方向串就触发」，行为完全不同。模板同理，右键手势没有形状概念。
        let mut kept_gestures = Vec::with_capacity(self.gestures.len());
        for mut g in std::mem::take(&mut self.gestures) {
            g.actions = std::mem::take(&mut g.actions)
                .into_iter()
                .map(migrate_action)
                .collect();
            if g.trigger.take().is_none() {
                kept_gestures.push(g);
            }
        }
        self.gestures = kept_gestures;
        for r in &mut self.hot_corners {
            r.actions = std::mem::take(&mut r.actions)
                .into_iter()
                .map(migrate_action)
                .collect();
        }
        for r in &mut self.wheel_edges {
            r.actions = std::mem::take(&mut r.actions)
                .into_iter()
                .map(migrate_action)
                .collect();
        }
        self.migrate_legacy_mouse_regions();
        self.settings.hints = std::mem::take(&mut self.settings.hints).normalized();
    }

    /// 旧版「触发角 / 边缘滚动」写在 `settings.hot_corners` / `settings.wheel_edges` 里，
    /// 每个位置只能一条。转成新根数组（同位置可多条）后清空旧键，幂等。
    fn migrate_legacy_mouse_regions(&mut self) {
        let hc = std::mem::take(&mut self.settings.hot_corners);
        for (spot, entry) in [
            (HotCornerSpot::TopLeft, &hc.top_left),
            (HotCornerSpot::TopRight, &hc.top_right),
            (HotCornerSpot::BottomLeft, &hc.bottom_left),
            (HotCornerSpot::BottomRight, &hc.bottom_right),
        ] {
            if entry.enabled || !entry.actions.is_empty() || !entry.name.trim().is_empty() {
                self.hot_corners.push(HotCornerRule {
                    spot,
                    name: Some(entry.name.clone()).filter(|n| !n.trim().is_empty()),
                    description: None,
                    layer: None,
                    dwell_ms: 0,
                    #[cfg(feature = "automation")]
                    when: None,
                    folder: None,
                    order: 0,
                    actions: entry.actions.clone(),
                    enabled: hc.enabled && entry.enabled,
                });
            }
        }
        let we = std::mem::take(&mut self.settings.wheel_edges);
        for (edge, entry) in [
            (WheelEdgeSpot::Top, &we.top),
            (WheelEdgeSpot::Bottom, &we.bottom),
            (WheelEdgeSpot::Left, &we.left),
            (WheelEdgeSpot::Right, &we.right),
        ] {
            if entry.enabled || !entry.actions.is_empty() || !entry.name.trim().is_empty() {
                self.wheel_edges.push(WheelEdgeRule {
                    edge,
                    name: Some(entry.name.clone()).filter(|n| !n.trim().is_empty()),
                    description: None,
                    layer: None,
                    #[cfg(feature = "automation")]
                    when: None,
                    folder: None,
                    order: 0,
                    actions: entry.actions.clone(),
                    enabled: we.enabled && entry.enabled,
                });
            }
        }
    }
}
