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

/// 识别文本是否包含实际内容（非空白）。
pub fn has_recognized_text(text: &str) -> bool {
    !text.trim().is_empty()
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
}
