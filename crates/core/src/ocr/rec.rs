//! 文字识别（CRNN + CTC）的前处理、分桶批量与解码。

use crate::error::OcrDataError;
use crate::frame::CroppedImage;

use super::det::PixelBox;
use super::num::{to_usize, usize_to_f32};
use super::resample::{SourceRegion, resize_rgb};

/// 识别输入的固定高度。
pub const REC_HEIGHT: u32 = 48;
/// 识别输入的最小宽度，也是最小分桶宽度。
pub const REC_MIN_WIDTH: u32 = 320;
/// 识别输入的最大宽度。
pub const REC_MAX_WIDTH: u32 = 4000;
/// 分桶宽度步长。
const REC_BUCKET_STEP: u32 = 32;
/// 单次推理的最大批大小。
pub const REC_MAX_BATCH: usize = 2;

/// 单个文本框裁剪并缩放后的识别输入。
#[derive(Debug, Clone, PartialEq)]
pub struct RecCrop {
    /// 缩放后的有效宽度。
    pub width: u32,
    /// BGR 平面排列、已归一化的数据，长度为 3×[`REC_HEIGHT`]×宽度。
    pub data: Vec<f32>,
}

/// 同一宽度桶内的一批文本框。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecBatch {
    /// 批内统一填充到的宽度。
    pub width: u32,
    /// 成员在输入列表中的序号，按桶内顺序排列。
    pub members: Vec<usize>,
}

/// CTC 解码出的一个字符。
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedChar {
    /// 字典中对应的文本，通常为单个字符；字典里的空格项为 `" "`。
    pub text: String,
    /// 字符在文本框有效宽度内的相对位置，范围 0..=1。
    pub position: f32,
}

/// CTC 解码结果。
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedLine {
    /// 按从左到右顺序排列的字符。
    pub chars: Vec<DecodedChar>,
    /// 已输出字符的平均置信度，无字符时为 0。
    pub score: f32,
}

/// 文本框缩放到固定高度后的宽度：等比、向上取整，限制在 1..=[`REC_MAX_WIDTH`]。
///
/// 宽或高为 0 时返回 `None`。
pub fn rec_target_width(box_width: u32, box_height: u32) -> Option<u32> {
    if box_width == 0 || box_height == 0 {
        return None;
    }
    let scaled = u64::from(box_width)
        .checked_mul(u64::from(REC_HEIGHT))?
        .div_ceil(u64::from(box_height));
    Some(
        u32::try_from(scaled)
            .unwrap_or(REC_MAX_WIDTH)
            .clamp(1, REC_MAX_WIDTH),
    )
}

/// 从原图裁出文本框，缩放到固定高度，并归一化为 `(x/255-0.5)/0.5`、BGR 平面排列。
pub fn rec_prepare(image: &CroppedImage, region: &PixelBox) -> Result<RecCrop, OcrDataError> {
    let width = rec_target_width(region.width, region.height).ok_or(OcrDataError::EmptyImage)?;
    let source = SourceRegion {
        x: region.x,
        y: region.y,
        width: region.width,
        height: region.height,
    };
    let rgb = resize_rgb(image.rgba(), image.width(), source, width, REC_HEIGHT)?;
    let plane = to_usize(width)
        .checked_mul(to_usize(REC_HEIGHT))
        .ok_or(OcrDataError::SizeOverflow)?;
    let total = plane.checked_mul(3).ok_or(OcrDataError::SizeOverflow)?;
    let mut data = vec![0f32; total];
    for (channel, out) in data.chunks_exact_mut(plane).enumerate() {
        let source = 2usize.saturating_sub(channel);
        for (o, px) in out.iter_mut().zip(rgb.as_chunks::<3>().0) {
            let v = px.get(source).copied().unwrap_or(0.0);
            *o = (v / 255.0 - 0.5) / 0.5;
        }
    }
    Ok(RecCrop { width, data })
}

/// 每列的墨迹强度（0..=1）：该列像素与整块文本框背景（像素中位数）的最大灰度差。
///
/// 数据长度与 `width`×[`REC_HEIGHT`]×3 不符时返回空列表。
pub fn column_ink(crop: &RecCrop) -> Vec<f32> {
    let width = to_usize(crop.width);
    let plane = width.saturating_mul(to_usize(REC_HEIGHT));
    if width == 0 || crop.data.len() != plane.saturating_mul(3) {
        return Vec::new();
    }
    let gray: Vec<f32> = (0..plane)
        .map(|i| {
            let sum: f32 = (0..3).filter_map(|c| crop.data.get(c * plane + i)).sum();
            sum / 3.0
        })
        .collect();
    let mut sorted = gray.clone();
    sorted.sort_by(f32::total_cmp);
    let background = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
    let mut ink = vec![0f32; width];
    for row in gray.chunks_exact(width) {
        for (slot, &v) in ink.iter_mut().zip(row) {
            *slot = slot.max((v - background).abs() / 2.0);
        }
    }
    ink
}

/// 宽度所属的桶：不小于 [`REC_MIN_WIDTH`]，按 [`REC_BUCKET_STEP`] 向上取整，不超过 [`REC_MAX_WIDTH`]。
pub fn rec_bucket_width(width: u32) -> u32 {
    let stepped = width
        .div_ceil(REC_BUCKET_STEP)
        .saturating_mul(REC_BUCKET_STEP);
    stepped.clamp(REC_MIN_WIDTH, REC_MAX_WIDTH)
}

/// 按宽度分桶并切成不超过 [`REC_MAX_BATCH`] 的批；桶内保持输入顺序。
pub fn plan_rec_batches(widths: &[u32]) -> Vec<RecBatch> {
    let mut order: Vec<(u32, usize)> = widths
        .iter()
        .enumerate()
        .map(|(i, &w)| (rec_bucket_width(w), i))
        .collect();
    order.sort_unstable();
    let mut batches: Vec<RecBatch> = Vec::new();
    for (bucket, index) in order {
        match batches.last_mut() {
            Some(last) if last.width == bucket && last.members.len() < REC_MAX_BATCH => {
                last.members.push(index);
            }
            _ => batches.push(RecBatch {
                width: bucket,
                members: vec![index],
            }),
        }
    }
    batches
}

/// 把多个裁剪拼成 `[N,3,REC_HEIGHT,width]` 的批张量，右侧补 0。
///
/// 某个裁剪比 `width` 宽、或数据长度与其宽度不符时返回错误。
pub fn pack_rec_batch(crops: &[&RecCrop], width: u32) -> Result<Vec<f32>, OcrDataError> {
    if crops.is_empty() || width == 0 {
        return Err(OcrDataError::EmptyImage);
    }
    let row = to_usize(width);
    let rows = to_usize(REC_HEIGHT)
        .checked_mul(3)
        .ok_or(OcrDataError::SizeOverflow)?;
    let sample = row.checked_mul(rows).ok_or(OcrDataError::SizeOverflow)?;
    let total = sample
        .checked_mul(crops.len())
        .ok_or(OcrDataError::SizeOverflow)?;
    let mut out = vec![0f32; total];
    for (dst, crop) in out.chunks_exact_mut(sample).zip(crops) {
        let src_width = to_usize(crop.width);
        let expected = src_width
            .checked_mul(rows)
            .ok_or(OcrDataError::SizeOverflow)?;
        if crop.width == 0 || crop.width > width || crop.data.len() != expected {
            return Err(OcrDataError::ShapeMismatch {
                expected,
                actual: crop.data.len(),
            });
        }
        for (dst_row, src_row) in dst
            .chunks_exact_mut(row)
            .zip(crop.data.chunks_exact(src_width))
        {
            if let Some(head) = dst_row.get_mut(..src_width) {
                head.copy_from_slice(src_row);
            }
        }
    }
    Ok(out)
}

/// 由字典文本得到类别表：首位 `blank`，随后逐行一项，末位空格。
pub fn parse_charset(dict: &str) -> Vec<String> {
    let mut set = Vec::with_capacity(dict.lines().count().saturating_add(2));
    set.push("blank".to_string());
    set.extend(dict.lines().map(str::to_string));
    set.push(" ".to_string());
    set
}

/// 宽度为 `content_width` 的有效内容在 `bucket_width` 宽的输入里占多少个输出时间步。
pub fn valid_steps(total_steps: usize, content_width: u32, bucket_width: u32) -> usize {
    if total_steps == 0 || bucket_width == 0 {
        return 0;
    }
    let numerator =
        u64::from(content_width).saturating_mul(u64::try_from(total_steps).unwrap_or(u64::MAX));
    let steps = numerator.div_ceil(u64::from(bucket_width));
    usize::try_from(steps)
        .unwrap_or(total_steps)
        .clamp(1, total_steps)
}

/// CTC 贪心解码：逐步取最大类别，丢弃 blank 与连续重复，并记录字符位置与平均置信度。
///
/// `logits` 为 `steps`×`classes` 的概率矩阵；只解码前 `valid_steps` 步（其余是右侧填充）。
/// 数据长度不足、字典与类别数不符时返回错误。
pub fn decode_ctc(
    logits: &[f32],
    steps: usize,
    classes: usize,
    valid_steps: usize,
    charset: &[String],
) -> Result<DecodedLine, OcrDataError> {
    if classes == 0 || charset.len() != classes {
        return Err(OcrDataError::CharsetMismatch {
            charset: charset.len(),
            classes,
        });
    }
    let expected = steps
        .checked_mul(classes)
        .ok_or(OcrDataError::SizeOverflow)?;
    if logits.len() < expected {
        return Err(OcrDataError::ShapeMismatch {
            expected,
            actual: logits.len(),
        });
    }
    let used = valid_steps.min(steps);
    let span = usize_to_f32(used.max(1));
    let mut chars = Vec::new();
    let mut score_sum = 0f32;
    let mut last = 0usize;
    for (step, row) in logits.chunks_exact(classes).take(used).enumerate() {
        let (best, prob) = argmax(row);
        if best != 0
            && best != last
            && let Some(text) = charset.get(best)
        {
            chars.push(DecodedChar {
                text: text.clone(),
                position: ((usize_to_f32(step) + 0.5) / span).min(1.0),
            });
            score_sum += prob;
        }
        last = best;
    }
    let score = if chars.is_empty() {
        0.0
    } else {
        score_sum / usize_to_f32(chars.len())
    };
    Ok(DecodedLine { chars, score })
}

/// 最大值的序号与取值；并列取靠前的，空行返回 `(0, 0.0)`。
fn argmax(row: &[f32]) -> (usize, f32) {
    let mut best = (0usize, f32::NEG_INFINITY);
    for (i, &v) in row.iter().enumerate() {
        if v > best.1 {
            best = (i, v);
        }
    }
    if best.1 == f32::NEG_INFINITY {
        (0, 0.0)
    } else {
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray(width: u32, height: u32, v: u8) -> CroppedImage {
        let len = usize::try_from(width * height).expect("小尺寸");
        let mut buf = Vec::new();
        for _ in 0..len {
            buf.extend_from_slice(&[v, v, v, 255]);
        }
        CroppedImage::new(width, height, buf).expect("测试图")
    }

    fn charset() -> Vec<String> {
        parse_charset("甲\n乙\nA\nb\n")
    }

    /// 为每个时间步生成 one-hot 概率行。
    fn onehot(classes: usize, picks: &[usize], p: f32) -> Vec<f32> {
        let mut out = vec![0.0; classes * picks.len()];
        for (t, &c) in picks.iter().enumerate() {
            out[t * classes + c] = p;
        }
        out
    }

    #[test]
    fn target_width_keeps_aspect_and_clamps() {
        assert_eq!(rec_target_width(0, 10), None);
        assert_eq!(rec_target_width(10, 0), None);
        assert_eq!(rec_target_width(480, 48), Some(480));
        assert_eq!(rec_target_width(100, 33), Some(146));
        assert_eq!(rec_target_width(1, 1000), Some(1));
        assert_eq!(rec_target_width(100_000, 10), Some(REC_MAX_WIDTH));
    }

    #[test]
    fn prepare_normalizes_to_minus_one_one() {
        let white = rec_prepare(
            &gray(60, 24, 255),
            &PixelBox {
                x: 0,
                y: 0,
                width: 60,
                height: 24,
            },
        )
        .expect("白");
        assert_eq!(white.width, 120);
        assert_eq!(white.data.len(), 3 * 48 * 120);
        assert!(white.data.iter().all(|&v| (v - 1.0).abs() < 1e-4));
        let black = rec_prepare(
            &gray(10, 10, 0),
            &PixelBox {
                x: 2,
                y: 2,
                width: 5,
                height: 5,
            },
        )
        .expect("黑");
        assert!(black.data.iter().all(|&v| (v + 1.0).abs() < 1e-4));
        // 1 像素也能处理
        assert!(
            rec_prepare(
                &gray(1, 1, 128),
                &PixelBox {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1
                }
            )
            .is_ok()
        );
        // 越界或空框是错误
        assert!(
            rec_prepare(
                &gray(10, 10, 0),
                &PixelBox {
                    x: 8,
                    y: 0,
                    width: 5,
                    height: 5
                }
            )
            .is_err()
        );
        assert!(
            rec_prepare(
                &gray(10, 10, 0),
                &PixelBox {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 5
                }
            )
            .is_err()
        );
    }

    #[test]
    fn buckets_and_batches_group_by_width() {
        assert_eq!(rec_bucket_width(1), 320);
        assert_eq!(rec_bucket_width(320), 320);
        assert_eq!(rec_bucket_width(321), 352);
        assert_eq!(rec_bucket_width(4000), 4000);
        assert_eq!(rec_bucket_width(u32::MAX), 4000);
        assert!(plan_rec_batches(&[]).is_empty());

        let batches = plan_rec_batches(&[100, 700, 200, 650]);
        assert_eq!(
            batches,
            vec![
                RecBatch {
                    width: 320,
                    members: vec![0, 2]
                },
                RecBatch {
                    width: 672,
                    members: vec![3]
                },
                RecBatch {
                    width: 704,
                    members: vec![1]
                },
            ]
        );
        // 超过批大小要拆分，且不丢成员
        let many = vec![100u32; REC_MAX_BATCH * 2 + 1];
        let split = plan_rec_batches(&many);
        assert_eq!(split.len(), 3);
        assert!(split[..2].iter().all(|b| b.members.len() == REC_MAX_BATCH));
        assert_eq!(split[2].members.len(), 1);
        assert_eq!(
            split.iter().map(|b| b.members.len()).sum::<usize>(),
            many.len()
        );
    }

    #[test]
    fn pack_pads_right_with_zero() {
        let a = RecCrop {
            width: 2,
            data: vec![1.0; 3 * 48 * 2],
        };
        let b = RecCrop {
            width: 3,
            data: vec![2.0; 3 * 48 * 3],
        };
        let packed = pack_rec_batch(&[&a, &b], 4).expect("拼批");
        assert_eq!(packed.len(), 2 * 3 * 48 * 4);
        assert_eq!(&packed[0..4], &[1.0, 1.0, 0.0, 0.0]);
        let second = 3 * 48 * 4;
        assert_eq!(&packed[second..second + 4], &[2.0, 2.0, 2.0, 0.0]);

        assert_eq!(pack_rec_batch(&[], 4), Err(OcrDataError::EmptyImage));
        assert!(pack_rec_batch(&[&b], 2).is_err());
        let bad = RecCrop {
            width: 2,
            data: vec![0.0; 5],
        };
        assert!(pack_rec_batch(&[&bad], 4).is_err());
    }

    #[test]
    fn charset_has_blank_first_and_space_last() {
        let set = charset();
        assert_eq!(set.first().map(String::as_str), Some("blank"));
        assert_eq!(set.last().map(String::as_str), Some(" "));
        assert_eq!(set.len(), 4 + 2);
        assert_eq!(
            parse_charset(""),
            vec!["blank".to_string(), " ".to_string()]
        );
    }

    #[test]
    fn valid_steps_scales_with_content_width() {
        assert_eq!(valid_steps(0, 100, 320), 0);
        assert_eq!(valid_steps(40, 320, 320), 40);
        assert_eq!(valid_steps(40, 160, 320), 20);
        assert_eq!(valid_steps(40, 1, 320), 1);
        assert_eq!(valid_steps(40, 9999, 320), 40);
        assert_eq!(valid_steps(40, 10, 0), 0);
    }

    #[test]
    fn ctc_merges_repeats_and_drops_blank() {
        let set = charset();
        // 类别：0 blank，1 甲，2 乙，3 A，4 b，5 空格
        let picks = [0, 1, 1, 0, 1, 2, 2, 0, 5, 3];
        let logits = onehot(set.len(), &picks, 0.9);
        let out = decode_ctc(&logits, picks.len(), set.len(), picks.len(), &set).expect("解码");
        let text: Vec<&str> = out.chars.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(text, vec!["甲", "甲", "乙", " ", "A"]);
        assert!((out.score - 0.9).abs() < 1e-6);
        // 位置单调递增且落在 0..=1
        assert!(out.chars.windows(2).all(|w| w[0].position < w[1].position));
        assert!(out.chars.iter().all(|c| (0.0..=1.0).contains(&c.position)));
    }

    #[test]
    fn ctc_handles_all_blank_empty_and_padding() {
        let set = charset();
        let blank = onehot(set.len(), &[0, 0, 0], 0.99);
        let out = decode_ctc(&blank, 3, set.len(), 3, &set).expect("全空");
        assert!(out.chars.is_empty());
        assert_eq!(out.score, 0.0);

        let none = decode_ctc(&[], 0, set.len(), 0, &set).expect("零步");
        assert!(none.chars.is_empty());

        // 只解码有效步，右侧填充里的字符被忽略
        let picks = [1, 0, 2, 3];
        let logits = onehot(set.len(), &picks, 0.8);
        let out = decode_ctc(&logits, 4, set.len(), 2, &set).expect("有效步");
        assert_eq!(out.chars.len(), 1);
    }

    #[test]
    fn ctc_rejects_bad_shapes() {
        let set = charset();
        assert!(matches!(
            decode_ctc(&[0.0; 10], 2, set.len() - 1, 2, &set),
            Err(OcrDataError::CharsetMismatch { .. })
        ));
        assert!(matches!(
            decode_ctc(&[0.0; 5], 2, set.len(), 2, &set),
            Err(OcrDataError::ShapeMismatch { .. })
        ));
        assert!(matches!(
            decode_ctc(&[], 0, 0, 0, &[]),
            Err(OcrDataError::CharsetMismatch { .. })
        ));
    }

    #[test]
    fn long_line_decodes_without_loss() {
        let set = charset();
        let picks: Vec<usize> = (0..2000).map(|i| if i % 2 == 0 { 1 } else { 0 }).collect();
        let logits = onehot(set.len(), &picks, 0.7);
        let out = decode_ctc(&logits, picks.len(), set.len(), picks.len(), &set).expect("超长");
        assert_eq!(out.chars.len(), 1000);
    }
}
