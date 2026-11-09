//! 托盘气泡通知。

use windows::Win32::UI::Shell::{
    NIF_INFO, NIIF_ERROR, NIIF_INFO, NIIF_WARNING, NIM_MODIFY, Shell_NotifyIconW,
};

use crate::error::{PlatformError, last_error};
use crate::text::copy_wide;
use crate::tray::TrayIcon;

/// 通知级别，决定气泡图标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyLevel {
    /// 提示。
    Info,
    /// 警告。
    Warning,
    /// 错误。
    Error,
}

/// 显示气泡；标题与正文超出系统限制（63 / 255 个 UTF-16 单元）时截断。
pub fn balloon(
    tray: &TrayIcon,
    level: NotifyLevel,
    title: &str,
    text: &str,
) -> Result<(), PlatformError> {
    let mut nid = tray.base_data();
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = match level {
        NotifyLevel::Info => NIIF_INFO,
        NotifyLevel::Warning => NIIF_WARNING,
        NotifyLevel::Error => NIIF_ERROR,
    };
    copy_wide(&mut nid.szInfoTitle, title);
    copy_wide(&mut nid.szInfo, text);
    // SAFETY: nid 已按要求填充 cbSize、hWnd 与 uID。
    if !unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) }.as_bool() {
        return Err(last_error("Shell_NotifyIconW(NIM_MODIFY)"));
    }
    Ok(())
}
