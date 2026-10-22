//! Windows 圈选截图 / 显示器亮度（EXT-22）。
//!
//! 抓图只用 GDI：`BitBlt` 见屏幕；图片剪贴板用 `arboard`。圈选截图是**先冻结整块虚拟屏、
//! 再把用户画过的圈与包围盒画上去**：浮层本身不进图，等左键确认期间画面也不会变。
//!
//! 全部函数都在动作执行线程上跑（不是钩子回调），可以弹窗、阻塞等用户确认。

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
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection, CreatePen, DeleteDC, DeleteObject,
    EndPaint, GetDC, GetStockObject, MonitorFromPoint, Polyline, ReleaseDC, SelectObject,
    StretchDIBits, UpdateWindow, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
    HBRUSH, HDC, HGDIOBJ, MONITOR_DEFAULTTOPRIMARY, NULL_BRUSH, PAINTSTRUCT, PS_SOLID, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetSystemMetrics, LoadCursorW, PostQuitMessage, RegisterClassW, SetCursor, SetForegroundWindow,
    SetWindowLongPtrW, ShowWindow, TranslateMessage, GWLP_USERDATA, IDC_CROSS, MSG,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_SHOW, WM_CLOSE,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_PAINT, WM_RBUTTONDOWN, WM_SETCURSOR, WM_SYSKEYDOWN,
    WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
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
        // **先拷贝再删 DIB**：`DeleteObject` 之后 `bits` 指向的像素内存就没了，
        // 在删除之后读等于读已释放内存（截图发黑 / 花屏）。
        let rgba = if ok && !bits.is_null() {
            let len = (w as usize) * (h as usize) * 4;
            let bgra = std::slice::from_raw_parts(bits as *const u8, len);
            let mut rgba = Vec::with_capacity(len);
            for px in bgra.chunks_exact(4) {
                rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
            Some(rgba)
        } else {
            None
        };
        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if !ok {
            return Err("截取屏幕失败（BitBlt）".into());
        }
        let Some(rgba) = rgba else {
            return Err("截图缓冲区为空".into());
        };
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

// ---------------------------------------------------------------------------
// 圈选确认浮层（先冻结整屏，再从冻结帧裁剪）
// ---------------------------------------------------------------------------

struct LassoState {
    image: Image,
    /// 轨迹点（浮层本地坐标）。
    points: Vec<(i32, i32)>,
    /// 包围盒（浮层本地坐标）。
    rect: (i32, i32, i32, i32),
    /// 左键已按下（抬起才算确认，避免把 up 漏给下面的窗口）。
    armed: bool,
    /// 左键抬起 = 确认截图。
    confirmed: bool,
    done: bool,
}

static REGION_CLASS: Once = Once::new();

unsafe extern "system" fn lasso_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let p = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWLP_USERDATA)
        as *mut LassoState;
    if p.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let st = &mut *p;
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            draw_lasso(hdc, &st.image, &st.points, st.rect);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            st.armed = true;
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if st.armed {
                st.armed = false;
                st.confirmed = true;
                st.done = true;
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

fn draw_lasso(hdc: HDC, img: &Image, points: &[(i32, i32)], rect: (i32, i32, i32, i32)) {
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
        let pen = CreatePen(PS_SOLID, 2, COLORREF(0x00ff9c4c));
        let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
        let brush = HBRUSH(GetStockObject(NULL_BRUSH).0);
        let old_brush = SelectObject(hdc, HGDIOBJ(brush.0));
        let (l, t, r, b) = rect;
        let _ = windows::Win32::Graphics::Gdi::Rectangle(hdc, l, t, r, b);
        let pts: Vec<POINT> = points.iter().map(|&(x, y)| POINT { x, y }).collect();
        if pts.len() >= 2 {
            let _ = Polyline(hdc, &pts);
        }
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }
}

/// 冻结整块虚拟屏，画出用户画的圈与包围盒，等左键确认。
/// `Ok(None)` = 用户按 Esc / 右键取消（或轨迹不成圈）。
pub fn capture_lasso(points: &[(i32, i32)]) -> Result<Option<Image>, String> {
    let Some((l, t, r, b)) = kada_core::lasso_bbox(points) else {
        return Ok(None);
    };
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
                lpfnWndProc: Some(lasso_proc),
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
                lpszClassName: PCWSTR(wide("KadaCaptureRegion").as_ptr()),
                ..Default::default()
            };
            RegisterClassW(&cls);
        });
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let local: Vec<(i32, i32)> = points.iter().map(|&(x, y)| (x - vx, y - vy)).collect();
        let local_rect = (l - vx, t - vy, r - vx, b - vy);
        let mut st = Box::new(LassoState {
            image: frozen,
            points: local,
            rect: local_rect,
            armed: false,
            confirmed: false,
            done: false,
        });
        let ptr = &mut *st as *mut LassoState as *const c_void;
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
        .map_err(|e| format!("创建框选确认浮层失败：{e}"))?;
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
        let LassoState { image, confirmed, .. } = *st;
        Ok(confirmed.then(|| crop(&image, local_rect.0, local_rect.1, local_rect.2, local_rect.3)))
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
// 输出：图片剪贴板
// ---------------------------------------------------------------------------

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



