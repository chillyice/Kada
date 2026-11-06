// 配置清洗（C-04 从 lib.rs 搬出）：保存 / 导入前剔掉不可用条目并说明忽略原因。
// 加载路径不跑清洗（见 说明书 §5.6）。

use std::collections::BTreeSet;

use crate::*;

/// 清洗目录：去重（首个生效）、剔除空 id/空名、把指向不存在目录的 `parent` 置 None、
/// 打断父链成环（置为根级）。返回清洗后的目录与每处被忽略内容的说明。
fn clean_folders(folders: &[Folder]) -> (Vec<Folder>, Vec<String>) {
    use std::collections::HashMap;

    let mut out: Vec<Folder> = Vec::new();
    let mut ignored: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for f in folders {
        let id = f.id.trim().to_string();
        let name = f.name.trim().to_string();
        if id.is_empty() || name.is_empty() {
            ignored.push(format!("文件夹「{}」已忽略：id 或名称为空", name));
            continue;
        }
        if !seen.insert(id.clone()) {
            ignored.push(format!("文件夹「{name}」已忽略：id 重复"));
            continue;
        }
        out.push(Folder {
            id,
            name,
            parent: f.parent.clone(),
        });
    }

    let ids: BTreeSet<String> = out.iter().map(|f| f.id.clone()).collect();
    let parent_of: HashMap<String, Option<String>> = out
        .iter()
        .map(|f| (f.id.clone(), f.parent.clone()))
        .collect();

    for f in &mut out {
        let mut cur = f.parent.clone();
        let mut hops: BTreeSet<String> = BTreeSet::new();
        let mut broken = false;
        while let Some(p) = cur {
            if p == f.id || !ids.contains(&p) || !hops.insert(p.clone()) {
                broken = true;
                break;
            }
            cur = parent_of.get(&p).cloned().flatten();
        }
        if broken {
            f.parent = None;
            ignored.push(format!(
                "文件夹「{}」的父文件夹链无效（成环或不存在），已置为根级",
                f.name
            ));
        }
    }

    (out, ignored)
}

fn clean_layers(layers: &[Layer]) -> (Vec<Layer>, Vec<String>) {
    let mut out: Vec<Layer> = Vec::new();
    let mut ignored: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for l in layers {
        let id = l.id.trim().to_string();
        let name = l.name.trim().to_string();
        if id.is_empty() || name.is_empty() {
            ignored.push(format!("层「{name}」已忽略：id 或名称为空"));
            continue;
        }
        if !seen.insert(id.clone()) {
            ignored.push(format!("层「{name}」已忽略：id 重复"));
            continue;
        }
        let mark = l
            .mark
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(|m| m.to_lowercase());
        out.push(Layer { id, name, mark });
    }

    (out, ignored)
}

/// 清洗复制快捷菜单的条目：空标签、校验不过的动作、超深子菜单、空子菜单逐条丢弃，
/// 其余原样保留（顺序不变）。`depth` 是当前层号，顶层为 1。
fn clean_menu_items(
    items: &[MenuEntry],
    depth: usize,
    ignored: &mut Vec<String>,
) -> Vec<MenuEntry> {
    let mut out = Vec::new();
    for item in items {
        match item {
            MenuEntry::Separator => out.push(MenuEntry::Separator),
            MenuEntry::Action {
                label,
                enabled,
                action,
            } => {
                let label = label.trim();
                if label.is_empty() {
                    ignored.push("快捷菜单条目已忽略：标签为空".into());
                    continue;
                }
                if let Err(e) = action.validate() {
                    ignored.push(format!("快捷菜单「{label}」已忽略：{e}"));
                    continue;
                }
                out.push(MenuEntry::Action {
                    label: label.to_string(),
                    enabled: *enabled,
                    action: action.clone(),
                });
            }
            MenuEntry::Submenu {
                label,
                enabled,
                items,
            } => {
                let label = label.trim();
                if label.is_empty() {
                    ignored.push("快捷菜单子菜单已忽略：标签为空".into());
                    continue;
                }
                if depth >= ClipboardMenu::MAX_DEPTH {
                    ignored.push(format!(
                        "快捷菜单「{label}」已忽略：子菜单最多嵌套 {} 层",
                        ClipboardMenu::MAX_DEPTH
                    ));
                    continue;
                }
                let items = clean_menu_items(items, depth + 1, ignored);
                if items.is_empty() {
                    ignored.push(format!("快捷菜单「{label}」已忽略：子菜单为空"));
                    continue;
                }
                out.push(MenuEntry::Submenu {
                    label: label.to_string(),
                    enabled: *enabled,
                    items,
                });
            }
        }
    }
    out
}

/// 清洗配置：先迁移旧版动作，再丢弃无法解析的触发键/动作/改键，返回可安全保存的
/// 配置与每处被忽略内容的说明（供 UI 提示）。
///
/// 用于保证「单条配置有问题不影响其它配置保存」：坏条目被单独忽略，不再拖垮整份保存。
pub fn sanitize_config(cfg: &Config) -> (Config, Vec<String>) {
    let mut cfg = cfg.clone();
    cfg.migrate();
    let mut ignored: Vec<String> = Vec::new();

    let (folders, folder_ignored) = clean_folders(&cfg.folders);
    ignored.extend(folder_ignored);
    let valid_folder_ids: BTreeSet<String> = folders.iter().map(|f| f.id.clone()).collect();

    let (layers, layer_ignored) = clean_layers(&cfg.layers);
    ignored.extend(layer_ignored);
    let valid_layer_ids: BTreeSet<String> = layers.iter().map(|l| l.id.clone()).collect();

    let mut out = Config {
        settings: cfg.settings.clone(),
        folders,
        layers,
        ..Default::default()
    };
    out.settings.clipboard_menu.items =
        clean_menu_items(&cfg.settings.clipboard_menu.items, 1, &mut ignored);

    for s in &cfg.shortcuts {
        let label = s.name.clone().unwrap_or_else(|| "（未命名）".into());
        let mut item = s.clone();

        if let Some(fid) = &item.folder {
            if !valid_folder_ids.contains(fid) {
                ignored.push(format!("快捷键「{label}」所属文件夹已忽略：文件夹不存在"));
                item.folder = None;
            }
        }

        if let Some(lid) = &item.layer {
            if !valid_layer_ids.contains(lid) {
                ignored.push(format!("快捷键「{label}」所属层已忽略：层不存在"));
                item.layer = None;
            }
        }

        let mut kept_triggers = Vec::new();
        for t in &item.triggers {
            match Trigger::parse(t) {
                Ok(_) => kept_triggers.push(t.clone()),
                Err(e) => ignored.push(format!("快捷键「{label}」的触发键「{t}」已忽略：{e}")),
            }
        }
        item.triggers = kept_triggers;

        let mut kept_actions = Vec::new();
        for a in &item.actions {
            match a.validate() {
                Ok(()) => kept_actions.push(a.clone()),
                Err(e) => ignored.push(format!("快捷键「{label}」的某个动作已忽略：{e}")),
            }
        }
        item.actions = kept_actions;

        // 触发侧 `when` 门控：字段为空（如「前台应用」没填进程名）会让条件恒真/恒假，
        // 单条清掉并提示，不拖垮整条快捷键。
        #[cfg(feature = "automation")]
        if let Some(w) = &item.when {
            if let Err(e) = w.validate() {
                ignored.push(format!("快捷键「{label}」的触发条件已忽略：{e}"));
                item.when = None;
            }
        }

        let has_content = !item.triggers.is_empty()
            || !item.actions.is_empty()
            || item.name.is_some()
            || item.description.is_some();
        if has_content {
            out.shortcuts.push(item);
        } else {
            ignored.push(format!("快捷键「{label}」已忽略：无触发键、动作或名称"));
        }
    }

    for r in &cfg.remaps {
        let ok_from = r.from.parse::<Key>().is_ok();
        if !ok_from {
            ignored.push(format!("改键「{}」已忽略：原键无法解析", r.from));
            continue;
        }
        let mut item = r.clone();

        // 归属目录 / 归属层 / 长按切层引用校验（断引用清空）。
        if let Some(fid) = &item.folder {
            if !valid_folder_ids.contains(fid) {
                ignored.push(format!("改键「{}」所属文件夹已忽略：文件夹不存在", r.from));
                item.folder = None;
            }
        }
        if let Some(lid) = &item.layer {
            if !valid_layer_ids.contains(lid) {
                ignored.push(format!("改键「{}」所属层已忽略：层不存在", r.from));
                item.layer = None;
            }
        }
        if let Some(hl) = &item.hold_layer {
            if !valid_layer_ids.contains(hl) {
                ignored.push(format!("改键「{}」的长按切层已忽略：层不存在", r.from));
                item.hold_layer = None;
            }
        }
        if let Some(ll) = &item.lock_layer {
            if !valid_layer_ids.contains(ll) {
                ignored.push(format!("改键「{}」的长按锁定层已忽略：层不存在", r.from));
                item.lock_layer = None;
            }
        }
        // 长按进入层（momentary）与长按锁定层（切换式）互斥：两者都设时保留切换式——层内
        // 放和弦/序列时 momentary 要同时按住切层键凑 3~4 个键，只有切换式用得起来。
        let nonblank = |s: &Option<String>| s.as_deref().is_some_and(|v| !v.trim().is_empty());
        if nonblank(&item.hold_layer) && nonblank(&item.lock_layer) {
            ignored.push(format!(
                "改键「{}」同时设了长按进入层与长按锁定层，保留长按锁定层（切换式切层）",
                r.from
            ));
            item.hold_layer = None;
        }

        // 单次/粘滞/双击/三击键名逐项校验（坏键名单独清空并提示）。
        let mut clear_key = |field: &mut Option<String>, label: &str| {
            if let Some(v) = field {
                if v.parse::<Key>().is_err() {
                    ignored.push(format!(
                        "改键「{}」的{label}「{v}」已忽略：键名无法解析",
                        r.from
                    ));
                    *field = None;
                }
            }
        };
        clear_key(&mut item.oneshot, "单次键");
        clear_key(&mut item.sticky, "粘滞键");
        clear_key(&mut item.tap2, "双击键");
        clear_key(&mut item.tap3, "三击键");

        // 触发侧 `when` 门控：字段为空的条件单条清掉并提示。
        #[cfg(feature = "automation")]
        if let Some(w) = &item.when {
            if let Err(e) = w.validate() {
                ignored.push(format!("改键「{}」的触发条件已忽略：{e}", r.from));
                item.when = None;
            }
        }

        // 归一化（优先级 sticky > oneshot > tap-hold > 普通 to）：修饰模式清掉其它形态字段。
        if item.is_sticky() || item.is_oneshot() {
            if item.is_sticky() && item.is_oneshot() {
                ignored.push(format!(
                    "改键「{}」同时设了粘滞与单次修饰，保留粘滞",
                    r.from
                ));
                item.oneshot = None;
            }
            let mode = if item.is_sticky() { "粘滞" } else { "单次" };
            if item.is_tap_hold() || !item.to.trim().is_empty() {
                ignored.push(format!(
                    "改键「{}」已归一化为{mode}修饰（忽略短按/长按/双击/三击/切层/普通改键）",
                    r.from
                ));
            }
            item.tap = None;
            item.hold = None;
            item.tap2 = None;
            item.tap3 = None;
            item.hold_layer = None;
            item.lock_layer = None;
            item.to = String::new();
            out.remaps.push(item);
        } else if item.is_tap_hold() {
            // tap-hold / 切层：短按/长按键名逐项校验，坏的单独清空并提示。
            if let Some(t) = &item.tap {
                if t.parse::<Key>().is_err() {
                    ignored.push(format!(
                        "改键「{}」的短按键「{t}」已忽略：键名无法解析",
                        r.from
                    ));
                    item.tap = None;
                }
            }
            if let Some(h) = &item.hold {
                if h.parse::<Key>().is_err() {
                    ignored.push(format!(
                        "改键「{}」的长按键「{h}」已忽略：键名无法解析",
                        r.from
                    ));
                    item.hold = None;
                }
            }
            if item.is_tap_hold() {
                out.remaps.push(item);
            } else {
                ignored.push(format!("改键「{}」已忽略：短按/长按键名均无法解析", r.from));
            }
        } else if item.to.parse::<Key>().is_ok() {
            out.remaps.push(item);
        } else if item.to.trim().is_empty() {
            // 没有普通「改为」、也没落下任何有效形态（多半是「长按进入层」指向的层不存在，
            // 那条提示已在上面给过）。说「键名无法解析」会把人引偏，直接说没有可用输出。
            ignored.push(format!(
                "改键「{}」已忽略：没有可用的输出（改为/短按/长按/切层都为空或已失效）",
                item.from
            ));
        } else {
            ignored.push(format!(
                "改键「{} → {}」已忽略：键名无法解析",
                item.from, item.to
            ));
        }
    }

    for e in &cfg.expansions {
        match e.validate() {
            Ok(()) => {
                let mut item = e.clone();
                if let Some(fid) = &item.folder {
                    if !valid_folder_ids.contains(fid) {
                        ignored.push(format!(
                            "文本扩展「{}」所属文件夹已忽略：文件夹不存在",
                            e.trigger
                        ));
                        item.folder = None;
                    }
                }
                out.expansions.push(item);
            }
            Err(err) => ignored.push(format!("文本扩展「{}」已忽略：{err}", e.trigger)),
        }
    }

    // 鼠标手势（规划 7.2-MG）：方向串归一化后为空 = 没法匹配，整条忽略；
    // 同方向串 + 同 `when` 才算重复（不同 `when` 是按条件分流，与快捷键同一口径）；
    // 动作逐个校验，坏动作单独摘掉（一条坏动作不该拖垮整条手势）。
    let mut seen_gesture: Vec<(String, WhenKey)> = Vec::new();
    for g in &cfg.gestures {
        let label = g.label();
        let mut item = g.clone();
        if let Some(fid) = &item.folder {
            if !valid_folder_ids.contains(fid) {
                ignored.push(format!("鼠标手势「{label}」所属文件夹已忽略：文件夹不存在"));
                item.folder = None;
            }
        }
        let pattern = normalize_pattern(&g.pattern);
        if pattern.is_empty() {
            ignored.push(format!("鼠标手势「{label}」已忽略：方向串无法解析"));
            continue;
        }
        if seen_gesture
            .iter()
            .any(|(p, w)| *p == pattern && *w == g.when_key())
        {
            ignored.push(format!(
                "鼠标手势「{label}」已忽略：方向串「{pattern}」与前面的手势重复"
            ));
            continue;
        }
        item.pattern = pattern.clone();
        // 兼容字段只由 `migrate` 读；清洗后一律清掉，别让老字段又被写回磁盘。
        item.trigger = None;
        item.template = None;
        let mut kept_actions = Vec::new();
        for a in &item.actions {
            match a.validate() {
                Ok(()) => kept_actions.push(a.clone()),
                Err(e) => ignored.push(format!("鼠标手势「{label}」的某个动作已忽略：{e}")),
            }
        }
        item.actions = kept_actions;
        #[cfg(feature = "automation")]
        if let Some(w) = &item.when {
            if let Err(e) = w.validate() {
                ignored.push(format!("鼠标手势「{label}」的触发条件已忽略：{e}"));
                item.when = None;
            }
        }
        seen_gesture.push((pattern, item.when_key()));
        out.gestures.push(item);
    }

    // 触摸板手势：**触发键必填且只能是键盘键**（鼠标键 / 缺键 = 永远采集不到，整条忽略）。
    // 方向串可以留空——那就是**只认形状**的条目（靠模板兜底匹配，见 §5.9）；方向串与模板
    // 都为空 = 永远匹配不上，整条忽略。判重指纹：方向串非空时看「触发键 + 方向串 + `when`」
    // （同一方向串配不同模板仍会互相遮蔽，只会触发先配置的那条）；**方向串为空时看模板**，
    // 否则两条纯形状条目会被误判成重复。
    type TouchpadFingerprint = (Key, String, WhenKey, Option<Vec<[i16; 2]>>);
    let mut seen_touchpad: Vec<TouchpadFingerprint> = Vec::new();
    for g in &cfg.touchpad_gestures {
        let label = g.label();
        let mut item = g.clone();
        let Some(trigger) = item.trigger else {
            ignored.push(format!("触摸板手势「{label}」已忽略：没有触发键"));
            continue;
        };
        if trigger.is_mouse() {
            ignored.push(format!(
                "触摸板手势「{label}」已忽略：触发键「{}」是鼠标键，只能选键盘键",
                key_name(trigger)
            ));
            continue;
        }
        if let Some(fid) = &item.folder {
            if !valid_folder_ids.contains(fid) {
                ignored.push(format!(
                    "触摸板手势「{label}」所属文件夹已忽略：文件夹不存在"
                ));
                item.folder = None;
            }
        }
        // 模板先校验（它是纯形状条目的唯一判据）：坏了就丢掉，不值得把整条手势也废掉。
        if let Some(t) = &item.template {
            let ok = t.len() == crate::gesture::TEMPLATE_POINTS
                && t.iter()
                    .all(|p| p[0] >= 0 && p[0] <= 1000 && p[1] >= 0 && p[1] <= 1000);
            if !ok {
                ignored.push(format!(
                    "触摸板手势「{label}」的形状模板已忽略：点数或坐标不合法"
                ));
                item.template = None;
            }
        }
        let pattern = normalize_pattern(&g.pattern);
        if pattern.is_empty() && item.template.is_none() {
            ignored.push(format!(
                "触摸板手势「{label}」已忽略：方向串为空且没有形状模板"
            ));
            continue;
        }
        // 纯形状条目的指纹带上模板；有方向串时模板不参与（同方向串依然是重复）。
        let fp_template = if pattern.is_empty() {
            item.template.clone()
        } else {
            None
        };
        if seen_touchpad.iter().any(|(k, p, w, t)| {
            *k == trigger && *p == pattern && *w == g.when_key() && *t == fp_template
        }) {
            let what = if pattern.is_empty() {
                "形状模板完全相同".to_string()
            } else {
                format!("方向串「{pattern}」")
            };
            ignored.push(format!(
                "触摸板手势「{label}」已忽略：触发键「{}」的{what}与前面的手势重复",
                key_name(trigger)
            ));
            continue;
        }
        item.pattern = pattern.clone();

        let mut kept_actions = Vec::new();
        for a in &item.actions {
            match a.validate() {
                Ok(()) => kept_actions.push(a.clone()),
                Err(e) => ignored.push(format!("触摸板手势「{label}」的某个动作已忽略：{e}")),
            }
        }
        item.actions = kept_actions;
        #[cfg(feature = "automation")]
        if let Some(w) = &item.when {
            if let Err(e) = w.validate() {
                ignored.push(format!("触摸板手势「{label}」的触发条件已忽略：{e}"));
                item.when = None;
            }
        }
        seen_touchpad.push((trigger, pattern, item.when_key(), fp_template));
        out.touchpad_gestures.push(item);
    }

    // 触发角（EXT-10）：动作逐个校验，坏动作单独摘掉（与手势同一口径），规则本身保留。
    for rule in &cfg.hot_corners {
        let label = rule.label();
        let mut item = rule.clone();
        if let Some(fid) = &item.folder {
            if !valid_folder_ids.contains(fid) {
                ignored.push(format!("角动作「{label}」所属文件夹已忽略：文件夹不存在"));
                item.folder = None;
            }
        }
        if let Some(name) = &item.name {
            let trimmed = name.trim().to_string();
            item.name = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            };
        }
        if let Some(lid) = &item.layer {
            if !valid_layer_ids.contains(lid) {
                ignored.push(format!("触发角「{label}」所属层已忽略：层不存在"));
                item.layer = None;
            }
        }
        // 防误触停留时长：超出范围的（手改 JSON 写了 10 秒）钳回上限，别让一次触发挂死。
        item.dwell_ms = item.dwell_ms.min(HOT_CORNER_DWELL_MAX_MS);
        let mut kept = Vec::new();
        for a in &item.actions {
            match a.validate() {
                Ok(()) => kept.push(a.clone()),
                Err(e) => ignored.push(format!("触发角「{label}」的某个动作已忽略：{e}")),
            }
        }
        item.actions = kept;
        #[cfg(feature = "automation")]
        if let Some(w) = &item.when {
            if let Err(e) = w.validate() {
                ignored.push(format!("触发角「{label}」的触发条件已忽略：{e}"));
                item.when = None;
            }
        }
        out.hot_corners.push(item);
    }

    // 边缘滚动（EXT-11）：动作逐个校验，口径同触发角。
    for rule in &cfg.wheel_edges {
        let label = rule.label();
        let mut item = rule.clone();
        if let Some(fid) = &item.folder {
            if !valid_folder_ids.contains(fid) {
                ignored.push(format!("边缘滚动「{label}」所属文件夹已忽略：文件夹不存在"));
                item.folder = None;
            }
        }
        if let Some(name) = &item.name {
            let trimmed = name.trim().to_string();
            item.name = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            };
        }
        if let Some(lid) = &item.layer {
            if !valid_layer_ids.contains(lid) {
                ignored.push(format!("边缘滚动「{label}」所属层已忽略：层不存在"));
                item.layer = None;
            }
        }
        let mut kept = Vec::new();
        for a in &item.actions {
            match a.validate() {
                Ok(()) => kept.push(a.clone()),
                Err(e) => ignored.push(format!("边缘滚动「{label}」的某个动作已忽略：{e}")),
            }
        }
        item.actions = kept;
        #[cfg(feature = "automation")]
        if let Some(w) = &item.when {
            if let Err(e) = w.validate() {
                ignored.push(format!("边缘滚动「{label}」的触发条件已忽略：{e}"));
                item.when = None;
            }
        }
        out.wheel_edges.push(item);
    }

    // 唤醒键：无法解析成单键时清空（关闭快速唤醒），不拖垮整份配置。
    if let Some(wake) = out.settings.wake_key.as_deref() {
        if wake.parse::<Key>().is_err() {
            ignored.push(format!("唤醒键「{wake}」已忽略：键名无法解析"));
            out.settings.wake_key = None;
        }
    }

    // 排除列表：去空白、丢空行（手写的多行文本常带尾随空行）。
    let clean_list = |list: &[String]| {
        list.iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
    };
    out.settings.gesture_exclude = clean_list(&out.settings.gesture_exclude);
    out.settings.touchpad_exclude = clean_list(&out.settings.touchpad_exclude);
    out.settings.exclude_apps = clean_list(&out.settings.exclude_apps);

    // 手势视觉（EXT-02）：颜色只收 `#RGB` / `#RRGGBB`（非法回落默认，避免把任意字符串
    // 塞进 CSS）；数值钳到界面可调范围，手改 JSON 写 0 / 负数不至于把浮层弄没。
    out.settings.gesture_trail_color = normalize_hex_color(
        &out.settings.gesture_trail_color,
        &default_gesture_trail_color(),
    );
    out.settings.gesture_fail_color = normalize_hex_color(
        &out.settings.gesture_fail_color,
        &default_gesture_fail_color(),
    );
    out.settings.gesture_trail_width = out.settings.gesture_trail_width.clamp(1, 20);
    out.settings.gesture_label_font_size = out.settings.gesture_label_font_size.clamp(8, 48);
    out.settings.gesture_label_offset = out.settings.gesture_label_offset.clamp(0, 200);

    (out, ignored)
}

/// 归一化 `#RGB` / `#RRGGBB`；不合法时回落到 `fallback`。
fn normalize_hex_color(raw: &str, fallback: &str) -> String {
    let hex = raw.trim().trim_start_matches('#');
    let valid = matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit());
    if !valid {
        return fallback.to_string();
    }
    let expanded: String = if hex.len() == 3 {
        hex.chars().flat_map(|c| [c, c]).collect()
    } else {
        hex.to_string()
    };
    format!("#{}", expanded.to_ascii_lowercase())
}
