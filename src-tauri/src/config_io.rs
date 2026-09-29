//! 配置文件的读取与落盘：**原子写 + 损坏自愈**。
//!
//! 原先直接 `fs::write`（先截断再写），崩溃 / 断电会留下半截 JSON；读取端解析失败
//! 又静默回退成空配置，用户随手一保存就把空配置写回去——快捷键 / 改键 / 层 / 文本
//! 扩展「凭空消失」。这里立两条规矩：
//!
//! 1. **写入原子**：写同目录临时文件 → `fsync` → `rename` 覆盖目标。任何时刻磁盘上的
//!    `config.json` 要么是旧的完整内容、要么是新的完整内容，不存在半截。
//! 2. **损坏绝不静默清空**：解析失败先把原文件改名留档（`config.json.corrupt-<时间戳>`），
//!    再尝试用上次保存的良好副本（`config.json.bak`）自愈并回写；两边都不行才以空配置
//!    启动，并把经过作为 [`LoadWarning`] 返回（壳层推进消息中心告警）。原内容始终留在
//!    磁盘上，用户可人工修复后覆盖回去。
//!
//! 与 Tauri 无关，可单测。
//!
//! 运行期的**外部修改监听**（[`crate::config_watch`]）走另一条口子 [`read_only`]：同样解析
//! 配置，但**绝不碰磁盘**——自愈那套只属于启动，见该函数注释。

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use kada_core::Config;

/// 上次保存成功时的良好副本：损坏时的自愈来源，也是手动回滚点。
const BAK_SUFFIX: &str = ".bak";
/// 临时文件后缀（与主文件同目录，保证 `rename` 不跨卷）。
const TMP_SUFFIX: &str = ".tmp";
/// 损坏文件的留档名（带时间戳，不覆盖上一次事故的留档）。
const CORRUPT_MARK: &str = ".corrupt-";
/// 「替换导入」前的留档名（带时间戳）：导入不可逆，留一份能人工回滚的副本。
pub const PRE_IMPORT_MARK: &str = ".pre-import-";

/// 读取结果：配置本体 + 需要告警的异常（一切正常时 `warnings` 为空）。
pub struct LoadOutcome {
    pub config: Config,
    pub warnings: Vec<LoadWarning>,
}

/// 一条加载告警（壳层把它推进消息中心，让用户知道配置出过事、原内容在哪）。
pub struct LoadWarning {
    /// 摘要：发生了什么（消息中心卡片的「命令」字段）。
    pub summary: String,
    /// 细节：留档路径 / 恢复来源 / 解析器原始报错（卡片的「stderr」字段）。
    pub detail: String,
}

/// 读取配置：正常 → 直接返回；损坏 → 留档 + 尝试自愈 + 告警（绝不静默清空）。
pub fn load(file: &Path) -> LoadOutcome {
    let text = match fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return load_without_primary(file),
        Err(e) => return recover(file, &format!("配置读取失败：{e}")),
    };
    match parse(&text) {
        Ok(cfg) => LoadOutcome { config: cfg, warnings: Vec::new() },
        Err(e) => recover(file, &format!("配置解析失败：{e}")),
    }
}

/// **只读**解析文件内容（无副作用），外部修改监听用（见 [`crate::config_watch`]）。
///
/// [`load`] 的损坏自愈（留档 + 从 `.bak` 回写）是**启动**路径的口径：那时没有别的数据源，
/// 恢复总比空配置强。运行期监听不能照搬这套：用户正拿编辑器改 JSON，中途保存出来的半截
/// 内容会被判成「损坏」，一留档一自愈就把用户改到一半的内容从磁盘挪走、再拿旧备份盖回去
/// ——监听功能反过来毁掉用户的编辑。所以这条路径**只读不写**：解析得动就采纳，解析不动
/// 就原地保留（改好后下一轮会按新内容重新解析），磁盘一个字节都不动。
pub fn read_only(file: &Path) -> Result<Config, String> {
    let text = fs::read_to_string(file).map_err(|e| e.to_string())?;
    parse(&text)
}

/// 主文件不存在：首次启动（正常，静默用默认配置）或只剩备份可用（用户删了主文件 /
/// 同步中断——此时用备份恢复远胜过给一份空配置）。
fn load_without_primary(file: &Path) -> LoadOutcome {
    let bak = bak_path(file);
    let Ok(text) = fs::read_to_string(&bak) else {
        // 真正的首次启动：没有任何配置，用默认值且不算异常。
        return LoadOutcome { config: Config::default(), warnings: Vec::new() };
    };
    match parse(&text) {
        Ok(cfg) => self_heal(
            file,
            cfg,
            LoadWarning {
                summary: "配置主文件缺失，已从备份恢复".into(),
                detail: format!(
                    "{} 不存在；已用上次良好备份 {} 恢复并写回。",
                    file.display(),
                    bak.display()
                ),
            },
        ),
        Err(e) => LoadOutcome {
            config: Config::default(),
            warnings: vec![LoadWarning {
                summary: "配置主文件缺失且备份无法解析，以空配置启动".into(),
                detail: format!(
                    "{} 不存在，备份 {} 解析失败：{e}。本次以空配置启动；备份文件未被改动，可人工修复后覆盖回 {}。",
                    file.display(),
                    bak.display(),
                    file.display()
                ),
            }],
        },
    }
}

/// 主文件损坏：留档原文件 → 尝试用 `.bak` 自愈 → 都不行则以空配置启动。
/// **绝不删除原内容**（改名留档，且损坏内容不会被留成 `.bak` 覆盖掉好备份）。
fn recover(file: &Path, reason: &str) -> LoadOutcome {
    let archive_note = match archive_corrupt(file) {
        Some(p) => format!("损坏文件已留档为 {}", p.display()),
        None => format!("{} 留档失败（文件可能被占用），原文件仍在原处", file.display()),
    };
    let bak = bak_path(file);
    match read_backup(file) {
        Some(cfg) => self_heal(
            file,
            cfg,
            LoadWarning {
                summary: "配置损坏，已从备份恢复".into(),
                detail: format!(
                    "{reason}；{archive_note}；已用上次良好备份 {} 恢复并写回。",
                    bak.display()
                ),
            },
        ),
        None => LoadOutcome {
            config: Config::default(),
            warnings: vec![LoadWarning {
                summary: "配置损坏，本次以空配置启动".into(),
                detail: format!(
                    "{reason}；{archive_note}；没有可用的备份，本次以空配置启动。\
                     原内容未丢失：修复留档文件后覆盖回 {} 即可恢复。",
                    file.display()
                ),
            }],
        },
    }
}

/// 用恢复出来的配置立刻回写主文件（这才是真「自愈」——否则下次启动还是同一个烂文件，
/// 且「主文件缺失」会被当成首次启动、告警消失）。回写失败只补一句说明，不影响本次配置。
fn self_heal(file: &Path, cfg: Config, mut warning: LoadWarning) -> LoadOutcome {
    if let Err(e) = save(file, &cfg) {
        warning.detail.push_str(&format!("；回写失败：{e}"));
    }
    LoadOutcome { config: cfg, warnings: vec![warning] }
}

/// 把损坏的原文件改名留档（`config.json.corrupt-20260929-153000`），返回留档路径。
/// 带时间戳是为了不与上一次事故的留档互相覆盖；改名而非复制，保证后续保存盖不掉它。
fn archive_corrupt(file: &Path) -> Option<PathBuf> {
    let dest = file.with_file_name(format!("{}{CORRUPT_MARK}{}", file_name(file), stamp()));
    fs::rename(file, &dest).ok().map(|_| dest)
}

/// 把当前配置文件**复制**留档为 `<文件名><mark><时间戳>`，返回留档路径。
///
/// 用于「替换导入」这类把现有配置整体换掉的不可逆操作：原内容留一份带名字的快照，
/// 出事可人工覆盖回去。与 [`archive_corrupt`] 的区别是**复制而非改名**——主文件还要
/// 继续用（随后才被导入内容覆盖）；时间戳相同则加序号，绝不覆盖上一份留档。
/// 内容是原样快照（不做解析校验）：要的就是「覆盖前磁盘上是什么」这个事实。
pub fn archive_copy(file: &Path, mark: &str) -> Option<PathBuf> {
    let text = fs::read_to_string(file).ok()?;
    let base = file_name(file);
    let stamp = stamp();
    for n in 0..64u32 {
        let seq = if n == 0 { String::new() } else { format!("-{n}") };
        let dest = file.with_file_name(format!("{base}{mark}{stamp}{seq}"));
        if dest.exists() {
            continue;
        }
        return write_atomic(&dest, text.as_bytes()).ok().map(|_| dest);
    }
    None
}

/// 留档用的时间戳（`20260929-153000`，本地时区）。
fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// 读取 `.bak`（上次良好副本）；不存在或解析失败返回 `None`。
fn read_backup(file: &Path) -> Option<Config> {
    let text = fs::read_to_string(bak_path(file)).ok()?;
    parse(&text).ok()
}

/// 解析配置文本（含旧版动作迁移）。
fn parse(text: &str) -> Result<Config, String> {
    let mut cfg: Config = serde_json::from_str(text).map_err(|e| e.to_string())?;
    cfg.migrate();
    Ok(cfg)
}

/// 原子落盘：先把现有良好配置留一份 `.bak`，再写临时文件 → `fsync` → `rename` 覆盖。
pub fn save(file: &Path, cfg: &Config) -> Result<(), String> {
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    if let Some(dir) = file.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败：{e}"))?;
        }
    }
    backup_existing(file);
    write_atomic(file, json.as_bytes())
}

/// 保存前把现有配置留一份 `.bak`（上次良好副本，供损坏时自愈 / 手动回滚）。
/// 只在现有内容确实能解析时才留——否则损坏内容会把好备份盖掉，自愈就断了来源。
/// 尽力而为：留档失败不影响本次保存。
fn backup_existing(file: &Path) {
    let Ok(text) = fs::read_to_string(file) else { return };
    if parse(&text).is_err() {
        return;
    }
    let _ = write_atomic(&bak_path(file), text.as_bytes());
}

/// 原子替换：临时文件 → `fsync` → `rename` 到目标。失败时清掉临时文件并报错，
/// 目标文件保持原样（宁可这次保存失败，也不留下半截配置）。
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let (mut f, tmp) = create_tmp(path).map_err(|e| format!("创建临时文件失败：{e}"))?;
    // fsync 不能省：只写不刷的话，rename 可能先落盘、数据后落盘，断电后留下空文件
    // ——那正是「配置凭空消失」的另一种走法。
    let written = f.write_all(bytes).and_then(|_| f.sync_all());
    drop(f);
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(format!("写入临时文件失败：{e}"));
    }
    if let Err(e) = rename_replace(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("替换 {} 失败：{e}", path.display()));
    }
    Ok(())
}

/// 同目录唯一临时名（`config.json.<pid>.<n>.tmp`）：并发保存（连点两次保存、导入与保存
/// 同时进行）不会互相踩同一份临时文件写出半截内容。
fn create_tmp(path: &Path) -> std::io::Result<(File, PathBuf)> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let base = file_name(path);
    let pid = std::process::id();
    for _ in 0..64 {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_file_name(format!("{base}.{pid}.{n}{TMP_SUFFIX}"));
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(f) => return Ok((f, tmp)),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(ErrorKind::AlreadyExists, "无法分配临时文件名"))
}

/// `rename` 覆盖目标。Windows 上目标被别的进程短暂持有（杀软扫描、同步客户端）会
/// 共享冲突，让几次就成；真失败则报错返回，绝不半途覆盖目标文件。
fn rename_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut last = None;
    for attempt in 0..5 {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                if attempt < 4 {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }
    Err(last.expect("循环至少执行一次"))
}

/// `.bak` 路径（与主文件同目录）。
fn bak_path(file: &Path) -> PathBuf {
    file.with_file_name(format!("{}{BAK_SUFFIX}", file_name(file)))
}

/// 文件名（用于拼同目录的兄弟文件；无法取名时退回整条路径）。
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kada_core::{Action, ShortcutItem, TextMode};

    /// 独立临时目录（进程 id + 序号，避免并行测试 / 多进程互相踩），析构时清理。
    struct TmpDir(PathBuf);

    impl TmpDir {
        fn new(tag: &str) -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "kada_cfgio_{}_{}_{}",
                tag,
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            TmpDir(dir)
        }

        /// 配置主文件路径（不预先创建）。
        fn config(&self) -> PathBuf {
            self.0.join("config.json")
        }

        /// 目录内文件名（排序后的）。
        fn entries(&self) -> Vec<String> {
            let mut v: Vec<String> = fs::read_dir(&self.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 一份带名字的配置（用于区分不同版本）。
    fn cfg_named(name: &str) -> Config {
        Config {
            shortcuts: vec![ShortcutItem {
                name: Some(name.into()),
                triggers: vec!["Ctrl+Alt+K".into()],
                actions: vec![Action::Text { text: name.into(), mode: TextMode::Input, description: None }],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn first_shortcut_name(cfg: &Config) -> String {
        cfg.shortcuts[0].name.clone().unwrap_or_default()
    }

    #[test]
    fn first_run_without_file_is_silent_default() {
        let tmp = TmpDir::new("firstrun");
        let out = load(&tmp.config());
        assert!(out.warnings.is_empty(), "首次启动不该告警");
        assert!(out.config.shortcuts.is_empty());
        assert_eq!(tmp.entries(), Vec::<String>::new(), "读取不该凭空造文件");
    }

    #[test]
    fn save_then_load_roundtrips() {
        let tmp = TmpDir::new("roundtrip");
        let file = tmp.config();
        save(&file, &cfg_named("v1")).unwrap();

        let out = load(&file);
        assert!(out.warnings.is_empty());
        assert_eq!(first_shortcut_name(&out.config), "v1");
        assert_eq!(tmp.entries(), vec!["config.json".to_string()], "落盘不留临时文件");
    }

    #[test]
    fn corrupt_config_is_archived_and_kept_on_disk() {
        let tmp = TmpDir::new("corrupt");
        let file = tmp.config();
        let garbage = "{ \"shortcuts\": [ 这不是 JSON";
        fs::write(&file, garbage).unwrap();

        let out = load(&file);
        assert_eq!(out.warnings.len(), 1, "损坏必须告警，不能静默");
        assert_eq!(out.config.shortcuts.len(), 0, "无备份可用时以空配置启动");

        // 原内容必须还在盘上（改名留档），且主文件没被空配置盖掉。
        let archived: Vec<String> = tmp
            .entries()
            .into_iter()
            .filter(|n| n.contains(CORRUPT_MARK))
            .collect();
        assert_eq!(archived.len(), 1, "损坏文件应留档：{:?}", tmp.entries());
        assert_eq!(fs::read_to_string(tmp.0.join(&archived[0])).unwrap(), garbage);
        assert!(!file.exists(), "留档用改名而非复制，主文件已移走");
        assert!(out.warnings[0].detail.contains("corrupt"), "细节里要给出留档位置");
    }

    #[test]
    fn corrupt_config_recovers_from_backup_and_rewrites() {
        let tmp = TmpDir::new("heal");
        let file = tmp.config();
        // 两次保存：第一次无旧文件，第二次把 v1 留成 .bak。
        save(&file, &cfg_named("v1")).unwrap();
        save(&file, &cfg_named("v2")).unwrap();
        fs::write(&file, "半截 JSON").unwrap();

        let out = load(&file);
        assert_eq!(out.warnings.len(), 1);
        assert_eq!(first_shortcut_name(&out.config), "v1", "应从 .bak 恢复上次良好副本");
        assert!(file.exists(), "自愈要把恢复出来的配置写回主文件");

        // 写回的主文件能正常解析，且 .bak 仍在（下次损坏还能救）。
        let again = load(&file);
        assert!(again.warnings.is_empty(), "自愈后再次加载应完全正常：{:?}", again.warnings.len());
        assert_eq!(first_shortcut_name(&again.config), "v1");
        assert!(tmp.0.join("config.json.bak").exists());
    }

    #[test]
    fn missing_primary_recovers_from_backup() {
        let tmp = TmpDir::new("missing");
        let file = tmp.config();
        save(&file, &cfg_named("v1")).unwrap();
        save(&file, &cfg_named("v2")).unwrap();
        fs::remove_file(&file).unwrap();

        let out = load(&file);
        assert_eq!(out.warnings.len(), 1);
        assert_eq!(first_shortcut_name(&out.config), "v1");
        assert!(file.exists(), "用备份恢复后要写回主文件");
    }

    #[test]
    fn corrupt_content_never_poisons_the_backup() {
        let tmp = TmpDir::new("nopoisons");
        let file = tmp.config();
        save(&file, &cfg_named("good")).unwrap();
        save(&file, &cfg_named("good2")).unwrap();
        // 主文件被外部写坏；此时用户又保存了一次（内存里是空配置）。
        fs::write(&file, "坏").unwrap();
        save(&file, &cfg_named("new")).unwrap();

        let bak = fs::read_to_string(tmp.0.join("config.json.bak")).unwrap();
        assert!(parse(&bak).is_ok(), "备份必须仍是可解析的良好配置");
        assert_eq!(first_shortcut_name(&parse(&bak).unwrap()), "good");
    }

    #[test]
    fn failed_save_keeps_old_file_and_no_litter() {
        let tmp = TmpDir::new("failsave");
        // 目标路径是个目录：rename 覆盖目录必然失败。
        let file = tmp.0.join("config.json");
        fs::create_dir(&file).unwrap();

        let err = save(&file, &cfg_named("x")).unwrap_err();
        assert!(!err.is_empty());
        assert!(file.is_dir(), "失败的保存不能破坏原目标");
        assert_eq!(
            tmp.entries(),
            vec!["config.json".to_string()],
            "失败后不留临时文件：{:?}",
            tmp.entries()
        );
    }

    #[test]
    fn archive_copy_snapshots_without_touching_the_original() {
        let tmp = TmpDir::new("archive");
        let file = tmp.config();
        save(&file, &cfg_named("v1")).unwrap();

        let a = archive_copy(&file, PRE_IMPORT_MARK).unwrap();
        let b = archive_copy(&file, PRE_IMPORT_MARK).unwrap();

        assert_ne!(a, b, "同一秒内的两次留档不能互相覆盖：{:?}", tmp.entries());
        assert!(file.exists(), "留档是复制，主文件必须还在");
        assert_eq!(first_shortcut_name(&parse(&fs::read_to_string(&a).unwrap()).unwrap()), "v1");
        assert_eq!(first_shortcut_name(&parse(&fs::read_to_string(&b).unwrap()).unwrap()), "v1");
        assert!(
            a.file_name().unwrap().to_string_lossy().contains(PRE_IMPORT_MARK),
            "留档名要能一眼看出是导入前快照：{}",
            a.display()
        );
        // 留档是快照：主文件被导入内容覆盖后，快照仍是旧内容。
        save(&file, &cfg_named("v2")).unwrap();
        assert_eq!(first_shortcut_name(&parse(&fs::read_to_string(&a).unwrap()).unwrap()), "v1");
    }

    #[test]
    fn archive_copy_without_target_is_none() {
        let tmp = TmpDir::new("archive_none");
        assert!(archive_copy(&tmp.config(), PRE_IMPORT_MARK).is_none(), "没有配置可留档时返回 None");
        assert_eq!(tmp.entries(), Vec::<String>::new(), "失败不留垃圾文件");
    }

    #[test]
    fn concurrent_saves_use_distinct_temp_files() {
        let tmp = TmpDir::new("concurrent");
        let file = tmp.config();
        // 先落一份，保证两个线程的「留档旧文件」都有东西可读（否则是否生成 .bak 取决于竞态）。
        save(&file, &cfg_named("base")).unwrap();
        let (a, b) = (file.clone(), file.clone());
        let h1 = std::thread::spawn(move || save(&a, &cfg_named("a")));
        let h2 = std::thread::spawn(move || save(&b, &cfg_named("b")));
        h1.join().unwrap().unwrap();
        h2.join().unwrap().unwrap();

        // 两次保存都成功，磁盘上只剩主文件 + 备份（没有残留临时文件）。
        assert_eq!(tmp.entries(), vec!["config.json".to_string(), "config.json.bak".to_string()]);
        assert!(load(&file).warnings.is_empty());
    }
}
