//! 配置合并：把一份外来配置**并进**当前配置（导入的「合并」方式）。
//!
//! 「替换」导入是整份覆盖（`import_config` 的另一分支），一旦导入的是别人的配置，
//! 本机原有的快捷键/改键会**一次性消失**；这里提供另一条更安全的路：只往里加。
//!
//! 三条铁律：
//!
//! 1. **只增不删**：现有条目一条都不动（不改写、不删除，也不跑清洗——清洗会顺手丢掉
//!    用户手里那些不合规的老条目，那就成了「导入顺便帮你删东西」）。
//! 2. **不制造新冲突**：改键按 `from`、文本扩展按 `trigger`、快捷键按触发键判定「同一条」，
//!    撞车时保留现有的、跳过导入的那条并**逐条报出**；因此重复导入同一份文件是幂等的。
//! 3. **设置不合并**：`settings`（开机自启 / 快速唤醒键 / 超时 / 暂停）保留本机现值——
//!    导入别人分享的配置不该悄悄改掉本机开机自启这种事。要连设置一起换就用「替换」。
//!
//! 与 Tauri 无关，纯函数，可单测。

use serde::{Deserialize, Serialize};

use crate::{Config, ShortcutItem};

/// 合并结果的计数与明细（供 UI 报「新增了什么、跳过了什么」）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeReport {
    pub folders: usize,
    pub layers: usize,
    pub shortcuts: usize,
    pub remaps: usize,
    pub expansions: usize,
    /// 跳过的条目说明（逐条带名称/触发键，只报个数的话用户查不出是哪条没进来）。
    pub skipped: Vec<String>,
}

impl MergeReport {
    /// 本次合并实际新增的条目总数。
    pub fn added(&self) -> usize {
        self.folders + self.layers + self.shortcuts + self.remaps + self.expansions
    }
}

/// 合并配置：返回合并后的配置与明细。
///
/// 入参两侧都应是**已清洗**的配置（导入侧走 [`crate::sanitize_config`]），否则坏条目会
/// 原样进结果——本函数只做「按标识去重 + 追加」，不做逐条校验。
pub fn merge_configs(current: &Config, incoming: &Config) -> (Config, MergeReport) {
    let mut out = current.clone();
    let mut report = MergeReport::default();

    // 目录 / 层按 id 去重：id 相同即同一个（保留现有的名字，别让导入的配置改掉本机目录名）。
    // 先并目录与层再并条目，保证下列条目引用的 folder/layer id 一定已经存在。
    for f in &incoming.folders {
        if out.folders.iter().any(|x| x.id == f.id) {
            continue;
        }
        out.folders.push(f.clone());
        report.folders += 1;
    }
    for l in &incoming.layers {
        if out.layers.iter().any(|x| x.id == l.id) {
            continue;
        }
        out.layers.push(l.clone());
        report.layers += 1;
    }

    for s in &incoming.shortcuts {
        // 触发键有交集 = 同一条：合并因此**永远不会引入新的触发键冲突**（否则用户看到的
        // 是一堆自己没配过的红冲突），撞车时保留现有那条。
        if let Some(hit) = s
            .triggers
            .iter()
            .find(|t| out.shortcuts.iter().any(|c| c.triggers.contains(t)))
        {
            report.skipped.push(format!(
                "快捷键「{}」未导入：触发键 {hit} 已存在",
                shortcut_label(s)
            ));
            continue;
        }
        out.shortcuts.push(s.clone());
        report.shortcuts += 1;
    }

    // 改键按原键去重：一个物理键只能有一条改键，两条同名改键的行为是未定义的。
    for r in &incoming.remaps {
        if out.remaps.iter().any(|x| x.from == r.from) {
            report
                .skipped
                .push(format!("改键「{}」未导入：该键已有改键", r.from));
            continue;
        }
        out.remaps.push(r.clone());
        report.remaps += 1;
    }

    // 文本扩展按触发词去重：同一触发词两条扩展会由「最长匹配」随机决出胜者。
    for e in &incoming.expansions {
        if out.expansions.iter().any(|x| x.trigger == e.trigger) {
            report
                .skipped
                .push(format!("文本扩展「{}」未导入：该触发词已存在", e.trigger));
            continue;
        }
        out.expansions.push(e.clone());
        report.expansions += 1;
    }

    (out, report)
}

/// 快捷键的展示名（无名时退回触发键，都没有则「未命名」）。
fn shortcut_label(s: &ShortcutItem) -> String {
    if let Some(n) = s.name.as_deref().filter(|n| !n.trim().is_empty()) {
        return n.to_string();
    }
    if !s.triggers.is_empty() {
        return s.triggers.join(" / ");
    }
    "未命名".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, Folder, Layer, Remap, Settings, TextExpansion, TextMode};

    fn text(t: &str) -> Action {
        Action::Text { text: t.into(), mode: TextMode::Input, description: None }
    }

    fn shortcut(name: &str, trigger: &str) -> ShortcutItem {
        ShortcutItem {
            name: Some(name.into()),
            triggers: vec![trigger.into()],
            actions: vec![text(name)],
            enabled: true,
            ..Default::default()
        }
    }

    fn remap(from: &str, to: &str) -> Remap {
        Remap { from: from.into(), to: to.into(), ..Default::default() }
    }

    /// 本机现有配置：一条快捷键 + 一条改键 + 一条扩展 + 设置全开。
    fn local() -> Config {
        Config {
            folders: vec![Folder { id: "f-local".into(), name: "本机目录".into(), parent: None }],
            shortcuts: vec![shortcut("本机快捷键", "Ctrl+Alt+K")],
            remaps: vec![remap("CapsLock", "Ctrl")],
            expansions: vec![TextExpansion { trigger: ";addr".into(), replace: "本机地址".into(), enabled: true }],
            settings: Settings {
                autostart: true,
                paused: true,
                wake_key: Some("Alt".into()),
                action_timeout_ms: 5000,
                sequence_timeout_ms: 700,
                chord_timeout_ms: 700,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn names(cfg: &Config) -> Vec<String> {
        cfg.shortcuts
            .iter()
            .map(|s| s.name.clone().unwrap_or_default())
            .collect()
    }

    #[test]
    fn merge_appends_and_keeps_existing() {
        let cur = local();
        let incoming = Config {
            folders: vec![Folder { id: "f-local".into(), name: "对方目录名".into(), parent: None }],
            layers: vec![Layer { id: "l-nav".into(), name: "导航层".into() }],
            shortcuts: vec![shortcut("对方快捷键", "Ctrl+Alt+J")],
            ..Default::default()
        };
        let (out, report) = merge_configs(&cur, &incoming);

        assert_eq!(names(&out), vec!["本机快捷键", "对方快捷键"], "导入的条目追加在后面");
        assert_eq!(report.shortcuts, 1);
        assert_eq!(report.layers, 1);
        assert_eq!(report.folders, 0, "同 id 目录视为同一个，不重复加入");
        assert_eq!(out.folders[0].name, "本机目录", "同 id 目录保留现有的名字");
        // 现有条目一条不动。
        assert_eq!(out.remaps.len(), 1);
        assert_eq!(out.expansions[0].replace, "本机地址");
    }

    #[test]
    fn merge_never_touches_settings() {
        let cur = local();
        let incoming = Config {
            settings: Settings {
                autostart: false,
                paused: false,
                wake_key: None,
                action_timeout_ms: 0,
                sequence_timeout_ms: 50,
                chord_timeout_ms: 50,
                ..Default::default()
            },
            ..Default::default()
        };
        let (out, _) = merge_configs(&cur, &incoming);
        assert_eq!(out.settings, cur.settings, "合并保留本机设置（要覆盖设置就用替换导入）");
    }

    #[test]
    fn merge_skips_entries_that_already_exist() {
        let cur = local();
        let incoming = Config {
            // 触发键撞车（哪怕名字不同、或只是多触发键里有一个交集）整条跳过。
            shortcuts: vec![
                shortcut("换了名字的同一条", "Ctrl+Alt+K"),
                ShortcutItem { triggers: vec!["F9 K".into(), "Ctrl+Alt+K".into()], ..Default::default() },
                shortcut("真正的新条目", "F9 L"),
            ],
            remaps: vec![remap("CapsLock", "Escape")],
            expansions: vec![TextExpansion { trigger: ";addr".into(), replace: "对方地址".into(), enabled: true }],
            ..Default::default()
        };
        let (out, report) = merge_configs(&cur, &incoming);

        assert_eq!(report.added(), 1, "只有触发键完全不撞的那条进来");
        assert_eq!(names(&out), vec!["本机快捷键", "真正的新条目"]);
        assert_eq!(report.skipped.len(), 4);
        assert!(report.skipped[0].contains("Ctrl+Alt+K"), "跳过要报出是哪条：{:?}", report.skipped);
        assert!(report.skipped.iter().any(|s| s.contains("CapsLock")));
        assert!(report.skipped.iter().any(|s| s.contains(";addr")));
        // 撞车的现有条目保持原样。
        assert_eq!(out.remaps[0].to, "Ctrl");
        assert_eq!(out.expansions[0].replace, "本机地址");
    }

    #[test]
    fn merge_is_idempotent() {
        let cur = local();
        let incoming = Config {
            shortcuts: vec![shortcut("对方快捷键", "Ctrl+Alt+J")],
            remaps: vec![remap("F1", "F2")],
            ..Default::default()
        };
        let (once, first) = merge_configs(&cur, &incoming);
        assert_eq!(first.added(), 2);

        let (twice, second) = merge_configs(&once, &incoming);
        assert_eq!(second.added(), 0, "重复导入同一份文件不该翻倍");
        assert_eq!(second.skipped.len(), 2);
        assert_eq!(twice, once, "第二次合并结果与第一次完全相同");
    }

    #[test]
    fn merge_keeps_layer_reference_intact() {
        // 导入的改键「长按进入」的层随导入一起进来，引用不能断（断了该改键会整条作废）。
        let cur = local();
        let incoming = Config {
            layers: vec![Layer { id: "l-nav".into(), name: "导航层".into() }],
            remaps: vec![Remap {
                from: "Space".into(),
                hold_layer: Some("l-nav".into()),
                tap: Some("Space".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let (out, report) = merge_configs(&cur, &incoming);
        assert_eq!(report.layers, 1);
        assert_eq!(report.remaps, 1);
        let layer_ids: Vec<&str> = out.layers.iter().map(|l| l.id.as_str()).collect();
        assert!(layer_ids.contains(&"l-nav"), "被引用的层必须真的进来了");
        assert_eq!(out.remaps[1].hold_layer.as_deref(), Some("l-nav"));
    }

    #[test]
    fn merge_into_empty_config_takes_everything() {
        let incoming = local();
        let (out, report) = merge_configs(&Config::default(), &incoming);
        assert_eq!(report.added(), 4, "目录 / 快捷键 / 改键 / 文本扩展各一");
        assert_eq!(out.shortcuts.len(), 1);
        assert_eq!(out.folders.len(), 1);
        // 即便是空配置，设置也保持「本机」（默认）值——不跟着导入文件走。
        assert_eq!(out.settings, Settings::default());
    }
}
