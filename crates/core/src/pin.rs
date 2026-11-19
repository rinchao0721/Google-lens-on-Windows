//! 贴图纯逻辑：事件、旋转、缩放与透明度计算、工作区适配、DIB 解码。

use thiserror::Error;

use crate::error::FrameError;
use crate::frame::CroppedImage;
use crate::geometry::{Point, Rect};

/// 贴图窗口数量上限。
pub const MAX_PIN_COUNT: usize = 32;

/// 默认缩放比例（千分比，1000 表示 100%）。
pub const ZOOM_DEFAULT: u32 = 1000;
/// 滚轮缩放下限（千分比，100 表示 10%）。
pub const ZOOM_MIN: u32 = 100;
/// 滚轮缩放上限（千分比，5000 表示 500%）。
pub const ZOOM_MAX: u32 = 5000;
/// 滚轮每次缩放步进（千分比，100 表示 10%）。
pub const ZOOM_STEP: u32 = 100;

/// 默认不透明度（百分比，100 表示 100%）。
pub const OPACITY_DEFAULT: u32 = 100;
/// 不透明度下限（百分比）。
pub const OPACITY_MIN: u32 = 10;
/// 不透明度上限（百分比）。
pub const OPACITY_MAX: u32 = 100;
/// 每次调节不透明度的步进（百分比）。
pub const OPACITY_STEP: u32 = 10;

/// 允许解码的单边最大像素数。
pub const MAX_IMAGE_SIDE: u32 = 16_384;
/// 允许解码的总像素上限。
pub const MAX_IMAGE_PIXELS: u64 = 100_000_000;

const BITMAPINFOHEADER_SIZE: u32 = 40;
const BITMAPV2HEADER_SIZE: u32 = 52;
const BITMAPV3HEADER_SIZE: u32 = 56;
const BITMAPV4HEADER_SIZE: u32 = 108;
const BITMAPV5HEADER_SIZE: u32 = 124;

const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;

/// 贴图窗口回投给主线程的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinEvent {
    /// 复制当前贴图（已应用旋转，不含缩放与透明度）。
    Copy(CroppedImage),
    /// 保存当前贴图（已应用旋转，不含缩放与透明度）。
    Save(CroppedImage),
    /// 以图搜图（已应用旋转的图，以及贴图当前屏幕矩形）。
    Search(CroppedImage, Rect),
    /// 识别文字（已应用旋转，不含缩放与透明度）。
    Ocr(CroppedImage),
}

/// 顺时针旋转角度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation {
    /// 0°（未旋转）。
    #[default]
    Deg0,
    /// 顺时针 90°。
    Deg90,
    /// 180°。
    Deg180,
    /// 顺时针 270°（逆时针 90°）。
    Deg270,
}

impl Rotation {
    /// 顺时针旋转 90°。
    pub const fn clockwise(self) -> Self {
        match self {
            Rotation::Deg0 => Rotation::Deg90,
            Rotation::Deg90 => Rotation::Deg180,
            Rotation::Deg180 => Rotation::Deg270,
            Rotation::Deg270 => Rotation::Deg0,
        }
    }

    /// 逆时针旋转 90°。
    pub const fn counter_clockwise(self) -> Self {
        match self {
            Rotation::Deg0 => Rotation::Deg270,
            Rotation::Deg90 => Rotation::Deg0,
            Rotation::Deg180 => Rotation::Deg90,
            Rotation::Deg270 => Rotation::Deg180,
        }
    }

    /// 旋转后的逻辑像素宽高。
    pub const fn dimensions(self, width: u32, height: u32) -> (u32, u32) {
        match self {
            Rotation::Deg0 | Rotation::Deg180 => (width, height),
            Rotation::Deg90 | Rotation::Deg270 => (height, width),
        }
    }

    /// 对应的顺时针角度（供 Direct2D 变换使用）。
    pub const fn degrees_f32(self) -> f32 {
        match self {
            Rotation::Deg0 => 0.0,
            Rotation::Deg90 => 90.0,
            Rotation::Deg180 => 180.0,
            Rotation::Deg270 => 270.0,
        }
    }
}

/// 按步数调节缩放千分比并夹紧到 `[ZOOM_MIN, ZOOM_MAX]`。
///
/// 若初始缩放因工作区适配低于 `ZOOM_MIN`，继续缩小时保持原值不反跳。
pub fn step_zoom(current_permille: u32, delta_steps: i32) -> u32 {
    if delta_steps == 0 {
        return if current_permille < ZOOM_MIN {
            current_permille.max(1)
        } else {
            current_permille.min(ZOOM_MAX)
        };
    }
    if current_permille < ZOOM_MIN && delta_steps < 0 {
        return current_permille.max(1);
    }
    let delta = i64::from(delta_steps).saturating_mul(i64::from(ZOOM_STEP));
    let next = i64::from(current_permille).saturating_add(delta);
    let clamped = next.clamp(i64::from(ZOOM_MIN), i64::from(ZOOM_MAX));
    u32::try_from(clamped).unwrap_or(ZOOM_MIN)
}

/// 按步数调节不透明度百分比并夹紧到 `[OPACITY_MIN, OPACITY_MAX]`。
pub fn step_opacity(current_pct: u32, delta_steps: i32) -> u32 {
    let delta = i64::from(delta_steps).saturating_mul(i64::from(OPACITY_STEP));
    let next = i64::from(current_pct).saturating_add(delta);
    let clamped = next.clamp(i64::from(OPACITY_MIN), i64::from(OPACITY_MAX));
    u32::try_from(clamped).unwrap_or(OPACITY_DEFAULT)
}

/// 不透明度百分比转 `0..=255` Alpha 字节。
pub fn opacity_to_alpha(pct: u32) -> u8 {
    let clamped = pct.clamp(OPACITY_MIN, OPACITY_MAX);
    let alpha = (clamped * 255 + 50) / 100;
    u8::try_from(alpha).unwrap_or(255)
}

/// 根据旋转后尺寸与缩放千分比计算窗口物理像素尺寸，宽高至少为 1。
pub fn scaled_size(rot_width: u32, rot_height: u32, zoom_permille: u32) -> (u32, u32) {
    let scale = |side: u32| -> u32 {
        let px = (u64::from(side) * u64::from(zoom_permille) + 500) / 1000;
        u32::try_from(px.clamp(1, u64::from(u32::MAX))).unwrap_or(u32::MAX)
    };
    (scale(rot_width), scale(rot_height))
}

/// 以当前窗口中心为基准计算旋转后的新窗口矩形。
pub fn rotate_around_center(rect: Rect, new_width: u32, new_height: u32) -> Rect {
    let dx = (i64::from(rect.width) - i64::from(new_width)) / 2;
    let dy = (i64::from(rect.height) - i64::from(new_height)) / 2;
    let x = i64::from(rect.x)
        .saturating_add(dx)
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    let y = i64::from(rect.y)
        .saturating_add(dy)
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    Rect::new(
        i32::try_from(x).unwrap_or(rect.x),
        i32::try_from(y).unwrap_or(rect.y),
        new_width.max(1),
        new_height.max(1),
    )
}

/// 以鼠标屏幕坐标为中心计算缩放后的新窗口矩形。
pub fn zoom_around_cursor(rect: Rect, cursor: Point, new_width: u32, new_height: u32) -> Rect {
    let nw = new_width.max(1);
    let nh = new_height.max(1);
    let old_w = rect.width.max(1);
    let old_h = rect.height.max(1);

    let new_x = zoom_axis(cursor.x, rect.x, nw, old_w);
    let new_y = zoom_axis(cursor.y, rect.y, nh, old_h);
    let x = i32::try_from(new_x.clamp(i64::from(i32::MIN), i64::from(i32::MAX))).unwrap_or(rect.x);
    let y = i32::try_from(new_y.clamp(i64::from(i32::MIN), i64::from(i32::MAX))).unwrap_or(rect.y);
    Rect::new(x, y, nw, nh)
}

/// 单轴缩放：保持光标在窗口内的相对位置不变，偏移量四舍五入（半数远离零）。
fn zoom_axis(cursor: i32, origin: i32, new_len: u32, old_len: u32) -> i64 {
    let offset = i64::from(cursor) - i64::from(origin);
    let num = offset.saturating_mul(i64::from(new_len));
    let den = i64::from(old_len.max(1));
    let half = den / 2;
    let scaled = if num >= 0 {
        num.saturating_add(half) / den
    } else {
        num.saturating_sub(half) / den
    };
    i64::from(cursor).saturating_sub(scaled)
}

/// 计算图片贴到指定工作区中央时的初始矩形与缩放千分比；超出工作区时等比缩小。
pub fn fit_work_area(image_width: u32, image_height: u32, work_area: Rect) -> (Rect, u32) {
    let img_w = image_width.max(1);
    let img_h = image_height.max(1);
    let area_w = work_area.width.max(1);
    let area_h = work_area.height.max(1);

    let mut zoom = if img_w <= area_w && img_h <= area_h {
        ZOOM_DEFAULT
    } else {
        let zw = u64::from(area_w) * 1000 / u64::from(img_w);
        let zh = u64::from(area_h) * 1000 / u64::from(img_h);
        u32::try_from(zw.min(zh).clamp(1, u64::from(ZOOM_DEFAULT))).unwrap_or(1)
    };

    let (mut w, mut h) = scaled_size(img_w, img_h, zoom);
    if (w > area_w || h > area_h) && zoom > 1 {
        zoom -= 1;
        (w, h) = scaled_size(img_w, img_h, zoom);
    }
    w = w.min(area_w).max(1);
    h = h.min(area_h).max(1);

    let x = i64::from(work_area.x) + (i64::from(area_w) - i64::from(w)) / 2;
    let y = i64::from(work_area.y) + (i64::from(area_h) - i64::from(h)) / 2;
    let clamped_x = x.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    let clamped_y = y.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    (
        Rect::new(
            i32::try_from(clamped_x).unwrap_or(work_area.x),
            i32::try_from(clamped_y).unwrap_or(work_area.y),
            w,
            h,
        ),
        zoom,
    )
}

/// 按旋转角度转置导出图像像素（只转置，不做缩放和透明度合成）。
pub fn rotate_image(image: &CroppedImage, rotation: Rotation) -> Result<CroppedImage, FrameError> {
    if rotation == Rotation::Deg0 {
        return Ok(image.clone());
    }
    let w = usize::try_from(image.width()).map_err(|_| FrameError::SizeOverflow)?;
    let h = usize::try_from(image.height()).map_err(|_| FrameError::SizeOverflow)?;
    let (dst_w, dst_h) = rotation.dimensions(image.width(), image.height());
    let dw = usize::try_from(dst_w).map_err(|_| FrameError::SizeOverflow)?;
    let dh = usize::try_from(dst_h).map_err(|_| FrameError::SizeOverflow)?;

    let total = dw
        .checked_mul(dh)
        .and_then(|px| px.checked_mul(4))
        .ok_or(FrameError::SizeOverflow)?;
    let src = image.rgba();
    let mut dst = Vec::with_capacity(total);

    for dy in 0..dh {
        for dx in 0..dw {
            let (sx, sy) = match rotation {
                Rotation::Deg0 => (dx, dy),
                Rotation::Deg90 => (dy, h - 1 - dx),
                Rotation::Deg180 => (w - 1 - dx, h - 1 - dy),
                Rotation::Deg270 => (w - 1 - dy, dx),
            };
            let offset = sy
                .checked_mul(w)
                .and_then(|px| px.checked_add(sx))
                .and_then(|px| px.checked_mul(4))
                .ok_or(FrameError::SizeOverflow)?;
            let px = src
                .get(offset..offset + 4)
                .ok_or(FrameError::SizeOverflow)?;
            dst.extend_from_slice(px);
        }
    }

    CroppedImage::new(dst_w, dst_h, dst)
}

/// DIB 解码错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DibError {
    /// 数据长度不足。
    #[error("DIB 数据长度不足")]
    TooShort,
    /// 不支持的头版本、位深或压缩方式。
    #[error("不支持的 DIB 格式")]
    UnsupportedFormat,
    /// 图像宽高为 0、超出上限或尺寸溢出。
    #[error("DIB 尺寸无效或过大")]
    InvalidSize,
    /// 像素构造失败。
    #[error("构建图像失败：{0}")]
    Frame(#[from] FrameError),
}

/// 解码 Windows DIB（`CF_DIB` / `CF_DIBV5` 原始字节，不含 `BITMAPFILEHEADER`）。
///
/// 支持 `BITMAPINFOHEADER` / `BITMAPV5HEADER`（及 V2/V3/V4）、24 / 32 位、`BI_RGB` / `BI_BITFIELDS`、
/// 自底向上与自顶向下；32 位图像若 Alpha 全为 0 则按不透明处理。
pub fn decode_dib(bytes: &[u8]) -> Result<CroppedImage, DibError> {
    if bytes.len() < usize::try_from(BITMAPINFOHEADER_SIZE).unwrap_or(40) {
        return Err(DibError::TooShort);
    }

    let header_size = read_u32(bytes, 0)?;
    if !matches!(
        header_size,
        BITMAPINFOHEADER_SIZE
            | BITMAPV2HEADER_SIZE
            | BITMAPV3HEADER_SIZE
            | BITMAPV4HEADER_SIZE
            | BITMAPV5HEADER_SIZE
    ) {
        return Err(DibError::UnsupportedFormat);
    }
    let header_len = usize::try_from(header_size).map_err(|_| DibError::InvalidSize)?;
    if bytes.len() < header_len {
        return Err(DibError::TooShort);
    }

    let raw_w = read_i32(bytes, 4)?;
    let raw_h = read_i32(bytes, 8)?;
    let planes = read_u16(bytes, 12)?;
    let bit_count = read_u16(bytes, 14)?;
    let compression = read_u32(bytes, 16)?;
    let clr_used = read_u32(bytes, 32)?;

    if planes != 1 || !matches!(bit_count, 24 | 32) {
        return Err(DibError::UnsupportedFormat);
    }
    if raw_w <= 0 || raw_h == 0 || raw_h == i32::MIN {
        return Err(DibError::InvalidSize);
    }
    let width = u32::try_from(raw_w).map_err(|_| DibError::InvalidSize)?;
    let top_down = raw_h < 0;
    let height = raw_h.unsigned_abs();
    if width > MAX_IMAGE_SIDE
        || height > MAX_IMAGE_SIDE
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return Err(DibError::InvalidSize);
    }

    let masks = parse_masks(bytes, header_size, bit_count, compression)?;
    let palette_bytes = usize::try_from(clr_used)
        .ok()
        .and_then(|n| n.checked_mul(4))
        .ok_or(DibError::InvalidSize)?;
    let extra_masks = if header_size == BITMAPINFOHEADER_SIZE && compression == BI_BITFIELDS {
        12usize
    } else {
        0usize
    };
    let pixel_offset = header_len
        .checked_add(extra_masks)
        .and_then(|n| n.checked_add(palette_bytes))
        .ok_or(DibError::InvalidSize)?;

    let w = usize::try_from(width).map_err(|_| DibError::InvalidSize)?;
    let h = usize::try_from(height).map_err(|_| DibError::InvalidSize)?;
    let stride = row_stride(w, bit_count)?;
    let required_bytes = stride.checked_mul(h).ok_or(DibError::InvalidSize)?;
    let total_end = pixel_offset
        .checked_add(required_bytes)
        .ok_or(DibError::InvalidSize)?;
    let pixel_data = bytes
        .get(pixel_offset..total_end)
        .ok_or(DibError::TooShort)?;

    let out_len = w
        .checked_mul(h)
        .and_then(|px| px.checked_mul(4))
        .ok_or(DibError::InvalidSize)?;
    let mut out = Vec::with_capacity(out_len);
    let mut any_nonzero_alpha = bit_count == 24 || masks.is_some_and(|m| m.a == 0);

    for dst_row in 0..h {
        let src_row = if top_down { dst_row } else { h - 1 - dst_row };
        let row_start = src_row.checked_mul(stride).ok_or(DibError::InvalidSize)?;
        let row = pixel_data
            .get(row_start..row_start + stride)
            .ok_or(DibError::TooShort)?;

        if bit_count == 24 {
            let row_bytes = w.checked_mul(3).ok_or(DibError::InvalidSize)?;
            for px in row
                .get(..row_bytes)
                .ok_or(DibError::TooShort)?
                .as_chunks::<3>()
                .0
            {
                let [b, g, r] = *px;
                out.extend_from_slice(&[r, g, b, 255]);
            }
        } else {
            let row_bytes = w.checked_mul(4).ok_or(DibError::InvalidSize)?;
            for px in row
                .get(..row_bytes)
                .ok_or(DibError::TooShort)?
                .as_chunks::<4>()
                .0
            {
                if let Some(m) = masks {
                    let raw = u32::from_le_bytes([
                        *px.first().ok_or(DibError::TooShort)?,
                        *px.get(1).ok_or(DibError::TooShort)?,
                        *px.get(2).ok_or(DibError::TooShort)?,
                        *px.get(3).ok_or(DibError::TooShort)?,
                    ]);
                    let r = extract_channel(raw, m.r);
                    let g = extract_channel(raw, m.g);
                    let b = extract_channel(raw, m.b);
                    let a = if m.a == 0 {
                        255
                    } else {
                        let a = extract_channel(raw, m.a);
                        any_nonzero_alpha |= a != 0;
                        a
                    };
                    out.extend_from_slice(&[r, g, b, a]);
                } else {
                    let [b, g, r, a] = *px;
                    any_nonzero_alpha |= a != 0;
                    out.extend_from_slice(&[r, g, b, a]);
                }
            }
        }
    }

    if !any_nonzero_alpha {
        for px in out.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
    }

    Ok(CroppedImage::new(width, height, out)?)
}

#[derive(Debug, Clone, Copy)]
struct BitMasks {
    r: u32,
    g: u32,
    b: u32,
    a: u32,
}

fn parse_masks(
    bytes: &[u8],
    header_size: u32,
    bit_count: u16,
    compression: u32,
) -> Result<Option<BitMasks>, DibError> {
    match (bit_count, compression) {
        (24 | 32, BI_RGB) => Ok(None),
        (32, BI_BITFIELDS) => {
            if header_size == BITMAPINFOHEADER_SIZE {
                let r = read_u32(bytes, 40)?;
                let g = read_u32(bytes, 44)?;
                let b = read_u32(bytes, 48)?;
                Ok(Some(BitMasks { r, g, b, a: 0 }))
            } else {
                let r = read_u32(bytes, 40)?;
                let g = read_u32(bytes, 44)?;
                let b = read_u32(bytes, 48)?;
                let a = if header_size >= BITMAPV3HEADER_SIZE {
                    read_u32(bytes, 52)?
                } else {
                    0
                };
                Ok(Some(BitMasks { r, g, b, a }))
            }
        }
        _ => Err(DibError::UnsupportedFormat),
    }
}

fn row_stride(width: usize, bit_count: u16) -> Result<usize, DibError> {
    let bytes_per_px = usize::from(bit_count / 8);
    let raw = width
        .checked_mul(bytes_per_px)
        .ok_or(DibError::InvalidSize)?;
    raw.checked_add(3)
        .map(|n| (n / 4) * 4)
        .ok_or(DibError::InvalidSize)
}

fn extract_channel(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let max = mask >> shift;
    if max == 0 {
        return 0;
    }
    let raw = (pixel & mask) >> shift;
    let scaled = (u64::from(raw) * 255 + u64::from(max) / 2) / u64::from(max);
    u8::try_from(scaled.min(255)).unwrap_or(255)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, DibError> {
    let end = offset.checked_add(2).ok_or(DibError::TooShort)?;
    let slice = bytes.get(offset..end).ok_or(DibError::TooShort)?;
    let arr: [u8; 2] = slice.try_into().map_err(|_| DibError::TooShort)?;
    Ok(u16::from_le_bytes(arr))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, DibError> {
    let end = offset.checked_add(4).ok_or(DibError::TooShort)?;
    let slice = bytes.get(offset..end).ok_or(DibError::TooShort)?;
    let arr: [u8; 4] = slice.try_into().map_err(|_| DibError::TooShort)?;
    Ok(u32::from_le_bytes(arr))
}

fn read_i32(bytes: &[u8], offset: usize) -> Result<i32, DibError> {
    let end = offset.checked_add(4).ok_or(DibError::TooShort)?;
    let slice = bytes.get(offset..end).ok_or(DibError::TooShort)?;
    let arr: [u8; 4] = slice.try_into().map_err(|_| DibError::TooShort)?;
    Ok(i32::from_le_bytes(arr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_3x2() -> CroppedImage {
        // 像素用 R 通道区分位置：
        // 行0: [1, 2, 3]
        // 行1: [4, 5, 6]
        let mut rgba = Vec::new();
        for id in 1u8..=6 {
            rgba.extend_from_slice(&[id, id * 10, id * 20, 255]);
        }
        CroppedImage::new(3, 2, rgba).expect("构造样图")
    }

    fn reds(img: &CroppedImage) -> Vec<u8> {
        img.rgba()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|px| *px.first().unwrap_or(&0))
            .collect()
    }

    #[test]
    fn rotation_cycle_and_dimensions() {
        let mut r = Rotation::Deg0;
        assert_eq!(r.dimensions(300, 100), (300, 100));
        r = r.clockwise();
        assert_eq!(r, Rotation::Deg90);
        assert_eq!(r.dimensions(300, 100), (100, 300));
        r = r.clockwise();
        assert_eq!(r, Rotation::Deg180);
        assert_eq!(r.dimensions(300, 100), (300, 100));
        r = r.clockwise();
        assert_eq!(r, Rotation::Deg270);
        assert_eq!(r.dimensions(300, 100), (100, 300));
        r = r.clockwise();
        assert_eq!(r, Rotation::Deg0);

        assert_eq!(Rotation::Deg0.counter_clockwise(), Rotation::Deg270);
        assert_eq!(Rotation::Deg270.counter_clockwise(), Rotation::Deg180);
    }

    #[test]
    fn zoom_and_opacity_step_and_clamp() {
        assert_eq!(step_zoom(1000, 1), 1100);
        assert_eq!(step_zoom(1000, -1), 900);
        assert_eq!(step_zoom(100, -5), 100);
        assert_eq!(step_zoom(5000, 5), 5000);
        // 初始大于工作区缩小到 5% 后继续缩小不反跳到 10%
        assert_eq!(step_zoom(50, -1), 50);
        assert_eq!(step_zoom(50, 1), 150);

        assert_eq!(step_opacity(100, 1), 100);
        assert_eq!(step_opacity(100, -1), 90);
        assert_eq!(step_opacity(10, -3), 10);
        assert_eq!(opacity_to_alpha(100), 255);
        assert_eq!(opacity_to_alpha(50), 128);
        assert_eq!(opacity_to_alpha(10), 26);
    }

    #[test]
    fn rotate_around_center_has_zero_drift_after_full_turn() {
        let orig = Rect::new(123, 456, 301, 140);
        let mut cur = orig;
        for _ in 0..4 {
            cur = rotate_around_center(cur, cur.height, cur.width);
        }
        assert_eq!(cur, orig);
    }

    #[test]
    fn zoom_around_cursor_keeps_cursor_anchor() {
        let rect = Rect::new(100, 100, 200, 100);
        // 光标在正中心 (200, 150)，放大两倍后中心仍应保持不变
        let zoomed = zoom_around_cursor(rect, Point::new(200, 150), 400, 200);
        assert_eq!(zoomed, Rect::new(0, 50, 400, 200));

        // 光标在左上角 (100, 100)，左上角保持不动
        let corner = zoom_around_cursor(rect, Point::new(100, 100), 400, 200);
        assert_eq!(corner, Rect::new(100, 100, 400, 200));
    }

    #[test]
    fn fit_work_area_centers_or_shrinks_to_fit() {
        let work = Rect::new(1920, 40, 1000, 800);
        let (r1, z1) = fit_work_area(400, 200, work);
        assert_eq!(z1, 1000);
        assert_eq!(r1, Rect::new(2220, 340, 400, 200));

        let (r2, z2) = fit_work_area(2000, 1000, work);
        assert_eq!(z2, 500);
        assert_eq!(r2, Rect::new(1920, 190, 1000, 500));

        let (r3, z3) = fit_work_area(1000, 1600, work);
        assert_eq!(z3, 500);
        assert_eq!(r3, Rect::new(2170, 40, 500, 800));
    }

    #[test]
    fn rotate_image_all_four_angles_and_1xn() {
        let img = sample_3x2();
        let d0 = rotate_image(&img, Rotation::Deg0).expect("0度");
        assert_eq!((d0.width(), d0.height()), (3, 2));
        assert_eq!(reds(&d0), vec![1, 2, 3, 4, 5, 6]);

        let d90 = rotate_image(&img, Rotation::Deg90).expect("90度");
        assert_eq!((d90.width(), d90.height()), (2, 3));
        assert_eq!(reds(&d90), vec![4, 1, 5, 2, 6, 3]);

        let d180 = rotate_image(&img, Rotation::Deg180).expect("180度");
        assert_eq!((d180.width(), d180.height()), (3, 2));
        assert_eq!(reds(&d180), vec![6, 5, 4, 3, 2, 1]);

        let d270 = rotate_image(&img, Rotation::Deg270).expect("270度");
        assert_eq!((d270.width(), d270.height()), (2, 3));
        assert_eq!(reds(&d270), vec![3, 6, 2, 5, 1, 4]);

        // 1x4 细长图旋转
        let strip = CroppedImage::new(
            1,
            4,
            vec![1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255],
        )
        .expect("1x4");
        let strip90 = rotate_image(&strip, Rotation::Deg90).expect("1x4->4x1");
        assert_eq!((strip90.width(), strip90.height()), (4, 1));
        assert_eq!(reds(&strip90), vec![4, 3, 2, 1]);
        let strip270 = rotate_image(&strip, Rotation::Deg270).expect("1x4->4x1 270");
        assert_eq!((strip270.width(), strip270.height()), (4, 1));
        assert_eq!(reds(&strip270), vec![1, 2, 3, 4]);
    }

    fn push_header(
        buf: &mut Vec<u8>,
        header_size: u32,
        width: i32,
        height: i32,
        bits: u16,
        compression: u32,
    ) {
        buf.extend_from_slice(&header_size.to_le_bytes());
        buf.extend_from_slice(&width.to_le_bytes());
        buf.extend_from_slice(&height.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&bits.to_le_bytes());
        buf.extend_from_slice(&compression.to_le_bytes());
        buf.extend_from_slice(&[0u8; 20]);
    }

    #[test]
    fn decode_dib_24bit_bottom_up_with_row_padding() {
        // 1x2，每行 3 字节 + 1 字节 4 对齐填充；自底向上先底行后顶行
        let mut buf = Vec::new();
        push_header(&mut buf, 40, 1, 2, 24, BI_RGB);
        // 底行：蓝 (BGR=255,0,0) + padding
        buf.extend_from_slice(&[255, 0, 0, 0]);
        // 顶行：红 (BGR=0,0,255) + padding
        buf.extend_from_slice(&[0, 0, 255, 0]);

        let img = decode_dib(&buf).expect("解码 24 位 DIB");
        assert_eq!((img.width(), img.height()), (1, 2));
        assert_eq!(img.rgba(), &[255, 0, 0, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn decode_dib_32bit_top_down_all_zero_alpha_becomes_opaque() {
        let mut buf = Vec::new();
        push_header(&mut buf, 40, 2, -1, 32, BI_RGB);
        // 顶行自顶向下：像素1 红、像素2 绿，alpha 全 0
        buf.extend_from_slice(&[0, 0, 255, 0, 0, 255, 0, 0]);

        let img = decode_dib(&buf).expect("解码 32 位顶向下");
        assert_eq!((img.width(), img.height()), (2, 1));
        assert_eq!(img.rgba(), &[255, 0, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn decode_dib_32bit_preserves_nonzero_alpha() {
        let mut buf = Vec::new();
        push_header(&mut buf, 40, 2, -1, 32, BI_RGB);
        buf.extend_from_slice(&[0, 0, 255, 0, 0, 255, 0, 128]);

        let img = decode_dib(&buf).expect("保留部分透明");
        assert_eq!(img.rgba(), &[255, 0, 0, 0, 0, 255, 0, 128]);
    }

    #[test]
    fn decode_dib_v1_bitfields_and_v5_bitfields() {
        // V1 BI_BITFIELDS，12 字节掩码跟在 40 字节头后面
        let mut v1 = Vec::new();
        push_header(&mut v1, 40, 1, -1, 32, BI_BITFIELDS);
        v1.extend_from_slice(&0x0000_00FFu32.to_le_bytes()); // R
        v1.extend_from_slice(&0x0000_FF00u32.to_le_bytes()); // G
        v1.extend_from_slice(&0x00FF_0000u32.to_le_bytes()); // B
        v1.extend_from_slice(&[10, 20, 30, 0]);
        let img1 = decode_dib(&v1).expect("V1 BITFIELDS");
        assert_eq!(img1.rgba(), &[10, 20, 30, 255]);

        // V5 BI_BITFIELDS（124 字节头），掩码位于偏移 40..56
        let mut v5 = Vec::new();
        push_header(&mut v5, 124, 1, 1, 32, BI_BITFIELDS);
        v5.extend_from_slice(&0x00FF_0000u32.to_le_bytes()); // R
        v5.extend_from_slice(&0x0000_FF00u32.to_le_bytes()); // G
        v5.extend_from_slice(&0x0000_00FFu32.to_le_bytes()); // B
        v5.extend_from_slice(&0xFF00_0000u32.to_le_bytes()); // A
        v5.resize(124, 0);
        v5.extend_from_slice(&[30, 20, 10, 200]); // BGRA
        let img5 = decode_dib(&v5).expect("V5 BITFIELDS");
        assert_eq!(img5.rgba(), &[10, 20, 30, 200]);
    }

    #[test]
    fn decode_dib_rejects_invalid_inputs() {
        assert_eq!(decode_dib(&[0; 10]).unwrap_err(), DibError::TooShort);

        // 非法头长度
        let mut bad_hdr = vec![0u8; 44];
        bad_hdr[0..4].copy_from_slice(&12u32.to_le_bytes());
        assert_eq!(
            decode_dib(&bad_hdr).unwrap_err(),
            DibError::UnsupportedFormat
        );

        // 不支持 8 位
        let mut bpp8 = Vec::new();
        push_header(&mut bpp8, 40, 2, 2, 8, BI_RGB);
        assert_eq!(decode_dib(&bpp8).unwrap_err(), DibError::UnsupportedFormat);

        // 超出单边上限
        let mut too_wide = Vec::new();
        push_header(&mut too_wide, 40, 16_385, 1, 32, BI_RGB);
        assert_eq!(decode_dib(&too_wide).unwrap_err(), DibError::InvalidSize);

        // 超过总像素 1 亿
        let mut too_many = Vec::new();
        push_header(&mut too_many, 40, 10_001, 10_001, 32, BI_RGB);
        assert_eq!(decode_dib(&too_many).unwrap_err(), DibError::InvalidSize);

        // 截断的像素区
        let mut truncated = Vec::new();
        push_header(&mut truncated, 40, 2, 2, 32, BI_RGB);
        truncated.extend_from_slice(&[0; 12]);
        assert_eq!(decode_dib(&truncated).unwrap_err(), DibError::TooShort);
    }
}
