//! Windows 截图 / 贴图 / 显示器亮度（EXT-20 / EXT-22）。
//!
//! 抓图只用 GDI：`BitBlt` 见屏幕、`PrintWindow` 见窗口；编码 PNG 用已在依赖树里的
//! `image`；图片剪贴板用 `arboard`。区域选择是**先冻结整块虚拟屏、再在浮层上框选**：
//! 浮层本身不进图，选择期间画面也不会变。
//!
//! 全部函数都在动作执行线程上跑（不是钩子回调），可以弹窗、阻塞等用户框选。

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::{null_mut};
use std::sync::{Mutex, Once};

use windows::core::PCWSTR;
use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, GetVCPFeatureAndVCPFeatureReply, SetVCPFeature,
    PHYSICAL_MONITOR,
};
use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection, CreatePen, DeleteDC, DeleteObject,
    EndPaint, GetDC, GetMonitorInfoW, GetStockObject, MonitorFromPoint, ReleaseDC, SelectObject,
    StretchDIBits, UpdateWindow, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
    HBRUSH, HDC, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTOPRIMARY, NULL_BRUSH, PAINTSTRUCT, PS_SOLID,
    SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::Storage::Xps::PrintWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
    GetSystemMetrics, GetWindowRect, LoadCursorW, PostQuitMessage, RegisterClassW, SetCursor,
    SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage,
    GWLP_USERDATA, IDC_ARROW, IDC_CROSS, IDC_SIZEALL, MSG, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SW_SHOW,
    WM_CLOSE, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_SYSKEYDOWN, WNDCLASSW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

use crate::{BrightnessOp, CapturedImage};

/// 一张抓到的图（RGBA8，行序从上到下）。
pub type Image = CapturedImage;

unsafe fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 抓指定屏幕区域（虚拟屏坐标，可为负）。
fn grab(x: i32, y: i32, w: i32, h: i32) -> Result<Image, String> {
    if w <= 0 || h <= 0 {
        return Err("要截取的区域是空的".into());
    }
    unsafe {
        let screen = GetDC(None);
        if screen.0.is_null() {
            return Err("取不到屏幕 DC".into());
        }
        let mem = CreateCompatibleDC(Some(screen));
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = null_mut();
        let dib = match CreateDIBSection(Some(mem), &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(d) => d,
            Err(e) => {
                let _ = DeleteDC(mem);
                ReleaseDC(None, screen);
                return Err(format!("创建截图缓冲区失败：{e}"));
            }
        };
        let old = SelectObject(mem, HGDIOBJ(dib.0));
        let ok = BitBlt(mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT).is_ok();
        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if !ok {
            return Err("截取屏幕失败（BitBlt）".into());
        }
        if bits.is_null() {
            return Err("截图缓冲区为空".into());
        }
        let len = (w as usize) * (h as usize) * 4;
        let bgra = std::slice::from_raw_parts(bits as *const u8, len);
        let mut rgba = Vec::with_capacity(len);
        for px in bgra.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        Ok(Image { width: w as u32, height: h as u32, rgba })
    }
}

/// 抓整块虚拟屏幕（多显示器拼合，含负坐标）。
pub fn capture_full_screen() -> Result<Image, String> {
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        grab(x, y, w, h)
    }
}

/// 抓一个窗口（`PrintWindow` 优先，失败回落窗口 DC 的 `BitBlt`）。
pub fn capture_window(hwnd: Option<isize>) -> Result<Image, String> {
    let Some(raw) = hwnd else {
        return Err("取不到触发那一刻的前台窗口（前台是 Kada 自己的窗口，或当前没有前台窗口）".into());
    };
    let hwnd = HWND(raw as *mut c_void);
    if hwnd.0.is_null() {
        return Err("前台窗口句柄无效".into());
    }
    unsafe {
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).map_err(|e| format!("取窗口位置失败：{e}"))?;
        let (x, y) = (rect.left, rect.top);
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        if w <= 0 || h <= 0 {
            return Err("目标窗口没有可见区域（已最小化？）".into());
        }
        if let Ok(img) = grab(x, y, w, h) {
            return Ok(img);
        }
        // 被遮挡时 BitBlt 可能截到别的窗口；换 PrintWindow 再试一次（DWM 下通常拿到窗口自身）。
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = null_mut();
        let dib = CreateDIBSection(Some(mem), &bi, DIB_RGB_COLORS, &mut bits, None, 0)
            .map_err(|e| format!("创建窗口截图缓冲区失败：{e}"))?;
        let old = SelectObject(mem, HGDIOBJ(dib.0));
        let ok = PrintWindow(hwnd, mem, windows::Win32::Storage::Xps::PRINT_WINDOW_FLAGS(2)).as_bool();
        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if !ok || bits.is_null() {
            return Err("截取窗口失败（窗口可能受保护或已关闭）".into());
        }
        let len = (w as usize) * (h as usize) * 4;
        let bgra = std::slice::from_raw_parts(bits as *const u8, len);
        let mut rgba = Vec::with_capacity(len);
        for px in bgra.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        Ok(Image { width: w as u32, height: h as u32, rgba })
    }
}

// ---------------------------------------------------------------------------
// 区域选择浮层（先冻结整屏，再从冻结帧裁剪）
// ---------------------------------------------------------------------------

struct RegionState {
    image: Image,
    origin: (i32, i32),
    dragging: bool,
    start: (i32, i32),
    current: (i32, i32),
    rect: Option<(i32, i32, i32, i32)>,
    done: bool,
}

static REGION_CLASS: Once = Once::new();

unsafe extern "system" fn region_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let p = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWLP_USERDATA)
        as *mut RegionState;
    if p.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let st = &mut *p;
    let x = (lp.0 & 0xffff) as u16 as i16 as i32;
    let y = ((lp.0 >> 16) & 0xffff) as u16 as i16 as i32;
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            draw_frozen(hdc, &st.image, st.dragging, st.start, st.current);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            st.dragging = true;
            st.start = (x, y);
            st.current = (x, y);
            SetCapture(hwnd);
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), None, false);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if st.dragging {
                st.current = (x, y);
                let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if st.dragging {
                st.dragging = false;
                let (l, t, r, b) = rect_of(st.start, st.current);
                if r - l >= 3 && b - t >= 3 {
                    st.rect = Some((l, t, r, b));
                }
                st.done = true;
                let _ = ReleaseCapture();
                let _ = DestroyWindow(hwnd);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            st.done = true;
            let _ = DestroyWindow(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if wp.0 as u16 == VK_ESCAPE.0 {
                st.done = true;
                let _ = DestroyWindow(hwnd);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            if let Ok(cur) = LoadCursorW(None, IDC_CROSS) {
                SetCursor(Some(cur));
            }
            LRESULT(1)
        }
        WM_CLOSE => {
            st.done = true;
            let _ = DestroyWindow(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn rect_of(a: (i32, i32), b: (i32, i32)) -> (i32, i32, i32, i32) {
    (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
}

fn draw_frozen(hdc: HDC, img: &Image, dragging: bool, start: (i32, i32), cur: (i32, i32)) {
    unsafe {
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: img.width as i32,
                biHeight: -(img.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // 冻结帧是 RGBA；GDI 的 32bpp DIB 读 BGRA，交换 R/B 再画。
        let mut bgra = img.rgba.clone();
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        StretchDIBits(
            hdc,
            0,
            0,
            img.width as i32,
            img.height as i32,
            0,
            0,
            img.width as i32,
            img.height as i32,
            Some(bgra.as_ptr() as *const c_void),
            &bi,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
        if dragging {
            let (l, t, r, b) = rect_of(start, cur);
            let pen = CreatePen(PS_SOLID, 2, COLORREF(0x00ff9c4c));
            let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
            let brush = HBRUSH(GetStockObject(NULL_BRUSH).0);
            let old_brush = SelectObject(hdc, HGDIOBJ(brush.0));
            let _ = windows::Win32::Graphics::Gdi::Rectangle(hdc, l, t, r, b);
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(HGDIOBJ(pen.0));
        }
    }
}

/// 冻结整块虚拟屏并让用户框选。`Ok(None)` = 用户按 Esc / 右键取消。
pub fn capture_region() -> Result<Option<Image>, String> {
    let frozen = capture_full_screen()?;
    unsafe {
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        REGION_CLASS.call_once(|| {
            let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let cls = WNDCLASSW {
                style: windows::Win32::UI::WindowsAndMessaging::CS_HREDRAW
                    | windows::Win32::UI::WindowsAndMessaging::CS_VREDRAW,
                lpfnWndProc: Some(region_proc),
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
                lpszClassName: PCWSTR(wide("KadaCaptureRegion").as_ptr()),
                ..Default::default()
            };
            RegisterClassW(&cls);
        });
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let mut st = Box::new(RegionState {
            image: frozen,
            origin: (vx, vy),
            dragging: false,
            start: (0, 0),
            current: (0, 0),
            rect: None,
            done: false,
        });
        let ptr = &mut *st as *mut RegionState as *const c_void;
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            PCWSTR(wide("KadaCaptureRegion").as_ptr()),
            PCWSTR(wide("Kada 截图").as_ptr()),
            WS_POPUP | WS_VISIBLE,
            vx,
            vy,
            vw,
            vh,
            None,
            None,
            Some(hinst),
            Some(ptr),
        )
        .map_err(|e| format!("创建区域选择浮层失败：{e}"))?;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        let _ = UpdateWindow(hwnd);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if st.done {
                break;
            }
        }
        let rect = st.rect;
        let base = st.image.clone();
        let _ = &st.origin;
        Ok(rect.map(|(l, t, r, b)| crop(&base, l, t, r, b)))
    }
}

fn crop(img: &Image, l: i32, t: i32, r: i32, b: i32) -> Image {
    let l = l.max(0) as u32;
    let t = t.max(0) as u32;
    let r = (r.max(0) as u32).min(img.width);
    let b = (b.max(0) as u32).min(img.height);
    let w = r.saturating_sub(l);
    let h = b.saturating_sub(t);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for row in t..b {
        let start = ((row * img.width + l) * 4) as usize;
        rgba.extend_from_slice(&img.rgba[start..start + (w * 4) as usize]);
    }
    Image { width: w, height: h, rgba }
}

// ---------------------------------------------------------------------------
// 贴图（PiP）：置顶、不激活、可拖拽，右键关闭
// ---------------------------------------------------------------------------

struct PinState {
    image: Image,
    win_w: i32,
    win_h: i32,
    dragging: bool,
    grab: POINT,
    origin: POINT,
}

static PIN_CLASS: Once = Once::new();

unsafe extern "system" fn pin_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let p = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWLP_USERDATA)
        as *mut PinState;
    if p.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let st = &mut *p;
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            draw_scaled(hdc, &st.image, st.win_w, st.win_h);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            st.dragging = true;
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            st.grab = pt;
            let mut wr = RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            st.origin = POINT { x: wr.left, y: wr.top };
            SetCapture(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if st.dragging {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let nx = st.origin.x + (pt.x - st.grab.x);
                let ny = st.origin.y + (pt.y - st.grab.y);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    nx,
                    ny,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            st.dragging = false;
            let _ = ReleaseCapture();
            LRESULT(0)
        }
        WM_RBUTTONDOWN | WM_RBUTTONUP => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_SETCURSOR => {
            if let Ok(cur) = LoadCursorW(None, if st.dragging { IDC_SIZEALL } else { IDC_ARROW }) {
                SetCursor(Some(cur));
            }
            LRESULT(1)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn draw_scaled(hdc: HDC, img: &Image, w: i32, h: i32) {
    unsafe {
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: img.width as i32,
                biHeight: -(img.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = img.rgba.clone();
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        StretchDIBits(
            hdc,
            0,
            0,
            w,
            h,
            0,
            0,
            img.width as i32,
            img.height as i32,
            Some(bgra.as_ptr() as *const c_void),
            &bi,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
}

/// 贴图：开一个置顶、不激活的窗口显示图片（拖拽移动，右键关闭）。`right`/`bottom`
/// 给出时按窗口位置放，否则放到光标所在处。
pub fn pin_image(img: Image) -> Result<(), String> {
    let w0 = img.width as i32;
    let h0 = img.height as i32;
    if w0 <= 0 || h0 <= 0 {
        return Err("贴图内容为空".into());
    }
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let work_w = (mi.rcWork.right - mi.rcWork.left).max(1);
        let work_h = (mi.rcWork.bottom - mi.rcWork.top).max(1);
        let scale = (work_w as f32 * 0.9 / w0 as f32)
            .min(work_h as f32 * 0.9 / h0 as f32)
            .min(1.0);
        let win_w = ((w0 as f32) * scale).max(1.0) as i32;
        let win_h = ((h0 as f32) * scale).max(1.0) as i32;
        let x = (pt.x - win_w / 2).clamp(mi.rcWork.left, mi.rcWork.right - win_w);
        let y = (pt.y - win_h / 2).clamp(mi.rcWork.top, mi.rcWork.bottom - win_h);

        PIN_CLASS.call_once(|| {
            let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let cls = WNDCLASSW {
                style: windows::Win32::UI::WindowsAndMessaging::CS_HREDRAW
                    | windows::Win32::UI::WindowsAndMessaging::CS_VREDRAW,
                lpfnWndProc: Some(pin_proc),
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: PCWSTR(wide("KadaPin").as_ptr()),
                ..Default::default()
            };
            RegisterClassW(&cls);
        });
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let mut st = Box::new(PinState {
            image: img,
            win_w,
            win_h,
            dragging: false,
            grab: POINT::default(),
            origin: POINT::default(),
        });
        let ptr = &mut *st as *mut PinState as *const c_void;
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            PCWSTR(wide("KadaPin").as_ptr()),
            PCWSTR(wide("Kada 贴图").as_ptr()),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            win_w,
            win_h,
            None,
            None,
            Some(hinst),
            Some(ptr),
        )
        .map_err(|e| format!("创建贴图窗口失败：{e}"))?;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        // 窗口在自己的消息循环里跑；动作线程不等它（关掉之前它一直置顶）。
        std::thread::spawn(move || {
            let _keep = st;
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        });
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 显示器亮度（DDC/CI，VCP 0x10）
// ---------------------------------------------------------------------------

/// 第一次调节前的原始亮度（主显示器；进程内缓存，退出即丢）。
static ORIGINAL_BRIGHTNESS: Mutex<Option<u32>> = Mutex::new(None);

fn primary_monitors() -> Result<(Vec<PHYSICAL_MONITOR>, u32), String> {
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        if mon.0.is_null() {
            return Err("取不到主显示器".into());
        }
        let mut count = 0u32;
        GetNumberOfPhysicalMonitorsFromHMONITOR(mon, &mut count)
            .map_err(|e| format!("枚举显示器失败：{e}"))?;
        if count == 0 {
            return Err("主显示器没有可用的物理显示器句柄".into());
        }
        let mut monitors = vec![PHYSICAL_MONITOR::default(); count as usize];
        GetPhysicalMonitorsFromHMONITOR(mon, &mut monitors)
            .map_err(|e| format!("取物理显示器失败：{e}"))?;
        Ok((monitors, count))
    }
}

fn read_primary() -> Result<(Vec<PHYSICAL_MONITOR>, u32, u32, u32), String> {
    let (monitors, count) = primary_monitors()?;
    unsafe {
        for m in &monitors {
            let mut cur = 0u32;
            let mut max = 0u32;
            if GetVCPFeatureAndVCPFeatureReply(
                m.hPhysicalMonitor,
                0x10,
                None,
                &mut cur,
                Some(&mut max),
            ) != 0
                && max > 0
            {
                return Ok((monitors, count, cur, max));
            }
        }
        let _ = DestroyPhysicalMonitors(&monitors);
    }
    Err("读不到主显示器亮度：显示器不支持 DDC/CI 或驱动不支持".into())
}

/// 调高 / 调低 / 恢复主显示器亮度。成功返回一句可直接展示的中文结果。
pub fn brightness(op: BrightnessOp, step: i64) -> Result<String, String> {
    if matches!(op, BrightnessOp::Restore) {
        let original = ORIGINAL_BRIGHTNESS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ok_or_else(|| {
                "还没有记录原始亮度：先执行一次「调高 / 调低」，本次运行内才能恢复".to_string()
            })?;
        let (monitors, count) = primary_monitors()?;
        let mut ok = false;
        unsafe {
            for m in &monitors {
                if SetVCPFeature(m.hPhysicalMonitor, 0x10, original) != 0 {
                    ok = true;
                    break;
                }
            }
            let _ = DestroyPhysicalMonitors(&monitors);
        }
        let _ = count;
        return if ok {
            Ok(format!("亮度已恢复到 {original}"))
        } else {
            Err("恢复亮度失败：显示器不支持 DDC/CI 或驱动不支持".into())
        };
    }

    let (monitors, count, current, max) = read_primary()?;
    let original = {
        let mut cache = ORIGINAL_BRIGHTNESS.lock().unwrap_or_else(|e| e.into_inner());
        *cache.get_or_insert(current)
    };
    let delta = step.unsigned_abs() as i64;
    let target = match op {
        BrightnessOp::Up => (current as i64 + delta).min(max as i64),
        BrightnessOp::Down => (current as i64 - delta).max(0),
        BrightnessOp::Restore => unreachable!(),
    } as u32;
    let mut ok = false;
    unsafe {
        for m in &monitors {
            if SetVCPFeature(m.hPhysicalMonitor, 0x10, target) != 0 {
                ok = true;
                break;
            }
        }
        let _ = DestroyPhysicalMonitors(&monitors);
    }
    let _ = count;
    if !ok {
        return Err("设置亮度失败：显示器不支持 DDC/CI 或驱动不支持".into());
    }
    Ok(format!(
        "亮度 {current} → {target}（原始 {original}，上限 {max}）"
    ))
}

// ---------------------------------------------------------------------------
// 输出：PNG 文件 / 图片剪贴板
// ---------------------------------------------------------------------------

/// 把图片编码成 PNG 并原子落盘（同目录临时文件 → rename；目标已存在时留一份 `.bak`）。
pub fn save_png(img: &Image, path: &str) -> Result<(), String> {
    use image::ImageFormat;
    let p = std::path::Path::new(path);
    if let Some(dir) = p.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败：{e}"))?;
        }
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    let rgba = image::RgbaImage::from_raw(img.width, img.height, img.rgba.clone())
        .ok_or_else(|| "截图数据尺寸不符".to_string())?;
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| format!("PNG 编码失败：{e}"))?;
    let tmp = p.with_extension(format!(
        "{}.tmp",
        p.extension().and_then(|e| e.to_str()).unwrap_or("png")
    ));
    std::fs::write(&tmp, buf.get_ref()).map_err(|e| format!("写临时文件失败：{e}"))?;
    if p.exists() {
        let bak = p.with_extension(format!(
            "{}.bak",
            p.extension().and_then(|e| e.to_str()).unwrap_or("png")
        ));
        let _ = std::fs::copy(p, bak);
    }
    std::fs::rename(&tmp, p).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("保存截图失败：{e}")
    })
}

/// 把图片写进系统剪贴板（`arboard` 的图片剪贴板，不是只支持文本）。
pub fn set_clipboard_image(img: &Image) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("打开剪贴板失败：{e}"))?;
    cb.set_image(arboard::ImageData {
        width: img.width as usize,
        height: img.height as usize,
        bytes: std::borrow::Cow::Borrowed(&img.rgba),
    })
    .map_err(|e| format!("写入图片剪贴板失败：{e}"))
}



