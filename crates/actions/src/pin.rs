//! 剪贴板图片解码：把原始 PNG 或 DIB 字节解码成内存中的 [`CroppedImage`]。

use std::io::Cursor;

use lenswin_core::error::ActionError;
use lenswin_core::frame::CroppedImage;
use lenswin_core::pin::{MAX_IMAGE_PIXELS, MAX_IMAGE_SIDE, decode_dib};
use lenswin_platform::clipboard::{ClipboardImage, ClipboardImageFormat};

/// 解码从剪贴板取出的原始图像字节（应在工作线程中调用）。
pub fn decode_clipboard_image(image: &ClipboardImage) -> Result<CroppedImage, ActionError> {
    match image.format {
        ClipboardImageFormat::Png => decode_png(&image.bytes),
        ClipboardImageFormat::DibV5 | ClipboardImageFormat::Dib => {
            decode_dib(&image.bytes).map_err(|e| ActionError::Failed(format!("DIB 解码失败：{e}")))
        }
    }
}

fn decode_png(bytes: &[u8]) -> Result<CroppedImage, ActionError> {
    let limits = png::Limits {
        bytes: 512 * 1024 * 1024,
    };
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(bytes), limits);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|e| ActionError::Failed(format!("读取 PNG 头失败：{e}")))?;

    let (width, height) = {
        let info = reader.info();
        (info.width, info.height)
    };
    if width == 0
        || height == 0
        || width > MAX_IMAGE_SIDE
        || height > MAX_IMAGE_SIDE
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return Err(ActionError::Failed("PNG 尺寸无效或超出上限".to_string()));
    }

    let buf_size = reader
        .output_buffer_size()
        .ok_or_else(|| ActionError::Failed("PNG 解码缓冲区过大".to_string()))?;
    let mut buf = vec![0u8; buf_size];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| ActionError::Failed(format!("PNG 解码失败：{e}")))?;
    if info.bit_depth != png::BitDepth::Eight {
        return Err(ActionError::Failed("不支持的 PNG 位深".to_string()));
    }

    let raw = buf
        .get(..info.buffer_size())
        .ok_or_else(|| ActionError::Failed("PNG 帧缓冲区长度不足".to_string()))?;
    let rgba = match info.color_type {
        png::ColorType::Rgba => raw.to_vec(),
        png::ColorType::Rgb => rgb_to_rgba(raw),
        png::ColorType::GrayscaleAlpha => gray_alpha_to_rgba(raw),
        png::ColorType::Grayscale => gray_to_rgba(raw),
        png::ColorType::Indexed => {
            return Err(ActionError::Failed("不支持索引色 PNG".to_string()));
        }
    };
    CroppedImage::new(info.width, info.height, rgba)
        .map_err(|e| ActionError::Failed(format!("构造图像失败：{e}")))
}

fn rgb_to_rgba(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity((raw.len() / 3).saturating_mul(4));
    for &[r, g, b] in raw.as_chunks::<3>().0 {
        out.extend_from_slice(&[r, g, b, 255]);
    }
    out
}

fn gray_alpha_to_rgba(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len().saturating_mul(2));
    for &[y, a] in raw.as_chunks::<2>().0 {
        out.extend_from_slice(&[y, y, y, a]);
    }
    out
}

fn gray_to_rgba(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len().saturating_mul(4));
    for &y in raw {
        out.extend_from_slice(&[y, y, y, 255]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode_png;

    #[test]
    fn decode_png_rgb_roundtrip() {
        let orig =
            CroppedImage::new(2, 1, vec![10, 20, 30, 255, 40, 50, 60, 255]).expect("构造原图");
        let bytes = encode_png(&orig).expect("编码 PNG");
        let clip = ClipboardImage {
            format: ClipboardImageFormat::Png,
            bytes,
        };
        let decoded = decode_clipboard_image(&clip).expect("解码 PNG");
        assert_eq!(decoded, orig);
    }

    #[test]
    fn decode_png_rgba_preserves_alpha() {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("写头");
            writer
                .write_image_data(&[100, 150, 200, 77])
                .expect("写像素");
            writer.finish().expect("结束");
        }
        let clip = ClipboardImage {
            format: ClipboardImageFormat::Png,
            bytes: out,
        };
        let decoded = decode_clipboard_image(&clip).expect("解码 RGBA PNG");
        assert_eq!(decoded.rgba(), &[100, 150, 200, 77]);
    }

    #[test]
    fn decode_dib_via_clipboard_image() {
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&(-1i32).to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&0u32.to_le_bytes());
        dib.extend_from_slice(&[0u8; 20]);
        dib.extend_from_slice(&[30, 20, 10, 0]);
        let clip = ClipboardImage {
            format: ClipboardImageFormat::Dib,
            bytes: dib,
        };
        let decoded = decode_clipboard_image(&clip).expect("解码 DIB");
        assert_eq!(decoded.rgba(), &[10, 20, 30, 255]);
    }

    #[test]
    fn decode_invalid_png_fails_cleanly() {
        let clip = ClipboardImage {
            format: ClipboardImageFormat::Png,
            bytes: b"not a png".to_vec(),
        };
        assert!(decode_clipboard_image(&clip).is_err());
    }
}
