//! 进程 DPI 感知设置。

use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

use crate::error::PlatformError;
use crate::error::win32;

/// 声明 Per-Monitor V2 感知，之后所有坐标都是物理像素；须在创建任何窗口之前调用。
///
/// 正式分发时由 manifest 声明，此处作为兜底；已被 manifest 设置时会返回错误，调用方可忽略。
pub fn enable_per_monitor_v2() -> Result<(), PlatformError> {
    // SAFETY: 传入系统定义的常量上下文，无其它前置条件。
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
        .map_err(win32("SetProcessDpiAwarenessContext"))
}
