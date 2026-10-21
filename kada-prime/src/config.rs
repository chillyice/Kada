use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub settings: Settings,
    pub gestures: Vec<Gesture>,
    pub wheel: WheelSwitch,
    pub edge: Edge,
    pub corners: Corners,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 手势总开关（托盘可切）。
    pub enabled: bool,
    /// 光标移动超过这个距离才算「开始画手势」（像素）。
    pub start_distance: i32,
    /// 手势途中多久没移动就放弃（毫秒）。
    pub timeout_ms: u64,
    /// 方向量化的一段最少多长（像素）；越大越不敏感。
    pub min_segment: i32,
    /// 是否绘制轨迹。
    pub trace: bool,
    /// 未配置的轨迹是否在日志 / 气泡里报出方向串（方便照抄进配置）。
    pub capture_unknown: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Gesture {
    pub name: String,
    /// 方向串：U/D/L/R 与 UR/DR/DL/UL，例如 "DRUL"。
    pub pattern: String,
    pub enabled: bool,
    pub actions: Vec<Action>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WheelSwitch {
    pub enabled: bool,
    pub up: Vec<Action>,
    pub down: Vec<Action>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Edge {
    pub enabled: bool,
    /// left / right / top / bottom
    pub side: String,
    /// 距边缘多少像素以内算「在边上」。
    pub width: i32,
    pub up: Vec<Action>,
    pub down: Vec<Action>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Corners {
    pub enabled: bool,
    /// 角区边长（像素）。
    pub size: i32,
    pub top_left: Vec<Action>,
    pub top_right: Vec<Action>,
    pub bottom_left: Vec<Action>,
    pub bottom_right: Vec<Action>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VolumeDir {
    Up,
    Down,
    Mute,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// 组合键，例如 "Ctrl+Alt+K" / "Alt+Left" / "VolumeUp"。
    Keys { keys: String },
    /// 用系统默认方式打开程序 / 文件 / 网址。
    Run { cmd: String },
    Volume { dir: VolumeDir },
    /// 右下角气泡提示（测试动作反馈）。
    Notify { text: String },
    /// 在当前光标位置点一下鼠标。
    Click { button: String },
    /// 滚轮：正数向上、负数向下。
    Scroll { dy: i32 },
    /// 相对移动光标。
    Move { dx: i32, dy: i32 },
    /// 逐字输入 Unicode 文本。
    Text { text: String },
    /// 停顿。
    Sleep { ms: u64 },
}

impl Default for Config {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            gestures: default_gestures(),
            wheel: WheelSwitch::default(),
            edge: Edge::default(),
            corners: Corners::default(),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            start_distance: 22,
            timeout_ms: 1500,
            min_segment: 16,
            trace: true,
            capture_unknown: true,
        }
    }
}

impl Default for WheelSwitch {
    fn default() -> Self {
        Self {
            enabled: true,
            up: vec![Action::Keys { keys: "Ctrl+Tab".into() }],
            down: vec![Action::Keys { keys: "Ctrl+Shift+Tab".into() }],
        }
    }
}

impl Default for Edge {
    fn default() -> Self {
        Self {
            enabled: true,
            side: "left".into(),
            width: 3,
            up: vec![Action::Volume { dir: VolumeDir::Up }],
            down: vec![Action::Volume { dir: VolumeDir::Down }],
        }
    }
}

impl Default for Corners {
    fn default() -> Self {
        Self {
            enabled: false,
            size: 4,
            top_left: vec![Action::Notify { text: "左上角".into() }],
            top_right: vec![Action::Notify { text: "右上角".into() }],
            bottom_left: vec![Action::Notify { text: "左下角".into() }],
            bottom_right: vec![Action::Notify { text: "右下角".into() }],
        }
    }
}

fn default_gestures() -> Vec<Gesture> {
    vec![
        gesture("音量 +", "U", vec![Action::Volume { dir: VolumeDir::Up }]),
        gesture("音量 −", "D", vec![Action::Volume { dir: VolumeDir::Down }]),
        gesture("后退", "L", vec![Action::Keys { keys: "Alt+Left".into() }]),
        gesture("前进", "R", vec![Action::Keys { keys: "Alt+Right".into() }]),
        gesture("复制", "DL", vec![Action::Keys { keys: "Ctrl+C".into() }]),
        gesture("粘贴", "DR", vec![Action::Keys { keys: "Ctrl+V".into() }]),
        gesture("显示桌面", "UD", vec![Action::Keys { keys: "Win+D".into() }]),
        gesture(
            "打开记事本",
            "DRUL",
            vec![Action::Run { cmd: "notepad.exe".into() }],
        ),
    ]
}

fn gesture(name: &str, pattern: &str, actions: Vec<Action>) -> Gesture {
    Gesture {
        name: name.into(),
        pattern: pattern.into(),
        enabled: true,
        actions,
    }
}

impl Config {
    /// 有启用的手势（或开启了「记录未配置手势」）才值得抢右键。
    pub fn should_arm(&self) -> bool {
        self.settings.enabled
            && (self.settings.capture_unknown || self.gestures.iter().any(|g| g.enabled))
    }

    pub fn enabled_gesture_count(&self) -> usize {
        self.gestures.iter().filter(|g| g.enabled).count()
    }
}

pub fn load_or_create(path: &Path) -> (Config, Option<String>) {
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Config>(&text) {
            Ok(cfg) => (cfg, None),
            Err(e) => (
                Config::default(),
                Some(format!("配置解析失败，先用默认配置：{e}")),
            ),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let cfg = Config::default();
            let warn = save(path, &cfg).err().map(|e| format!("写入默认配置失败：{e}"));
            (cfg, warn)
        }
        Err(e) => (
            Config::default(),
            Some(format!("读取配置失败，先用默认配置：{e}")),
        ),
    }
}

/// 原子写：临时文件 → rename 覆盖（Kada' 是测试软件，但仍然不原地写半截 JSON）。
pub fn save(path: &Path, cfg: &Config) -> io::Result<()> {
    let text = serde_json::to_string_pretty(cfg).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

pub fn config_path() -> PathBuf {
    exe_dir().join("config.json")
}

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_usable() {
        let cfg = Config::default();
        assert!(cfg.should_arm());
        assert!(cfg.enabled_gesture_count() >= 4);
        assert!(cfg.gestures.iter().all(|g| !g.pattern.is_empty()));
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let cfg: Config = serde_json::from_str(r#"{"settings":{"enabled":false}}"#).unwrap();
        assert!(!cfg.settings.enabled);
        assert_eq!(cfg.settings.start_distance, 22);
        assert!(!cfg.gestures.is_empty());
        assert!(cfg.wheel.enabled);
    }
}
