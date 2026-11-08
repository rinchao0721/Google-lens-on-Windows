//! 各模块共用的错误类型。

use thiserror::Error;

use crate::config::ValidationIssue;

/// 帧与裁剪相关错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FrameError {
    /// 区域宽或高为 0。
    #[error("区域宽或高为 0")]
    EmptyRegion,
    /// 裁剪范围与帧区域没有交集。
    #[error("裁剪范围与帧区域没有交集")]
    EmptyCrop,
    /// 像素缓冲区长度与宽×高×4 不符。
    #[error("像素缓冲区长度不符：期望 {expected}，实际 {actual}")]
    LengthMismatch {
        /// 期望的字节数。
        expected: usize,
        /// 实际的字节数。
        actual: usize,
    },
    /// 尺寸运算溢出。
    #[error("尺寸运算溢出")]
    SizeOverflow,
}

/// 选区构造错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SelectionError {
    /// 选区宽或高为 0。
    #[error("选区宽或高为 0")]
    EmptyBounds,
    /// 套索没有任何轨迹点。
    #[error("套索轨迹为空")]
    EmptyLasso,
}

/// 快捷键字符串解析错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HotkeyError {
    /// 字符串为空。
    #[error("快捷键为空")]
    Empty,
    /// 含有空片段，例如 `Ctrl++A`。
    #[error("快捷键含有空片段")]
    EmptyToken,
    /// 主键之前出现了无法识别的修饰键。
    #[error("无法识别的修饰键：{0}")]
    UnknownModifier(String),
    /// 同一个修饰键出现多次。
    #[error("修饰键重复：{0}")]
    DuplicateModifier(String),
    /// 最后一项是修饰键，缺少主键。
    #[error("缺少主键")]
    MissingKey,
    /// 无法识别的主键。
    #[error("无法识别的主键：{0}")]
    UnknownKey(String),
    /// 没有任何修饰键。
    #[error("至少需要一个修饰键")]
    NoModifier,
}

/// 配置读写与校验错误。
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ConfigError {
    /// TOML 解析失败。
    #[error("配置解析失败：{0}")]
    Parse(#[from] toml::de::Error),
    /// TOML 序列化失败。
    #[error("配置序列化失败：{0}")]
    Serialize(#[from] toml::ser::Error),
    /// 校验未通过。
    #[error("配置校验失败，共 {} 项", .0.len())]
    Invalid(Vec<ValidationIssue>),
}

/// 动作执行错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ActionError {
    /// 输入类型与动作要求不符。
    #[error("输入类型与动作不匹配")]
    InvalidInput,
    /// 用户取消。
    #[error("操作已取消")]
    Cancelled,
    /// 其它失败，附带面向用户的说明。
    #[error("{0}")]
    Failed(String),
}
