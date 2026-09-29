//! 配置文件的**外部修改监听**（见 `docs/需求设计说明书.md` §5.6 / 规划 7.2-⑩）。
//!
//! 配置此前只在启动时读一次：手改 JSON、用 `.bak` 恢复备份、同步客户端在别处落盘，全都不
//! 生效；更糟的是下一个应用内保存会把整份内存配置写回去，**悄悄盖掉外部改动**。这里补上
//! 运行期的「发现外部改动 → 采纳」：定时比对磁盘内容，变了就重新解析并交给壳层替换。
//!
//! 三条设计口径：
//!
//! 1. **按内容比对，不按修改时间**。`mtime` 粒度粗（FAT 系 2 秒）、同步客户端可能保留原时间
//!    戳，还会被「同一秒内改两次」骗过去；配置文件只有几 KB，整份读进来算指纹最省事也最准。
//! 2. **只有语义真的变了才算改动**。编辑器重新排版、`migrate()` 补字段都会让字节不同而配置
//!    相同——解析出来与内存里那份 [`Eq`] 就静默放过，免得红点白闪一次。
//! 3. **解析不动就原地不动**。外部内容坏了（编辑器写到一半）绝不采纳、也绝不改磁盘，只在
//!    **同一份坏内容上提示一次**；用户修好后指纹变了，下一轮自然重新解析。
//!
//! 本模块与 Tauri 无关，可单测。

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;

use kada_core::Config;

use crate::config_io;

/// 巡检间隔（毫秒）。手改配置不需要更快的响应；每秒读一次几 KB 的文件可以忽略。
pub const POLL_MS: u64 = 1_000;

/// 一次巡检的结论。
#[derive(Debug)]
pub enum Change {
    /// 外部把配置换成了另一份**能解析**的内容：采纳它（含内存里那份的更新）。
    Reloaded(Box<Config>),
    /// 外部内容解析不了（多半是编辑器写到一半）：**保持现状**，只把原因报一次。
    Unreadable(String),
}

/// 监听状态：磁盘内容指纹基线 + 「这份坏内容已报过」的记账。
#[derive(Debug)]
pub struct ConfigWatcher {
    /// 上次见过（或本进程自己写过）的磁盘内容指纹；`None` = 上次看的时候文件不在。
    seen: Option<u64>,
    /// 已经报过警的坏内容指纹：同一份坏内容只提示一次，不然每秒刷一条消息。
    reported: Option<u64>,
}

impl ConfigWatcher {
    /// 建立基线：把**当前**磁盘内容认成「已知状态」。启动时调用——那份内容加载时已经读过。
    pub fn new(file: &Path) -> Self {
        Self { seen: fingerprint(file), reported: None }
    }

    /// 本进程自己刚写完盘（保存 / 导入）：把新内容认成「已知」，否则下一秒就会把自己的写入
    /// 当成外部修改再加载一遍。
    ///
    /// 调用方要**与写盘同持一把锁**（见 `kada` 壳层里 `KadaState::cfg_watch` 的用法）：写完再
    /// 销账之间存在窗口，巡检线程若正好插在中间，就会先看见新内容、后看见旧基线而误报一次。
    pub fn adopt_own_write(&mut self, file: &Path) {
        self.seen = fingerprint(file);
        self.reported = None;
    }

    /// 巡检一次。`current` = 内存里正在生效的配置（与磁盘内容等价时不算改动）。
    pub fn poll(&mut self, file: &Path, current: &Config) -> Option<Change> {
        let now = fingerprint(file);
        if now == self.seen {
            return None;
        }
        let Some(fp) = now else {
            // 文件被删了 / 读不动：**保持在跑的配置不动**（此时既没内容可采纳，也不该把
            // 用户正在用的配置清掉）。基线跟着走，等它再出现时按改动处理——用户恢复备份、
            // 同步客户端补回文件都走这条。
            self.seen = None;
            return None;
        };
        self.seen = Some(fp);
        match config_io::read_only(file) {
            // 解析出来与内存里那份相同：只是排版 / 字段顺序变了，不当改动。
            Ok(cfg) if cfg == *current => {
                self.reported = None;
                None
            }
            Ok(cfg) => {
                self.reported = None;
                Some(Change::Reloaded(Box::new(cfg)))
            }
            Err(e) => {
                if self.reported == Some(fp) {
                    return None; // 同一份坏内容已经报过，别每秒刷屏
                }
                self.reported = Some(fp);
                Some(Change::Unreadable(e))
            }
        }
    }
}

/// 文件内容指纹（不存在 / 读不动 → `None`）。
fn fingerprint(file: &Path) -> Option<u64> {
    let bytes = fs::read(file).ok()?;
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    Some(h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kada_core::{Action, ShortcutItem, TextMode};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 独立临时目录（进程 id + 序号，避免并行测试 / 多进程互相踩），析构时清理。
    struct TmpDir(PathBuf);

    impl TmpDir {
        fn new(tag: &str) -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "kada_cfgwatch_{}_{}_{}",
                tag,
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            TmpDir(dir)
        }

        fn config(&self) -> PathBuf {
            self.0.join("config.json")
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
                actions: vec![Action::Text {
                    text: name.into(),
                    mode: TextMode::Input,
                    description: None,
                }],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn name_of(cfg: &Config) -> String {
        cfg.shortcuts[0].name.clone().unwrap_or_default()
    }

    /// 外部写一份有效配置：返回巡检到的结论。
    fn write_and_poll(w: &mut ConfigWatcher, file: &Path, current: &Config, cfg: &Config) -> Option<Change> {
        fs::write(file, serde_json::to_string_pretty(cfg).unwrap()).unwrap();
        w.poll(file, current)
    }

    /// 建立基线时不看内容新旧：`new` 之后第一次巡检不该报改动。
    #[test]
    fn baseline_is_silent() {
        let tmp = TmpDir::new("baseline");
        let file = tmp.config();
        fs::write(&file, serde_json::to_string(&cfg_named("v1")).unwrap()).unwrap();
        let cur = cfg_named("v1");
        let mut w = ConfigWatcher::new(&file);
        assert!(w.poll(&file, &cur).is_none(), "刚建基线不该报改动");
    }

    /// 外部改动：报一次，采纳后再度巡检静默（基线已推进）。
    #[test]
    fn external_edit_is_reported_once_then_adopted() {
        let tmp = TmpDir::new("edit");
        let file = tmp.config();
        let cur = cfg_named("v1");
        fs::write(&file, serde_json::to_string(&cur).unwrap()).unwrap();
        let mut w = ConfigWatcher::new(&file);

        match write_and_poll(&mut w, &file, &cur, &cfg_named("v2")) {
            Some(Change::Reloaded(cfg)) => assert_eq!(name_of(&cfg), "v2"),
            other => panic!("应检出外部改动：{other:?}"),
        }
        // 采纳之后内存里的那份就是 v2，再来一轮不该重复报。
        assert!(w.poll(&file, &cfg_named("v2")).is_none(), "同一份内容不该报第二遍");
    }

    /// 本进程自己写的盘不该被当成外部修改（保存 / 导入后的销账）。
    #[test]
    fn own_write_is_not_an_external_change() {
        let tmp = TmpDir::new("ownwrite");
        let file = tmp.config();
        let v1 = cfg_named("v1");
        fs::write(&file, serde_json::to_string(&v1).unwrap()).unwrap();
        let mut w = ConfigWatcher::new(&file);

        let v2 = cfg_named("v2");
        fs::write(&file, serde_json::to_string(&v2).unwrap()).unwrap();
        w.adopt_own_write(&file);
        assert!(w.poll(&file, &v2).is_none(), "自己写的盘不算外部改动");
    }

    /// 走真实保存路径（`config_io::save`）复现 `set_config` 的时序：写盘 → 销账 → 巡检静默。
    /// 顺带固定一件事：`.bak` 的生成不会影响主文件的指纹判定。
    #[test]
    fn save_through_config_io_then_adopt_is_silent() {
        let tmp = TmpDir::new("savepath");
        let file = tmp.config();
        let v1 = cfg_named("v1");
        crate::config_io::save(&file, &v1).unwrap();
        let mut w = ConfigWatcher::new(&file);

        let v2 = cfg_named("v2");
        crate::config_io::save(&file, &v2).unwrap();
        w.adopt_own_write(&file);
        assert!(w.poll(&file, &v2).is_none(), "应用内保存不该被当成外部修改");
    }

    /// 只是换了排版（语义相同）不算改动——红点不该白闪，基线照常推进。
    #[test]
    fn reformatted_but_equal_content_is_not_a_change() {
        let tmp = TmpDir::new("reformat");
        let file = tmp.config();
        let cur = cfg_named("v1");
        fs::write(&file, serde_json::to_string(&cur).unwrap()).unwrap();
        let mut w = ConfigWatcher::new(&file);

        fs::write(&file, serde_json::to_string_pretty(&cur).unwrap()).unwrap();
        assert!(w.poll(&file, &cur).is_none(), "排版不同、配置相同 → 不算改动");

        // 基线已推进到新指纹：之后的真改动仍能检出。
        match write_and_poll(&mut w, &file, &cur, &cfg_named("v2")) {
            Some(Change::Reloaded(cfg)) => assert_eq!(name_of(&cfg), "v2"),
            other => panic!("排版变化后应仍能检出真改动：{other:?}"),
        }
    }

    /// 坏内容（编辑器写到一半）：只提示一次、不采纳、不动磁盘；修好后自动重新采纳。
    #[test]
    fn broken_content_reports_once_and_keeps_disk_untouched() {
        let tmp = TmpDir::new("broken");
        let file = tmp.config();
        let cur = cfg_named("v1");
        fs::write(&file, serde_json::to_string(&cur).unwrap()).unwrap();
        let mut w = ConfigWatcher::new(&file);

        let broken = "{ \"shortcuts\": [ 这不是 JSON";
        fs::write(&file, broken).unwrap();
        assert!(matches!(w.poll(&file, &cur), Some(Change::Unreadable(_))), "坏内容要提示");
        assert!(w.poll(&file, &cur).is_none(), "同一份坏内容只提示一次");
        assert_eq!(fs::read_to_string(&file).unwrap(), broken, "监听路径绝不改磁盘");

        match write_and_poll(&mut w, &file, &cur, &cfg_named("v2")) {
            Some(Change::Reloaded(cfg)) => assert_eq!(name_of(&cfg), "v2"),
            other => panic!("修好后应重新采纳：{other:?}"),
        }
    }

    /// 文件消失：保持现状（不清空内存配置、不报错）；再出现时按改动处理。
    #[test]
    fn disappearing_file_keeps_current_config() {
        let tmp = TmpDir::new("disappear");
        let file = tmp.config();
        let cur = cfg_named("v1");
        fs::write(&file, serde_json::to_string(&cur).unwrap()).unwrap();
        let mut w = ConfigWatcher::new(&file);

        fs::remove_file(&file).unwrap();
        assert!(w.poll(&file, &cur).is_none(), "文件没了不该当作空配置采纳");
        assert!(w.poll(&file, &cur).is_none(), "也不该反复报");

        // 恢复备份 / 同步客户端补回文件：按外部改动采纳。
        match write_and_poll(&mut w, &file, &cur, &cfg_named("restored")) {
            Some(Change::Reloaded(cfg)) => assert_eq!(name_of(&cfg), "restored"),
            other => panic!("文件重新出现应被采纳：{other:?}"),
        }
    }
}
