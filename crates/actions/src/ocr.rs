//! 本地文字识别动作：引擎给出文本行，这里负责拼接与空结果判定。

use std::fmt;
use std::path::Path;

use lenswin_core::frame::CroppedImage;
use lenswin_core::ocr::{has_recognized_text, join_lines};
use lenswin_ocr::OcrError as EngineError;

pub use lenswin_ocr::OcrEngine;

/// 本地文字识别失败的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrError {
    /// 缺少 ONNX Runtime 动态库。
    DllMissing(String),
    /// 动态库或模型加载失败。
    LoadFailed(String),
    /// 识别过程失败。
    Failed(String),
}

impl fmt::Display for OcrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OcrError::DllMissing(path) => write!(
                f,
                "缺少文字识别组件 onnxruntime.dll，请把它放回 lenswin.exe 所在目录（{path}）"
            ),
            OcrError::LoadFailed(reason) => write!(f, "文字识别组件加载失败：{reason}"),
            OcrError::Failed(reason) => write!(f, "文字识别失败：{reason}"),
        }
    }
}

impl std::error::Error for OcrError {}

impl From<EngineError> for OcrError {
    fn from(err: EngineError) -> Self {
        match err {
            EngineError::DllMissing(path) => OcrError::DllMissing(path.display().to_string()),
            EngineError::Runtime(_) | EngineError::Model { .. } => {
                OcrError::LoadFailed(err.to_string())
            }
            other => OcrError::Failed(other.to_string()),
        }
    }
}

/// 从 `dll_path`（绝对路径）加载识别引擎。
pub fn load_ocr_engine(dll_path: &Path) -> Result<OcrEngine, OcrError> {
    OcrEngine::load(dll_path).map_err(OcrError::from)
}

/// 识别图像中的文字。
///
/// 识别出非空文本返回 `Ok(Some(text))`，未识别到文字返回 `Ok(None)`。
pub fn recognize_image(
    engine: &OcrEngine,
    image: &CroppedImage,
) -> Result<Option<String>, OcrError> {
    let lines = engine.recognize(image)?;
    Ok(finish(&lines))
}

fn finish(lines: &[String]) -> Option<String> {
    let text = join_lines(lines);
    has_recognized_text(&text).then_some(text)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn lines_are_joined_with_crlf_and_blank_results_are_none() {
        assert_eq!(finish(&[]), None);
        assert_eq!(finish(&["  ".to_string()]), None);
        assert_eq!(finish(&[String::new(), String::new()]), None);
        assert_eq!(
            finish(&["第一行".to_string(), "Second".to_string()]),
            Some("第一行\r\nSecond".to_string())
        );
    }

    #[test]
    fn engine_errors_map_to_user_facing_categories() {
        let missing = OcrError::from(EngineError::DllMissing(PathBuf::from(
            "C:\\a\\onnxruntime.dll",
        )));
        assert_eq!(
            missing,
            OcrError::DllMissing("C:\\a\\onnxruntime.dll".to_string())
        );
        assert!(missing.to_string().contains("onnxruntime.dll"));

        assert!(matches!(
            OcrError::from(EngineError::Runtime("x".to_string())),
            OcrError::LoadFailed(_)
        ));
        assert!(matches!(
            OcrError::from(EngineError::Model {
                model: "检测",
                reason: "y".to_string()
            }),
            OcrError::LoadFailed(_)
        ));
        assert!(matches!(
            OcrError::from(EngineError::Inference("z".to_string())),
            OcrError::Failed(_)
        ));
        assert!(matches!(
            OcrError::from(EngineError::BadOutput("w")),
            OcrError::Failed(_)
        ));
    }

    #[test]
    fn load_reports_missing_dll_without_touching_runtime() {
        let result = load_ocr_engine(Path::new("Z:\\definitely\\missing\\onnxruntime.dll"));
        assert!(matches!(result, Err(OcrError::DllMissing(_))));
    }
}
