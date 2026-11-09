//! 用默认浏览器打开网址。

use lenswin_core::config::is_http_url;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use crate::error::{PlatformError, last_error};
use crate::text::to_wide;

/// `ShellExecuteW` 返回值大于该阈值表示成功。
const SHELL_EXECUTE_SUCCESS_ABOVE: usize = 32;

/// 打开 http/https 网址；其它协议一律拒绝，避免被用来启动本地程序。
pub fn open_url(url: &str) -> Result<(), PlatformError> {
    if !is_http_url(url) {
        return Err(PlatformError::Invalid(
            "只允许打开 http 或 https 地址".to_string(),
        ));
    }
    let wide = to_wide(url);
    // SAFETY: wide 以 NUL 结尾且在调用期间有效，其余参数为静态字符串或空。
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if result.0.addr() > SHELL_EXECUTE_SUCCESS_ABOVE {
        Ok(())
    } else {
        Err(last_error("ShellExecuteW"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_schemes() {
        for url in [
            "file:///C:/Windows/notepad.exe",
            "calc.exe",
            "javascript:1",
            "",
        ] {
            assert!(open_url(url).is_err(), "{url}");
        }
    }
}
