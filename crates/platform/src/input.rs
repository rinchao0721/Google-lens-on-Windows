//! 输入模拟与光标位置读写：滚轮事件、光标读取与移动。

use lenswin_core::geometry::Point;
use lenswin_core::stitch::ScrollAxis;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;

use crate::error::{PlatformError, last_error, win32};

pub use crate::monitor::cursor_position;

/// Windows 标准一格滚轮的增量（与 `WHEEL_DELTA` 一致）。
pub const WHEEL_NOTCH: i32 = 120;

/// 把系统光标移动到虚拟桌面物理坐标 `point`。
pub fn set_cursor_position(point: Point) -> Result<(), PlatformError> {
    // SAFETY: SetCursorPos 无指针参数，坐标超出屏幕时系统会自动夹紧。
    unsafe { SetCursorPos(point.x, point.y) }.map_err(win32("SetCursorPos"))
}

/// 在屏幕坐标 `point` 处发送一次鼠标滚轮事件。
///
/// 竖向滚动时 `delta < 0` 向下滚、`delta > 0` 向上滚；横向滚动时 `delta > 0` 向右滚、`delta < 0` 向左滚。
pub fn scroll_wheel(point: Point, axis: ScrollAxis, delta: i32) -> Result<(), PlatformError> {
    if delta == 0 {
        return Ok(());
    }
    set_cursor_position(point)?;
    let flag = match axis {
        ScrollAxis::Vertical => MOUSEEVENTF_WHEEL,
        ScrollAxis::Horizontal => MOUSEEVENTF_HWHEEL,
    };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: u32::from_ne_bytes(delta.to_ne_bytes()),
                dwFlags: flag,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let size = i32::try_from(std::mem::size_of::<INPUT>())
        .map_err(|_| PlatformError::Invalid("INPUT 结构尺寸溢出".to_string()))?;
    // SAFETY: 传入长度 1 的有效 INPUT 切片，cbSize 与结构体尺寸一致。
    let sent = unsafe { SendInput(&[input], size) };
    if sent == 0 {
        return Err(last_error("SendInput"));
    }
    Ok(())
}
