#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! 咔哒桌面壳。
//!
//! 配置驱动：启动时加载 JSON 配置 → 把平台钩子接入配置匹配 → 托盘常驻
//! （关窗不退出）。命令层承载 UI 的读写配置。
//!
//! 跨平台要点：事件管道用自有的 [`Ev`]，各平台钩子都把它喂给 [`decide`] /
//! [`Recorder`] 判定。非 Windows 尚无钩子实现，壳仍可编译运行（改键/
//! 快捷键/录制暂不可用，托盘与配置界面可用）。

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, WindowEvent};

use kada_core::{matches, Action, Config, Key, Modifier, RawEvent, Shortcut, Step};

/// 平台输入层。
#[cfg(target_os = "windows")]
mod input {
    //! Windows：全局低层键盘钩子（kada-hook）。
    pub use kada_hook::win::{simulate, start, Action as HookAction, HookHandle, KeyEvent};

    pub fn hooks_supported() -> bool {
        true
    }
}

#[cfg(not(target_os = "windows"))]
mod input {
    //! 非 Windows：输入层待实现（M4 Linux / M5 macOS）。占位类型保证壳可编译。
    /// 平台事件（无实现时不可构造）。
    pub enum KeyEvent {}

    pub struct HookHandle;

    pub fn hooks_supported() -> bool {
        false
    }
}

/// 跨平台键盘事件：各平台钩子统一喂给决策器/录制器。
#[derive(Clone, Debug)]
enum Ev {
    Down { key: Key, mods: BTreeSet<Modifier>, repeat: bool },
    Up { key: Key },
}

#[cfg(target_os = "windows")]
fn to_ev(ev: &input::KeyEvent) -> Ev {
    match ev {
        input::KeyEvent::Down { key, mods, repeat } => Ev::Down {
            key: *key,
            mods: mods.clone(),
            repeat: *repeat,
        },
        input::KeyEvent::Up { key, .. } => Ev::Up { key: *key },
    }
}

/// 运行时状态：钩子持有的配置 + 配置落盘路径。
struct KadaState {
    config: Arc<RwLock<Config>>,
    paused: Arc<AtomicBool>,
    /// 录制器：Some = 正在录制（此时快捷键/改键全暂停）。
    rec: Arc<Mutex<Option<Recorder>>>,
    file: PathBuf,
    _hook: Mutex<Option<input::HookHandle>>,
}

/// 单条事件的决定。
enum Outcome {
    /// 命中快捷键动作。
    Shortcut(Action),
    /// 命中改键：改发某个键。
    Replace(Key),
    /// 放行。
    Pass,
}

fn decide(ev: &Ev, cfg: &Config) -> Outcome {
    match ev {
        Ev::Down { key, mods, repeat } => {
            // 改键优先于快捷键：先消耗掉原生键，避免改键后键再触发快捷键。
            for r in &cfg.remaps {
                if !r.enabled {
                    continue;
                }
                if let (Ok(from), Ok(to)) = (r.from.parse::<Key>(), r.to.parse::<Key>()) {
                    if from == *key {
                        return Outcome::Replace(to);
                    }
                }
            }
            let raw = RawEvent { key: *key, mods: mods.clone(), pressed: true };
            for s in &cfg.shortcuts {
                if !s.enabled || *repeat {
                    continue;
                }
                for t in &s.triggers {
                    if let Ok(sc) = t.parse::<Shortcut>() {
                        if matches(&raw, &sc) {
                            return Outcome::Shortcut(s.action.clone());
                        }
                    }
                }
            }
            Outcome::Pass
        }
        Ev::Up { .. } => Outcome::Pass,
    }
}

/// 执行一个动作（异步跑，避免阻塞钩子回调）。
#[cfg(target_os = "windows")]
fn fire(action: Action) {
    std::thread::spawn(move || {
        if let Err(e) = run_action(&action) {
            eprintln!("动作执行失败: {e}");
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn fire(_action: Action) {
    // 无钩子即无触发入口，本分支不会运行。
}

/// 顺序执行一个动作。文本/按键走模拟输入，停顿按时间等待。
#[cfg(target_os = "windows")]
fn run_action(action: &Action) -> Result<(), String> {
    let play = |key: &str| key.parse::<Key>().map_err(|e| e.to_string());
    match action {
        Action::Text { text } => input::simulate::type_text(text).map_err(|e| e.to_string())?,
        Action::Sequence { steps } => {
            for step in steps {
                match step {
                    Step::Text { text } => {
                        input::simulate::type_text(text).map_err(|e| e.to_string())?
                    }
                    Step::Tap { key } => input::simulate::tap(play(key)?),
                    Step::Keys { keys } => {
                        let ks: Vec<Key> = keys
                            .iter()
                            .map(|k| k.parse::<Key>().map_err(|e| e.to_string()))
                            .collect::<Result<_, _>>()?;
                        // 全部按下 → 稍停 → 逆序松开
                        for k in &ks {
                            input::simulate::down(*k);
                        }
                        std::thread::sleep(Duration::from_millis(30));
                        for k in ks.iter().rev() {
                            input::simulate::up(*k);
                        }
                    }
                    Step::Down { key } => input::simulate::down(play(key)?),
                    Step::Up { key } => input::simulate::up(play(key)?),
                    Step::PauseMs { ms } => std::thread::sleep(Duration::from_millis(*ms as u64)),
                }
            }
        }
    }
    Ok(())
}

/// 宏录制器：把全局键盘事件转录成 [`Step`] 时间线。
/// (重复 down 折叠、停顿超 20ms 记 PauseMs、键名落盘为可读字符串)
struct Recorder {
    started: std::time::Instant,
    last_ms: u64,
    held: Vec<(Key, u64)>,
    steps: Vec<Step>,
}

impl Recorder {
    fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            last_ms: 0,
            held: vec![],
            steps: vec![],
        }
    }

    fn now(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// 事件 → 时间线；事件间时差超过阈值则补 PauseMs。
    fn push(&mut self, ev: &Ev) {
        let (key, down) = match ev {
            Ev::Down { key, repeat, .. } if !*repeat => (*key, true),
            Ev::Up { key, .. } => (*key, false),
            _ => return, // 重复 down 是按住，折叠
        };
        let ms = self.now();
        let gap = ms.saturating_sub(self.last_ms);
        if gap >= 20 {
            self.steps.push(Step::PauseMs { ms: gap });
        }
        self.last_ms = ms;
        if down {
            self.held.push((key, ms));
            self.steps.push(Step::Down { key: key_name(key).to_string() });
        } else {
            self.held.retain(|(k, _)| *k != key);
            self.steps.push(Step::Up { key: key_name(key).to_string() });
        }
    }

    /// 结束：补全仍在按住的键的 keyup，返回录制所得步骤。
    fn finish(mut self) -> Vec<Step> {
        let now = self.now();
        for (key, _) in self.held.drain(..) {
            let gap = now.saturating_sub(self.last_ms);
            if gap >= 20 {
                self.steps.push(Step::PauseMs { ms: gap });
            }
            self.last_ms = now;
            self.steps.push(Step::Up { key: key_name(key).to_string() });
        }
        self.steps
    }
}

fn key_name(k: Key) -> &'static str {
    kada_core::key_name(k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_transcribes_events() {
        let mut r = Recorder::new();
        // 模拟：按下 K → 弹起 K → 按下 Ctrl → 按下 C → 弹起 C → 弹起 Ctrl
        r.push(&Ev::Down { key: Key::K, mods: BTreeSet::new(), repeat: false });
        r.push(&Ev::Up { key: Key::K });
        r.push(&Ev::Down { key: Key::Control, mods: BTreeSet::new(), repeat: false });
        r.push(&Ev::Down { key: Key::C, mods: BTreeSet::new(), repeat: false });
        r.push(&Ev::Up { key: Key::C });
        r.push(&Ev::Up { key: Key::Control });
        let steps = r.finish();
        // 步骤以 Down/Up 交替呈现，且无重复 down 折叠问题
        assert!(steps.iter().any(|s| matches!(s, Step::Down { key } if key == "K")));
        assert!(steps.iter().any(|s| matches!(s, Step::Up { key } if key == "C")));
        // Ctrl 的重复 down 被折叠
        r = Recorder::new();
        r.push(&Ev::Down { key: Key::Control, mods: BTreeSet::new(), repeat: false });
        r.push(&Ev::Down { key: Key::Control, mods: BTreeSet::new(), repeat: true });
        r.push(&Ev::Up { key: Key::Control });
        let steps = r.finish();
        let downs = steps
            .iter()
            .filter(|s| matches!(s, Step::Down { key } if key == "Ctrl"))
            .count();
        assert_eq!(downs, 1, "重复 down 必须折叠");
    }
}

fn load_config(file: &PathBuf) -> Config {
    match fs::read_to_string(file) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("配置解析失败 {e}，已回退为默认空配置");
                Config::default()
            }
        },
        Err(_) => Config::default(),
    }
}

fn save_config(file: &PathBuf, cfg: &Config) -> Result<(), String> {
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(file, json).map_err(|e| e.to_string())
}

fn validate(cfg: &Config) -> Result<(), String> {
    for s in &cfg.shortcuts {
        for t in &s.triggers {
            t.parse::<Shortcut>().map_err(|e| format!("触发器「{t}」无效：{e}"))?;
        }
        s.action.validate()?;
    }
    for r in &cfg.remaps {
        r.from.parse::<Key>().map_err(|e| format!("改键来源「{}」无效：{e}", r.from))?;
        r.to.parse::<Key>().map_err(|e| format!("改键目标「{}」无效：{e}", r.to))?;
    }
    Ok(())
}

/// 读取当前配置。
#[tauri::command]
fn get_config(state: tauri::State<'_, KadaState>) -> Config {
    state.config.read().unwrap().clone()
}

/// 校验并保存配置（写入磁盘 + 即时生效，钩子无需重启）。
#[tauri::command]
fn set_config(state: tauri::State<'_, KadaState>, config: Config) -> Result<(), String> {
    validate(&config)?;
    save_config(&state.file, &config)?;
    *state.config.write().unwrap() = config;
    Ok(())
}

/// 暂停/恢复快捷键触发：录入组合键时暂停，避免自触发。
#[tauri::command]
fn set_paused(state: tauri::State<'_, KadaState>, paused: bool) {
    state.paused.store(paused, Ordering::Relaxed);
}

/// 开始录制宏：快捷键/改键随即暂停，所有按键进时间线。
#[tauri::command]
fn start_record(state: tauri::State<'_, KadaState>) -> Result<(), String> {
    if !input::hooks_supported() {
        return Err("当前平台暂不支持宏录制".into());
    }
    let mut g = state.rec.lock().unwrap();
    if g.is_some() {
        return Err("已在录制中".into());
    }
    *g = Some(Recorder::new());
    state.paused.store(true, Ordering::Relaxed);
    Ok(())
}

/// 停止录制，返回时间线步骤（可直接作为 Sequence 动作保存）。
#[tauri::command]
fn stop_record(state: tauri::State<'_, KadaState>) -> Result<Vec<Step>, String> {
    if !input::hooks_supported() {
        return Err("当前平台暂不支持宏录制".into());
    }
    let mut g = state.rec.lock().unwrap();
    let Some(rec) = g.take() else {
        return Err("没有正在进行的录制".into());
    };
    state.paused.store(false, Ordering::Relaxed);
    Ok(rec.finish())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_config,
            set_config,
            set_paused,
            start_record,
            stop_record
        ])
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let file = dir.join("config.json");

            let config = Arc::new(RwLock::new(load_config(&file)));
            let paused = Arc::new(AtomicBool::new(false));
            let rec = Arc::new(Mutex::new(None::<Recorder>));

            // 钩子接线：事件即时查表；（仅 Windows 有实现）
            #[cfg(target_os = "windows")]
            let hook_handle: Option<input::HookHandle> = Some(
                {
                    let cfg = config.clone();
                    let p = paused.clone();
                    let r = rec.clone();
                    input::start(move |ev: input::KeyEvent| {
                        let ev = to_ev(&ev);
                        // 录制中：所有事件进时间线、放行；快捷键/改键全暂停。
                        {
                            let mut g = r.lock().unwrap();
                            if let Some(recorder) = g.as_mut() {
                                recorder.push(&ev);
                                return input::HookAction::Allow;
                            }
                        }
                        if p.load(Ordering::Relaxed) {
                            return input::HookAction::Allow;
                        }
                        match decide(&ev, &cfg.read().unwrap()) {
                            Outcome::Shortcut(action) => {
                                fire(action);
                                input::HookAction::Block
                            }
                            Outcome::Replace(to) => input::HookAction::Replace(to),
                            Outcome::Pass => input::HookAction::Allow,
                        }
                    })
                }
                .map_err(|e: std::io::Error| e.to_string())?,
            );

            #[cfg(not(target_os = "windows"))]
            let hook_handle: Option<input::HookHandle> = None;

            let state = KadaState {
                config,
                paused,
                rec,
                file,
                _hook: Mutex::new(hook_handle),
            };
            app.manage(state);

            // 托盘：常驻后台，关窗不退出。
            let show_i = MenuItem::with_id(app, "show", "打开咔哒", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;
            let icon = app.default_window_icon().cloned();
            TrayIconBuilder::new()
                .icon(icon.unwrap())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            // 关窗 = 隐藏到托盘，应用继续跑。
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Kada");
}