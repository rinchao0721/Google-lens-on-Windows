//! OCR 纯逻辑：图像缩放尺寸与重采样、行文本拼接与 CJK 去空格、有效字符计数、策略选择与语言标签校验。

use crate::error::FrameError;
use crate::frame::CroppedImage;

/// OCR 输入图像允许的最小高度（像素）；低于此值按整数倍放大。
pub const MIN_OCR_HEIGHT: u32 = 40;
/// 最多允许配置的识别语言数量。
pub const MAX_OCR_LANGUAGES: usize = 8;

/// 计算 OCR 预处理后的目标宽高；宽高或上限为 0、或尺寸溢出时返回 `None`。
///
/// 高度小于 [`MIN_OCR_HEIGHT`] 时先按整数倍放大到不低于该高度，再保证宽高都不超过 `max_dim`。
pub fn target_size(width: u32, height: u32, max_dim: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 || max_dim == 0 {
        return None;
    }
    let w0 = u64::from(width);
    let h0 = u64::from(height);
    let min_h = u64::from(MIN_OCR_HEIGHT);
    let (w, h) = if h0 < min_h {
        let factor = min_h.div_ceil(h0);
        (w0.checked_mul(factor)?, h0.checked_mul(factor)?)
    } else {
        (w0, h0)
    };
    let limit = u64::from(max_dim);
    if w <= limit && h <= limit {
        return Some((u32::try_from(w).ok()?, u32::try_from(h).ok()?));
    }
    let long = w.max(h);
    let dw = w.checked_mul(limit)?.checked_div(long)?.max(1);
    let dh = h.checked_mul(limit)?.checked_div(long)?.max(1);
    Some((u32::try_from(dw).ok()?, u32::try_from(dh).ok()?))
}

/// 按 OCR 引擎约束预处理图像：过小按最近邻放大，过大按区域平均等比缩小。
pub fn prepare_image(image: CroppedImage, max_dim: u32) -> Result<CroppedImage, FrameError> {
    let (sw, sh) = (image.width(), image.height());
    let (dw, dh) = target_size(sw, sh, max_dim).ok_or(FrameError::EmptyRegion)?;
    if (dw, dh) == (sw, sh) {
        return Ok(image);
    }
    if dw >= sw && dh >= sh {
        upscale_nearest(&image, dw, dh)
    } else {
        downscale_average(&image, dw, dh)
    }
}

fn upscale_nearest(image: &CroppedImage, dw: u32, dh: u32) -> Result<CroppedImage, FrameError> {
    let sw = image.width();
    let sh = image.height();
    let src = image.rgba();
    let row_bytes = usize::try_from(sw)
        .ok()
        .and_then(|w| w.checked_mul(4))
        .ok_or(FrameError::SizeOverflow)?;
    let out_len = usize::try_from(dw)
        .ok()
        .and_then(|w| usize::try_from(dh).ok().and_then(|h| w.checked_mul(h)))
        .and_then(|px| px.checked_mul(4))
        .ok_or(FrameError::SizeOverflow)?;
    let mut out = Vec::with_capacity(out_len);
    for dy in 0..dh {
        let sy = u64::from(dy) * u64::from(sh) / u64::from(dh);
        let row_start = usize::try_from(sy)
            .ok()
            .and_then(|y| y.checked_mul(row_bytes))
            .ok_or(FrameError::SizeOverflow)?;
        for dx in 0..dw {
            let sx = u64::from(dx) * u64::from(sw) / u64::from(dw);
            let offset = usize::try_from(sx)
                .ok()
                .and_then(|x| x.checked_mul(4))
                .and_then(|x| row_start.checked_add(x))
                .ok_or(FrameError::SizeOverflow)?;
            let px = src
                .get(offset..offset.saturating_add(4))
                .ok_or(FrameError::SizeOverflow)?;
            out.extend_from_slice(px);
        }
    }
    CroppedImage::new(dw, dh, out)
}

fn downscale_average(image: &CroppedImage, dw: u32, dh: u32) -> Result<CroppedImage, FrameError> {
    let sw = image.width();
    let sh = image.height();
    let src = image.rgba();
    let row_bytes = usize::try_from(sw)
        .ok()
        .and_then(|w| w.checked_mul(4))
        .ok_or(FrameError::SizeOverflow)?;
    let out_len = usize::try_from(dw)
        .ok()
        .and_then(|w| usize::try_from(dh).ok().and_then(|h| w.checked_mul(h)))
        .and_then(|px| px.checked_mul(4))
        .ok_or(FrameError::SizeOverflow)?;
    let mut out = Vec::with_capacity(out_len);
    for dy in 0..dh {
        let (y0, y1) = source_span(dy, dh, sh).ok_or(FrameError::SizeOverflow)?;
        for dx in 0..dw {
            let (x0, x1) = source_span(dx, dw, sw).ok_or(FrameError::SizeOverflow)?;
            let mut sum = [0u64; 4];
            let mut count = 0u64;
            for y in y0..y1 {
                let row_start = usize::try_from(y)
                    .ok()
                    .and_then(|y| y.checked_mul(row_bytes))
                    .ok_or(FrameError::SizeOverflow)?;
                for x in x0..x1 {
                    let offset = usize::try_from(x)
                        .ok()
                        .and_then(|x| x.checked_mul(4))
                        .and_then(|o| o.checked_add(row_start))
                        .ok_or(FrameError::SizeOverflow)?;
                    let px = src
                        .get(offset..offset.saturating_add(4))
                        .ok_or(FrameError::SizeOverflow)?;
                    for (acc, &v) in sum.iter_mut().zip(px) {
                        *acc = acc.saturating_add(u64::from(v));
                    }
                    count = count.saturating_add(1);
                }
            }
            for acc in sum {
                let avg = acc.checked_div(count).unwrap_or(0);
                out.push(u8::try_from(avg).unwrap_or(u8::MAX));
            }
        }
    }
    CroppedImage::new(dw, dh, out)
}

fn source_span(d: u32, dst: u32, src: u32) -> Option<(u32, u32)> {
    let start = u64::from(d)
        .checked_mul(u64::from(src))?
        .checked_div(u64::from(dst))?;
    let end = u64::from(d)
        .checked_add(1)?
        .checked_mul(u64::from(src))?
        .checked_div(u64::from(dst))?;
    let start = u32::try_from(start).ok()?;
    let end = u32::try_from(end).ok()?.max(start.checked_add(1)?).min(src);
    Some((start, end))
}

/// 字符是否属于 CJK 字符或全角标点（与 Windows OCR 在字间插入的空格匹配）。
fn is_cjk_or_fullwidth_punct(c: char) -> bool {
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

/// 全角标点：CJK 范围内非字母数字的字符。
fn is_fullwidth_punct(c: char) -> bool {
    is_cjk_or_fullwidth_punct(c) && !c.is_alphanumeric()
}

/// 两个非空字符之间的空格是否应删除。
fn should_strip_between(prev: char, next: char) -> bool {
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

/// 去掉单行内 OCR 插入的多余空格：CJK 字符之间、全角标点两侧、ASCII 括号与 CJK 之间，以及 CJK 后的 ASCII 标点前；CJK 与字母数字之间的空格保留。
pub fn clean_cjk_spaces(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut prev_non_space: Option<char> = None;
    let mut pending_spaces = 0usize;

    for c in line.chars() {
        if c == ' ' {
            pending_spaces = pending_spaces.saturating_add(1);
            continue;
        }
        if pending_spaces > 0 {
            let strip = prev_non_space.is_some_and(|prev| should_strip_between(prev, c));
            if !strip {
                for _ in 0..pending_spaces {
                    out.push(' ');
                }
            }
            pending_spaces = 0;
        }
        out.push(c);
        prev_non_space = Some(c);
    }
    for _ in 0..pending_spaces {
        out.push(' ');
    }
    out
}

/// 把 OCR 识别出的多行文本按 `\r\n` 拼接，并去掉相邻 CJK 字符间插入的空格。
pub fn join_lines(lines: &[String]) -> String {
    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx > 0 {
            out.push_str("\r\n");
        }
        out.push_str(&clean_cjk_spaces(line));
    }
    out
}

/// 统计有效字符数：字母、数字和 CJK 字符，不含空白与标点。
pub fn valid_char_count(text: &str) -> usize {
    text.chars().filter(|c| c.is_alphanumeric()).count()
}

/// 识别文本是否包含实际内容（非空白）。
pub fn has_recognized_text(text: &str) -> bool {
    !text.trim().is_empty()
}

/// 回退策略：把单次识别的多行拼接后判断是否可以停止；有非空文本时返回 `Some`。
pub fn accept_fallback_lines(lines: &[String]) -> Option<String> {
    let joined = join_lines(lines);
    has_recognized_text(&joined).then_some(joined)
}

/// 择优策略：从多组已拼接的识别结果中挑出有效字符数最多的一项；相同时取靠前的一项。
///
/// 全为空或仅含空白时返回 `None`。
pub fn pick_best<I, S>(candidates: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut best: Option<(String, usize)> = None;
    for item in candidates {
        let text = item.as_ref();
        if !has_recognized_text(text) {
            continue;
        }
        let score = valid_char_count(text);
        if best.as_ref().is_none_or(|(_, top)| score > *top) {
            best = Some((text.to_string(), score));
        }
    }
    best.map(|(text, _)| text)
}

/// 校验 BCP-47 语言标签的基本语法（如 `zh-Hans-CN`、`en-US`、`ja`）。
pub fn is_valid_language_tag(tag: &str) -> bool {
    if tag.is_empty() || tag.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let mut parts = tag.split('-');
    let Some(primary) = parts.next() else {
        return false;
    };
    if !(2..=8).contains(&primary.len()) || !primary.bytes().all(|b| b.is_ascii_alphabetic()) {
        return false;
    }
    for sub in parts {
        if !(1..=8).contains(&sub.len()) || !sub.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_image(width: u32, height: u32, rgba: [u8; 4]) -> CroppedImage {
        let len = usize::try_from(width)
            .expect("测试宽度")
            .checked_mul(usize::try_from(height).expect("测试高度"))
            .expect("测试像素数")
            * 4;
        let mut buf = Vec::with_capacity(len);
        for _ in 0..(width * height) {
            buf.extend_from_slice(&rgba);
        }
        CroppedImage::new(width, height, buf).expect("构造测试图")
    }

    #[test]
    fn target_size_scales_small_and_large_images() {
        assert_eq!(target_size(0, 10, 10000), None);
        assert_eq!(target_size(10, 0, 10000), None);
        assert_eq!(target_size(10, 10, 0), None);

        // 高度 < 40：整数倍放大到 >= 40
        assert_eq!(target_size(100, 10, 10000), Some((400, 40)));
        assert_eq!(target_size(100, 15, 10000), Some((300, 45)));
        assert_eq!(target_size(100, 20, 10000), Some((200, 40)));
        assert_eq!(target_size(100, 39, 10000), Some((200, 78)));
        // 刚好 40 不放大
        assert_eq!(target_size(100, 40, 10000), Some((100, 40)));

        // 超大图等比缩小到不超过上限
        assert_eq!(target_size(20000, 5000, 10000), Some((10000, 2500)));
        assert_eq!(target_size(3000, 15000, 10000), Some((2000, 10000)));
        assert_eq!(target_size(10000, 10000, 10000), Some((10000, 10000)));

        // 放大后触发上限：等比压回上限内
        assert_eq!(target_size(6000, 20, 10000), Some((10000, 33)));
    }

    #[test]
    fn prepare_image_upscales_nearest_and_downscales_average() {
        // 2x1 像素，整数倍放大 40 倍到 80x40
        let tiny =
            CroppedImage::new(2, 1, vec![10, 20, 30, 255, 200, 210, 220, 255]).expect("小图");
        let up = prepare_image(tiny, 10000).expect("放大");
        assert_eq!((up.width(), up.height()), (80, 40));
        assert_eq!(&up.rgba()[0..4], &[10, 20, 30, 255]);
        assert_eq!(&up.rgba()[40 * 4..40 * 4 + 4], &[200, 210, 220, 255]);

        // 尺寸合适时不复制不修改
        let normal = solid_image(80, 40, [1, 2, 3, 255]);
        let same = prepare_image(normal.clone(), 10000).expect("原样");
        assert_eq!(same, normal);

        // 超出上限时缩小到上限内
        let huge = solid_image(200, 100, [40, 80, 120, 255]);
        let down = prepare_image(huge, 100).expect("缩小");
        assert_eq!((down.width(), down.height()), (100, 50));
        assert_eq!(&down.rgba()[0..4], &[40, 80, 120, 255]);
    }

    #[test]
    fn join_lines_strips_cjk_spaces_and_preserves_other_spaces() {
        assert_eq!(join_lines(&[]), "");
        assert_eq!(join_lines(&[String::new()]), "");
        assert_eq!(join_lines(&["   \t ".to_string()]), "   \t ");
        assert_eq!(join_lines(&["， 。 ！".to_string()]), "，。！");
        assert_eq!(
            join_lines(&[
                "你 好 ， 世 界 ！".to_string(),
                "Hello world".to_string(),
                "中 文 English 混 排 123 测试".to_string(),
            ]),
            "你好，世界！\r\nHello world\r\n中文 English 混排 123 测试"
        );
        // 两头空白和英文间多空格保留；中文间多空格同样合并去除
        assert_eq!(
            join_lines(&[" 中  文  a  b （ 括 号 ） ".to_string()]),
            " 中文  a  b（括号） "
        );
        // 括号、全角标点与数字、CJK 后的 ASCII 标点
        assert_eq!(
            join_lines(&["[ 识 别 文 字 ] 1 ． 第 2 步 ， ok".to_string()]),
            "[识别文字] 1．第 2 步，ok"
        );
        assert_eq!(
            join_lines(&["中 文 . a ( b ) c".to_string()]),
            "中文. a ( b ) c"
        );
        // 超长行不截断不丢字符
        let long_cjk = vec!["字"; 5000].join(" ");
        let joined = join_lines(&[long_cjk]);
        assert_eq!(joined.chars().count(), 5000);
    }

    #[test]
    fn valid_char_count_excludes_whitespace_and_punctuation() {
        assert_eq!(valid_char_count(""), 0);
        assert_eq!(valid_char_count("   \r\n\t "), 0);
        assert_eq!(valid_char_count("...,,,!!，。！？（）"), 0);
        assert_eq!(valid_char_count("Hello, 世界 123!"), 10);
        assert_eq!(valid_char_count("あいう アイウ 한글"), 8);
    }

    #[test]
    fn strategy_selection_best_and_fallback() {
        // 空、空白、纯标点在择优中的表现
        assert_eq!(pick_best(["", "   ", "\r\n"]), None);
        assert_eq!(pick_best(["...", "abc"]), Some("abc".to_string()));
        assert_eq!(pick_best(["...", ""]), Some("...".to_string()));
        // 得分相同时取靠前的语言
        assert_eq!(pick_best(["abcd", "甲乙丙丁"]), Some("abcd".to_string()));
        assert_eq!(
            pick_best(["a b", "中 文 测 试"]),
            Some("中 文 测 试".to_string())
        );

        // 回退判定
        assert_eq!(accept_fallback_lines(&[]), None);
        assert_eq!(accept_fallback_lines(&["   ".to_string()]), None);
        assert_eq!(
            accept_fallback_lines(&["你 好".to_string(), "世 界".to_string()]),
            Some("你好\r\n世界".to_string())
        );
    }

    #[test]
    fn language_tag_validation() {
        for ok in ["zh-Hans-CN", "zh-Hant-TW", "en-US", "ja", "es-419", "yue"] {
            assert!(is_valid_language_tag(ok), "{ok}");
        }
        for bad in [
            "",
            " ",
            "en-",
            "-US",
            "zh--CN",
            "zh_CN",
            " zh-CN",
            "zh-CN ",
            "a",
            "12-US",
            "en-123456789",
            "中文",
        ] {
            assert!(!is_valid_language_tag(bad), "{bad}");
        }
    }
}
