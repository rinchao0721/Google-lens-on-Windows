//! 本地文字识别动作：按配置语言列表与多语言策略组合识别流程。

use std::fmt;

use lenswin_core::config::{OcrConfig, OcrStrategy};
use lenswin_core::frame::CroppedImage;
use lenswin_core::ocr::{accept_fallback_lines, join_lines, pick_best, prepare_image};
use lenswin_platform::error::PlatformError;
use lenswin_platform::ocr as platform_ocr;

/// 本地文字识别失败的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrError {
    /// 当前系统不支持本地文字识别。
    Unsupported,
    /// 所有目标语言均未安装或不可用。
    NoLanguageAvailable,
    /// 图像尺寸无效或超出系统引擎上限。
    ImageTooLarge,
    /// 其它系统调用错误。
    Failed(String),
}

impl fmt::Display for OcrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OcrError::Unsupported => f.write_str("当前系统不支持本地文字识别"),
            OcrError::NoLanguageAvailable => f.write_str(
                "所选识别语言均未安装，请在系统「设置 → 时间和语言 → 语言」中安装对应语言包",
            ),
            OcrError::ImageTooLarge => f.write_str("图像尺寸超出文字识别上限"),
            OcrError::Failed(reason) => write!(f, "文字识别失败：{reason}"),
        }
    }
}

impl std::error::Error for OcrError {}

/// 对裁剪图像执行本地文字识别；调用方须已在当前线程初始化 COM MTA。
///
/// 识别出非空文本返回 `Ok(Some(text))`，未识别到文字返回 `Ok(None)`。
pub fn recognize_image(
    config: &OcrConfig,
    image: CroppedImage,
) -> Result<Option<String>, OcrError> {
    recognize_with(
        config,
        image,
        platform_ocr::max_image_dimension,
        platform_ocr::recognize,
    )
}

fn map_platform_error(err: PlatformError) -> OcrError {
    match err {
        PlatformError::OcrUnsupported => OcrError::Unsupported,
        PlatformError::OcrLanguageUnavailable(_) => OcrError::NoLanguageAvailable,
        PlatformError::OcrImageTooLarge => OcrError::ImageTooLarge,
        other => OcrError::Failed(other.to_string()),
    }
}

fn recognize_with<M, F>(
    config: &OcrConfig,
    image: CroppedImage,
    max_dimension: M,
    mut run_one: F,
) -> Result<Option<String>, OcrError>
where
    M: FnOnce() -> Result<u32, PlatformError>,
    F: FnMut(&CroppedImage, Option<&str>) -> Result<Vec<String>, PlatformError>,
{
    let max_dim = max_dimension().map_err(map_platform_error)?;
    let prepared = prepare_image(image, max_dim).map_err(|_| OcrError::ImageTooLarge)?;

    if config.languages.is_empty() {
        let lines = run_one(&prepared, None).map_err(map_platform_error)?;
        return Ok(accept_fallback_lines(&lines));
    }

    let mut available_count = 0usize;
    match config.strategy {
        OcrStrategy::Fallback => {
            for tag in &config.languages {
                match run_one(&prepared, Some(tag)) {
                    Ok(lines) => {
                        available_count = available_count.saturating_add(1);
                        if let Some(text) = accept_fallback_lines(&lines) {
                            return Ok(Some(text));
                        }
                    }
                    Err(PlatformError::OcrLanguageUnavailable(_)) => {
                        log::warn!("OCR 语言 {tag} 未安装或不可用，已跳过");
                    }
                    Err(other) => return Err(map_platform_error(other)),
                }
            }
            if available_count == 0 {
                Err(OcrError::NoLanguageAvailable)
            } else {
                Ok(None)
            }
        }
        OcrStrategy::Best => {
            let mut candidates = Vec::with_capacity(config.languages.len());
            for tag in &config.languages {
                match run_one(&prepared, Some(tag)) {
                    Ok(lines) => {
                        available_count = available_count.saturating_add(1);
                        candidates.push(join_lines(&lines));
                    }
                    Err(PlatformError::OcrLanguageUnavailable(_)) => {
                        log::warn!("OCR 语言 {tag} 未安装或不可用，已跳过");
                    }
                    Err(other) => return Err(map_platform_error(other)),
                }
            }
            if available_count == 0 {
                Err(OcrError::NoLanguageAvailable)
            } else {
                Ok(pick_best(candidates))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_image(width: u32, height: u32) -> CroppedImage {
        let len = usize::try_from(width)
            .expect("宽")
            .checked_mul(usize::try_from(height).expect("高"))
            .expect("像素数")
            * 4;
        CroppedImage::new(width, height, vec![255; len]).expect("测试图")
    }

    #[test]
    fn empty_languages_uses_system_profile_and_returns_none_when_blank() {
        let cfg = OcrConfig::default();
        let mut seen = Vec::new();
        let got = recognize_with(
            &cfg,
            sample_image(10, 10),
            || Ok(4096),
            |img, lang| {
                // 高度 < 40 已按整数倍放大到 40x40
                assert_eq!((img.width(), img.height()), (40, 40));
                seen.push(lang.map(str::to_string));
                Ok(vec!["  ".to_string()])
            },
        );
        assert_eq!(got, Ok(None));
        assert_eq!(seen, vec![None]);
    }

    #[test]
    fn fallback_stops_at_first_non_empty_and_skips_missing_languages() {
        let cfg = OcrConfig {
            languages: vec![
                "ja".to_string(),
                "en-US".to_string(),
                "zh-Hans-CN".to_string(),
            ],
            strategy: OcrStrategy::Fallback,
        };
        let mut calls = Vec::new();
        let got = recognize_with(
            &cfg,
            sample_image(100, 50),
            || Ok(4096),
            |_, lang| {
                let tag = lang.expect("非空列表应传具体标签").to_string();
                calls.push(tag.clone());
                match tag.as_str() {
                    "ja" => Err(PlatformError::OcrLanguageUnavailable(tag)),
                    "en-US" => Ok(vec!["Hello".to_string()]),
                    _ => Ok(vec!["你好".to_string()]),
                }
            },
        );
        assert_eq!(got, Ok(Some("Hello".to_string())));
        assert_eq!(calls, vec!["ja", "en-US"]);
    }

    #[test]
    fn best_runs_all_available_languages_and_picks_highest_score() {
        let cfg = OcrConfig {
            languages: vec![
                "en-US".to_string(),
                "fr-FR".to_string(),
                "zh-Hans-CN".to_string(),
            ],
            strategy: OcrStrategy::Best,
        };
        let mut calls = Vec::new();
        let got = recognize_with(
            &cfg,
            sample_image(100, 50),
            || Ok(4096),
            |_, lang| {
                let tag = lang.expect("非空列表").to_string();
                calls.push(tag.clone());
                match tag.as_str() {
                    "en-US" => Ok(vec!["a b".to_string()]),
                    "fr-FR" => Err(PlatformError::OcrLanguageUnavailable(tag)),
                    "zh-Hans-CN" => Ok(vec!["中 文 测 试".to_string()]),
                    _ => Ok(Vec::new()),
                }
            },
        );
        assert_eq!(got, Ok(Some("中文测试".to_string())));
        assert_eq!(calls, vec!["en-US", "fr-FR", "zh-Hans-CN"]);
    }

    #[test]
    fn returns_no_language_available_when_all_skipped() {
        let cfg = OcrConfig {
            languages: vec!["ja".to_string(), "ko".to_string()],
            strategy: OcrStrategy::Best,
        };
        let got = recognize_with(
            &cfg,
            sample_image(100, 50),
            || Ok(4096),
            |_, lang| {
                Err(PlatformError::OcrLanguageUnavailable(
                    lang.unwrap_or("").to_string(),
                ))
            },
        );
        assert_eq!(got, Err(OcrError::NoLanguageAvailable));
        assert!(got.expect_err("错误").to_string().contains("时间和语言"));
    }

    #[test]
    fn maps_unsupported_and_image_too_large_errors() {
        let cfg = OcrConfig::default();
        let unsupported = recognize_with(
            &cfg,
            sample_image(100, 50),
            || Err(PlatformError::OcrUnsupported),
            |_, _| Ok(Vec::new()),
        );
        assert_eq!(unsupported, Err(OcrError::Unsupported));

        let too_large = recognize_with(
            &cfg,
            sample_image(100, 50),
            || Ok(4096),
            |_, _| Err(PlatformError::OcrImageTooLarge),
        );
        assert_eq!(too_large, Err(OcrError::ImageTooLarge));
    }
}
