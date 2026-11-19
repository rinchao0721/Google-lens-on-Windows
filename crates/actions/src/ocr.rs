//! 本地文字识别动作：引擎给出文本行，这里负责拼接与空结果判定。

use std::fmt;
use std::path::Path;

use lenswin_core::frame::CroppedImage;
use lenswin_core::ocr::{
    crop_ocr_chunk, has_recognized_text, join_lines, merge_ocr_chunks, plan_ocr_chunks,
};
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

/// 识别图像中的文字；超长图像自动按屏分段识别并合并重叠行。
///
/// 识别出非空文本返回 `Ok(Some(text))`，未识别到文字返回 `Ok(None)`。
pub fn recognize_image(
    engine: &OcrEngine,
    image: &CroppedImage,
) -> Result<Option<String>, OcrError> {
    recognize_with(image, |chunk| {
        engine.recognize(chunk).map_err(OcrError::from)
    })
}

fn recognize_with(
    image: &CroppedImage,
    mut recognize: impl FnMut(&CroppedImage) -> Result<Vec<String>, OcrError>,
) -> Result<Option<String>, OcrError> {
    let chunks = plan_ocr_chunks(image.width(), image.height());
    if chunks.len() <= 1 {
        let lines = recognize(image)?;
        return Ok(finish(&lines));
    }
    let mut chunk_lines = Vec::with_capacity(chunks.len());
    for rect in chunks {
        let sub = crop_ocr_chunk(image, rect)
            .ok_or_else(|| OcrError::Failed("分段裁剪超出图像范围".to_string()))?;
        chunk_lines.push(recognize(&sub)?);
    }
    let merged = merge_ocr_chunks(&chunk_lines);
    Ok(finish(&merged))
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

    #[test]
    fn segmented_ocr_splits_long_image_and_deduplicates_overlap() {
        // 构造高度 2100 px 的竖向长图（超过 1400 px 阈值，应切成 3 段）
        let width = 400u32;
        let height = 2100u32;
        let mut rgba = vec![0u8; usize::try_from(width * height * 4).expect("长度")];
        // 在每 100 px 标记不同灰度，便于确认传入识别器的每段子图确实对应不同纵向位置
        for row in 0..height {
            let tag = u8::try_from((row / 100) % 250).unwrap_or(0);
            let start = usize::try_from(row * width * 4).expect("起点");
            if let Some(px) = rgba.get_mut(start) {
                *px = tag;
            }
        }
        let long_img = CroppedImage::new(width, height, rgba).expect("构造长图");
        let mut seen_heights = Vec::new();
        let text = recognize_with(&long_img, |chunk| {
            seen_heights.push((
                chunk.width(),
                chunk.height(),
                *chunk.rgba().first().unwrap_or(&0),
            ));
            let idx = seen_heights.len();
            Ok(match idx {
                1 => vec!["第一行".to_string(), "第二行".to_string()],
                2 => vec!["第二行".to_string(), "第三行".to_string()],
                _ => vec!["第三行".to_string(), "第四行".to_string()],
            })
        })
        .expect("分段识别");
        assert_eq!(
            seen_heights,
            vec![(400, 1000, 0), (400, 1000, 8), (400, 1000, 11)]
        );
        assert_eq!(
            text.as_deref(),
            Some("第一行\r\n第二行\r\n第三行\r\n第四行")
        );
    }
}
