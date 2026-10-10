//! 配置片段（规划 7.2-㉕）：从整份配置里摘出「被点名的那几条」组成一份可分享的 JSON。
//!
//! 片段就是一份**子集 `Config`**——所以接收方走现有的「导入配置 → 合并」原样就能吃下，
//! 导入侧一行都不用改。片段额外带一个顶层 `kada_snippet` 元信息键（`Config` 反序列化时
//! 忽略未知字段），将来真做在线索引时它就是索引条目的来源。
//!
//! 摘取时的两条口径：
//!
//! - **目录引用剥掉**（`folder = None`）：目录 id 是本机的 UUID，带到别人机器上只会指向一个
//!   不存在的目录（导入时 `sanitize_config` 会清掉它，还报一句「目录不存在」的噪音）。
//! - **被引用的层一起带走**：条目引用了层却没带上层定义，导入时那条的层引用会被清成「基础层」
//!   ——一条静默改了行为的快捷键，比导入直接失败更难查。
//!
//! 与 Tauri 无关，纯函数，可单测。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::Config;

/// 片段里点名的是哪一类条目。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PickKind {
    Shortcut,
    Remap,
    Expansion,
}

/// 一条被点名要分享的条目（前端按「类别 + 下标」指；下标越界者跳过）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetPick {
    pub kind: PickKind,
    pub idx: usize,
}

/// 摘出被点名的条目，组成一份可分享的子集配置（`settings` 恒为默认值、`folders` 为空）。
pub fn snippet(cfg: &Config, picks: &[SnippetPick]) -> Config {
    let mut out = Config::default();
    let mut layer_ids: BTreeSet<String> = BTreeSet::new();

    for p in picks {
        match p.kind {
            PickKind::Shortcut => {
                let Some(mut s) = cfg.shortcuts.get(p.idx).cloned() else { continue };
                s.folder = None;
                layer_ids.extend(s.layer.clone());
                out.shortcuts.push(s);
            }
            PickKind::Remap => {
                let Some(mut r) = cfg.remaps.get(p.idx).cloned() else { continue };
                r.folder = None;
                layer_ids.extend(r.layer.clone());
                layer_ids.extend(r.hold_layer.clone());
                layer_ids.extend(r.lock_layer.clone());
                out.remaps.push(r);
            }
            PickKind::Expansion => {
                let Some(mut e) = cfg.expansions.get(p.idx).cloned() else { continue };
                e.folder = None;
                out.expansions.push(e);
            }
        }
    }

    out.layers = cfg.layers.iter().filter(|l| layer_ids.contains(&l.id)).cloned().collect();
    out
}

/// 片段标题（写进 `kada_snippet.name`，也给分享者一个人看的名字）：单条取它的名字 / 触发键，
/// 多条报条数，空片段空串。
pub fn snippet_title(part: &Config) -> String {
    let one = if let Some(s) = part.shortcuts.first() {
        s.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| s.triggers.join(" / "))
    } else if let Some(r) = part.remaps.first() {
        r.from.clone()
    } else if let Some(e) = part.expansions.first() {
        e.trigger.clone()
    } else {
        String::new()
    };
    let n = part.shortcuts.len() + part.remaps.len() + part.expansions.len();
    if n <= 1 {
        one
    } else {
        format!("{n} 项片段")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, Folder, Layer, Remap, ShortcutItem, TextExpansion, TextMode};

    fn base() -> Config {
        Config {
            folders: vec![Folder { id: "f1".into(), name: "本机目录".into(), parent: None }],
            layers: vec![Layer { id: "l-nav".into(), name: "导航层".into() }],
            shortcuts: vec![ShortcutItem {
                name: Some("备份".into()),
                triggers: vec!["Ctrl+Alt+B".into()],
                actions: vec![Action::Text { text: "x".into(), mode: TextMode::Input, description: None }],
                enabled: true,
                ..Default::default()
            }],
            remaps: vec![Remap { from: "CapsLock".into(), to: "Ctrl".into(), ..Default::default() }],
            expansions: vec![TextExpansion { trigger: ";a".into(), replace: "甲".into(), enabled: true, ..Default::default() }],
            ..Default::default()
        }
    }

    fn pick(kind: PickKind, idx: usize) -> SnippetPick {
        SnippetPick { kind, idx }
    }

    #[test]
    fn snippet_keeps_only_picked_entries() {
        let part = snippet(&base(), &[pick(PickKind::Shortcut, 0)]);
        assert_eq!(part.shortcuts.len(), 1);
        assert!(part.remaps.is_empty() && part.expansions.is_empty());
        assert_eq!(part.folders.len(), 0, "目录 id 是本机的，不进片段");
        assert!(part.settings == crate::Settings::default(), "片段不带设置");
    }

    #[test]
    fn snippet_pulls_in_referenced_layers() {
        let mut cfg = base();
        cfg.shortcuts[0].layer = Some("l-nav".into());
        cfg.remaps[0].lock_layer = Some("l-nav".into());
        cfg.layers.push(Layer { id: "l-unused".into(), name: "没人引用的层".into() });

        let part = snippet(
            &cfg,
            &[pick(PickKind::Shortcut, 0), pick(PickKind::Remap, 0)],
        );
        let ids: Vec<&str> = part.layers.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, vec!["l-nav"], "被引用的层必须一起带走，没人用的不带");
        assert_eq!(part.shortcuts[0].layer.as_deref(), Some("l-nav"));
        assert_eq!(part.remaps[0].lock_layer.as_deref(), Some("l-nav"));
    }

    #[test]
    fn snippet_strips_folder_and_skips_bad_index() {
        let mut cfg = base();
        cfg.shortcuts[0].folder = Some("f1".into());
        let part = snippet(&cfg, &[pick(PickKind::Shortcut, 0), pick(PickKind::Remap, 99)]);
        assert_eq!(part.shortcuts[0].folder, None, "目录引用剥掉");
        assert!(part.remaps.is_empty(), "下标越界的点名要跳过而不是崩");
    }

    #[test]
    fn snippet_title_names_single_entry_or_counts() {
        assert_eq!(snippet_title(&snippet(&base(), &[pick(PickKind::Shortcut, 0)])), "备份");
        assert_eq!(snippet_title(&snippet(&base(), &[pick(PickKind::Remap, 0)])), "CapsLock");
        assert_eq!(
            snippet_title(&snippet(
                &base(),
                &[pick(PickKind::Shortcut, 0), pick(PickKind::Remap, 0)]
            )),
            "2 项片段"
        );
        assert_eq!(snippet_title(&Config::default()), "");
    }

    /// 关键不变式：写出去的片段文件必须能被 `import_config` 原样解析——
    /// 「顶层多一个 `kada_snippet` 键」既不该让解析失败，也不该把设置带进去。
    #[test]
    fn exported_fragment_parses_back_as_config() {
        let part = snippet(&base(), &[pick(PickKind::Shortcut, 0)]);
        let mut val = serde_json::to_value(&part).unwrap();
        let obj = val.as_object_mut().unwrap();
        obj.remove("settings");
        obj.insert("kada_snippet".into(), serde_json::json!({ "version": 1, "name": "备份" }));
        let text = serde_json::to_string_pretty(&val).unwrap();

        let back: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(back.shortcuts.len(), 1);
        assert_eq!(back.shortcuts[0].name.as_deref(), Some("备份"));
        assert!(back.remaps.is_empty() && back.folders.is_empty());
    }
}