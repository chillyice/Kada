//! 显示器亮度（EXT-22，Windows DDC/CI）。
//!
//! 只走 VCP 0x10：读当前值 / 上限，写目标值，本次运行内记住第一次调节前的原始值供「恢复」。
//! 全部函数都在动作执行线程上跑（不是钩子回调），可以阻塞做系统调用。
//!
//! 原「圈选截图」的 GDI 抓屏 / 裁剪 / 图片剪贴板已随 EXT-20 一起下线，圈选只负责给
//! Snipaste 发 F1（见 说明书 §5.15）。

use std::sync::Mutex;

use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, GetVCPFeatureAndVCPFeatureReply, SetVCPFeature,
    PHYSICAL_MONITOR,
};
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};

use crate::BrightnessOp;

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
        let mut cache = ORIGINAL_BRIGHTNESS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
