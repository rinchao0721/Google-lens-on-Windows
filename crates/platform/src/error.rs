//! 平台层错误类型。

use thiserror::Error;

/// Win32 / 系统调用相关错误。
#[derive(Debug, Error)]
pub enum PlatformError {
    /// Win32 API 返回失败。
    #[error("{api} 调用失败：{source}")]
    Win32 {
        /// 失败的 API 名称。
        api: &'static str,
        /// 系统错误。
        #[source]
        source: windows::core::Error,
    },
    /// 快捷键已被其它程序占用。
    #[error("快捷键 {0} 已被其它程序占用")]
    HotkeyConflict(String),
    /// 参数或状态不合法。
    #[error("{0}")]
    Invalid(String),
    /// 文件系统操作失败。
    #[error("{context}：{source}")]
    Io {
        /// 失败的操作说明。
        context: &'static str,
        /// 底层 IO 错误。
        #[source]
        source: std::io::Error,
    },
}

/// 为 `map_err` 提供带 API 名称的转换。
pub(crate) fn win32(api: &'static str) -> impl FnOnce(windows::core::Error) -> PlatformError {
    move |source| PlatformError::Win32 { api, source }
}

/// 最近一次 Win32 错误，用于返回 `BOOL`/空句柄的 API。
pub(crate) fn last_error(api: &'static str) -> PlatformError {
    PlatformError::Win32 {
        api,
        source: windows::core::Error::from_thread(),
    }
}
