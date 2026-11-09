//! 模态确认对话框。

use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO,
    MessageBoxW,
};

use crate::error::{PlatformError, last_error};
use crate::text::to_wide;
use windows::core::PCWSTR;

/// 弹出「是 / 否」确认框，选「是」返回 `true`。
///
/// 对话框置顶并抢前台，调用线程在用户作答前阻塞。
pub fn confirm(title: &str, text: &str) -> Result<bool, PlatformError> {
    let title_wide = to_wide(title);
    let text_wide = to_wide(text);
    // SAFETY: 两个字符串都以 NUL 结尾且在调用期间有效，无所有者窗口。
    let result = unsafe {
        MessageBoxW(
            None,
            PCWSTR(text_wide.as_ptr()),
            PCWSTR(title_wide.as_ptr()),
            MB_YESNO | MB_ICONINFORMATION | MB_TOPMOST | MB_SETFOREGROUND,
        )
    };
    if result.0 == 0 {
        return Err(last_error("MessageBoxW"));
    }
    Ok(result == IDYES)
}

/// 弹出只有「确定」的错误提示框，供没有日志和控制台的进程报告致命错误。
pub fn error(title: &str, text: &str) -> Result<(), PlatformError> {
    let title_wide = to_wide(title);
    let text_wide = to_wide(text);
    // SAFETY: 两个字符串都以 NUL 结尾且在调用期间有效，无所有者窗口。
    let result = unsafe {
        MessageBoxW(
            None,
            PCWSTR(text_wide.as_ptr()),
            PCWSTR(title_wide.as_ptr()),
            MB_OK | MB_ICONERROR | MB_TOPMOST | MB_SETFOREGROUND,
        )
    };
    if result.0 == 0 {
        return Err(last_error("MessageBoxW"));
    }
    Ok(())
}
