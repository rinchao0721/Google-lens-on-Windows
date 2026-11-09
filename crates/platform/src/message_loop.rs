//! 主线程消息循环。

use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, TranslateMessage,
};

use crate::error::{PlatformError, last_error};
use crate::handle::discard;
use crate::window::{WindowEvent, pop_event};

/// 事件处理后的循环走向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopControl {
    /// 继续等待事件。
    Continue,
    /// 退出循环。
    Quit,
}

/// 运行消息循环，直到处理函数返回 [`LoopControl::Quit`] 或收到 `WM_QUIT`。
pub fn run(mut handler: impl FnMut(WindowEvent) -> LoopControl) -> Result<(), PlatformError> {
    loop {
        let mut msg = MSG::default();
        // SAFETY: msg 是有效的局部变量，None 表示接收本线程所有窗口的消息。
        let status = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        match status.0 {
            0 => return Ok(()),
            -1 => return Err(last_error("GetMessageW")),
            _ => {}
        }
        // TranslateMessage 的返回值只表示是否产生了字符消息
        // SAFETY: msg 由 GetMessageW 填充。
        discard(unsafe { TranslateMessage(&msg) });
        // SAFETY: msg 由 GetMessageW 填充。
        discard(unsafe { DispatchMessageW(&msg) });
        while let Some(event) = pop_event() {
            if handler(event) == LoopControl::Quit {
                return Ok(());
            }
        }
    }
}
