//! RGBA 图像区域的三角滤波缩放，输出交错 RGB 的 `f32`（0..255）。

use crate::error::OcrDataError;

use super::num::{ceil_u32, floor_u32, to_usize, u32_to_f32};

/// 缩放源区域，单位为源图像素。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SourceRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// 某个目标坐标对应的源像素权重。
struct Taps {
    start: usize,
    weights: Vec<f32>,
}

/// 三角滤波权重；缩小时按比例放宽支撑半径，等价于带抗锯齿的双线性。
fn make_taps(src_len: u32, dst_len: u32) -> Vec<Taps> {
    let ratio = u32_to_f32(src_len) / u32_to_f32(dst_len);
    let support = ratio.max(1.0);
    (0..dst_len)
        .map(|d| {
            let center = (u32_to_f32(d) + 0.5) * ratio;
            let last = src_len.saturating_sub(1);
            let start = floor_u32((center - support).max(0.0)).min(last);
            let end = ceil_u32(center + support)
                .min(src_len)
                .max(start.saturating_add(1));
            let mut weights: Vec<f32> = (start..end)
                .map(|i| {
                    let dist = (u32_to_f32(i) + 0.5 - center) / support;
                    (1.0 - dist.abs()).max(0.0)
                })
                .collect();
            let sum: f32 = weights.iter().sum();
            if sum > f32::EPSILON {
                for w in &mut weights {
                    *w /= sum;
                }
                Taps {
                    start: to_usize(start),
                    weights,
                }
            } else {
                let nearest = floor_u32(center).min(last);
                Taps {
                    start: to_usize(nearest),
                    weights: vec![1.0],
                }
            }
        })
        .collect()
}

/// 把 `src`（RGBA，宽 `src_width`）中的 `region` 缩放到 `dst_width`×`dst_height`。
///
/// 区域越界、缓冲区长度不符或任一尺寸为 0 时返回错误。
pub(super) fn resize_rgb(
    src: &[u8],
    src_width: u32,
    region: SourceRegion,
    dst_width: u32,
    dst_height: u32,
) -> Result<Vec<f32>, OcrDataError> {
    if region.width == 0 || region.height == 0 || dst_width == 0 || dst_height == 0 {
        return Err(OcrDataError::EmptyImage);
    }
    let right = region
        .x
        .checked_add(region.width)
        .ok_or(OcrDataError::SizeOverflow)?;
    let bottom = region
        .y
        .checked_add(region.height)
        .ok_or(OcrDataError::SizeOverflow)?;
    if right > src_width {
        return Err(OcrDataError::SizeOverflow);
    }
    let stride = to_usize(src_width)
        .checked_mul(4)
        .ok_or(OcrDataError::SizeOverflow)?;
    if src.len() < to_usize(bottom).saturating_mul(stride) {
        return Err(OcrDataError::ShapeMismatch {
            expected: to_usize(bottom).saturating_mul(stride),
            actual: src.len(),
        });
    }

    let dst_row = to_usize(dst_width)
        .checked_mul(3)
        .ok_or(OcrDataError::SizeOverflow)?;
    let mid_len = dst_row
        .checked_mul(to_usize(region.height))
        .ok_or(OcrDataError::SizeOverflow)?;
    let out_len = dst_row
        .checked_mul(to_usize(dst_height))
        .ok_or(OcrDataError::SizeOverflow)?;

    let h_taps = make_taps(region.width, dst_width);
    let mut mid = vec![0f32; mid_len];
    for (row_idx, mid_row) in mid.chunks_exact_mut(dst_row).enumerate() {
        let src_y = to_usize(region.y).saturating_add(row_idx);
        let row_start = src_y.saturating_mul(stride);
        let row = src
            .get(row_start..row_start.saturating_add(stride))
            .ok_or(OcrDataError::SizeOverflow)?;
        let region_px = row
            .get(to_usize(region.x).saturating_mul(4)..to_usize(right).saturating_mul(4))
            .ok_or(OcrDataError::SizeOverflow)?;
        for (out_px, taps) in mid_row.as_chunks_mut::<3>().0.iter_mut().zip(&h_taps) {
            let mut acc = [0f32; 3];
            for (k, &w) in taps.weights.iter().enumerate() {
                let off = taps.start.saturating_add(k).saturating_mul(4);
                let Some(px) = region_px.get(off..off.saturating_add(3)) else {
                    continue;
                };
                for (a, &v) in acc.iter_mut().zip(px) {
                    *a += f32::from(v) * w;
                }
            }
            for (o, a) in out_px.iter_mut().zip(acc) {
                *o = a;
            }
        }
    }

    let v_taps = make_taps(region.height, dst_height);
    let mut out = vec![0f32; out_len];
    for (out_row, taps) in out.chunks_exact_mut(dst_row).zip(&v_taps) {
        for (k, &w) in taps.weights.iter().enumerate() {
            let r = taps.start.saturating_add(k);
            let Some(mid_row) =
                mid.get(r.saturating_mul(dst_row)..r.saturating_add(1).saturating_mul(dst_row))
            else {
                continue;
            };
            for (o, &m) in out_row.iter_mut().zip(mid_row) {
                *o += m * w;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
        let count = usize::try_from(width * height).expect("小尺寸");
        let mut buf = Vec::new();
        for _ in 0..count {
            buf.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        buf
    }

    fn full(width: u32, height: u32) -> SourceRegion {
        SourceRegion {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    #[test]
    fn solid_color_survives_up_and_down_scaling() {
        let src = solid(10, 6, [10, 120, 250]);
        for (dw, dh) in [(32, 32), (5, 3), (1, 1), (10, 6)] {
            let out = resize_rgb(&src, 10, full(10, 6), dw, dh).expect("缩放");
            assert_eq!(out.len(), usize::try_from(dw * dh * 3).expect("小尺寸"));
            for px in out.as_chunks::<3>().0 {
                assert!((px[0] - 10.0).abs() < 0.01);
                assert!((px[1] - 120.0).abs() < 0.01);
                assert!((px[2] - 250.0).abs() < 0.01);
            }
        }
    }

    #[test]
    fn one_pixel_source_fills_target() {
        let src = solid(1, 1, [7, 8, 9]);
        let out = resize_rgb(&src, 1, full(1, 1), 4, 4).expect("缩放");
        assert!(
            out.as_chunks::<3>()
                .0
                .iter()
                .all(|p| (p[0] - 7.0).abs() < 0.01)
        );
    }

    #[test]
    fn region_limits_the_sampled_area() {
        // 左半红、右半蓝，取右半区域应得到纯蓝
        let mut src = Vec::new();
        for _ in 0..4 {
            src.extend_from_slice(&[
                255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255,
            ]);
        }
        let region = SourceRegion {
            x: 2,
            y: 0,
            width: 2,
            height: 4,
        };
        let out = resize_rgb(&src, 4, region, 3, 3).expect("缩放");
        assert!(
            out.as_chunks::<3>()
                .0
                .iter()
                .all(|p| p[0] < 0.01 && p[2] > 254.9)
        );
    }

    #[test]
    fn invalid_input_is_rejected() {
        let src = solid(4, 4, [0, 0, 0]);
        assert_eq!(
            resize_rgb(&src, 4, full(0, 4), 4, 4),
            Err(OcrDataError::EmptyImage)
        );
        assert_eq!(
            resize_rgb(&src, 4, full(4, 4), 0, 4),
            Err(OcrDataError::EmptyImage)
        );
        let outside = SourceRegion {
            x: 3,
            y: 0,
            width: 4,
            height: 4,
        };
        assert!(resize_rgb(&src, 4, outside, 4, 4).is_err());
        assert!(resize_rgb(&src[..10], 4, full(4, 4), 4, 4).is_err());
    }
}
