// 配置读写命令（T-04 从 lib.rs 搬出）：`get_config` / `set_config`。
// 写盘口径见 说明书 §5.6：先清洗、原子写、`cfg_watch` 销账、广播 `config-updated`。

use tauri::{Emitter, Manager};

use serde::{Deserialize, Serialize};

use crate::*;
use crate::{
    apply_hints_visible, config_io, push_sync, sanitize_config, sync_autostart, Config, KadaState,
};

/// 读取当前配置。
#[tauri::command]
pub(crate) fn get_config(state: tauri::State<'_, KadaState>) -> Config {
    state.config.read().unwrap().clone()
}

/// 保存配置：先逐条清洗（坏触发键/动作/改键被单独忽略，不影响其余配置），
/// 写入磁盘并即时生效。返回被忽略内容的说明（供 UI 提示），只有真正失败（写盘等）才报错。
/// 落盘走 [`config_io::save`]（原子替换 + 留存上次良好副本），不会留下半截 JSON。
#[tauri::command]
pub(crate) fn set_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
    config: Config,
) -> Result<Vec<String>, String> {
    let (clean, ignored) = sanitize_config(&config);
    let autostart = clean.settings.autostart;
    // 提示框的显示开关也在这份配置里（设置页勾选 / 取消就走的这条路）：改了就顺带把窗口与
    // 托盘勾选对齐——否则「设置里勾上了但窗口不出来」得重启才知道。
    let hints_visible = clean.settings.hints.visible;
    let hints_before = state.config.read().unwrap().settings.hints.visible;
    // 「写盘 + 改内存 + 给监听销账」三件事同持 `cfg_watch` 锁：巡检线程拿同一把锁，
    // 不会插在中间把刚写的配置当成外部修改再加载一遍（外部真的改了也照样检得出）。
    let mut watch = state.cfg_watch.lock().unwrap();
    config_io::save(&state.file, &clean)?;
    *state.config.write().unwrap() = clean;
    watch.adopt_own_write(&state.file);
    drop(watch);
    sync_autostart(&app, autostart);
    // 广播给别的窗口（提示框窗口据 `config-updated` 重画列表，主界面据它跟上提示框的位置 /
    // 外观）——`set_config` 只改内存不通知的话，提示框会一直显示保存前的旧快捷键。
    if let Ok(snapshot) = state.config.read() {
        let _ = app.emit("config-updated", &*snapshot);
        push_sync(&snapshot);
    }
    if hints_visible != hints_before {
        apply_hints_visible(&app, hints_visible);
    }
    Ok(ignored)
}
/// 立即把当前配置同步写一份到「同步存档」目录，返回写入的文件路径（前端据此给明确反馈）。
/// 与自动推送（每次保存时的 [`push_sync`]）不同：这条失败要报错给用户看。
#[tauri::command]
pub(crate) fn sync_now(state: tauri::State<'_, KadaState>) -> Result<String, String> {
    let cfg = state.config.read().unwrap().clone();
    let file = sync_file(&cfg).ok_or("尚未设置同步存档文件夹")?;
    config_io::save(&file, &cfg)?;
    Ok(file.display().to_string())
}

/// 「从同步存档恢复」用：返回存档文件的路径，前端拿它走既有的导入流程（合并 / 替换 + 结果弹窗）。
/// 未配置目录 / 存档文件不存在都报错。
#[tauri::command]
pub(crate) fn sync_import_path(state: tauri::State<'_, KadaState>) -> Result<String, String> {
    let cfg = state.config.read().unwrap().clone();
    let file = sync_file(&cfg).ok_or("尚未设置同步存档文件夹")?;
    if !file.exists() {
        return Err(format!("同步存档里还没有配置：{}", file.display()));
    }
    Ok(file.display().to_string())
}

/// 探测本机 OneDrive 目录（给「选择目录」对话框做默认位置）：Windows 读 OneDrive 客户端写的
/// 环境变量（个人版 / 商业版 / 消费者版都试），macOS / Linux 猜常见的 `~/OneDrive`；都没有则 `None`。
#[tauri::command]
pub(crate) fn default_sync_dir(app: tauri::AppHandle) -> Option<String> {
    for key in ["OneDrive", "OneDriveCommercial", "OneDriveConsumer"] {
        if let Ok(dir) = std::env::var(key) {
            if !dir.trim().is_empty() && Path::new(&dir).is_dir() {
                return Some(dir);
            }
        }
    }
    let p = app.path().home_dir().ok()?.join("OneDrive");
    p.is_dir().then(|| p.display().to_string())
}

/// 导出当前配置到指定路径（JSON）。
#[tauri::command]
pub(crate) fn export_config(
    state: tauri::State<'_, KadaState>,
    path: String,
) -> Result<(), String> {
    let cfg = state.config.read().unwrap().clone();
    let json = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("写入失败：{e}"))
}

/// 导入结果：给 UI 报清「新增了什么 / 跳过了什么 / 忽略了多少坏条目 / 原配置留档在哪」。
/// 原先只回一个忽略条数，用户根本查不出是哪条没进来。
#[derive(Serialize)]
pub(crate) struct ImportOutcome {
    /// 实际采用的导入方式（`merge` / `replace`）。
    mode: String,
    /// 新增条目总数（合并方式下是并入的条数；替换方式下是导入文件里的条目数）。
    added: usize,
    /// 明细：逐条说明被跳过的条目（合并）与被忽略的坏条目（清洗）。
    notes: Vec<String>,
    /// 替换导入时「导入前配置」的留档路径（合并导入为 `None`：没覆盖任何东西）。
    backup: Option<String>,
}

/// 把一份「来路不同的子集配置」合并进当前配置并落盘——「导入配置 → 合并」「载入示例配置」
/// 「从卡带市场安装卡带」三条路共用这一条：只增不删、不碰本机设置、坏条目清洗后逐条报出
/// （口径见 [`kada_core::merge_configs`]）。
///
/// 落盘口径与 [`set_config`] 一致：写盘 + 改内存 + 给外部修改监听销账同持 `cfg_watch` 锁。
fn merge_into_config(
    app: &tauri::AppHandle,
    state: &KadaState,
    incoming: Config,
) -> Result<ImportOutcome, String> {
    let (incoming, mut notes) = sanitize_config(&incoming);
    let current = state.config.read().unwrap().clone();
    let (merged, report) = kada_core::merge_configs(&current, &incoming);
    let added = report.added(); // 先取计数，再把 skipped 移进 notes（部分移动后不能再借 report）
    notes.extend(report.skipped);
    // 合并保留本机设置，自启按现值同步（不做系统层改动，只是让状态机与配置一致）。
    let autostart = current.settings.autostart;

    let mut watch = state.cfg_watch.lock().unwrap();
    config_io::save(&state.file, &merged)?;
    *state.config.write().unwrap() = merged;
    watch.adopt_own_write(&state.file);
    drop(watch);
    sync_autostart(app, autostart);
    // 合并不碰 settings，提示框可见性不会变——不需要 apply_hints_visible 那条对齐路径。
    if let Ok(snapshot) = state.config.read() {
        let _ = app.emit("config-updated", &*snapshot);
        push_sync(&snapshot);
    }
    Ok(ImportOutcome {
        mode: "merge".into(),
        added,
        notes,
        backup: None,
    })
}

/// 从指定路径导入配置。
///
/// 两种方式：
/// - `merge`：并进当前配置——**只增不删**，现有条目一条不动；改键按原键、文本扩展按触发词、
///   快捷键按触发键判重，撞车的保留现有并逐条报出；`settings` 保留本机现值
///   （口径见 [`kada_core::merge_configs`]）。
/// - `replace`：整份覆盖（含设置）。因为不可逆，覆盖前先把现有配置复制留档
///   （`config.json.pre-import-<时间戳>`；随后 `config_io::save` 还会把旧内容留成 `.bak`）。
///
/// 两种方式都先对导入内容逐条清洗，坏条目被忽略并报出。
#[tauri::command]
pub(crate) fn import_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
    path: String,
    mode: Option<String>,
) -> Result<ImportOutcome, String> {
    let replace = match mode.as_deref() {
        Some("merge") => false,
        // 缺省按整份替换（老前端 / 手调 IPC 的既有行为）。
        None | Some("replace") => true,
        Some(other) => return Err(format!("未知的导入方式：{other}")),
    };

    let s = std::fs::read_to_string(&path).map_err(|e| format!("读取失败：{e}"))?;
    let imported: Config = serde_json::from_str(&s).map_err(|e| format!("解析失败：{e}"))?;
    if !replace {
        // 合并交给共用的落地路径（导入 / 载入示例 / 安装卡带同一条）：它自己清洗、自己落盘。
        return merge_into_config(&app, &state, imported);
    }

    let (incoming, mut notes) = sanitize_config(&imported);
    let backup = config_io::archive_copy(&state.file, config_io::PRE_IMPORT_MARK);
    // 留档是尽力而为（可能没配置可读或写不进去），失败要说一声：`save` 那层的 `.bak`
    // 仍是被覆盖内容的回滚点，所以不为它中断导入。
    if backup.is_none() && state.file.exists() {
        notes.push("导入前的配置留档失败（配置仍已替换；可回滚到 config.json.bak）".into());
    }
    let added = incoming.folders.len()
        + incoming.layers.len()
        + incoming.shortcuts.len()
        + incoming.remaps.len()
        + incoming.expansions.len();
    let autostart = incoming.settings.autostart;
    let next = incoming;

    // 与 set_config 同一套口径：写盘 + 改内存 + 给监听销账同持一把锁（导入也是一种「自己写的盘」）。
    let hints_before = state.config.read().unwrap().settings.hints.visible;
    let hints_after = next.settings.hints.visible;
    let mut watch = state.cfg_watch.lock().unwrap();
    config_io::save(&state.file, &next)?;
    *state.config.write().unwrap() = next;
    watch.adopt_own_write(&state.file);
    drop(watch);
    sync_autostart(&app, autostart);
    // 导入也可能换掉提示框的设置（替换导入会连设置一起覆盖）：与 set_config 走同一条对齐路径。
    if let Ok(snapshot) = state.config.read() {
        let _ = app.emit("config-updated", &*snapshot);
        push_sync(&snapshot);
    }
    if hints_after != hints_before {
        apply_hints_visible(&app, hints_after);
    }
    Ok(ImportOutcome {
        mode: if replace {
            "replace".into()
        } else {
            "merge".into()
        },
        added,
        notes,
        backup: backup.map(|p| p.display().to_string()),
    })
}

/// 内置示例卡带（`snippets/*.json`，与卡带市场里的是同一批文件，编译期嵌进二进制）：
/// 「载入示例」把 [`kada_core::sample::sample_config`] 和它们一起并入。
///
/// 基础版（`--no-default-features`）里窗口 / 搜索 / 文件三份含 automation 专属动作，整份
/// 解析会失败，所以只在完整版编进来；剪贴板与延时用的都是基础版动作，始终可用。
pub(crate) const BASIC_EXAMPLE_SNIPPETS: &[&str] =
    &[include_str!("../../../snippets/delay-actions.json")];
#[cfg(feature = "automation")]
pub(crate) const AUTOMATION_EXAMPLE_SNIPPETS: &[&str] = &[
    include_str!("../../../snippets/window-actions.json"),
    include_str!("../../../snippets/current-window.json"),
    include_str!("../../../snippets/search-web.json"),
    include_str!("../../../snippets/file-actions.json"),
];

/// 内置示例 = 代码里搭的样例配置 + 静态卡带文件（顶层的 `kada_snippet` 反序列化时忽略）。
pub(crate) fn bundled_examples() -> Result<Config, String> {
    let mut cfg = kada_core::sample::sample_config();
    #[cfg_attr(not(feature = "automation"), allow(unused_mut))]
    let mut texts: Vec<&str> = BASIC_EXAMPLE_SNIPPETS.to_vec();
    #[cfg(feature = "automation")]
    texts.extend_from_slice(AUTOMATION_EXAMPLE_SNIPPETS);
    for text in texts {
        let part: Config =
            serde_json::from_str(text).map_err(|e| format!("内置示例卡带解析失败：{e}"))?;
        cfg.folders.extend(part.folders);
        cfg.layers.extend(part.layers);
        cfg.shortcuts.extend(part.shortcuts);
        cfg.remaps.extend(part.remaps);
        cfg.expansions.extend(part.expansions);
        cfg.gestures.extend(part.gestures);
        cfg.touchpad_gestures.extend(part.touchpad_gestures);
    }
    Ok(cfg)
}

/// 载入内置示例配置（设置页「载入示例配置」）：并入当前配置，走合并语义——只增不删、
/// 不碰本机设置（与 `import_config(mode = "merge")` 同一口径，只是内容内置在二进制里）。
#[tauri::command]
pub(crate) fn load_sample_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
) -> Result<ImportOutcome, String> {
    merge_into_config(&app, &state, bundled_examples()?)
}

/// 导出配置卡带（规划 7.2-㉕，配置分享 / 卡带市场）：把点名的那几条（单条、多选、或某个
/// 目录下的全部）写成一个可分享的 JSON 文件。
///
/// 卡带就是一份**子集 `Config`** + 一个顶层 `kada_snippet` 元信息键，所以接收方走现有的
/// 「导入配置 → 合并」原样就能吃下，导入侧无需改动、也不会碰本机设置。目录引用在摘取时剥掉
/// （id 是本机 UUID，带过去只会指向不存在的目录）。
///
/// 写入失败只报错，不碰磁盘上的配置（它压根不读配置路径）。
#[tauri::command]
pub(crate) fn export_fragment(
    state: tauri::State<'_, KadaState>,
    path: String,
    picks: Vec<SnippetPick>,
) -> Result<usize, String> {
    let part = snippet(&state.config.read().unwrap(), &picks);
    let n = part.shortcuts.len()
        + part.remaps.len()
        + part.expansions.len()
        + part.gestures.len()
        + part.touchpad_gestures.len();
    if n == 0 {
        return Err("没有可导出的条目".into());
    }
    let title = snippet_title(&part);

    let mut val = serde_json::to_value(&part).map_err(|e| e.to_string())?;
    if let Some(obj) = val.as_object_mut() {
        // 设置不带（合并导入压根不碰设置，带着走只会让分享出来的文件看着像整份配置）。
        obj.remove("settings");
        // 元信息键：`Config` 反序列化忽略未知字段，故卡带与整份配置同一套解析。
        // `version` 给将来的在线索引/格式演进留位置。
        obj.insert(
            "kada_snippet".into(),
            serde_json::json!({ "version": 1, "name": title }),
        );
    }
    let json = serde_json::to_string_pretty(&val).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("写入失败：{e}"))?;
    Ok(n)
}

// ---- 卡带市场（规划 7.2-㉕ 在线索引） ----
//
// 没有服务端：索引就是一份能 GET 到的静态 JSON（默认指向本仓库 `snippets/index.json`，
// 设置里可改），里面的每条指向一个卡带文件——卡带文件本身仍是「子集 Config + kada_snippet」，
// 安装走的是和「导入配置 → 合并」同一条落地路径。

/// 索引地址：设置里留空等价于「用内置默认」（见 `Settings::snippet_index_url`）。
fn snippet_index_url(state: &KadaState) -> String {
    let url = state
        .config
        .read()
        .unwrap()
        .settings
        .snippet_index_url
        .trim()
        .to_string();
    if url.is_empty() {
        DEFAULT_SNIPPET_INDEX_URL.to_string()
    } else {
        url
    }
}

/// 拉取卡带索引（浏览 / 搜索 / 一键安装都从它出发）。地址取设置里的索引地址。
///
/// 用官方索引时再并一份**社区发布索引**（人工审核通过的卡带）：两份分开存放，
/// 镜像每天覆盖官方索引也不会把社区卡带抹掉。社区索引拉失败只当没有，不影响官方的。
#[tauri::command]
pub(crate) async fn fetch_snippet_index(
    state: tauri::State<'_, KadaState>,
) -> Result<Vec<IndexEntry>, String> {
    let url = snippet_index_url(&state);
    let text = http_get_text(&url).await?;
    let mut entries = parse_snippet_index(&text)?;
    if url == DEFAULT_SNIPPET_INDEX_URL {
        let bust = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let community_url = format!("{DEFAULT_SNIPPET_COMMUNITY_INDEX_URL}?t={bust}");
        if let Ok(text) = http_get_text(&community_url).await {
            if let Ok(community) = parse_snippet_index(&text) {
                entries = merge_index_entries(entries, community);
            }
        }
    }
    if entries.is_empty() {
        return Err("索引里一条卡带也没有".into());
    }
    Ok(entries)
}

/// 官方索引 + 社区索引：社区在前（新上架的更容易被看到），同一 `url` 只留一份。
/// 社区索引单独一份文件，镜像每天覆盖官方 `index.json` 不会影响它。
pub(crate) fn merge_index_entries(
    official: Vec<IndexEntry>,
    community: Vec<IndexEntry>,
) -> Vec<IndexEntry> {
    let mut merged: Vec<IndexEntry> = community
        .into_iter()
        .filter(|e| !official.iter().any(|x| x.url == e.url))
        .collect();
    merged.extend(official);
    merged
}

/// 一个卡带的下载结果：原文 + 清洗后的条目。
///
/// 装的时候回传 `text` 而不是重新下载：用户确认时看到的正是要装进去的那份字节，
/// 中间不会换成另一份内容（卡带是别人托管的文件，改内容是常事）。
#[derive(Serialize)]
pub(crate) struct SnippetFetch {
    pub(crate) text: String,
    scan: SnippetScan,
    pub(crate) preview: SnippetPreview,
}

/// 卡带预览：条目原样交给前端渲染（前端已有各条类的展示与动作摘要，后端不重复造一遍）。
#[derive(Serialize)]
pub(crate) struct SnippetPreview {
    pub(crate) shortcuts: Vec<ShortcutItem>,
    pub(crate) remaps: Vec<Remap>,
    pub(crate) expansions: Vec<TextExpansion>,
    pub(crate) gestures: Vec<kada_core::Gesture>,
    pub(crate) touchpad_gestures: Vec<kada_core::TouchpadGesture>,
    pub(crate) hot_corners: Vec<kada_core::HotCornerRule>,
    pub(crate) wheel_edges: Vec<kada_core::WheelEdgeRule>,
}

/// 下载一个卡带并给出预览，**不落盘**。
///
/// 预览这步不能省：卡带能带命令 / 脚本动作，等于「装进来就会跑」，必须让人先看清里面是什么。
/// 同时跑一遍机审：危险等级与法务标记随预览一起给前端，装之前就能看到。
#[tauri::command]
pub(crate) async fn fetch_snippet(url: String) -> Result<SnippetFetch, String> {
    let text = http_get_text(&url).await?;
    let part: Config = serde_json::from_str(&text).map_err(|e| format!("卡带解析失败：{e}"))?;
    let (part, _ignored) = sanitize_config(&part);
    if part.shortcuts.is_empty()
        && part.remaps.is_empty()
        && part.expansions.is_empty()
        && part.gestures.is_empty()
        && part.touchpad_gestures.is_empty()
        && part.hot_corners.is_empty()
        && part.wheel_edges.is_empty()
    {
        return Err("卡带里没有可用的条目（全是坏条目或本来就是空的）".into());
    }
    let scan = scan_snippet(&text)?;
    Ok(SnippetFetch {
        text,
        scan,
        preview: SnippetPreview {
            shortcuts: part.shortcuts,
            remaps: part.remaps,
            expansions: part.expansions,
            gestures: part.gestures,
            touchpad_gestures: part.touchpad_gestures,
            hot_corners: part.hot_corners,
            wheel_edges: part.wheel_edges,
        },
    })
}

/// 把预览时拿到的卡带文本合并进配置：与「导入配置 → 合并」同一口径（只增不删、不碰设置）。
#[tauri::command]
pub(crate) fn install_snippet(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
    text: String,
) -> Result<ImportOutcome, String> {
    let part: Config = serde_json::from_str(&text).map_err(|e| format!("卡带解析失败：{e}"))?;
    merge_into_config(&app, &state, part)
}

// ---- 卡带发布（机器审核在本机与 ihomy 各跑一遍，人工审核在 ihomy OPS 页） ----

/// 发布草稿：提交前给用户看的机器检查结果与最终 JSON。
#[derive(Serialize)]
pub(crate) struct PreparedPublish {
    content: String,
    scan: SnippetScan,
}

/// 社区发布只对官方市场开放：用自定义索引的人看到的是另一份市场，把卡带发到 ihomy
/// 既不在他眼前、也没经过他同意。
fn snippet_market_is_official(state: &KadaState) -> bool {
    let url = state
        .config
        .read()
        .unwrap()
        .settings
        .snippet_index_url
        .trim()
        .to_string();
    url.is_empty() || url == DEFAULT_SNIPPET_INDEX_URL
}

/// 生成发布草稿并跑本机机审（不联网、不落盘）。发布页改选项 / 改元信息时反复调用。
#[tauri::command]
pub(crate) fn prepare_publish(
    state: tauri::State<'_, KadaState>,
    picks: Vec<SnippetPick>,
    meta: SnippetMeta,
) -> Result<PreparedPublish, String> {
    if !snippet_market_is_official(&state) {
        return Err("当前使用自定义索引，不能发布到官方市场".into());
    }
    let (content, scan) = publish_payload(&state.config.read().unwrap(), &picks, &meta)?;
    Ok(PreparedPublish { content, scan })
}

/// 发布受理结果（ihomy 返回）：id + 待审状态 + 服务端机审判定的危险等级。
#[derive(Deserialize, Serialize)]
pub(crate) struct PublishReceipt {
    pub(crate) id: i64,
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) level: u8,
}

#[derive(Deserialize)]
pub(crate) struct PublishEnvelope {
    pub(crate) code: i32,
    #[serde(default)]
    pub(crate) message: String,
    #[serde(default)]
    pub(crate) data: Option<PublishReceipt>,
}

/// 提交发布：本机再算一遍（不信任前端拼的字节）→ POST 到固定官方端点 → 进入人工审核。
///
/// **人工审核通过前不会出现在市场**；服务端会独立重算危险等级与法务标记，本机结果只作提示。
#[tauri::command]
pub(crate) async fn submit_publish(
    state: tauri::State<'_, KadaState>,
    picks: Vec<SnippetPick>,
    meta: SnippetMeta,
) -> Result<PublishReceipt, String> {
    if !snippet_market_is_official(&state) {
        return Err("当前使用自定义索引，不能发布到官方市场".into());
    }
    let (content, local_scan) = publish_payload(&state.config.read().unwrap(), &picks, &meta)?;
    let name = serde_json::from_str::<serde_json::Value>(&content)
        .ok()
        .and_then(|v| v["kada_snippet"]["name"].as_str().map(str::to_string))
        .unwrap_or_else(|| meta.name.trim().to_string());
    let body = serde_json::json!({
        "name": name,
        "description": meta.description.trim(),
        "author": meta.author.trim(),
        "tags": meta.tags,
        "content": content,
    })
    .to_string();
    let text = http_post_json(DEFAULT_SNIPPET_PUBLISH_URL, &body).await?;
    let envelope: PublishEnvelope =
        serde_json::from_str(&text).map_err(|e| format!("发布接口返回异常：{e}"))?;
    if envelope.code != 0 {
        return Err(if envelope.message.trim().is_empty() {
            "发布失败".into()
        } else {
            envelope.message
        });
    }
    let mut receipt = envelope.data.ok_or("发布接口没有返回受理编号")?;
    if receipt.level == 0 {
        receipt.level = local_scan.level;
    }
    Ok(receipt)
}
