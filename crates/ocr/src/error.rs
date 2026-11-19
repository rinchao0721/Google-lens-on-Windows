//! 识别引擎错误类型。

use std::path::PathBuf;

use lenswin_core::error::OcrDataError;
use thiserror::Error;

/// 识别引擎的加载与推理错误。
#[derive(Debug, Error)]
pub enum OcrError {
    /// 找不到 ONNX Runtime 动态库。
    #[error("缺少 {}", .0.display())]
    DllMissing(PathBuf),
    /// 动态库存在但无法加载或初始化。
    #[error("加载 ONNX Runtime 失败：{0}")]
    Runtime(String),
    /// 模型会话创建失败。
    #[error("加载{model}模型失败：{reason}")]
    Model {
        /// 模型名称。
        model: &'static str,
        /// 失败原因。
        reason: String,
    },
    /// 推理失败。
    #[error("文字识别推理失败：{0}")]
    Inference(String),
    /// 模型输出与预期的张量结构不符。
    #[error("模型输出异常：{0}")]
    BadOutput(&'static str),
    /// 图像或张量数据处理错误。
    #[error("图像处理失败：{0}")]
    Data(#[from] OcrDataError),
    /// 引擎内部锁已被破坏（先前推理发生过 panic）。
    #[error("识别引擎状态已损坏，请重试")]
    Poisoned,
}
