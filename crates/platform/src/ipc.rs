//! 设置进程向托盘进程同步提交数据（`WM_COPYDATA`）。

use lenswin_core::ipc::MAX_PAYLOAD_BYTES;
use thiserror::Error;
use windows::Win32::Foundation::{ERROR_TIMEOUT, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_COPYDATA,
};
use windows::core::PCWSTR;

use crate::text::to_wide;

/// 发送失败的原因。
#[derive(Debug, Error)]
pub enum SendError {
    /// 找不到托盘进程的消息窗口。
    #[error("找不到主程序，请确认 lenswin 正在运行")]
    NotRunning,
    /// 等待超时，主程序可能无响应。
    #[error("主程序没有响应")]
    TimedOut,
    /// 负载超过上限。
    #[error("数据过大")]
    TooLarge,
    /// 其它系统错误。
    #[error("发送失败：{0}")]
    Other(String),
}

/// 向窗口类名为 `window_class` 的窗口同步发送数据，返回对方的结果码数值。
pub fn send(
    window_class: &str,
    kind: u32,
    payload: &[u8],
    timeout_ms: u32,
) -> Result<u32, SendError> {
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(SendError::TooLarge);
    }
    let len = u32::try_from(payload.len()).map_err(|_| SendError::TooLarge)?;
    let class = to_wide(window_class);
    // SAFETY: class 以 NUL 结尾且在调用期间有效。
    let hwnd = unsafe { FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()) }
        .map_err(|_| SendError::NotRunning)?;
    let data = COPYDATASTRUCT {
        dwData: usize::try_from(kind).map_err(|_| SendError::TooLarge)?,
        cbData: len,
        lpData: payload.as_ptr().cast_mut().cast(),
    };
    let data_ptr = std::ptr::from_ref(&data);
    let lparam = isize::try_from(data_ptr.expose_provenance())
        .map_err(|_| SendError::Other("地址超出范围".to_string()))?;
    let mut result = 0usize;
    // SAFETY: data 与 payload 在同步调用返回前一直有效；接收方只读取 lpData。
    let sent = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(lparam),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            timeout_ms,
            Some(&mut result),
        )
    };
    if sent.0 == 0 {
        let error = windows::core::Error::from_thread();
        return if error.code() == windows::core::HRESULT::from_win32(ERROR_TIMEOUT.0) {
            Err(SendError::TimedOut)
        } else {
            Err(SendError::Other(error.to_string()))
        };
    }
    u32::try_from(result).map_err(|_| SendError::Other("返回值超出范围".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_window_is_reported_as_not_running() {
        let result = send("LenswinNoSuchWindowClass", 1, b"x", 100);
        assert!(matches!(result, Err(SendError::NotRunning)));
    }

    #[test]
    fn oversized_payload_is_rejected_before_sending() {
        let big = vec![0u8; MAX_PAYLOAD_BYTES + 1];
        assert!(matches!(
            send("LenswinNoSuchWindowClass", 1, &big, 100),
            Err(SendError::TooLarge)
        ));
    }
}
