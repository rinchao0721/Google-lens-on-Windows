//! 单实例互斥与第二实例通知。

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};
use windows::core::PCWSTR;

use crate::error::{PlatformError, win32};
use crate::handle::OwnedHandle;
use crate::text::to_wide;
use crate::window::WM_SECOND_INSTANCE;

/// 持有互斥量，进程结束或销毁时释放。
pub struct SingleInstance {
    _mutex: OwnedHandle,
}

/// 抢占互斥量的结果。
pub enum Acquired {
    /// 本进程是第一个实例。
    First(SingleInstance),
    /// 已有实例在运行。
    AlreadyRunning,
}

/// 按名称创建命名互斥量判断是否已有实例。
pub fn acquire(name: &str) -> Result<Acquired, PlatformError> {
    let wide = to_wide(name);
    // SAFETY: wide 以 NUL 结尾且在调用期间有效。
    let handle = unsafe { CreateMutexW(None, false, PCWSTR(wide.as_ptr())) }
        .map_err(win32("CreateMutexW"))?;
    // 必须紧跟 CreateMutexW 读取，中间不能插入其它系统调用
    // SAFETY: 无前置条件，读取当前线程的最近错误。
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let mutex = OwnedHandle::new(handle);
    if already {
        Ok(Acquired::AlreadyRunning)
    } else {
        Ok(Acquired::First(SingleInstance { _mutex: mutex }))
    }
}

/// 通知已运行的实例（按窗口类名查找）有人又启动了程序。
pub fn notify_existing(window_class: &str) -> Result<(), PlatformError> {
    let wide = to_wide(window_class);
    // SAFETY: wide 以 NUL 结尾且在调用期间有效。
    let hwnd = unsafe { FindWindowW(PCWSTR(wide.as_ptr()), PCWSTR::null()) }
        .map_err(win32("FindWindowW"))?;
    // SAFETY: hwnd 刚刚由 FindWindowW 返回，消息不携带指针。
    unsafe { PostMessageW(Some(hwnd), WM_SECOND_INSTANCE, WPARAM(0), LPARAM(0)) }
        .map_err(win32("PostMessageW"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_reports_already_running() {
        let name = format!("Local\\lenswin-test-{}", std::process::id());
        let first = acquire(&name).expect("第一次获取");
        assert!(matches!(first, Acquired::First(_)));
        let second = acquire(&name).expect("第二次获取");
        assert!(matches!(second, Acquired::AlreadyRunning));
    }

    #[test]
    fn notify_without_window_fails() {
        assert!(notify_existing("LenswinNoSuchWindowClass").is_err());
    }
}
