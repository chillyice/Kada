#![windows_subsystem = "windows"]

mod actions;
mod config;
mod gesture;

use config::{Action, Config};
use gesture::State;

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_TRAY: u32 = WM_APP + 1;
const TIMER_TICK: usize = 1;
const TIMER_OVERLAY: usize = 2;

const ID_GESTURES: usize = 100;
const ID_WHEEL: usize = 101;
const ID_EDGE: usize = 102;
const ID_CORNERS: usize = 103;
const ID_TRACE: usize = 104;
const ID_RELOAD: usize = 110;
const ID_OPEN_CONFIG: usize = 111;
const ID_OPEN_FOLDER: usize = 112;
const ID_EXIT: usize = 120;

static RUNTIME: OnceLock<Arc<Runtime>> = OnceLock::new();

fn rt() -> Option<&'static Arc<Runtime>> {
    RUNTIME.get()
}

/// 仅用于本机自测：允许处理 `SendInput` 合成事件，但永远忽略我们自己注入的鼠标事件。
fn selftest() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("KADA_PRIME_SELFTEST").is_some())
}

struct Runtime {
    config_path: PathBuf,
    config: Mutex<Config>,
    state: Mutex<State>,
    log: Mutex<File>,

    enabled: AtomicBool,
    wheel: AtomicBool,
    edge: AtomicBool,
    corners: AtomicBool,
    trace: AtomicBool,
    start_distance: AtomicI32,
    min_segment: AtomicI32,
    timeout_ms: AtomicU64,
    edge_width: AtomicI32,
    corner_size: AtomicI32,

    hidden: AtomicIsize,
    overlay: AtomicIsize,
    mouse_hook: AtomicIsize,
    overlay_shown: AtomicBool,

    mem_dc: AtomicIsize,
    mem_bmp: AtomicIsize,
    mem_w: AtomicI32,
    mem_h: AtomicI32,
}

impl Runtime {
    fn new(cfg: Config, config_path: PathBuf, log_path: PathBuf) -> io::Result<Self> {
        let log = OpenOptions::new().create(true).append(true).open(log_path)?;
        let rt = Self {
            config_path,
            config: Mutex::new(cfg),
            state: Mutex::new(State::default()),
            log: Mutex::new(log),
            enabled: AtomicBool::new(false),
            wheel: AtomicBool::new(false),
            edge: AtomicBool::new(false),
            corners: AtomicBool::new(false),
            trace: AtomicBool::new(true),
            start_distance: AtomicI32::new(22),
            min_segment: AtomicI32::new(16),
            timeout_ms: AtomicU64::new(1500),
            edge_width: AtomicI32::new(3),
            corner_size: AtomicI32::new(4),
            hidden: AtomicIsize::new(0),
            overlay: AtomicIsize::new(0),
            mouse_hook: AtomicIsize::new(0),
            overlay_shown: AtomicBool::new(false),
            mem_dc: AtomicIsize::new(0),
            mem_bmp: AtomicIsize::new(0),
            mem_w: AtomicI32::new(0),
            mem_h: AtomicI32::new(0),
        };
        rt.refresh_flags();
        Ok(rt)
    }

    fn refresh_flags(&self) {
        let cfg = self.config.lock().unwrap();
        self.enabled.store(cfg.settings.enabled, Ordering::Relaxed);
        self.wheel.store(cfg.wheel.enabled, Ordering::Relaxed);
        self.edge.store(cfg.edge.enabled, Ordering::Relaxed);
        self.corners.store(cfg.corners.enabled, Ordering::Relaxed);
        self.trace.store(cfg.settings.trace, Ordering::Relaxed);
        self.start_distance
            .store(cfg.settings.start_distance.max(1), Ordering::Relaxed);
        self.min_segment
            .store(cfg.settings.min_segment.max(1), Ordering::Relaxed);
        self.timeout_ms
            .store(cfg.settings.timeout_ms.max(50), Ordering::Relaxed);
        self.edge_width
            .store(cfg.edge.width.max(1), Ordering::Relaxed);
        self.corner_size
            .store(cfg.corners.size.max(1), Ordering::Relaxed);
    }

    fn apply(&self, cfg: Config) {
        *self.config.lock().unwrap() = cfg;
        self.refresh_flags();
        *self.state.lock().unwrap() = State::default();
    }

    fn save(&self) {
        let cfg = self.config.lock().unwrap().clone();
        if let Err(e) = config::save(&self.config_path, &cfg) {
            self.log(&format!("保存配置失败：{e}"));
        }
    }

    fn gesture_count(&self) -> usize {
        self.config.lock().unwrap().enabled_gesture_count()
    }

    fn log(&self, msg: &str) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Ok(mut f) = self.log.lock() {
            let _ = writeln!(f, "[{ts}] {msg}");
            let _ = f.flush();
        }
    }

    fn overlay_hwnd(&self) -> HWND {
        HWND(self.overlay.load(Ordering::Relaxed) as *mut c_void)
    }

    fn show_overlay(&self) {
        let hwnd = self.overlay_hwnd();
        if hwnd.0.is_null() {
            return;
        }
        if self.overlay_shown.swap(true, Ordering::Relaxed) {
            return;
        }
        let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1);
        let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1);
        self.ensure_backbuffer(vw, vh);
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                vx,
                vy,
                vw,
                vh,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetTimer(Some(hwnd), TIMER_OVERLAY, 16, None);
        }
    }

    fn hide_overlay(&self) {
        let hwnd = self.overlay_hwnd();
        if hwnd.0.is_null() {
            return;
        }
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_OVERLAY);
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
        self.overlay_shown.store(false, Ordering::Relaxed);
    }

    fn ensure_backbuffer(&self, w: i32, h: i32) {
        if w <= 0 || h <= 0 {
            return;
        }
        if self.mem_dc.load(Ordering::Relaxed) != 0
            && self.mem_w.load(Ordering::Relaxed) == w
            && self.mem_h.load(Ordering::Relaxed) == h
        {
            return;
        }
        self.free_backbuffer();
        unsafe {
            let screen = GetDC(None);
            if screen.0.is_null() {
                return;
            }
            let dc = CreateCompatibleDC(Some(screen));
            let bmp = CreateCompatibleBitmap(screen, w, h);
            let _ = ReleaseDC(None, screen);
            if dc.0.is_null() || bmp.0.is_null() {
                if !dc.0.is_null() {
                    let _ = DeleteDC(dc);
                }
                if !bmp.0.is_null() {
                    let _ = DeleteObject(HGDIOBJ(bmp.0));
                }
                return;
            }
            SelectObject(dc, HGDIOBJ(bmp.0));
            self.mem_dc.store(dc.0 as isize, Ordering::Relaxed);
            self.mem_bmp.store(bmp.0 as isize, Ordering::Relaxed);
            self.mem_w.store(w, Ordering::Relaxed);
            self.mem_h.store(h, Ordering::Relaxed);
        }
    }

    fn free_backbuffer(&self) {
        let dc = self.mem_dc.swap(0, Ordering::Relaxed);
        let bmp = self.mem_bmp.swap(0, Ordering::Relaxed);
        self.mem_w.store(0, Ordering::Relaxed);
        self.mem_h.store(0, Ordering::Relaxed);
        unsafe {
            if bmp != 0 {
                let _ = DeleteObject(HGDIOBJ(bmp as *mut c_void));
            }
            if dc != 0 {
                let _ = DeleteDC(HDC(dc as *mut c_void));
            }
        }
    }

    fn set_result(&self, text: &str, ms: u64) {
        let active = {
            let mut s = self.state.lock().unwrap();
            s.result = text.to_string();
            s.result_until = Some(Instant::now() + Duration::from_millis(ms));
            s.active
        };
        if active {
            self.show_overlay();
        }
    }

    fn begin_gesture(&self, pt: (i32, i32)) -> bool {
        if !self.enabled.load(Ordering::Relaxed) {
            return false;
        }
        let arm = {
            let cfg = self.config.lock().unwrap();
            cfg.should_arm()
        };
        if !arm {
            return false;
        }
        self.state.lock().unwrap().begin(pt);
        if selftest() {
            self.log(&format!("selftest: 右键按下 @{pt:?}"));
        }
        true
    }

    fn move_gesture(&self, pt: (i32, i32)) {
        let sd = self.start_distance.load(Ordering::Relaxed);
        let newly = {
            let mut s = self.state.lock().unwrap();
            s.move_to(pt, sd)
        };
        if newly {
            if selftest() {
                self.log("selftest: 越过起手距离，开始收集轨迹");
            }
            if self.trace.load(Ordering::Relaxed) {
                self.show_overlay();
            }
        }
    }

    fn tick(&self) {
        let timeout = Duration::from_millis(self.timeout_ms.load(Ordering::Relaxed));
        let hide = {
            let mut s = self.state.lock().unwrap();
            let now = Instant::now();
            if s.right_down
                && s.active
                && !s.cancelled
                && now.duration_since(s.last_move) > timeout
            {
                s.cancelled = true;
                s.result = "超时，已放弃".into();
                s.result_until = Some(now + Duration::from_millis(700));
            }
            let expired = s
                .result_until
                .map(|t| now >= t)
                .unwrap_or(false);
            let hide = !s.right_down && expired;
            if hide {
                s.result.clear();
                s.result_until = None;
            }
            hide
        };
        if hide {
            self.hide_overlay();
        }
    }

    fn fire(&self, actions: Vec<Action>, label: &str) {
        if actions.is_empty() {
            return;
        }
        let hwnd = self.hidden.load(Ordering::Relaxed);
        self.log(&format!("触发：{label}（{} 个动作）", actions.len()));
        std::thread::spawn(move || {
            let notify = move |text: &str| balloon(hwnd, text);
            actions::execute(&actions, &notify);
        });
    }

    fn wheel_event(&self, delta: i16, pt: (i32, i32)) -> bool {
        let right_down = self.state.lock().unwrap().right_down;
        if right_down {
            if !self.wheel.load(Ordering::Relaxed) {
                return false;
            }
            let actions = {
                let cfg = self.config.lock().unwrap();
                if delta > 0 {
                    cfg.wheel.up.clone()
                } else {
                    cfg.wheel.down.clone()
                }
            };
            self.state.lock().unwrap().wheel_used = true;
            self.fire(actions, if delta > 0 { "右键+滚轮上" } else { "右键+滚轮下" });
            return true;
        }

        if !self.edge.load(Ordering::Relaxed) {
            return false;
        }
        let (side, width) = {
            let cfg = self.config.lock().unwrap();
            (cfg.edge.side.clone(), cfg.edge.width.max(1))
        };
        let Some(rect) = monitor_rect(pt) else {
            return false;
        };
        let at_edge = match side.as_str() {
            "right" => pt.0 >= rect.right - width,
            "top" => pt.1 <= rect.top + width,
            "bottom" => pt.1 >= rect.bottom - width,
            _ => pt.0 <= rect.left + width,
        };
        if !at_edge {
            return false;
        }
        let allowed = {
            let mut s = self.state.lock().unwrap();
            let now = Instant::now();
            let allowed = s
                .last_edge_fire
                .map(|t| now.duration_since(t) >= Duration::from_millis(150))
                .unwrap_or(true);
            if allowed {
                s.last_edge_fire = Some(now);
            }
            allowed
        };
        if !allowed {
            return false;
        }
        let actions = {
            let cfg = self.config.lock().unwrap();
            if delta > 0 {
                cfg.edge.up.clone()
            } else {
                cfg.edge.down.clone()
            }
        };
        self.fire(actions, "边缘滚轮");
        true
    }

    fn check_corners(&self, pt: (i32, i32)) {
        if !self.corners.load(Ordering::Relaxed) {
            return;
        }
        if self.state.lock().unwrap().right_down {
            return;
        }
        let Some(rect) = monitor_rect(pt) else {
            return;
        };
        let size = self.corner_size.load(Ordering::Relaxed);
        let inside = [
            pt.0 <= rect.left + size && pt.1 <= rect.top + size,
            pt.0 >= rect.right - size && pt.1 <= rect.top + size,
            pt.0 <= rect.left + size && pt.1 >= rect.bottom - size,
            pt.0 >= rect.right - size && pt.1 >= rect.bottom - size,
        ];
        let fire = {
            let mut s = self.state.lock().unwrap();
            let now = Instant::now();
            let mut fire = None;
            for (i, &is_inside) in inside.iter().enumerate() {
                if is_inside && !s.corner_inside[i] {
                    let ok = s
                        .last_corner_fire
                        .map(|t| now.duration_since(t) >= Duration::from_millis(700))
                        .unwrap_or(true);
                    if ok {
                        s.last_corner_fire = Some(now);
                        fire = Some(i);
                    }
                }
                s.corner_inside[i] = is_inside;
            }
            fire
        };
        let Some(i) = fire else {
            return;
        };
        let (label, actions) = {
            let cfg = self.config.lock().unwrap();
            match i {
                0 => ("左上角", cfg.corners.top_left.clone()),
                1 => ("右上角", cfg.corners.top_right.clone()),
                2 => ("左下角", cfg.corners.bottom_left.clone()),
                _ => ("右下角", cfg.corners.bottom_right.clone()),
            }
        };
        self.fire(actions, label);
    }
}

fn update_config(rt: &Runtime, f: impl FnOnce(&mut Config)) {
    {
        let mut cfg = rt.config.lock().unwrap();
        f(&mut cfg);
    }
    rt.refresh_flags();
    rt.save();
    *rt.state.lock().unwrap() = State::default();
}

fn handle_release(rt: Arc<Runtime>, pt: (i32, i32)) {
    let (active, wheel_used, cancelled, points, min_segment) = {
        let mut s = rt.state.lock().unwrap();
        s.finish(pt);
        (
            s.active,
            s.wheel_used,
            s.cancelled,
            s.points.clone(),
            rt.min_segment.load(Ordering::Relaxed),
        )
    };

    if wheel_used {
        return;
    }
    if selftest() {
        rt.log(&format!(
            "selftest: 右键抬起 active={active} cancelled={cancelled} points={}",
            points.len()
        ));
    }
    if !active || cancelled {
        if cancelled {
            rt.set_result("超时，已放弃", 700);
        }
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(10));
            actions::replay_right_click();
        });
        return;
    }

    std::thread::spawn(move || {
        rt.set_result("识别中…", 1500);
        let pattern = gesture::pattern_from_points(&points, min_segment);
        if pattern.is_empty() {
            rt.set_result("未识别", 700);
            actions::replay_right_click();
            return;
        }
        let matched = {
            let cfg = rt.config.lock().unwrap();
            gesture::find_gesture(&cfg, &pattern).cloned()
        };
        match matched {
            Some(g) => {
                rt.log(&format!("手势 {pattern} -> {}（{} 个动作）", g.name, g.actions.len()));
                rt.set_result(&format!("{}  {pattern}", g.name), 1100);
                let hwnd = rt.hidden.load(Ordering::Relaxed);
                let notify = move |text: &str| balloon(hwnd, text);
                actions::execute(&g.actions, &notify);
            }
            None => {
                rt.log(&format!("未配置手势：{pattern}"));
                rt.set_result(&format!("未配置：{pattern}"), 900);
                actions::replay_right_click();
            }
        }
    });
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        if let Some(rt) = rt() {
            let ms = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            // 注入事件一律放行：右键回放、动作注入都不会被自己再吃一次。
            // 自测模式额外放行外部合成事件，但带我们标记的注入仍然忽略。
            let ours = ms.dwExtraInfo == actions::MOUSE_MARKER;
            if !ours && (ms.flags & LLMHF_INJECTED == 0 || selftest()) {
                let pt = (ms.pt.x, ms.pt.y);
                match wparam.0 as u32 {
                    WM_RBUTTONDOWN => {
                        if rt.begin_gesture(pt) {
                            return LRESULT(1);
                        }
                    }
                    WM_RBUTTONUP => {
                        let armed = rt.state.lock().unwrap().right_down;
                        if armed {
                            handle_release(rt.clone(), pt);
                            return LRESULT(1);
                        }
                    }
                    WM_MOUSEMOVE => {
                        rt.move_gesture(pt);
                        rt.check_corners(pt);
                    }
                    WM_MOUSEWHEEL => {
                        let delta = ((ms.mouseData >> 16) as u16) as i16;
                        if rt.wheel_event(delta, pt) {
                            return LRESULT(1);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let hook = rt()
        .map(|r| r.mouse_hook.load(Ordering::Relaxed))
        .unwrap_or(0);
    let hhook = if hook == 0 {
        None
    } else {
        Some(HHOOK(hook as *mut c_void))
    };
    unsafe { CallNextHookEx(hhook, code, wparam, lparam) }
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_overlay(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_TIMER => {
            if wparam.0 == TIMER_OVERLAY {
                let keep = match rt() {
                    Some(rt) => {
                        let mut s = rt.state.lock().unwrap();
                        let now = Instant::now();
                        let keep = s.active
                            && (s.right_down || s.result_until.map(|t| now <= t).unwrap_or(false));
                        if !keep {
                            s.result.clear();
                            s.result_until = None;
                        }
                        keep
                    }
                    None => false,
                };
                if keep {
                    unsafe {
                        let _ = InvalidateRect(Some(hwnd), None, true);
                    }
                } else if let Some(rt) = rt() {
                    rt.hide_overlay();
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn paint_overlay(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
    if hdc.0.is_null() {
        return;
    }
    let Some(rt) = rt() else {
        unsafe {
            let _ = EndPaint(hwnd, &ps);
        }
        return;
    };
    let (points, result) = {
        let s = rt.state.lock().unwrap();
        let now = Instant::now();
        let visible = s.active
            && (s.right_down || s.result_until.map(|t| now <= t).unwrap_or(false));
        if !visible {
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            return;
        }
        (s.points.clone(), s.result.clone())
    };
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    let mem = HDC(rt.mem_dc.load(Ordering::Relaxed) as *mut c_void);
    let target = if mem.0.is_null() { hdc } else { mem };

    unsafe {
        let black = HBRUSH(GetStockObject(BLACK_BRUSH).0);
        FillRect(target, &rc, black);
        SetBkMode(target, TRANSPARENT);

        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);

        if points.len() >= 2 && rt.trace.load(Ordering::Relaxed) {
            let pts: Vec<POINT> = points
                .iter()
                .map(|&(x, y)| POINT { x: x - vx, y: y - vy })
                .collect();
            let dark = CreatePen(PS_SOLID, 6, rgb(18, 18, 18));
            let old = SelectObject(target, HGDIOBJ(dark.0));
            let _ = Polyline(target, &pts);
            SelectObject(target, old);
            let _ = DeleteObject(HGDIOBJ(dark.0));

            let orange = CreatePen(PS_SOLID, 3, rgb(238, 125, 70));
            let old = SelectObject(target, HGDIOBJ(orange.0));
            let _ = Polyline(target, &pts);
            SelectObject(target, old);
            let _ = DeleteObject(HGDIOBJ(orange.0));
        }

        if !result.is_empty() {
            let (lx, ly) = points.last().copied().unwrap_or((vx + 40, vy + 40));
            let x = (lx - vx + 18).min((rc.right - 300).max(8));
            let y = (ly - vy - 44).max(8);
            let font = CreateFontW(
                -26,
                0,
                0,
                0,
                FW_BOLD.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI"),
            );
            let old = SelectObject(target, HGDIOBJ(font.0));
            let text: Vec<u16> = result.encode_utf16().collect();
            SetTextColor(target, rgb(18, 18, 18));
            let _ = TextOutW(target, x + 2, y + 2, &text);
            SetTextColor(target, rgb(248, 248, 248));
            let _ = TextOutW(target, x, y, &text);
            SelectObject(target, old);
            let _ = DeleteObject(HGDIOBJ(font.0));
        }

        if !mem.0.is_null() {
            let _ = BitBlt(hdc, 0, 0, rc.right, rc.bottom, Some(mem), 0, 0, SRCCOPY);
        }
    }
    unsafe {
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn hidden_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            let event = (lparam.0 & 0xFFFF) as u32;
            match event {
                WM_LBUTTONUP | WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_TICK {
                if let Some(rt) = rt() {
                    rt.tick();
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn show_menu(hwnd: HWND) {
    let Some(rt) = rt() else {
        return;
    };
    let menu = match unsafe { CreatePopupMenu() } {
        Ok(m) => m,
        Err(_) => return,
    };
    let count = rt.gesture_count();
    let status = wide(&format!("Kada' · {count} 条手势"));
    unsafe {
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(status.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    }

    let check = |on: bool| {
        if on {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        }
    };
    unsafe {
        let _ = AppendMenuW(
            menu,
            check(rt.enabled.load(Ordering::Relaxed)),
            ID_GESTURES,
            w!("启用手势"),
        );
        let _ = AppendMenuW(
            menu,
            check(rt.wheel.load(Ordering::Relaxed)),
            ID_WHEEL,
            w!("右键 + 滚轮切换"),
        );
        let _ = AppendMenuW(
            menu,
            check(rt.edge.load(Ordering::Relaxed)),
            ID_EDGE,
            w!("屏幕边缘滚轮"),
        );
        let _ = AppendMenuW(
            menu,
            check(rt.corners.load(Ordering::Relaxed)),
            ID_CORNERS,
            w!("触发角"),
        );
        let _ = AppendMenuW(
            menu,
            check(rt.trace.load(Ordering::Relaxed)),
            ID_TRACE,
            w!("绘制轨迹"),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_RELOAD, w!("重新加载配置"));
        let _ = AppendMenuW(menu, MF_STRING, ID_OPEN_CONFIG, w!("编辑配置（记事本）"));
        let _ = AppendMenuW(menu, MF_STRING, ID_OPEN_FOLDER, w!("打开配置所在文件夹"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, w!("退出 Kada'"));
    }

    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
    }
    let cmd = unsafe {
        TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        )
    };
    unsafe {
        let _ = DestroyMenu(menu);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
    }

    let id = cmd.0 as usize;
    match id {
        ID_GESTURES => {
            update_config(rt, |c| c.settings.enabled = !c.settings.enabled);
            balloon(
                hwnd.0 as isize,
                if rt.enabled.load(Ordering::Relaxed) {
                    "手势已开启"
                } else {
                    "手势已暂停"
                },
            );
        }
        ID_WHEEL => {
            update_config(rt, |c| c.wheel.enabled = !c.wheel.enabled);
            balloon(
                hwnd.0 as isize,
                if rt.wheel.load(Ordering::Relaxed) {
                    "右键 + 滚轮已开启"
                } else {
                    "右键 + 滚轮已关闭"
                },
            );
        }
        ID_EDGE => {
            update_config(rt, |c| c.edge.enabled = !c.edge.enabled);
            balloon(
                hwnd.0 as isize,
                if rt.edge.load(Ordering::Relaxed) {
                    "屏幕边缘滚轮已开启"
                } else {
                    "屏幕边缘滚轮已关闭"
                },
            );
        }
        ID_CORNERS => {
            update_config(rt, |c| c.corners.enabled = !c.corners.enabled);
            balloon(
                hwnd.0 as isize,
                if rt.corners.load(Ordering::Relaxed) {
                    "触发角已开启"
                } else {
                    "触发角已关闭"
                },
            );
        }
        ID_TRACE => {
            update_config(rt, |c| c.settings.trace = !c.settings.trace);
            balloon(
                hwnd.0 as isize,
                if rt.trace.load(Ordering::Relaxed) {
                    "轨迹绘制已开启"
                } else {
                    "轨迹绘制已关闭"
                },
            );
        }
        ID_RELOAD => reload(rt, hwnd.0 as isize),
        ID_OPEN_CONFIG => open_config(rt),
        ID_OPEN_FOLDER => open_folder(rt),
        ID_EXIT => unsafe { PostQuitMessage(0) },
        _ => {}
    }
}

fn reload(rt: &Runtime, hwnd: isize) {
    let (cfg, warn) = config::load_or_create(&rt.config_path);
    rt.apply(cfg);
    if let Some(w) = warn {
        rt.log(&w);
        balloon(hwnd, &w);
    } else {
        balloon(hwnd, &format!("已重新加载：{} 条手势", rt.gesture_count()));
    }
    rt.log("配置已重新加载");
}

fn open_config(rt: &Runtime) {
    let params = wide(&format!("\"{}\"", rt.config_path.display()));
    unsafe {
        let _ = ShellExecuteW(
            None,
            w!("open"),
            w!("notepad.exe"),
            PCWSTR(params.as_ptr()),
            None,
            SW_SHOWNORMAL,
        );
    }
}

fn open_folder(rt: &Runtime) {
    let params = wide(&format!("/select,\"{}\"", rt.config_path.display()));
    unsafe {
        let _ = ShellExecuteW(
            None,
            w!("open"),
            w!("explorer.exe"),
            PCWSTR(params.as_ptr()),
            None,
            SW_SHOWNORMAL,
        );
    }
}

fn balloon(hwnd: isize, text: &str) {
    if hwnd == 0 {
        return;
    }
    let mut nid = base_nid(HWND(hwnd as *mut c_void));
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_INFO;
    nid.Anonymous.uTimeout = 4000;
    copy_wide(&mut nid.szInfoTitle, "Kada'");
    copy_wide(&mut nid.szInfo, text);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

fn base_nid(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW::default();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid
}

fn tray_add(hwnd: HWND, rt: &Runtime) {
    let icon = unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default();
    let mut nid = base_nid(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = icon;
    copy_wide(&mut nid.szTip, "Kada' — 鼠标手势 / 滚轮快捷键试验台");
    let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &nid) };
    if !ok.as_bool() {
        rt.log("托盘图标添加失败");
    }
}

fn tray_remove(hwnd: HWND) {
    let nid = base_nid(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

fn copy_wide<const N: usize>(dst: &mut [u16; N], s: &str) {
    let mut i = 0;
    for u in s.encode_utf16().take(N.saturating_sub(1)) {
        dst[i] = u;
        i += 1;
    }
    dst[i] = 0;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn rgb(r: u32, g: u32, b: u32) -> COLORREF {
    COLORREF(r | (g << 8) | (b << 16))
}

fn monitor_rect(pt: (i32, i32)) -> Option<RECT> {
    unsafe {
        let monitor = MonitorFromPoint(
            POINT { x: pt.0, y: pt.1 },
            MONITOR_DEFAULTTONEAREST,
        );
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            Some(info.rcMonitor)
        } else {
            None
        }
    }
}

fn register_windows(hinstance: HINSTANCE) -> Result<(HWND, HWND), String> {
    unsafe {
        let icon = LoadIconW(None, IDI_APPLICATION).unwrap_or_default();
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();

        let hidden_class = WNDCLASSW {
            style: WNDCLASS_STYLES(0),
            lpfnWndProc: Some(hidden_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: icon,
            hCursor: cursor,
            hbrBackground: HBRUSH::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: w!("KadaPrimeHidden"),
        };
        if RegisterClassW(&hidden_class) == 0 {
            return Err("注册隐藏窗口类失败".into());
        }
        let hidden = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("KadaPrimeHidden"),
            w!("Kada'"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance),
            None,
        )
        .map_err(|e| format!("创建隐藏窗口失败：{e}"))?;

        let overlay_class = WNDCLASSW {
            style: WNDCLASS_STYLES(0),
            lpfnWndProc: Some(overlay_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: icon,
            hCursor: cursor,
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: w!("KadaPrimeOverlay"),
        };
        if RegisterClassW(&overlay_class) == 0 {
            return Err("注册轨迹窗口类失败".into());
        }
        let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        let overlay = CreateWindowExW(
            ex,
            w!("KadaPrimeOverlay"),
            w!("Kada' trace"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(hinstance),
            None,
        )
        .map_err(|e| format!("创建轨迹窗口失败：{e}"))?;
        let _ = SetLayeredWindowAttributes(overlay, COLORREF(0), 255, LWA_COLORKEY);
        Ok((hidden, overlay))
    }
}

fn install_hook(rt: &Runtime) -> Result<HHOOK, String> {
    let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0) }
        .map_err(|e| format!("安装鼠标钩子失败：{e}"))?;
    rt.mouse_hook.store(hook.0 as isize, Ordering::Relaxed);
    Ok(hook)
}

fn fatal(msg: &str) {
    let text = wide(msg);
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            w!("Kada'"),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let mutex = unsafe { CreateMutexW(None, false, w!("KadaPrime.SingleInstance")) };
    if let Ok(handle) = &mutex {
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                MessageBoxW(
                    None,
                    w!("Kada' 已在运行，请查看系统托盘（双击图标看状态）。"),
                    w!("Kada'"),
                    MB_OK | MB_ICONINFORMATION,
                );
            }
            return;
        }
        let _keep_alive = handle;
    }

    if let Err(e) = run() {
        fatal(&e);
    }
}

fn run() -> Result<(), String> {
    let path = config::config_path();
    let (cfg, warning) = config::load_or_create(&path);
    let log_path = config::exe_dir().join("kada-prime.log");
    let rt = Arc::new(
        Runtime::new(cfg, path.clone(), log_path).map_err(|e| format!("打开日志失败：{e}"))?,
    );
    let _ = RUNTIME.set(rt.clone());
    rt.log("=== Kada' 启动 ===");
    rt.log(&format!("selftest={}", selftest()));

    let hmodule = unsafe { GetModuleHandleW(None) }.map_err(|e| format!("取模块句柄失败：{e}"))?;
    let hinstance = HINSTANCE(hmodule.0);
    let (hidden, overlay) = register_windows(hinstance)?;
    rt.hidden.store(hidden.0 as isize, Ordering::Relaxed);
    rt.overlay.store(overlay.0 as isize, Ordering::Relaxed);
    // 预建全屏双缓冲，避免第一次画手势时在钩子回调里临时分配几十 MB 位图。
    let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1);
    let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1);
    rt.ensure_backbuffer(vw, vh);

    tray_add(hidden, &rt);
    if let Some(w) = warning {
        rt.log(&w);
        balloon(hidden.0 as isize, &w);
    }

    let hook = install_hook(&rt)?;
    unsafe {
        SetTimer(Some(hidden), TIMER_TICK, 50, None);
    }
    rt.log(&format!(
        "已启用：手势 {} 条；配置：{}",
        rt.gesture_count(),
        path.display()
    ));
    balloon(
        hidden.0 as isize,
        "按住右键画手势：↑ 音量+ / ↓ 音量- / ← 后退 / → 前进 / DRUL 记事本。右键+滚轮切标签；托盘菜单可关。",
    );

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe {
        let _ = UnhookWindowsHookEx(hook);
        tray_remove(hidden);
        let _ = DestroyWindow(overlay);
        let _ = DestroyWindow(hidden);
    }
    rt.free_backbuffer();
    rt.log("=== Kada' 退出 ===");
    Ok(())
}
