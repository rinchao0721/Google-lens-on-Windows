//! 弹窗的 Win32 窗口：创建、定位、隐藏、空闲释放和窗口过程。

use lenswin_core::geometry::Rect;
use lenswin_core::lens::popup_placement;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_WINDOW, GetMonitorInfoW, HBRUSH, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, HWND_TOPMOST, IDC_ARROW, IsWindowVisible,
    KillTimer, LoadCursorW, RegisterClassExW, SW_HIDE, SWP_NOACTIVATE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow, UnregisterClassW,
    WM_CLOSE, WM_DESTROY, WM_DPICHANGED, WM_KEYDOWN, WM_MOVE, WM_SETFOCUS, WM_SIZE, WM_TIMER,
    WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW,
};
use windows::core::{PCWSTR, w};

use crate::error::{PopupError, last_error, win32};
use crate::{Placement, web, with_host};

const CLASS_NAME: PCWSTR = w!("LenswinLensPopup");
const TITLE: PCWSTR = w!("Google 智能镜头");
const IDLE_TIMER_ID: usize = 1;
/// 隐藏后多久释放 WebView（毫秒）。
const IDLE_RELEASE_MS: u32 = 5 * 60 * 1000;
/// 96 DPI 下的默认尺寸与离选区的间距。
const LOGICAL_WIDTH: u32 = 460;
const LOGICAL_HEIGHT: u32 = 760;
const LOGICAL_GAP: u32 = 16;

fn discard<T>(_value: T) {}

pub(crate) fn register_class(instance: HINSTANCE) -> Result<(), PopupError> {
    // SAFETY: IDC_ARROW 为系统预置光标，hinstance 为 None。
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.map_err(win32("LoadCursorW"))?;
    let class = WNDCLASSEXW {
        cbSize: u32::try_from(std::mem::size_of::<WNDCLASSEXW>()).unwrap_or(0),
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        hCursor: cursor,
        // 系统颜色画刷约定为颜色索引加一
        hbrBackground: HBRUSH(std::ptr::without_provenance_mut(
            usize::try_from(COLOR_WINDOW.0)
                .unwrap_or(0)
                .saturating_add(1),
        )),
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: class 内各字段有效，类名为静态字符串。
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(last_error("RegisterClassExW"));
    }
    Ok(())
}

pub(crate) fn unregister_class(instance: HINSTANCE) {
    // 程序退出阶段注销失败没有后续可做
    // SAFETY: 类名与注册时一致，窗口已在此前销毁。
    discard(unsafe { UnregisterClassW(CLASS_NAME, Some(instance)) });
}

/// 取得弹窗窗口，不存在时创建（初始隐藏）。
pub(crate) fn ensure_window() -> Result<HWND, PopupError> {
    let (existing, instance) = with_host(|host| (host.hwnd, host.instance))
        .ok_or_else(|| PopupError::Invalid("弹窗状态不可用".to_string()))?;
    if let Some(hwnd) = existing {
        return Ok(hwnd);
    }
    // SAFETY: 窗口类已注册，标题为静态字符串；尺寸稍后由 place 设置。
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            CLASS_NAME,
            TITLE,
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(win32("CreateWindowExW"))?;
    with_host(|host| host.hwnd = Some(hwnd));
    Ok(hwnd)
}

pub(crate) fn is_visible() -> bool {
    match with_host(|host| host.hwnd).flatten() {
        // SAFETY: hwnd 由本模块创建，销毁时会清空。
        Some(hwnd) => unsafe { IsWindowVisible(hwnd) }.as_bool(),
        None => false,
    }
}

fn to_rect(r: &RECT) -> Option<Rect> {
    Rect::from_edges(
        i64::from(r.left),
        i64::from(r.top),
        i64::from(r.right),
        i64::from(r.bottom),
    )
}

fn scale(logical: u32, dpi: u32) -> u32 {
    u32::try_from(u64::from(logical).saturating_mul(u64::from(dpi)) / 96).unwrap_or(logical)
}

/// 按选区所在显示器的工作区和 DPI 放置并显示弹窗。
pub(crate) fn place(hwnd: HWND, anchor: &Rect) -> Result<(), PopupError> {
    let anchor_rect = RECT {
        left: anchor.x,
        top: anchor.y,
        right: i32::try_from(anchor.right()).unwrap_or(i32::MAX),
        bottom: i32::try_from(anchor.bottom()).unwrap_or(i32::MAX),
    };
    // SAFETY: anchor_rect 为有效局部变量；NEAREST 保证返回某个显示器。
    let monitor = unsafe { MonitorFromRect(&anchor_rect, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(0),
        ..Default::default()
    };
    // SAFETY: info.cbSize 已设置，monitor 来自 MonitorFromRect。
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return Err(last_error("GetMonitorInfoW"));
    }
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    // SAFETY: 输出参数为有效局部变量。
    if let Err(e) = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
    {
        log::warn!("获取显示器 DPI 失败，按 96 处理：{e}");
    }
    let work =
        to_rect(&info.rcWork).ok_or_else(|| PopupError::Invalid("显示器工作区为空".to_string()))?;
    let target = popup_placement(
        anchor,
        &work,
        scale(LOGICAL_WIDTH, dpi_x),
        scale(LOGICAL_HEIGHT, dpi_x),
        scale(LOGICAL_GAP, dpi_x),
    )
    .ok_or_else(|| PopupError::Invalid("无法计算弹窗位置".to_string()))?;
    with_host(|host| host.placement = Placement::Placing);
    // SAFETY: hwnd 由本模块创建且存活。
    let placed = unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            target.x,
            target.y,
            i32::try_from(target.width).unwrap_or(i32::MAX),
            i32::try_from(target.height).unwrap_or(i32::MAX),
            SWP_SHOWWINDOW,
        )
    };
    with_host(|host| host.placement = Placement::Idle);
    placed.map_err(win32("SetWindowPos"))?;
    // 前台切换可能被系统拒绝，弹窗仍然可见可点
    // SAFETY: hwnd 有效。
    discard(unsafe { SetForegroundWindow(hwnd) });
    // SAFETY: 计时器可能不存在，失败无影响。
    discard(unsafe { KillTimer(Some(hwnd), IDLE_TIMER_ID) });
    Ok(())
}

/// 隐藏弹窗，并在空闲一段时间后释放 WebView。
pub(crate) fn hide(hwnd: HWND) {
    // 返回值只表示之前是否可见
    // SAFETY: hwnd 由本模块创建且存活。
    discard(unsafe { ShowWindow(hwnd, SW_HIDE) });
    web::reset_page();
    web::set_visible(false);
    // SAFETY: hwnd 有效，无回调函数，到期时收到 WM_TIMER。
    if unsafe { SetTimer(Some(hwnd), IDLE_TIMER_ID, IDLE_RELEASE_MS, None) } == 0 {
        log::warn!("设置空闲释放计时器失败，WebView 将保留到退出");
    }
}

/// WebView 创建失败或浏览器进程退出时收起，不留空白窗口。
pub(crate) fn hide_current() {
    if let Some(hwnd) = with_host(|host| host.hwnd).flatten() {
        hide(hwnd);
    }
}

pub(crate) fn destroy() {
    if let Some(hwnd) = with_host(|host| host.hwnd.take()).flatten() {
        // 程序退出阶段销毁失败只会泄漏窗口
        // SAFETY: hwnd 由本模块创建，此处只销毁一次。
        discard(unsafe { DestroyWindow(hwnd) });
    }
}

fn on_dpi_changed(hwnd: HWND, lparam: LPARAM) {
    // 跨屏放置时系统会在已按目标 DPI 算好的尺寸上再缩放一次
    if with_host(|host| host.placement) == Some(Placement::Placing) {
        return;
    }
    let suggested = lparam.0 as *const RECT;
    // SAFETY: WM_DPICHANGED 的 lParam 指向系统提供的有效 RECT，仅在本消息期间有效。
    let Some(r) = (unsafe { suggested.as_ref() }) else {
        return;
    };
    // SAFETY: hwnd 有效，r 已复制为数值。
    if let Err(e) = unsafe {
        SetWindowPos(
            hwnd,
            None,
            r.left,
            r.top,
            r.right.saturating_sub(r.left),
            r.bottom.saturating_sub(r.top),
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    } {
        log::warn!("DPI 变化后调整弹窗尺寸失败：{e}");
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => web::fit_to_window(hwnd),
        WM_MOVE => web::notify_moved(),
        WM_SETFOCUS => web::focus(),
        WM_DPICHANGED => on_dpi_changed(hwnd, lparam),
        WM_CLOSE => {
            hide(hwnd);
            return LRESULT(0);
        }
        WM_KEYDOWN if wparam.0 == usize::from(VK_ESCAPE.0) => {
            hide(hwnd);
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == IDLE_TIMER_ID => {
            // SAFETY: 计时器由 hide 设置，失败无影响。
            discard(unsafe { KillTimer(Some(hwnd), IDLE_TIMER_ID) });
            log::info!("弹窗空闲超时，释放 WebView");
            web::release();
            return LRESULT(0);
        }
        WM_DESTROY => {
            with_host(|host| {
                if host.hwnd == Some(hwnd) {
                    host.hwnd = None;
                }
            });
        }
        _ => {}
    }
    // SAFETY: 未处理的消息原样交给默认窗口过程。
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
