//! 配置卡带（规划 7.2-㉕）：从整份配置里摘出「被点名的那几条」组成一份可分享的 JSON。
//!
//! 卡带就是一份**子集 `Config`**——所以接收方走现有的「导入配置 → 合并」原样就能吃下，
//! 导入侧一行都不用改。卡带额外带一个顶层 `kada_snippet` 元信息键（`Config` 反序列化时
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

/// 在线索引的格式版本（索引文件顶层 `version`）。老客户端读到更大的版本会直接拒绝，
/// 而不是按老字段猜——猜错的代价是装上一堆用户看不懂的条目。
pub const SNIPPET_INDEX_VERSION: u32 = 1;

/// 内置索引地址（设置页可改，别处自建索引就填自己的）。指向本仓库的 `snippets/index.json`，
/// 由 `scripts/build-snippet-index.mjs` 从 `snippets/*.json` 生成——**没有任何服务端**：
/// 索引就是一份能 GET 到的静态 JSON。
pub const DEFAULT_SNIPPET_INDEX_URL: &str =
    "https://raw.githubusercontent.com/chillyice/Kada/main/snippets/index.json";

/// 索引里的一条卡带（展示用；卡带本体是另一个 JSON，走现有「导入 → 合并」）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    /// 展示名（列表 / 搜索都靠它）。
    pub name: String,
    /// 卡带 JSON 的地址（绝对 URL）。
    pub url: String,
    /// 一句话说明，可空。
    #[serde(default)]
    pub description: String,
    /// 分享者，可空。
    #[serde(default)]
    pub author: String,
    /// 搜索关键词，可空。
    #[serde(default)]
    pub tags: Vec<String>,
}

/// 索引文件本体（顶层）。`snippets` 之外的字段忽略，方便索引方再塞自己的东西。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnippetIndex {
    #[serde(default = "default_snippet_index_version")]
    pub version: u32,
    #[serde(default)]
    pub snippets: Vec<IndexEntry>,
}

/// `SnippetIndex.version` 的缺省值（缺 `version` 的旧索引按 v1 读）。
pub fn default_snippet_index_version() -> u32 {
    SNIPPET_INDEX_VERSION
}

/// 解析索引文本：结构不对 / 版本过新报错；缺名字或地址的条目丢掉（半个条目在列表里没法点）。
pub fn parse_snippet_index(text: &str) -> Result<Vec<IndexEntry>, String> {
    let idx: SnippetIndex =
        serde_json::from_str(text).map_err(|e| format!("索引解析失败：{e}"))?;
    if idx.version > SNIPPET_INDEX_VERSION {
        return Err(format!(
            "索引格式版本 v{} 过新（当前支持到 v{}），请更新 Kada",
            idx.version, SNIPPET_INDEX_VERSION
        ));
    }
    Ok(idx
        .snippets
        .into_iter()
        .filter(|e| !e.name.trim().is_empty() && !e.url.trim().is_empty())
        .collect())
}

/// 卡带里点名的是哪一类条目。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PickKind {
    Shortcut,
    Remap,
    Expansion,
    Gesture,
    Hotcorner,
    Wheeledge,
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
            PickKind::Gesture => {
                // 手势没有目录归属，也没有层引用，原样带走即可（`when` 也跟着走）。
                let Some(g) = cfg.gestures.get(p.idx).cloned() else { continue };
                out.gestures.push(g);
            }
            PickKind::Hotcorner => {
                let Some(r) = cfg.hot_corners.get(p.idx).cloned() else { continue };
                out.hot_corners.push(r);
            }
            PickKind::Wheeledge => {
                let Some(r) = cfg.wheel_edges.get(p.idx).cloned() else { continue };
                out.wheel_edges.push(r);
            }
        }
    }

    out.layers = cfg.layers.iter().filter(|l| layer_ids.contains(&l.id)).cloned().collect();
    out
}

/// 卡带标题（写进 `kada_snippet.name`，也给分享者一个人看的名字）：单条取它的名字 / 触发键，
/// 多条报条数，空卡带空串。
pub fn snippet_title(part: &Config) -> String {
    let one = if let Some(s) = part.shortcuts.first() {
        s.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| s.triggers.join(" / "))
    } else if let Some(r) = part.remaps.first() {
        r.from.clone()
    } else if let Some(e) = part.expansions.first() {
        e.trigger.clone()
    } else if let Some(g) = part.gestures.first() {
        g.label()
    } else if let Some(c) = part.hot_corners.first() {
        c.label()
    } else if let Some(w) = part.wheel_edges.first() {
        w.label()
    } else {
        String::new()
    };
    let n = part.shortcuts.len()
        + part.remaps.len()
        + part.expansions.len()
        + part.gestures.len()
        + part.hot_corners.len()
        + part.wheel_edges.len();
    if n <= 1 {
        one
    } else {
        format!("卡带（{n} 项）")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, Folder, Layer, Remap, ShortcutItem, TextExpansion, TextMode};

    fn base() -> Config {
        Config {
            folders: vec![Folder { id: "f1".into(), name: "本机目录".into(), parent: None }],
            layers: vec![Layer { id: "l-nav".into(), name: "导航层".into(), ..Default::default() }],
            shortcuts: vec![ShortcutItem {
                name: Some("备份".into()),
                triggers: vec!["Ctrl+Alt+B".into()],
                actions: vec![Action::Text { text: "x".into(), mode: TextMode::Input, masked: false, description: None }],
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
        assert_eq!(part.folders.len(), 0, "目录 id 是本机的，不进卡带");
        assert!(part.settings == crate::Settings::default(), "卡带不带设置");
    }

    #[test]
    fn snippet_pulls_in_referenced_layers() {
        let mut cfg = base();
        cfg.shortcuts[0].layer = Some("l-nav".into());
        cfg.remaps[0].lock_layer = Some("l-nav".into());
        cfg.layers.push(Layer { id: "l-unused".into(), name: "没人引用的层".into(), ..Default::default() });

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
            "卡带（2 项）"
        );
        assert_eq!(snippet_title(&Config::default()), "");
    }

    /// 关键不变式：写出去的卡带文件必须能被 `import_config` 原样解析——
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

    #[test]
    fn snippet_carries_gestures() {
        let mut cfg = base();
        cfg.gestures = vec![crate::Gesture {
            name: Some("复制".into()),
            pattern: "DR".into(),
            enabled: true,
            ..Default::default()
        }];
        let part = snippet(&cfg, &[pick(PickKind::Gesture, 0)]);
        assert_eq!(part.gestures.len(), 1, "鼠标手势要能进卡带");
        assert_eq!(snippet_title(&part), "复制");
        // 越界下标照旧跳过。
        assert!(snippet(&cfg, &[pick(PickKind::Gesture, 9)]).gestures.is_empty());
    }

    #[test]
    fn index_parses_and_drops_incomplete_entries() {
        let text = serde_json::json!({
            "version": 1,
            "snippets": [
                { "name": "Vim 光标", "url": "https://x/vim.json", "tags": ["vim"] },
                { "name": "没有地址", "url": "  " },
                { "name": "", "url": "https://x/a.json" }
            ]
        })
        .to_string();
        let entries = parse_snippet_index(&text).unwrap();
        assert_eq!(entries.len(), 1, "缺名字 / 缺地址的条目没法点，直接丢");
        assert_eq!(entries[0].name, "Vim 光标");
        assert!(entries[0].description.is_empty() && entries[0].author.is_empty());
        assert_eq!(entries[0].tags, vec!["vim".to_string()]);
    }

    #[test]
    fn index_rejects_future_version_and_garbage() {
        let future = serde_json::json!({ "version": SNIPPET_INDEX_VERSION + 1, "snippets": [] });
        assert!(parse_snippet_index(&future.to_string()).unwrap_err().contains("过新"));
        assert!(parse_snippet_index("not json").is_err());
        // 缺 version 的索引按 v1 读（默认走 default_snippet_index_version）。
        assert!(parse_snippet_index(r#"{"snippets":[]}"#).is_ok());
    }
}