//! 显示器枚举，坐标为虚拟桌面物理像素（进程须为 Per-Monitor DPI 感知）。

use lenswin_core::geometry::{Point, Rect};
use lenswin_core::monitor::MonitorInfo;
use windows::Win32::Foundation::{LPARAM, POINT, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, MONITORINFOF_PRIMARY};
use windows::core::BOOL;

use crate::error::{PlatformError, last_error, win32};

/// 100% 缩放对应的 DPI。
const DEFAULT_DPI: u32 = 96;

/// 返回当前光标的屏幕坐标（物理像素）。
pub fn cursor_position() -> Result<Point, PlatformError> {
    let mut pt = POINT::default();
    // SAFETY: pt 是有效的输出局部变量。
    unsafe { GetCursorPos(&mut pt) }.map_err(win32("GetCursorPos"))?;
    Ok(Point::new(pt.x, pt.y))
}

/// 返回当前光标所在显示器的工作区矩形（物理像素，排除任务栏）。
pub fn work_area_at_cursor() -> Result<Rect, PlatformError> {
    let pos = cursor_position()?;
    let pt = POINT { x: pos.x, y: pos.y };
    // SAFETY: MONITOR_DEFAULTTONEAREST 保证在无精确命中时返回最近显示器。
    let monitor = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        return Err(PlatformError::Invalid("未检测到光标所在显示器".to_string()));
    }
    let mut mi = MONITORINFO {
        cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>())
            .map_err(|_| PlatformError::Invalid("MONITORINFO 尺寸溢出".to_string()))?,
        ..Default::default()
    };
    // SAFETY: mi 的 cbSize 已设置，monitor 来自 MonitorFromPoint。
    if !unsafe { GetMonitorInfoW(monitor, &mut mi) }.as_bool() {
        return Err(last_error("GetMonitorInfoW"));
    }
    rect_from_win32(mi.rcWork)
        .or_else(|| rect_from_win32(mi.rcMonitor))
        .ok_or_else(|| PlatformError::Invalid("显示器工作区无效".to_string()))
}

fn rect_from_win32(r: RECT) -> Option<Rect> {
    Rect::from_edges(
        i64::from(r.left),
        i64::from(r.top),
        i64::from(r.right),
        i64::from(r.bottom),
    )
    .filter(|b| !b.is_empty())
}

/// 枚举所有显示器；一个都取不到时返回错误。
pub fn enumerate() -> Result<Vec<MonitorInfo>, PlatformError> {
    let mut list: Vec<MonitorInfo> = Vec::new();
    // SAFETY: 回调只在本调用期间执行，lparam 指向的 list 在此期间保持有效且无其它借用。
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(enum_proc),
            LPARAM(std::ptr::from_mut(&mut list) as isize),
        )
    };
    if !ok.as_bool() {
        return Err(last_error("EnumDisplayMonitors"));
    }
    if list.is_empty() {
        return Err(PlatformError::Invalid("未检测到任何显示器".to_string()));
    }
    Ok(list)
}

unsafe extern "system" fn enum_proc(
    monitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    // SAFETY: enumerate 传入的 lparam 是指向 Vec<MonitorInfo> 的独占指针。
    let list = unsafe { &mut *(lparam.0 as *mut Vec<MonitorInfo>) };
    // 单个显示器信息取不到时跳过，继续枚举其余的
    if let Some(info) = query(monitor) {
        list.push(info);
    }
    TRUE
}

fn query(monitor: HMONITOR) -> Option<MonitorInfo> {
    let mut mi = MONITORINFO {
        cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).ok()?,
        ..Default::default()
    };
    // SAFETY: mi 的 cbSize 已按要求设置，monitor 来自系统枚举。
    if !unsafe { GetMonitorInfoW(monitor, &mut mi) }.as_bool() {
        return None;
    }
    let bounds = rect_from_win32(mi.rcMonitor)?;
    let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
    // SAFETY: 两个输出参数指向有效的局部变量。
    let dpi = match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
    {
        Ok(()) if dpi_x > 0 => dpi_x,
        // 取不到 DPI 时按 100% 处理
        _ => DEFAULT_DPI,
    };
    Some(MonitorInfo {
        bounds,
        dpi,
        primary: (mi.dwFlags & MONITORINFOF_PRIMARY) != 0,
    })
}
