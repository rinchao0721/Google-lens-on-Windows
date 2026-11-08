//! 冻结帧与裁剪结果：RGBA 像素缓冲区加所在虚拟桌面区域。

use crate::error::FrameError;
use crate::geometry::Rect;

const BYTES_PER_PIXEL: usize = 4;

/// 宽×高×4，任何一步溢出都返回错误。
fn rgba_len(width: u32, height: u32) -> Result<usize, FrameError> {
    let w = usize::try_from(width).map_err(|_| FrameError::SizeOverflow)?;
    let h = usize::try_from(height).map_err(|_| FrameError::SizeOverflow)?;
    w.checked_mul(h)
        .and_then(|px| px.checked_mul(BYTES_PER_PIXEL))
        .ok_or(FrameError::SizeOverflow)
}

fn to_usize(value: i64) -> Result<usize, FrameError> {
    usize::try_from(value).map_err(|_| FrameError::SizeOverflow)
}

fn check_len(width: u32, height: u32, actual: usize) -> Result<(), FrameError> {
    if width == 0 || height == 0 {
        return Err(FrameError::EmptyRegion);
    }
    let expected = rgba_len(width, height)?;
    if expected != actual {
        return Err(FrameError::LengthMismatch { expected, actual });
    }
    Ok(())
}

/// 单个显示器的一帧画面，像素为 RGBA 每通道 8 位，按行从上到下排列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    region: Rect,
    rgba: Vec<u8>,
}

impl Frame {
    /// 构造帧；`region` 的宽高必须非零，且 `rgba` 长度等于宽×高×4。
    pub fn new(region: Rect, rgba: Vec<u8>) -> Result<Self, FrameError> {
        check_len(region.width, region.height, rgba.len())?;
        Ok(Self { region, rgba })
    }

    /// 帧在虚拟桌面上的区域（物理像素）。
    pub fn region(&self) -> Rect {
        self.region
    }

    /// 原始 RGBA 像素。
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// 按虚拟桌面坐标裁剪；`bounds` 先与帧区域求交，交集为空时返回错误。
    pub fn crop(&self, bounds: &Rect) -> Result<CroppedImage, FrameError> {
        let area = self.region.intersect(bounds).ok_or(FrameError::EmptyCrop)?;
        let frame_width = to_usize(i64::from(self.region.width))?;
        let offset_x = to_usize(i64::from(area.x) - i64::from(self.region.x))?;
        let offset_y = to_usize(i64::from(area.y) - i64::from(self.region.y))?;
        let rows = to_usize(i64::from(area.height))?;
        let row_bytes = rgba_len(area.width, 1)?;

        let mut out = Vec::with_capacity(rgba_len(area.width, area.height)?);
        for row in 0..rows {
            let start = offset_y
                .checked_add(row)
                .and_then(|y| y.checked_mul(frame_width))
                .and_then(|px| px.checked_add(offset_x))
                .and_then(|px| px.checked_mul(BYTES_PER_PIXEL))
                .ok_or(FrameError::SizeOverflow)?;
            let end = start
                .checked_add(row_bytes)
                .ok_or(FrameError::SizeOverflow)?;
            let line = self.rgba.get(start..end).ok_or(FrameError::SizeOverflow)?;
            out.extend_from_slice(line);
        }
        CroppedImage::new(area.width, area.height, out)
    }
}

/// 裁剪后的图像，仅在内存中流转。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CroppedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl CroppedImage {
    /// 构造图像；宽高必须非零，且 `rgba` 长度等于宽×高×4。
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, FrameError> {
        check_len(width, height, rgba.len())?;
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    /// 宽度（像素）。
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 高度（像素）。
    pub fn height(&self) -> u32 {
        self.height
    }

    /// 原始 RGBA 像素。
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// 取出像素缓冲区。
    pub fn into_rgba(self) -> Vec<u8> {
        self.rgba
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个像素的 R 通道记录其线性序号，便于校验裁剪位置。
    fn numbered_frame(region: Rect) -> Frame {
        let count = usize::try_from(region.width * region.height).expect("测试尺寸很小");
        let mut buf = Vec::new();
        for i in 0..count {
            let r = u8::try_from(i % 256).expect("取模后不超过 255");
            buf.extend_from_slice(&[r, 0, 0, 255]);
        }
        Frame::new(region, buf).expect("构造测试帧")
    }

    #[test]
    fn new_rejects_length_mismatch() {
        let err = Frame::new(Rect::new(0, 0, 2, 2), vec![0; 15]).unwrap_err();
        assert_eq!(
            err,
            FrameError::LengthMismatch {
                expected: 16,
                actual: 15
            }
        );
    }

    #[test]
    fn new_rejects_zero_size() {
        assert_eq!(
            Frame::new(Rect::new(0, 0, 0, 5), vec![]).unwrap_err(),
            FrameError::EmptyRegion
        );
        assert_eq!(
            CroppedImage::new(3, 0, vec![]).unwrap_err(),
            FrameError::EmptyRegion
        );
    }

    #[test]
    fn crop_inner_region_copies_correct_pixels() {
        let frame = numbered_frame(Rect::new(0, 0, 4, 4));
        let img = frame.crop(&Rect::new(1, 1, 2, 2)).expect("裁剪");
        assert_eq!((img.width(), img.height()), (2, 2));
        let reds: Vec<u8> = img
            .rgba()
            .chunks(4)
            .map(|px| px.first().copied().unwrap_or(0))
            .collect();
        assert_eq!(reds, vec![5, 6, 9, 10]);
    }

    #[test]
    fn crop_uses_virtual_desktop_coordinates_with_negative_origin() {
        let frame = numbered_frame(Rect::new(-4, 0, 4, 2));
        let img = frame.crop(&Rect::new(-3, 1, 2, 1)).expect("裁剪");
        let reds: Vec<u8> = img
            .rgba()
            .chunks(4)
            .map(|px| px.first().copied().unwrap_or(0))
            .collect();
        assert_eq!(reds, vec![5, 6]);
    }

    #[test]
    fn crop_out_of_bounds_is_clamped() {
        let frame = numbered_frame(Rect::new(0, 0, 4, 4));
        let img = frame.crop(&Rect::new(2, 2, 100, 100)).expect("裁剪");
        assert_eq!((img.width(), img.height()), (2, 2));
        assert_eq!(img.rgba().len(), 16);
    }

    #[test]
    fn crop_without_overlap_fails() {
        let frame = numbered_frame(Rect::new(0, 0, 4, 4));
        assert_eq!(
            frame.crop(&Rect::new(10, 10, 5, 5)).unwrap_err(),
            FrameError::EmptyCrop
        );
    }

    #[test]
    fn crop_zero_size_fails() {
        let frame = numbered_frame(Rect::new(0, 0, 4, 4));
        assert_eq!(
            frame.crop(&Rect::new(1, 1, 0, 3)).unwrap_err(),
            FrameError::EmptyCrop
        );
    }

    #[test]
    fn crop_whole_frame_equals_original() {
        let frame = numbered_frame(Rect::new(0, 0, 3, 3));
        let img = frame.crop(&frame.region()).expect("裁剪");
        assert_eq!(img.rgba(), frame.rgba());
    }
}
