//! 用系统默认程序打开网址或文件夹。

use std::path::Path;

use lenswin_core::config::is_http_url;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use crate::error::PlatformError;
use crate::text::to_wide;

/// `ShellExecuteW` 返回值大于该阈值表示成功。
const SHELL_EXECUTE_SUCCESS_ABOVE: usize = 32;

/// 当前线程的 COM 初始化，离开作用域时按需反初始化。
struct ComScope {
    initialized: bool,
}

impl ComScope {
    /// 初始化为单线程套间；线程已按其它模式初始化时沿用，不算失败。
    fn enter() -> Self {
        // SAFETY: 无前置条件；成功（含 S_FALSE）时必须与 CoUninitialize 配对。
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Self {
            initialized: hr.is_ok(),
        }
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: 与 enter 中成功的 CoInitializeEx 配对，仅调用一次。
            unsafe { CoUninitialize() };
        }
    }
}

/// 以 `open` 动词交给系统打开；`what` 只用于错误信息。
fn shell_open(target: &str, what: &str) -> Result<(), PlatformError> {
    let wide = to_wide(target);
    let _com = ComScope::enter();
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
    // 失败时返回值才是错误码，GetLastError 不可靠
    let code = result.0.addr();
    if code > SHELL_EXECUTE_SUCCESS_ABOVE {
        Ok(())
    } else {
        Err(PlatformError::Invalid(format!(
            "ShellExecuteW 打开{what}失败，错误码 {code}"
        )))
    }
}

/// 打开 http/https 网址；其它协议一律拒绝，避免被用来启动本地程序。
///
/// 可在任意线程调用，内部自行初始化 COM。
pub fn open_url(url: &str) -> Result<(), PlatformError> {
    if !is_http_url(url) {
        return Err(PlatformError::Invalid(
            "只允许打开 http 或 https 地址".to_string(),
        ));
    }
    shell_open(url, "浏览器")
}

/// 在资源管理器中打开已存在的文件夹；不是目录一律拒绝，避免被用来启动文件。
pub fn open_folder(path: &Path) -> Result<(), PlatformError> {
    if !path.is_dir() {
        return Err(PlatformError::Invalid(format!(
            "{} 不是已存在的文件夹",
            path.display()
        )));
    }
    shell_open(&path.display().to_string(), "文件夹")
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

    #[test]
    fn open_folder_rejects_files_and_missing_paths() {
        assert!(open_folder(Path::new(r"C:\Windows\notepad.exe")).is_err());
        assert!(open_folder(Path::new(r"C:\no\such\dir\lenswin")).is_err());
    }
}
