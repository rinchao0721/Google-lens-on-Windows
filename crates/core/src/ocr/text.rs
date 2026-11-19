//! 识别文本的空格规则与行拼接。

/// 字符是否属于 CJK 字符或全角标点。
pub(super) fn is_cjk_or_fullwidth_punct(c: char) -> bool {
    matches!(
        c,
        '\u{2E80}'..='\u{2FDF}'
            | '\u{2FF0}'..='\u{2FFF}'
            | '\u{3001}'..='\u{303F}'
            | '\u{3040}'..='\u{30FF}'
            | '\u{3100}'..='\u{31BF}'
            | '\u{31F0}'..='\u{33FF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{AC00}'..='\u{D7AF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FE10}'..='\u{FE1F}'
            | '\u{FE30}'..='\u{FE4F}'
            | '\u{FF01}'..='\u{FF60}'
            | '\u{FFE0}'..='\u{FFEE}'
            | '\u{20000}'..='\u{2FA1F}'
    )
}

/// CJK 文字（汉字、假名、谚文），不含标点。
pub(super) fn is_cjk_letter(c: char) -> bool {
    is_cjk_or_fullwidth_punct(c) && c.is_alphanumeric()
}

/// 全角标点：CJK 范围内非字母数字的字符。
fn is_fullwidth_punct(c: char) -> bool {
    is_cjk_or_fullwidth_punct(c) && !c.is_alphanumeric()
}

/// 两个非空字符之间的空格是否应删除。
pub(super) fn should_strip_between(prev: char, next: char) -> bool {
    let prev_cjk = is_cjk_or_fullwidth_punct(prev);
    let next_cjk = is_cjk_or_fullwidth_punct(next);
    (prev_cjk && next_cjk)
        || is_fullwidth_punct(prev)
        || is_fullwidth_punct(next)
        || (matches!(prev, '(' | '[' | '{' | '<') && next_cjk)
        || (prev_cjk
            && matches!(
                next,
                ')' | ']' | '}' | '>' | '.' | ',' | ':' | ';' | '!' | '?'
            ))
}

/// 在 CJK 文字与 ASCII 字母数字交界处补一个空格；已有空格不重复补。
pub fn insert_script_spaces(line: &str) -> String {
    let mut out = String::with_capacity(line.len().saturating_add(line.len() / 4));
    let mut prev: Option<char> = None;
    for c in line.chars() {
        if let Some(p) = prev {
            let boundary = (is_cjk_letter(p) && c.is_ascii_alphanumeric())
                || (p.is_ascii_alphanumeric() && is_cjk_letter(c));
            if boundary {
                out.push(' ');
            }
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

/// 按 `\r\n` 拼接逐行文本。
pub fn join_lines(lines: &[String]) -> String {
    lines.join("\r\n")
}

use crate::frame::CroppedImage;
use crate::geometry::Rect;

/// 长图触发分段识别的长度阈值（像素）。
const LONG_OCR_THRESHOLD: u32 = 1400;
/// 每段识别窗口沿长轴的长度（接近单屏尺寸）。
const OCR_CHUNK_WINDOW: u32 = 1000;
/// 相邻分段之间的重叠像素数，避免在边界处切断文字行。
const OCR_CHUNK_OVERLAP: u32 = 200;

/// 识别文本是否包含实际内容（非空白）。
pub fn has_recognized_text(text: &str) -> bool {
    !text.trim().is_empty()
}

/// 为长图切分分段识别区域；普通尺寸图片只返回整图一块。
pub fn plan_ocr_chunks(width: u32, height: u32) -> Vec<Rect> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    if height > LONG_OCR_THRESHOLD && height >= width {
        return split_axis(height, OCR_CHUNK_WINDOW, OCR_CHUNK_OVERLAP)
            .into_iter()
            .map(|(start, len)| Rect::new(0, start, width, len))
            .collect();
    }
    if width > LONG_OCR_THRESHOLD && width > height {
        return split_axis(width, OCR_CHUNK_WINDOW, OCR_CHUNK_OVERLAP)
            .into_iter()
            .map(|(start, len)| Rect::new(start, 0, len, height))
            .collect();
    }
    vec![Rect::new(0, 0, width, height)]
}

fn split_axis(total: u32, window: u32, overlap: u32) -> Vec<(i32, u32)> {
    let win = window.min(total).max(1);
    let step = win.saturating_sub(overlap).max(1);
    let mut out = Vec::new();
    let mut start = 0u32;
    loop {
        let len = win.min(total.saturating_sub(start));
        let start_i32 = i32::try_from(start).unwrap_or(i32::MAX);
        out.push((start_i32, len));
        if start.saturating_add(len) >= total {
            break;
        }
        let next = start.saturating_add(step);
        // 最后一段贴齐末尾，避免留下过小的残段
        if next.saturating_add(win) >= total {
            start = total.saturating_sub(win);
            let tail_i32 = i32::try_from(start).unwrap_or(i32::MAX);
            out.push((tail_i32, win));
            break;
        }
        start = next;
    }
    out
}

/// 从 `image` 中裁出一块分段区域；范围不合法时返回 `None`。
pub fn crop_ocr_chunk(image: &CroppedImage, rect: Rect) -> Option<CroppedImage> {
    if rect.is_empty() || rect.x < 0 || rect.y < 0 {
        return None;
    }
    let rx = u32::try_from(rect.x).ok()?;
    let ry = u32::try_from(rect.y).ok()?;
    if rx.checked_add(rect.width)? > image.width() || ry.checked_add(rect.height)? > image.height()
    {
        return None;
    }
    let img_w = usize::try_from(image.width()).ok()?;
    let chunk_w = usize::try_from(rect.width).ok()?;
    let chunk_h = usize::try_from(rect.height).ok()?;
    let start_x = usize::try_from(rx).ok()?;
    let start_y = usize::try_from(ry).ok()?;
    let row_bytes = chunk_w.checked_mul(4)?;
    let mut out = Vec::with_capacity(row_bytes.checked_mul(chunk_h)?);
    let src = image.rgba();
    for dy in 0..chunk_h {
        let row_start = start_y
            .checked_add(dy)?
            .checked_mul(img_w)?
            .checked_add(start_x)?
            .checked_mul(4)?;
        let row_end = row_start.checked_add(row_bytes)?;
        out.extend_from_slice(src.get(row_start..row_end)?);
    }
    CroppedImage::new(rect.width, rect.height, out).ok()
}

/// 合并多段识别出的文本行，去除重叠区间内的重复行。
pub fn merge_ocr_chunks(chunks: &[Vec<String>]) -> Vec<String> {
    let mut merged: Vec<String> = Vec::new();
    for chunk in chunks {
        let non_empty: Vec<String> = chunk
            .iter()
            .filter(|line| !line.trim().is_empty())
            .cloned()
            .collect();
        if non_empty.is_empty() {
            continue;
        }
        if merged.is_empty() {
            merged.extend(non_empty);
            continue;
        }
        if let Some(skip) = exact_overlap(&merged, &non_empty) {
            merged.extend(non_empty.into_iter().skip(skip));
            continue;
        }
        // 上一段末行可能在分界处被截断，退一步检查次末行起的重叠，并用完整行替换残行
        if merged.len() >= 2
            && let Some(prev_prefix) = merged.get(..merged.len() - 1)
            && let Some(skip) = exact_overlap(prev_prefix, &non_empty)
            && let (Some(last_merged), Some(full_next)) = (merged.last(), non_empty.get(skip))
            && is_clipped_of(last_merged, full_next)
        {
            merged.pop();
            merged.extend(non_empty.into_iter().skip(skip));
            continue;
        }
        // 下一段首行可能在分界处被截断，从第二行起匹配重叠
        if non_empty.len() >= 2
            && let Some(next_tail) = non_empty.get(1..)
            && let Some(skip) = exact_overlap(&merged, next_tail)
        {
            let head_pos = merged.len().checked_sub(skip + 1);
            if let (Some(pos), Some(first_next)) = (head_pos, non_empty.first())
                && let Some(prev_full) = merged.get(pos)
                && is_clipped_of(first_next, prev_full)
            {
                merged.extend(non_empty.into_iter().skip(skip + 1));
                continue;
            }
        }
        merged.extend(non_empty);
    }
    merged
}

fn exact_overlap(left: &[String], right: &[String]) -> Option<usize> {
    let max_k = left.len().min(right.len()).min(24);
    for k in (1..=max_k).rev() {
        let suffix = left.get(left.len() - k..)?;
        let prefix = right.get(..k)?;
        let matches = suffix.iter().zip(prefix).all(|(a, b)| a.trim() == b.trim());
        if matches {
            return Some(k);
        }
    }
    None
}

fn is_clipped_of(clipped: &str, full: &str) -> bool {
    let c = clipped.trim();
    let f = full.trim();
    !c.is_empty() && f.contains(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_spaces_are_added_only_at_cjk_latin_boundaries() {
        assert_eq!(insert_script_spaces(""), "");
        assert_eq!(insert_script_spaces("其他App正在放"), "其他 App 正在放");
        assert_eq!(insert_script_spaces("第2步"), "第 2 步");
        assert_eq!(insert_script_spaces("其他 App"), "其他 App");
        assert_eq!(insert_script_spaces("纯中文，标点。"), "纯中文，标点。");
        assert_eq!(insert_script_spaces("Hello world 123"), "Hello world 123");
        assert_eq!(insert_script_spaces("（App）"), "（App）");
        assert_eq!(insert_script_spaces("A"), "A");
        // 超长行不丢字符
        let long = "字a".repeat(5000);
        assert_eq!(insert_script_spaces(&long).chars().count(), 19_999);
    }

    #[test]
    fn strip_rule_matches_cjk_neighbours() {
        assert!(should_strip_between('中', '文'));
        assert!(should_strip_between('，', 'a'));
        assert!(should_strip_between('(', '中'));
        assert!(should_strip_between('中', ')'));
        assert!(!should_strip_between('中', 'a'));
        assert!(!should_strip_between('a', 'b'));
    }

    #[test]
    fn join_and_content_checks() {
        assert_eq!(join_lines(&[]), "");
        assert_eq!(join_lines(&["a".to_string()]), "a");
        assert_eq!(
            join_lines(&["你好".to_string(), "".to_string(), "x".to_string()]),
            "你好\r\n\r\nx"
        );
        assert!(!has_recognized_text(""));
        assert!(!has_recognized_text("  \r\n\t"));
        assert!(has_recognized_text(" a "));
    }

    #[test]
    fn chunk_planning_handles_normal_vertical_and_horizontal() {
        assert_eq!(plan_ocr_chunks(800, 600), vec![Rect::new(0, 0, 800, 600)]);
        let vert = plan_ocr_chunks(400, 2200);
        assert_eq!(
            vert,
            vec![
                Rect::new(0, 0, 400, 1000),
                Rect::new(0, 800, 400, 1000),
                Rect::new(0, 1200, 400, 1000),
            ]
        );
        let horiz = plan_ocr_chunks(2200, 300);
        assert_eq!(
            horiz,
            vec![
                Rect::new(0, 0, 1000, 300),
                Rect::new(800, 0, 1000, 300),
                Rect::new(1200, 0, 1000, 300),
            ]
        );
        assert!(plan_ocr_chunks(0, 100).is_empty());
    }

    #[test]
    fn crop_chunk_extracts_exact_subimage() {
        let mut rgba = Vec::new();
        for y in 0..4u8 {
            for x in 0..3u8 {
                rgba.extend_from_slice(&[x, y, 0, 255]);
            }
        }
        let img = CroppedImage::new(3, 4, rgba).expect("构造测试图");
        let sub = crop_ocr_chunk(&img, Rect::new(1, 1, 2, 2)).expect("裁剪");
        assert_eq!((sub.width(), sub.height()), (2, 2));
        assert_eq!(
            sub.rgba(),
            &[
                1, 1, 0, 255, 2, 1, 0, 255, //
                1, 2, 0, 255, 2, 2, 0, 255
            ]
        );
        assert_eq!(crop_ocr_chunk(&img, Rect::new(2, 2, 2, 2)), None);
    }

    #[test]
    fn merge_ocr_chunks_deduplicates_exact_and_clipped_overlaps() {
        let chunks = vec![
            vec!["L1".to_string(), "L2".to_string(), "L3".to_string()],
            vec!["L2".to_string(), "L3".to_string(), "L4".to_string()],
            vec![],
            vec!["L4".to_string(), "L5".to_string()],
        ];
        assert_eq!(
            merge_ocr_chunks(&chunks),
            vec!["L1", "L2", "L3", "L4", "L5"]
        );

        let clipped_tail = vec![
            vec!["A".to_string(), "B".to_string(), "hel".to_string()],
            vec!["B".to_string(), "hello".to_string(), "C".to_string()],
        ];
        assert_eq!(
            merge_ocr_chunks(&clipped_tail),
            vec!["A", "B", "hello", "C"]
        );

        let clipped_head = vec![
            vec!["A".to_string(), "hello".to_string(), "B".to_string()],
            vec!["lo".to_string(), "B".to_string(), "C".to_string()],
        ];
        assert_eq!(
            merge_ocr_chunks(&clipped_head),
            vec!["A", "hello", "B", "C"]
        );
    }
}
