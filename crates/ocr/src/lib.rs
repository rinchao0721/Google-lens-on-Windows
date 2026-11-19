//! 离线文字识别引擎：ONNX Runtime 加 PP-OCRv5 mobile（检测 + 识别），只负责「图像 → 文本行」。

#![forbid(unsafe_code)]

mod engine;
mod error;

pub use engine::OcrEngine;
pub use error::OcrError;
