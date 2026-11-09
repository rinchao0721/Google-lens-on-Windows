//! 激活已存在的窗口。

use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow,
};
use windows::core::PCWSTR;

use crate::handle::discard;
use crate::text::to_wide;

/// 按标题找到顶层窗口并置前；找不到返回 `false`。
pub fn activate_window_titled(title: &str) -> bool {
    let wide = to_wide(title);
    // SAFETY: wide 以 NUL 结尾且在调用期间有效。
    let Ok(hwnd) = (unsafe { FindWindowW(PCWSTR::null(), PCWSTR(wide.as_ptr())) }) else {
        return false;
    };
    // SAFETY: hwnd 刚由 FindWindowW 返回；窗口随时可能消失，失败只意味着没激活。
    unsafe {
        if IsIconic(hwnd).as_bool() {
            discard(ShowWindow(hwnd, SW_RESTORE));
        }
        discard(SetForegroundWindow(hwnd));
    }
    true
}
