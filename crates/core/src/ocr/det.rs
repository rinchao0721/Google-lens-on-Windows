//! 文本检测（DB）的前处理与后处理。

use crate::error::OcrDataError;
use crate::frame::CroppedImage;

use super::num::{round_u32, to_usize, u32_to_f32, usize_to_f32};
use super::resample::{SourceRegion, resize_rgb};

/// 检测输入的长边上限。
pub const DET_MAX_SIDE: u32 = 960;
/// 检测输入宽高需要对齐的倍数。
const DET_ALIGN: u32 = 32;
/// 概率图二值化阈值。
const DET_THRESH: f32 = 0.3;
/// 文本框平均得分下限。
const BOX_THRESH: f32 = 0.6;
/// 文本框外扩比例。
const UNCLIP_RATIO: f32 = 1.5;
/// 连通域最少像素数。
const MIN_COMPONENT_PIXELS: usize = 4;
/// 外扩前文本框的最小边长（检测图像素）。
const MIN_SIDE: f32 = 3.0;
/// 外扩后文本框的最小边长（检测图像素）。
const MIN_UNCLIPPED_SIDE: f32 = 5.0;
/// ImageNet 均值，按 BGR 排列。
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
/// ImageNet 标准差，按 BGR 排列。
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// 检测模型的输入张量（NCHW，批大小 1）。
#[derive(Debug, Clone, PartialEq)]
pub struct DetInput {
    /// 张量宽度。
    pub width: u32,
    /// 张量高度。
    pub height: u32,
    /// BGR 平面排列、已归一化的数据，长度为 3×高×宽。
    pub data: Vec<f32>,
}

/// 原图像素坐标下的轴对齐文本框。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelBox {
    /// 左边界。
    pub x: u32,
    /// 上边界。
    pub y: u32,
    /// 宽度。
    pub width: u32,
    /// 高度。
    pub height: u32,
}

/// 检测输入的目标尺寸：长边不超过 [`DET_MAX_SIDE`]，宽高取 32 的倍数且不小于 32。
///
/// 宽或高为 0 时返回 `None`。
pub fn det_target_size(width: u32, height: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let long = width.max(height);
    let scale = if long > DET_MAX_SIDE {
        u32_to_f32(DET_MAX_SIDE) / u32_to_f32(long)
    } else {
        1.0
    };
    let align = u32_to_f32(DET_ALIGN);
    let snap = |side: u32| {
        round_u32(u32_to_f32(side) * scale / align)
            .max(1)
            .checked_mul(DET_ALIGN)
    };
    Some((snap(width)?, snap(height)?))
}

/// 生成检测模型输入：缩放、BGR 排列、均值方差归一化。
pub fn det_preprocess(image: &CroppedImage) -> Result<DetInput, OcrDataError> {
    let (dw, dh) =
        det_target_size(image.width(), image.height()).ok_or(OcrDataError::EmptyImage)?;
    let full = SourceRegion {
        x: 0,
        y: 0,
        width: image.width(),
        height: image.height(),
    };
    let rgb = resize_rgb(image.rgba(), image.width(), full, dw, dh)?;
    let plane = to_usize(dw)
        .checked_mul(to_usize(dh))
        .ok_or(OcrDataError::SizeOverflow)?;
    let total = plane.checked_mul(3).ok_or(OcrDataError::SizeOverflow)?;
    let mut data = vec![0f32; total];
    for (channel, out) in data.chunks_exact_mut(plane).enumerate() {
        let mean = MEAN.get(channel).copied().unwrap_or(0.0);
        let std = STD.get(channel).copied().unwrap_or(1.0);
        // 输入按 BGR 排列，RGB 源取反序通道
        let source = 2usize.saturating_sub(channel);
        for (o, px) in out.iter_mut().zip(rgb.as_chunks::<3>().0) {
            let v = px.get(source).copied().unwrap_or(0.0);
            *o = (v / 255.0 - mean) / std;
        }
    }
    Ok(DetInput {
        width: dw,
        height: dh,
        data,
    })
}

/// 概率图的只读视图，越界读取得 0。
struct Plane<'a> {
    data: &'a [f32],
    width: usize,
    height: usize,
}

impl Plane<'_> {
    fn at(&self, x: usize, y: usize) -> f32 {
        if x >= self.width || y >= self.height {
            return 0.0;
        }
        y.checked_mul(self.width)
            .and_then(|row| row.checked_add(x))
            .and_then(|i| self.data.get(i))
            .copied()
            .unwrap_or(0.0)
    }
}

/// 一个 8 邻域连通域的包围盒。
struct Component {
    min_x: usize,
    max_x: usize,
    min_y: usize,
    max_y: usize,
    count: usize,
}

fn flood(plane: &Plane<'_>, visited: &mut [bool], start: (usize, usize)) -> Component {
    let mut comp = Component {
        min_x: start.0,
        max_x: start.0,
        min_y: start.1,
        max_y: start.1,
        count: 0,
    };
    let mut stack = vec![start];
    mark(visited, plane.width, start);
    while let Some((x, y)) = stack.pop() {
        comp.count = comp.count.saturating_add(1);
        comp.min_x = comp.min_x.min(x);
        comp.max_x = comp.max_x.max(x);
        comp.min_y = comp.min_y.min(y);
        comp.max_y = comp.max_y.max(y);
        for dy in 0..3usize {
            for dx in 0..3usize {
                let (Some(nx), Some(ny)) = (
                    x.saturating_add(dx).checked_sub(1),
                    y.saturating_add(dy).checked_sub(1),
                ) else {
                    continue;
                };
                if nx >= plane.width || ny >= plane.height {
                    continue;
                }
                if is_visited(visited, plane.width, (nx, ny)) || plane.at(nx, ny) <= DET_THRESH {
                    continue;
                }
                mark(visited, plane.width, (nx, ny));
                stack.push((nx, ny));
            }
        }
    }
    comp
}

fn is_visited(visited: &[bool], width: usize, (x, y): (usize, usize)) -> bool {
    visited
        .get(y.saturating_mul(width).saturating_add(x))
        .copied()
        .unwrap_or(true)
}

fn mark(visited: &mut [bool], width: usize, (x, y): (usize, usize)) {
    if let Some(v) = visited.get_mut(y.saturating_mul(width).saturating_add(x)) {
        *v = true;
    }
}

/// 包围盒内概率均值。
fn mean_score(plane: &Plane<'_>, comp: &Component) -> f32 {
    let mut sum = 0f32;
    let mut n = 0usize;
    for y in comp.min_y..=comp.max_y {
        for x in comp.min_x..=comp.max_x {
            sum += plane.at(x, y);
            n = n.saturating_add(1);
        }
    }
    if n == 0 { 0.0 } else { sum / usize_to_f32(n) }
}

/// 把检测图上的外扩框映回原图并与图像边界求交；结果为空时返回 `None`。
fn to_pixel_box(comp: &Component, pad: f32, det: (u32, u32), orig: (u32, u32)) -> Option<PixelBox> {
    let rx = u32_to_f32(orig.0) / u32_to_f32(det.0);
    let ry = u32_to_f32(orig.1) / u32_to_f32(det.1);
    let x0 = (usize_to_f32(comp.min_x) - pad).max(0.0) * rx;
    let y0 = (usize_to_f32(comp.min_y) - pad).max(0.0) * ry;
    let x1 = (usize_to_f32(comp.max_x) + 1.0 + pad) * rx;
    let y1 = (usize_to_f32(comp.max_y) + 1.0 + pad) * ry;
    let left = round_u32(x0).min(orig.0);
    let top = round_u32(y0).min(orig.1);
    let right = round_u32(x1).min(orig.0);
    let bottom = round_u32(y1).min(orig.1);
    let width = right.checked_sub(left)?;
    let height = bottom.checked_sub(top)?;
    if width == 0 || height == 0 {
        return None;
    }
    Some(PixelBox {
        x: left,
        y: top,
        width,
        height,
    })
}

/// 解析检测模型输出的概率图，得到原图坐标下的轴对齐文本框。
///
/// `prob` 为 `det_width`×`det_height` 的概率图；长度不符、任一尺寸为 0 时返回错误。
/// 空图或全低于阈值时返回空列表。
pub fn det_postprocess(
    prob: &[f32],
    det_width: u32,
    det_height: u32,
    orig_width: u32,
    orig_height: u32,
) -> Result<Vec<PixelBox>, OcrDataError> {
    if det_width == 0 || det_height == 0 || orig_width == 0 || orig_height == 0 {
        return Err(OcrDataError::EmptyImage);
    }
    let (w, h) = (to_usize(det_width), to_usize(det_height));
    let expected = w.checked_mul(h).ok_or(OcrDataError::SizeOverflow)?;
    if prob.len() != expected {
        return Err(OcrDataError::ShapeMismatch {
            expected,
            actual: prob.len(),
        });
    }
    let plane = Plane {
        data: prob,
        width: w,
        height: h,
    };
    let mut visited = vec![false; expected];
    let mut boxes = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if is_visited(&visited, w, (x, y)) || plane.at(x, y) <= DET_THRESH {
                continue;
            }
            let comp = flood(&plane, &mut visited, (x, y));
            if let Some(found) = component_to_box(
                &plane,
                &comp,
                (det_width, det_height),
                (orig_width, orig_height),
            ) {
                boxes.push(found);
            }
        }
    }
    Ok(boxes)
}

/// 连通域过滤、打分、外扩并映回原图。
fn component_to_box(
    plane: &Plane<'_>,
    comp: &Component,
    det: (u32, u32),
    orig: (u32, u32),
) -> Option<PixelBox> {
    if comp.count < MIN_COMPONENT_PIXELS {
        return None;
    }
    let bw = usize_to_f32(comp.max_x - comp.min_x + 1);
    let bh = usize_to_f32(comp.max_y - comp.min_y + 1);
    if bw.min(bh) < MIN_SIDE || mean_score(plane, comp) < BOX_THRESH {
        return None;
    }
    let pad = bw * bh * UNCLIP_RATIO / (2.0 * (bw + bh));
    if (bw + 2.0 * pad).min(bh + 2.0 * pad) < MIN_UNCLIPPED_SIDE {
        return None;
    }
    to_pixel_box(comp, pad, det, orig)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white(width: u32, height: u32) -> CroppedImage {
        let len = usize::try_from(width * height * 4).expect("小尺寸");
        CroppedImage::new(width, height, vec![255; len]).expect("测试图")
    }

    #[test]
    fn target_size_snaps_to_multiples_of_32() {
        assert_eq!(det_target_size(0, 10), None);
        assert_eq!(det_target_size(10, 0), None);
        assert_eq!(det_target_size(1, 1), Some((32, 32)));
        assert_eq!(det_target_size(505, 383), Some((512, 384)));
        assert_eq!(det_target_size(960, 100), Some((960, 96)));
        assert_eq!(det_target_size(1920, 1080), Some((960, 544)));
        assert_eq!(det_target_size(10_000, 40), Some((960, 32)));
        assert_eq!(det_target_size(40, 10_000), Some((32, 960)));
    }

    #[test]
    fn preprocess_normalizes_bgr_planes() {
        // 纯白图：每个通道 (1 - mean) / std
        let input = det_preprocess(&white(40, 40)).expect("预处理");
        assert_eq!((input.width, input.height), (32, 32));
        assert_eq!(input.data.len(), 3 * 32 * 32);
        let plane = 32 * 32;
        for c in 0..3 {
            let expected = (1.0 - MEAN[c]) / STD[c];
            assert!((input.data[c * plane] - expected).abs() < 1e-4);
            assert!((input.data[c * plane + plane - 1] - expected).abs() < 1e-4);
        }
    }

    #[test]
    fn preprocess_swaps_rgb_to_bgr() {
        // 纯红 RGB(255,0,0)：BGR 第 0 通道（B）应为 0，第 2 通道（R）应为 1
        let red = CroppedImage::new(2, 2, [255, 0, 0, 255].repeat(4)).expect("红图");
        let input = det_preprocess(&red).expect("预处理");
        let plane = 32 * 32;
        assert!((input.data[0] - (0.0 - MEAN[0]) / STD[0]).abs() < 1e-4);
        assert!((input.data[2 * plane] - (1.0 - MEAN[2]) / STD[2]).abs() < 1e-4);
    }

    fn prob_with_block(
        w: usize,
        h: usize,
        rect: (usize, usize, usize, usize),
        value: f32,
    ) -> Vec<f32> {
        let mut p = vec![0.0; w * h];
        for y in rect.1..rect.3 {
            for x in rect.0..rect.2 {
                p[y * w + x] = value;
            }
        }
        p
    }

    #[test]
    fn postprocess_finds_unclipped_box_in_original_coordinates() {
        // 检测图 64x64，原图是 2 倍
        let prob = prob_with_block(64, 64, (10, 20, 30, 28), 0.9);
        let boxes = det_postprocess(&prob, 64, 64, 128, 128).expect("后处理");
        assert_eq!(boxes.len(), 1);
        let b = boxes[0];
        // 外扩后必须包住原框（原框映射为 x 20..60，y 40..56）
        assert!(b.x <= 20 && b.y <= 40);
        assert!(b.x + b.width >= 60 && b.y + b.height >= 56);
        assert!(b.x + b.width <= 128 && b.y + b.height <= 128);
    }

    #[test]
    fn postprocess_clips_to_image_bounds() {
        let prob = prob_with_block(32, 32, (0, 0, 32, 10), 0.95);
        let boxes = det_postprocess(&prob, 32, 32, 32, 32).expect("后处理");
        assert_eq!(boxes.len(), 1);
        assert_eq!((boxes[0].x, boxes[0].y), (0, 0));
        assert!(boxes[0].x + boxes[0].width <= 32);
        assert!(boxes[0].y + boxes[0].height <= 32);
    }

    #[test]
    fn postprocess_filters_noise_and_low_scores() {
        // 空输出与全低于阈值
        assert!(
            det_postprocess(&vec![0.0; 32 * 32], 32, 32, 32, 32)
                .expect("空")
                .is_empty()
        );
        assert!(
            det_postprocess(&vec![0.29; 32 * 32], 32, 32, 32, 32)
                .expect("低")
                .is_empty()
        );
        // 得分高于二值化阈值但低于框得分阈值
        let weak = prob_with_block(64, 64, (10, 10, 40, 20), 0.5);
        assert!(
            det_postprocess(&weak, 64, 64, 64, 64)
                .expect("弱")
                .is_empty()
        );
        // 过小的连通域（2x2）
        let tiny = prob_with_block(64, 64, (10, 10, 12, 12), 0.99);
        assert!(
            det_postprocess(&tiny, 64, 64, 64, 64)
                .expect("小")
                .is_empty()
        );
        // 过细的线（高 2）
        let thin = prob_with_block(64, 64, (5, 10, 50, 12), 0.99);
        assert!(
            det_postprocess(&thin, 64, 64, 64, 64)
                .expect("细")
                .is_empty()
        );
    }

    #[test]
    fn postprocess_separates_components_and_validates_input() {
        let mut prob = prob_with_block(96, 64, (5, 5, 30, 15), 0.9);
        let second = prob_with_block(96, 64, (50, 30, 90, 42), 0.9);
        for (a, b) in prob.iter_mut().zip(&second) {
            *a = a.max(*b);
        }
        assert_eq!(
            det_postprocess(&prob, 96, 64, 96, 64).expect("两块").len(),
            2
        );

        assert_eq!(
            det_postprocess(&prob, 0, 64, 96, 64),
            Err(OcrDataError::EmptyImage)
        );
        assert_eq!(
            det_postprocess(&prob, 96, 64, 0, 64),
            Err(OcrDataError::EmptyImage)
        );
        assert!(matches!(
            det_postprocess(&prob[..10], 96, 64, 96, 64),
            Err(OcrDataError::ShapeMismatch { .. })
        ));
    }
}
