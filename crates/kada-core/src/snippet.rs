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

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Config;

/// 在线索引的格式版本（索引文件顶层 `version`）。老客户端读到更大的版本会直接拒绝，
/// 而不是按老字段猜——猜错的代价是装上一堆用户看不懂的条目。
pub const SNIPPET_INDEX_VERSION: u32 = 1;

/// 内置索引地址（设置页可改，别处自建索引就填自己的）。默认指向 ihomy 的静态索引
/// （`https://ihomy.top/files/kada/snippets/index.json`，与更新清单同一个站内目录），
/// 国内可直连；GitHub 上的 `snippets/index.json` 仍由 `scripts/build-snippet-index.mjs`
/// 生成、作为公开备份。**没有任何服务端**：索引就是一份能 GET 到的静态 JSON。
pub const DEFAULT_SNIPPET_INDEX_URL: &str =
    "https://ihomy.top/files/kada/snippets/index.json";

/// 社区发布卡的索引（人工审核通过后由 ihomy 生成）。默认与官方索引分开：镜像每天覆盖
/// 官方索引，社区卡不能写进那份文件，否则会被下一次同步抹掉。
pub const DEFAULT_SNIPPET_COMMUNITY_INDEX_URL: &str =
    "https://ihomy.top/files/kada/snippets/community/index.json";

/// 卡带发布提交地址（只由官方市场使用；自定义索引不发到官方）。
pub const DEFAULT_SNIPPET_PUBLISH_URL: &str =
    "https://ihomy.top/api/public/kada/snippets/submit";

/// 发布内容的体积 / 条目 / 嵌套上限（本机机审与服务端机审同一口径）。
pub const SNIPPET_MAX_BYTES: usize = 256 * 1024;
pub const SNIPPET_MAX_ENTRIES: usize = 200;
pub const SNIPPET_MAX_DEPTH: usize = 16;

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
    /// 机器判定危险等级（0 安全 / 1 低 / 2 中 / 3 高）。只有社区发布的卡带带这个字段；
    /// 官方内置索引没有，读成 None = 不显示徽标。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
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
    /// 触摸板手势（`Config.touchpad_gestures`）。
    Touchpad,
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
                // 手势没有层引用；目录归属在导出时剥掉（与快捷键同一口径）。
                let Some(mut g) = cfg.gestures.get(p.idx).cloned() else { continue };
                g.folder = None;
                out.gestures.push(g);
            }
            PickKind::Touchpad => {
                // 触摸板手势与鼠标手势分属两个数组（分类分开），摘取同样剥目录。
                let Some(mut g) = cfg.touchpad_gestures.get(p.idx).cloned() else { continue };
                g.folder = None;
                out.touchpad_gestures.push(g);
            }
            PickKind::Hotcorner => {
                let Some(mut r) = cfg.hot_corners.get(p.idx).cloned() else { continue };
                r.folder = None;
                out.hot_corners.push(r);
            }
            PickKind::Wheeledge => {
                let Some(mut r) = cfg.wheel_edges.get(p.idx).cloned() else { continue };
                r.folder = None;
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
    } else if let Some(g) = part.touchpad_gestures.first() {
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
        + part.touchpad_gestures.len()
        + part.hot_corners.len()
        + part.wheel_edges.len();
    if n <= 1 {
        one
    } else {
        format!("卡带（{n} 项）")
    }
}

/// 卡带机审结果：危险等级 + 解释性发现 + 法务关键词标记。
///
/// **等级只用于提示，不代替人工审核**：官方市场要求所有发布卡带先机器扫描、再经
/// ihomy 人工通过。等级口径两边（本机 / 服务端）必须一致，见 [`scan_snippet`]。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetScan {
    /// 0 安全 / 1 低 / 2 中 / 3 高。
    pub level: u8,
    /// 卡带里的条目总数。
    pub entries: usize,
    /// 风险说明（人话，给发布者与审核员看）。
    pub findings: Vec<String>,
    /// 命中的法务关键词分类（**只标记、不自动拒绝**，交给人工判断）。
    pub legal_flags: Vec<String>,
}

/// 发布时填的元信息（写进卡带顶层 `kada_snippet`，也是索引卡片的展示字段）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SnippetMeta {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// 会把东西执行起来的动作类型 → (等级, 说明)。按 serde 的 `type` 标签匹配，
/// 不依赖 `Action` 的 feature 门控（基础版也要能审自动化卡带）。
fn action_risk(kind: &str) -> Option<(u8, &'static str)> {
    match kind {
        "command" | "cmd" | "powershell" | "script" => {
            Some((3, "执行命令 / 脚本（可运行任意代码）"))
        }
        "launch" | "open_folder" | "close_program" | "os" | "system" | "app" | "open_url"
        | "window" | "capture" | "tool" | "brightness" => {
            Some((2, "启动程序 / 打开文件或网址 / 改系统状态"))
        }
        "text" | "keys" | "mouse" | "clipboard" | "transform" | "send_to" => {
            Some((1, "注入按键或文本 / 操作鼠标 / 改剪贴板"))
        }
        "pause_ms" | "group" | "if" | "parallel" => Some((0, "")),
        _ => None,
    }
}

/// 法务关键词分类。命中只进 `legal_flags` 供人工审核，不会自动拒绝——
/// 同一个词可能出现在「防诈骗提醒」这种正常文本里，机器判定不了语境。
const LEGAL_RULES: &[(&str, &[&str])] = &[
    (
        "分裂国家 / 颠覆",
        &["分裂国家", "颠覆国家", "台独", "藏独", "疆独", "港独", "东突", "法轮功", "六四事件"],
    ),
    ("侮辱 / 人身攻击", &["傻逼", "操你妈", "去死", "贱人", "杂种", "狗东西"]),
    (
        "违法 / 违禁",
        &["毒品", "冰毒", "K粉", "买枪", "枪支弹药", "赌博", "代开发票", "洗钱", "诈骗", "裸聊", "成人视频"],
    ),
];

/// 常见命令混淆指纹（小写匹配）。只加一条发现，不抬高等级——命令动作本来就是 3 级。
const SUSPICIOUS_PATTERNS: &[&str] = &[
    "-encodedcommand",
    "frombase64string",
    "invoke-expression",
    "iex(",
    "downloadstring",
    "downloadfile",
    "certutil -urlcache",
    "bitsadmin",
    "mshta ",
    "rundll32 ",
    "regsvr32 ",
    "schtasks ",
    "vssadmin delete",
    "bcdedit ",
    "netsh advfirewall",
    "set-mppreference",
    "add-mppreference",
    "mimikatz",
    "-windowstyle hidden",
    "powershell -w hidden",
];

fn scan_walk(
    value: &Value,
    depth: usize,
    level: &mut u8,
    findings: &mut BTreeMap<String, usize>,
    legal: &mut BTreeMap<String, BTreeSet<String>>,
) {
    if depth > SNIPPET_MAX_DEPTH {
        *level = 3;
        *findings.entry("结构嵌套过深（超过 16 层）".into()).or_insert(0) += 1;
        return;
    }
    match value {
        Value::Object(map) => {
            if let Some(Value::String(kind)) = map.get("type") {
                match action_risk(kind) {
                    Some((lv, desc)) => {
                        if lv > *level {
                            *level = lv;
                        }
                        if !desc.is_empty() {
                            *findings.entry(format!("{desc}")).or_insert(0) += 1;
                        }
                    }
                    None => {
                        *level = 3;
                        *findings
                            .entry(format!("未知动作类型「{kind}」（可能来自更新版本，需人工确认）"))
                            .or_insert(0) += 1;
                    }
                }
            }
            for v in map.values() {
                scan_walk(v, depth + 1, level, findings, legal);
            }
        }
        Value::Array(items) => {
            for v in items {
                scan_walk(v, depth + 1, level, findings, legal);
            }
            if items.len() > SNIPPET_MAX_ENTRIES {
                *findings
                    .entry(format!("单个列表条目过多（{} 条）", items.len()))
                    .or_insert(0) += 1;
            }
        }
        Value::String(text) => {
            let lower = text.to_lowercase();
            for pat in SUSPICIOUS_PATTERNS {
                if lower.contains(pat) {
                    *findings
                        .entry(format!("疑似命令混淆 / 下载执行：{}", pat.trim()))
                        .or_insert(0) += 1;
                }
            }
            for (category, terms) in LEGAL_RULES {
                for term in *terms {
                    if text.contains(term) {
                        legal
                            .entry((*category).to_string())
                            .or_default()
                            .insert((*term).to_string());
                    }
                }
            }
        }
        _ => {}
    }
}

/// 对卡带文本做本机机审：危险等级（0~3）、条目数、风险发现、法务关键词。
///
/// **纯函数**：只读 JSON，不执行内容、不访问网络。服务端 ihomy 用同一套规则重算，
/// 客户端算出来的结果只用于发布前提示，不作为上架依据。
pub fn scan_snippet(text: &str) -> Result<SnippetScan, String> {
    if text.len() > SNIPPET_MAX_BYTES {
        return Err(format!("卡带太大（{} KB），上限 {} KB", text.len() / 1024, SNIPPET_MAX_BYTES / 1024));
    }
    let root: Value = serde_json::from_str(text).map_err(|e| format!("卡带解析失败：{e}"))?;
    let obj = root.as_object().ok_or("卡带顶层必须是 JSON 对象")?;
    let mut entries = 0usize;
    for key in [
        "shortcuts",
        "remaps",
        "expansions",
        "gestures",
        "touchpad_gestures",
        "hot_corners",
        "wheel_edges",
    ] {
        if let Some(Value::Array(items)) = obj.get(key) {
            entries += items.len();
        }
    }
    if entries > SNIPPET_MAX_ENTRIES {
        return Err(format!("卡带条目过多（{entries} 条），上限 {SNIPPET_MAX_ENTRIES} 条"));
    }

    let mut level = 0u8;
    let mut findings = BTreeMap::new();
    let mut legal = BTreeMap::new();
    scan_walk(&root, 0, &mut level, &mut findings, &mut legal);

    let mut finding_list: Vec<String> = findings
        .into_iter()
        .map(|(desc, n)| if n > 1 { format!("{desc} × {n}") } else { desc })
        .collect();
    let legal_flags: Vec<String> = legal
        .into_iter()
        .map(|(category, hits)| format!("{category}（{}）", hits.into_iter().collect::<Vec<_>>().join("、")))
        .collect();
    for flag in &legal_flags {
        finding_list.push(format!("法务关键词：{flag}"));
    }
    Ok(SnippetScan { level, entries, findings: finding_list, legal_flags })
}

/// 生成**可发布**的卡带文本：只带被点名的条目、剥掉目录与设置、写入元信息，并跑一次机审。
///
/// 与「导出为卡带」同一个子集口径；区别只是把 `kada_snippet` 填全（导出只填名字），
/// 发布页的机器检查与提交用的是同一份字节——审核员通过后市场里下载到的就是它。
pub fn publish_payload(
    cfg: &Config,
    picks: &[SnippetPick],
    meta: &SnippetMeta,
) -> Result<(String, SnippetScan), String> {
    let mut part = snippet(cfg, picks);
    let name = meta.name.trim();
    let name = if name.is_empty() { snippet_title(&part) } else { name.to_string() };
    let description = meta.description.trim().to_string();
    let author = meta.author.trim().to_string();
    let mut tags: Vec<String> = Vec::new();
    for t in &meta.tags {
        let t = t.trim();
        if !t.is_empty() && !tags.iter().any(|x| x == t) {
            tags.push(t.to_string());
        }
    }
    if name.chars().count() > 60 {
        return Err("名称最多 60 个字".into());
    }
    if description.chars().count() > 500 {
        return Err("说明最多 500 个字".into());
    }
    if author.chars().count() > 40 {
        return Err("作者最多 40 个字".into());
    }
    if tags.len() > 8 || tags.iter().any(|t| t.chars().count() > 20) {
        return Err("标签最多 8 个、每个最多 20 个字".into());
    }
    let n = part.shortcuts.len()
        + part.remaps.len()
        + part.expansions.len()
        + part.gestures.len()
        + part.touchpad_gestures.len()
        + part.hot_corners.len()
        + part.wheel_edges.len();
    if n == 0 {
        return Err("没有可发布的条目".into());
    }
    // 目录是本机 UUID、设置不进卡带：真被点到也得先剥掉（`snippet` 已经剥目录，这里兜底）。
    part.folders.clear();
    part.settings = crate::Settings::default();

    let mut val = serde_json::to_value(&part).map_err(|e| e.to_string())?;
    let obj = val.as_object_mut().ok_or("卡带序列化失败")?;
    obj.remove("settings");
    obj.remove("folders");
    obj.insert(
        "kada_snippet".into(),
        serde_json::json!({
            "version": 1,
            "name": name,
            "description": description,
            "author": author,
            "tags": tags,
        }),
    );
    let text = serde_json::to_string_pretty(&val).map_err(|e| e.to_string())?;
    let scan = scan_snippet(&text)?;
    Ok((text, scan))
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
        cfg.touchpad_gestures = vec![crate::TouchpadGesture {
            name: Some("触摸板复制".into()),
            trigger: Some(crate::Key::CapsLock),
            pattern: "DR".into(),
            enabled: true,
            ..Default::default()
        }];
        let part = snippet(
            &cfg,
            &[pick(PickKind::Gesture, 0), pick(PickKind::Touchpad, 0)],
        );
        assert_eq!(part.gestures.len(), 1, "鼠标手势要能进卡带");
        assert_eq!(part.touchpad_gestures.len(), 1, "触摸板手势要能进卡带");
        assert_eq!(part.touchpad_gestures[0].trigger, Some(crate::Key::CapsLock));
        assert_eq!(snippet_title(&part), "卡带（2 项）");
        // 越界下标照旧跳过。
        assert!(snippet(&cfg, &[pick(PickKind::Gesture, 9)]).gestures.is_empty());
        assert!(snippet(&cfg, &[pick(PickKind::Touchpad, 9)]).touchpad_gestures.is_empty());
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

    #[test]
    fn scan_levels_follow_action_kind() {
        let text_only = serde_json::json!({
            "expansions": [{ "trigger": ";a", "replace": "甲" }]
        })
        .to_string();
        assert_eq!(scan_snippet(&text_only).unwrap().level, 0);

        let inject = serde_json::json!({
            "shortcuts": [{ "triggers": ["F1"], "actions": [{ "type": "keys", "keys": ["Ctrl+C"] }] }]
        })
        .to_string();
        assert_eq!(scan_snippet(&inject).unwrap().level, 1);

        let open = serde_json::json!({
            "shortcuts": [{ "triggers": ["F1"], "actions": [{ "type": "open_url", "url": "https://example.com" }] }]
        })
        .to_string();
        assert_eq!(scan_snippet(&open).unwrap().level, 2);

        let command = serde_json::json!({
            "shortcuts": [{ "triggers": ["F1"], "actions": [{ "type": "command", "shell": "powershell", "command": "rm -rf /" }] }]
        })
        .to_string();
        let scan = scan_snippet(&command).unwrap();
        assert_eq!(scan.level, 3);
        assert!(scan.findings.iter().any(|f| f.contains("命令")));
    }

    #[test]
    fn scan_flags_legal_terms_without_rejecting() {
        let text = serde_json::json!({
            "expansions": [{ "trigger": ";x", "replace": "台独 傻逼" }]
        })
        .to_string();
        let scan = scan_snippet(&text).unwrap();
        assert_eq!(scan.level, 0, "法务关键词不抬高危险等级");
        assert_eq!(scan.legal_flags.len(), 2);
        assert!(scan.legal_flags.iter().any(|f| f.contains("分裂国家")));
        assert!(scan.legal_flags.iter().any(|f| f.contains("侮辱")));
    }

    #[test]
    fn scan_flags_obfuscated_command_and_unknown_action() {
        let text = serde_json::json!({
            "shortcuts": [{ "triggers": ["F1"], "actions": [
                { "type": "command", "shell": "powershell", "command": "powershell -EncodedCommand AAAA" },
                { "type": "future_action" }
            ] }]
        })
        .to_string();
        let scan = scan_snippet(&text).unwrap();
        assert_eq!(scan.level, 3);
        assert!(scan.findings.iter().any(|f| f.contains("命令混淆")));
        assert!(scan.findings.iter().any(|f| f.contains("未知动作类型")));
    }

    #[test]
    fn scan_rejects_invalid_or_oversized_text() {
        assert!(scan_snippet("not json").is_err());
        assert!(scan_snippet("[]").is_err());
        let huge = " ".repeat(SNIPPET_MAX_BYTES + 1);
        assert!(scan_snippet(&huge).unwrap_err().contains("太大"));
    }

    #[test]
    fn publish_payload_strips_local_data_and_writes_meta() {
        let (text, scan) = publish_payload(
            &base(),
            &[pick(PickKind::Shortcut, 0), pick(PickKind::Expansion, 0)],
            &SnippetMeta {
                name: "  测试卡带  ".into(),
                description: "说明".into(),
                author: "小明".into(),
                tags: vec![" 测试 ".into(), "测试".into(), "".into()],
            },
        )
        .unwrap();
        assert_eq!(scan.entries, 2);
        let v: Value = serde_json::from_str(&text).unwrap();
        assert!(v.get("settings").is_none(), "发布内容不带设置");
        assert!(v.get("folders").is_none(), "发布内容不带本机目录");
        let meta = v.get("kada_snippet").unwrap();
        assert_eq!(meta["name"], "测试卡带");
        assert_eq!(meta["author"], "小明");
        assert_eq!(meta["tags"], serde_json::json!(["测试"]), "标签去空去重");
        assert!(v["shortcuts"][0]["folder"].is_null(), "条目的目录引用也要剥掉");
    }

    #[test]
    fn publish_payload_rejects_empty_and_long_fields() {
        let empty = publish_payload(&Config::default(), &[], &SnippetMeta::default()).unwrap_err();
        assert!(empty.contains("没有可发布"));
        let long = "字".repeat(61);
        assert!(publish_payload(
            &base(),
            &[pick(PickKind::Shortcut, 0)],
            &SnippetMeta { name: long, ..Default::default() }
        )
        .unwrap_err()
        .contains("名称"));
    }
}