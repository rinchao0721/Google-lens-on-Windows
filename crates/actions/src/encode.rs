//! 把裁剪后的图像编码为 PNG。

use lenswin_core::error::ActionError;
use lenswin_core::frame::CroppedImage;

/// 编码为 PNG；截屏像素本就不透明，丢弃 alpha 通道以缩小体积。
pub fn encode_png(image: &CroppedImage) -> Result<Vec<u8>, ActionError> {
    let fail = |e: png::EncodingError| ActionError::Failed(format!("PNG 编码失败：{e}"));
    let mut rgb = Vec::with_capacity(image.rgba().len() / 4 * 3);
    for px in image.rgba().chunks_exact(4) {
        if let [r, g, b, _] = *px {
            rgb.extend_from_slice(&[r, g, b]);
        }
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, image.width(), image.height());
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(fail)?;
        writer.write_image_data(&rgb).map_err(fail)?;
        writer.finish().map_err(fail)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip_preserves_pixels() {
        let rgba = vec![
            255, 0, 0, 255, 0, 255, 0, 255, //
            0, 0, 255, 255, 10, 20, 30, 255,
        ];
        let image = CroppedImage::new(2, 2, rgba).expect("图像");
        let bytes = encode_png(&image).expect("编码");
        assert_eq!(
            bytes.get(..8),
            Some(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A][..])
        );

        let mut reader = png::Decoder::new(Cursor::new(bytes))
            .read_info()
            .expect("解码头");
        let mut buf = vec![0; reader.output_buffer_size().expect("缓冲区大小")];
        let info = reader.next_frame(&mut buf).expect("解码");
        assert_eq!((info.width, info.height), (2, 2));
        assert_eq!(
            buf.get(..info.buffer_size()),
            Some(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 10, 20, 30][..])
        );
    }
}
