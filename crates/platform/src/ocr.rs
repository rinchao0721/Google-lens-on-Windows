//! Windows 本地文字识别（Windows.Media.Ocr WinRT 封装）。
//!
//! 所有直接调用 WinRT 的函数都要求当前线程已进入 COM MTA（参见 [`crate::com::MtaScope`]）。

use lenswin_core::frame::CroppedImage;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, E_NOINTERFACE, E_NOTIMPL, REGDB_E_CLASSNOTREG,
};
use windows::core::HSTRING;

use crate::error::{PlatformError, win32};

/// 系统已安装的 OCR 识别语言。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrLanguage {
    /// BCP-47 语言标签，如 `zh-Hans-CN`、`en-US`。
    pub tag: String,
    /// 系统提供的本地化显示名称。
    pub display_name: String,
}

fn is_unsupported_error(err: &windows::core::Error) -> bool {
    let code = err.code();
    code == REGDB_E_CLASSNOTREG
        || code == CLASS_E_CLASSNOTAVAILABLE
        || code == E_NOTIMPL
        || code == E_NOINTERFACE
}

fn map_ocr_statics_error(api: &'static str, err: windows::core::Error) -> PlatformError {
    if is_unsupported_error(&err) {
        PlatformError::OcrUnsupported
    } else {
        win32(api)(err)
    }
}

/// 查询系统 OCR 引擎允许的最大单边像素尺寸；调用方须处于 MTA 线程。
pub fn max_image_dimension() -> Result<u32, PlatformError> {
    OcrEngine::MaxImageDimension()
        .map_err(|e| map_ocr_statics_error("OcrEngine::MaxImageDimension", e))
}

/// 查询系统已安装的 OCR 语言列表；调用方须处于 MTA 线程。
pub fn available_languages() -> Result<Vec<OcrLanguage>, PlatformError> {
    let list = OcrEngine::AvailableRecognizerLanguages()
        .map_err(|e| map_ocr_statics_error("OcrEngine::AvailableRecognizerLanguages", e))?;
    let count = list.Size().map_err(win32("IVectorView::Size"))?;
    let cap = usize::try_from(count).unwrap_or(0);
    let mut out = Vec::with_capacity(cap);
    for idx in 0..count {
        let lang = list.GetAt(idx).map_err(win32("IVectorView::GetAt"))?;
        let tag = lang
            .LanguageTag()
            .map_err(win32("Language::LanguageTag"))?
            .to_string_lossy();
        let display_name = lang
            .DisplayName()
            .map_err(win32("Language::DisplayName"))?
            .to_string_lossy();
        out.push(OcrLanguage { tag, display_name });
    }
    Ok(out)
}

/// 对图像执行一次本地文字识别，返回按顺序排列的文本行；调用方须处于 MTA 线程。
///
/// `language_tag` 为 `None` 时使用系统用户配置文件语言。
pub fn recognize(
    image: &CroppedImage,
    language_tag: Option<&str>,
) -> Result<Vec<String>, PlatformError> {
    let max_dim = max_image_dimension()?;
    if image.width() > max_dim || image.height() > max_dim {
        return Err(PlatformError::OcrImageTooLarge);
    }

    let engine = match language_tag {
        Some(tag) => create_engine_for_tag(tag)?,
        None => OcrEngine::TryCreateFromUserProfileLanguages().map_err(|e| {
            if e.code().0 == 0 {
                PlatformError::OcrLanguageUnavailable("系统语言".to_string())
            } else {
                map_ocr_statics_error("OcrEngine::TryCreateFromUserProfileLanguages", e)
            }
        })?,
    };

    let bitmap = create_software_bitmap(image)?;
    let op = engine
        .RecognizeAsync(&bitmap)
        .map_err(win32("OcrEngine::RecognizeAsync"))?;
    let result = op
        .join()
        .map_err(win32("OcrEngine::RecognizeAsync::join"))?;
    let lines = result.Lines().map_err(win32("OcrResult::Lines"))?;
    let count = lines.Size().map_err(win32("IVectorView::Size"))?;
    let mut out = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    for idx in 0..count {
        let line = lines.GetAt(idx).map_err(win32("IVectorView::GetAt"))?;
        let text = line
            .Text()
            .map_err(win32("OcrLine::Text"))?
            .to_string_lossy();
        out.push(text);
    }
    Ok(out)
}

fn create_engine_for_tag(tag: &str) -> Result<OcrEngine, PlatformError> {
    let htag = HSTRING::from(tag);
    let lang = Language::CreateLanguage(&htag)
        .map_err(|_| PlatformError::OcrLanguageUnavailable(tag.to_string()))?;
    let supported = OcrEngine::IsLanguageSupported(&lang)
        .map_err(|e| map_ocr_statics_error("OcrEngine::IsLanguageSupported", e))?;
    if !supported {
        return Err(PlatformError::OcrLanguageUnavailable(tag.to_string()));
    }
    OcrEngine::TryCreateFromLanguage(&lang).map_err(|e| {
        if e.code().0 == 0 {
            PlatformError::OcrLanguageUnavailable(tag.to_string())
        } else {
            map_ocr_statics_error("OcrEngine::TryCreateFromLanguage", e)
        }
    })
}

fn create_software_bitmap(image: &CroppedImage) -> Result<SoftwareBitmap, PlatformError> {
    let width = i32::try_from(image.width()).map_err(|_| PlatformError::OcrImageTooLarge)?;
    let height = i32::try_from(image.height()).map_err(|_| PlatformError::OcrImageTooLarge)?;
    let bgra = rgba_to_premultiplied_bgra(image.rgba());
    let writer = DataWriter::new().map_err(win32("DataWriter::new"))?;
    writer
        .WriteBytes(&bgra)
        .map_err(win32("DataWriter::WriteBytes"))?;
    let buffer = writer
        .DetachBuffer()
        .map_err(win32("DataWriter::DetachBuffer"))?;
    SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        width,
        height,
        BitmapAlphaMode::Premultiplied,
    )
    .map_err(win32("SoftwareBitmap::CreateCopyWithAlphaFromBuffer"))
}

/// 把 RGBA8 像素转成预乘 Alpha 的 BGRA8，供 `SoftwareBitmap` 使用。
fn rgba_to_premultiplied_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        if let [r, g, b, a] = *px {
            if a == 255 {
                out.extend_from_slice(&[b, g, r, 255]);
            } else if a == 0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let premul = |c: u8| {
                    let v = (u16::from(c) * u16::from(a) + 127) / 255;
                    u8::try_from(v).unwrap_or(u8::MAX)
                };
                out.extend_from_slice(&[premul(b), premul(g), premul(r), a]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::MtaScope;

    #[test]
    fn premultiplied_bgra_swaps_channels_and_scales_alpha() {
        let rgba = [10, 20, 30, 255, 255, 100, 0, 0, 200, 100, 50, 128];
        let got = rgba_to_premultiplied_bgra(&rgba);
        assert_eq!(got.get(0..4), Some(&[30u8, 20, 10, 255][..]));
        assert_eq!(got.get(4..8), Some(&[0u8, 0, 0, 0][..]));
        // 50 * 128 / 255 ≈ 25, 100 * 128 / 255 ≈ 50, 200 * 128 / 255 ≈ 100
        assert_eq!(got.get(8..12), Some(&[25u8, 50, 100, 128][..]));
    }

    #[test]
    fn query_available_languages_on_mta_thread() {
        let handle = std::thread::spawn(|| {
            let _mta = MtaScope::enter().expect("进入 MTA");
            let max_dim = max_image_dimension().expect("查询最大尺寸");
            assert!(max_dim >= 1000);
            let langs = available_languages().expect("查询已安装语言");
            for lang in &langs {
                assert!(!lang.tag.is_empty());
                assert!(!lang.display_name.is_empty());
            }
        });
        handle.join().expect("工作线程完成");
    }
}
