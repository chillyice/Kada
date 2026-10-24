//! Windows 圈选截图 / 显示器亮度（EXT-22）。
//!
//! 抓图只用 GDI：`BitBlt` 见屏幕；图片剪贴板用 `arboard`。圈选截图只负责**按选区裁剪**——
//! 选区由钩子层的「右键画圈 + 点左键」确认（`kada_hook::gesture`），裁剪完写剪贴板，
//! 标注 / 贴图交给 Snipaste（壳层触发它的粘贴热键）。
//!
//! 全部函数都在动作执行线程上跑（不是钩子回调），可以阻塞做系统调用。

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::null_mut;
use std::sync::Mutex;

use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, GetVCPFeatureAndVCPFeatureReply, SetVCPFeature,
    PHYSICAL_MONITOR,
};
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, MonitorFromPoint,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
    HGDIOBJ, MONITOR_DEFAULTTOPRIMARY, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::{BrightnessOp, CapturedImage};

/// 一张抓到的图（RGBA8，行序从上到下）。
pub type Image = CapturedImage;


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
// 圈选截图：按确认时定下的包围盒裁剪
// ---------------------------------------------------------------------------

/// 抓虚拟屏并按包围盒（屏幕物理坐标，左 / 上 / 右 / 下）裁出选区。
///
/// 选区在钩子层已经由「右键画圈 + 点左键」确认过了（见 `kada_hook::gesture::on_left_down`），
/// 这里不再弹确认浮层：裁出来直接进剪贴板，标注 / 贴图交给 Snipaste（壳层 `run_lasso_capture`）。
pub fn capture_rect(rect: (i32, i32, i32, i32)) -> Result<Image, String> {
    let (l, t, r, b) = rect;
    if r <= l || b <= t {
        return Err("截图选区是空的".into());
    }
    let full = capture_full_screen()?;
    let (vx, vy) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
        )
    };
    Ok(crop(&full, l - vx, t - vy, r - vx, b - vy))
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



