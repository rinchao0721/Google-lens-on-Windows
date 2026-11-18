//! 贴图窗口的错误类型。

use std::fmt;

/// 贴图操作失败。
#[derive(Debug)]
pub enum PinError {
    /// 已达到贴图数量上限。
    TooMany,
    /// 输入图像或区域无效。
    InvalidInput(String),
    /// Win32 或 Direct2D 调用失败。
    Win32 {
        /// 失败的系统 API 名称。
        api: &'static str,
        /// 系统返回的错误详情。
        source: windows::core::Error,
    },
}

impl fmt::Display for PinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PinError::TooMany => f.write_str("贴图已达上限"),
            PinError::InvalidInput(msg) => f.write_str(msg),
            PinError::Win32 { api, source } => write!(f, "{api} 调用失败：{source}"),
        }
    }
}

impl std::error::Error for PinError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PinError::Win32 { source, .. } => Some(source),
            PinError::TooMany | PinError::InvalidInput(_) => None,
        }
    }
}

pub(crate) fn win32(api: &'static str) -> impl FnOnce(windows::core::Error) -> PinError {
    move |source| PinError::Win32 { api, source }
}

pub(crate) fn last_error(api: &'static str) -> PinError {
    PinError::Win32 {
        api,
        source: windows::core::Error::from_thread(),
    }
}
