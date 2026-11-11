//! 顶层窗口快照：在冻结画面的同一时刻记下各窗口的可见范围，供覆盖层吸附。

use lenswin_core::geometry::Rect;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, IsIconic, IsWindowVisible,
};
use windows::core::BOOL;

use crate::error::{PlatformError, win32};

/// 单次快照最多记录的窗口数，防止异常环境下无限增长。
const MAX_WINDOWS: usize = 1024;

/// 按 Z 序（最上层在前）列出所有可见顶层窗口的可见范围，单位为物理像素。
///
/// 跳过隐藏、最小化、被 DWM 隐藏（如其它虚拟桌面上）以及面积为 0 的窗口。
pub fn visible_windows() -> Result<Vec<Rect>, PlatformError> {
    let mut found: Vec<Rect> = Vec::new();
    let address = std::ptr::from_mut(&mut found).expose_provenance();
    let param = LPARAM(
        isize::try_from(address)
            .map_err(|_| PlatformError::Invalid("指针超出 LPARAM 范围".to_string()))?,
    );
    // SAFETY: 回调只在 EnumWindows 返回前被同步调用，期间 found 一直存活且无其它引用。
    let result = unsafe { EnumWindows(Some(collect), param) };
    // 达到上限时回调主动中止枚举，EnumWindows 会因此报失败，已收集的结果仍可用
    match result {
        Err(e) if found.len() < MAX_WINDOWS => Err(win32("EnumWindows")(e)),
        _ => Ok(found),
    }
}

unsafe extern "system" fn collect(hwnd: HWND, param: LPARAM) -> BOOL {
    let Ok(address) = usize::try_from(param.0) else {
        return BOOL::from(false);
    };
    // SAFETY: param 由 visible_windows 传入，指向调用期间存活的 Vec<Rect>。
    let found = unsafe { &mut *std::ptr::with_exposed_provenance_mut::<Vec<Rect>>(address) };
    if found.len() >= MAX_WINDOWS {
        return BOOL::from(false);
    }
    if let Some(rect) = window_bounds(hwnd) {
        found.push(rect);
    }
    BOOL::from(true)
}

/// 窗口的可见范围；不该参与吸附的窗口返回 `None`。
fn window_bounds(hwnd: HWND) -> Option<Rect> {
    // SAFETY: hwnd 来自 EnumWindows，窗口可能随时销毁，此时下列 API 只会返回失败。
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() || unsafe { IsIconic(hwnd) }.as_bool() {
        return None;
    }
    if is_cloaked(hwnd) {
        return None;
    }
    let raw = frame_bounds(hwnd)?;
    let rect = Rect::from_edges(
        i64::from(raw.left),
        i64::from(raw.top),
        i64::from(raw.right),
        i64::from(raw.bottom),
    )?;
    (!rect.is_empty()).then_some(rect)
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: 输出缓冲区为 u32，大小与 DWMWA_CLOAKED 要求一致。
    let result = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            std::ptr::from_mut(&mut cloaked).cast(),
            u32_size_of::<u32>(),
        )
    };
    // 查询失败时按未隐藏处理，宁可多吸附一个窗口也不漏掉
    result.is_ok() && cloaked != 0
}

/// 去掉不可见缩放边框后的范围；DWM 不可用时退回 `GetWindowRect`。
fn frame_bounds(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    // SAFETY: 输出缓冲区为 RECT，大小与 DWMWA_EXTENDED_FRAME_BOUNDS 要求一致。
    let dwm = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            std::ptr::from_mut(&mut rect).cast(),
            u32_size_of::<RECT>(),
        )
    };
    if dwm.is_ok() {
        return Some(rect);
    }
    // SAFETY: rect 是有效的局部变量。
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    Some(rect)
}

/// 结构体大小转 `u32`；这里只用于 `u32` 和 `RECT`，不会溢出。
fn u32_size_of<T>() -> u32 {
    u32::try_from(std::mem::size_of::<T>()).unwrap_or(0)
}
