//! 配置报告（给安卓界面看）：条数 / 冲突 / 本机不支持的条目 / 解析错误 / 告警。
//!
//! 口径来自 `docs/安卓版规划.md` §2 / §3：无 root 档只有 `shortcuts` 成立，`remaps` /
//! 层 / 热串 / 手势 / 角动作 / 边缘滚动整块不成立。报告**只提示、不拦保存**（软冲突同款）。

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::json;

use kada_core::{detect_conflicts, Config, Key, PlatformCaps, Trigger};

use crate::action_support;

/// 无 root 档的能力声明：能收键（监听），但**不能注入任何键**，也没有窗口 / 鼠标 / 后台
/// 发送 / 屏幕亮度这些桌面能力。
fn android_caps() -> PlatformCaps {
    PlatformCaps {
        listen_unsupported: BTreeSet::new(),
        // 关键一条：改键目标与 `Keys` 动作在安卓上注入不了。整表标上，冲突检测会逐条提示。
        inject_unsupported: Key::ALL.iter().copied().collect(),
        frontmost_conditions: true,
        device_conditions: false,
        window_actions: false,
        window_wait: false,
        mouse_absolute: false,
        background_input: false,
        shell_actions: false,
        hot_corners: false,
        wheel_edges: false,
        touchpad_gestures: false,
        brightness: false,
        // 语义动作是安卓的强项：无障碍的 ACTION_* / GLOBAL_ACTION_* 就在这台上。
        semantic_actions: true,
    }
}

pub(crate) fn config_report(
    path: &Path,
    tier: &str,
    cfg: &Config,
    warnings: &[(String, String)],
) -> String {
    let mut combo = 0;
    let mut sequence = 0;
    let mut chord = 0;
    let mut trigger_errors = Vec::new();
    let mut unsupported_actions = Vec::new();

    for s in cfg.shortcuts.iter().filter(|s| s.enabled) {
        for t in &s.triggers {
            match Trigger::parse(t) {
                Ok(Trigger::Combo(_)) => combo += 1,
                Ok(Trigger::Sequence(_)) => sequence += 1,
                Ok(Trigger::Chord(_)) => chord += 1,
                Err(e) => trigger_errors.push(json!({
                    "name": s.name.clone().unwrap_or_default(),
                    "trigger": t,
                    "error": e.to_string(),
                })),
            }
        }
        let kinds = action_support::collect(&s.actions);
        if !kinds.is_empty() {
            unsupported_actions.push(json!({
                "name": s.name.clone().unwrap_or_default(),
                "triggers": s.triggers,
                "kinds": kinds,
            }));
        }
    }

    // 层内条目在无 root 档不可达（层只能由 tap-hold 改键激活，而改键不成立）。
    let layered_shortcuts = cfg
        .shortcuts
        .iter()
        .filter(|s| s.enabled && s.layer.is_some())
        .count();

    let conflicts = detect_conflicts(cfg, &android_caps());

    let mut notes = Vec::new();
    if !cfg.remaps.is_empty() {
        notes.push(format!(
            "本机不支持改键（remaps {} 条）：无 root 档不能注入，条目保留但不参与判定",
            cfg.remaps.len()
        ));
    }
    if !cfg.layers.is_empty() {
        notes.push(format!(
            "本机不支持键位层（layers {} 层，其中 {} 条快捷键挂在层里）：层由改键激活，无 root 档不成立",
            cfg.layers.len(),
            layered_shortcuts
        ));
    }
    if !cfg.expansions.is_empty() {
        notes.push(format!(
            "本机不支持打字触发的文本扩展（{} 条）：安卓软键盘输入不产生 KeyEvent",
            cfg.expansions.len()
        ));
    }
    notes.push(
        "吞键策略：非破坏式——只有凑齐快捷键 / 和弦 / 序列的那一下会被吞，成员键照常给目标 app"
            .into(),
    );

    json!({
        "ok": true,
        "path": path.display().to_string(),
        "tier": tier,
        "counts": {
            "shortcuts": cfg.shortcuts.len(),
            "shortcuts_enabled": cfg.shortcuts.iter().filter(|s| s.enabled).count(),
            "remaps": cfg.remaps.len(),
            "layers": cfg.layers.len(),
            "expansions": cfg.expansions.len(),
            "gestures": cfg.gestures.len(),
            "touchpad_gestures": cfg.touchpad_gestures.len(),
            "hot_corners": cfg.hot_corners.len(),
            "wheel_edges": cfg.wheel_edges.len(),
        },
        "triggers": {
            "combo": combo,
            "sequence": sequence,
            "chord": chord,
            "errors": trigger_errors,
        },
        "unsupported": {
            "remaps": cfg.remaps.iter().filter(|r| r.enabled).count(),
            "layer_shortcuts": layered_shortcuts,
            "expansions": cfg.expansions.len(),
            "gestures": cfg.gestures.len(),
            "touchpad_gestures": cfg.touchpad_gestures.len(),
            "hot_corners": cfg.hot_corners.len(),
            "wheel_edges": cfg.wheel_edges.len(),
        },
        "unsupported_actions": unsupported_actions,
        "conflicts": conflicts,
        "warnings": warnings
            .iter()
            .map(|(summary, detail)| json!({ "summary": summary, "detail": detail }))
            .collect::<Vec<_>>(),
        "notes": notes,
    })
    .to_string()
}
