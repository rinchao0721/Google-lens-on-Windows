//! 弹窗错误类型。

use thiserror::Error;

/// 内置搜索弹窗相关错误。
#[derive(Debug, Error)]
pub enum PopupError {
    /// Win32 / COM 调用失败。
    #[error("{api} 调用失败：{source}")]
    Win32 {
        /// 失败的 API 名称。
        api: &'static str,
        /// 系统错误。
        #[source]
        source: windows::core::Error,
    },
    /// 本机没有可用的 WebView2 Runtime。
    #[error("未安装 Microsoft Edge WebView2 Runtime")]
    RuntimeMissing,
    /// 参数或状态不合法。
    #[error("{0}")]
    Invalid(String),
}

/// 为 `map_err` 提供带 API 名称的转换。
pub(crate) fn win32(api: &'static str) -> impl FnOnce(windows::core::Error) -> PopupError {
    move |source| PopupError::Win32 { api, source }
}

/// 最近一次 Win32 错误，用于返回空句柄的 API。
pub(crate) fn last_error(api: &'static str) -> PopupError {
    PopupError::Win32 {
        api,
        source: windows::core::Error::from_thread(),
    }
}
