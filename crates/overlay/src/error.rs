//! 覆盖层错误类型。

use thiserror::Error;

/// 覆盖层错误。
#[derive(Debug, Error)]
pub enum OverlayError {
    /// 输入与显示器不匹配等调用方错误。
    #[error("{0}")]
    InvalidInput(String),
    /// Win32 或 Direct2D 调用失败。
    #[error("{api} 调用失败：{source}")]
    Win32 {
        /// 失败的 API 名称。
        api: &'static str,
        /// 系统错误。
        #[source]
        source: windows::core::Error,
    },
}

/// 为 `map_err` 提供带 API 名称的转换。
pub(crate) fn win32(api: &'static str) -> impl FnOnce(windows::core::Error) -> OverlayError {
    move |source| OverlayError::Win32 { api, source }
}
