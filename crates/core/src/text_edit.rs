//! 文字标注的编辑缓冲与剪贴板文本清洗，按 Unicode 标量值处理，与平台无关。

/// 剪贴板快照最多保留的字符数。
pub const MAX_PASTE_CHARS: usize = 4096;
/// `Tab` 与剪贴板里的制表符展开成的空格数。
pub const TAB_SPACES: usize = 4;

/// 清洗剪贴板文本：`\r\n`、`\r` 统一为 `\n`，`\t` 转为空格，去掉其它控制字符，
/// 最多保留 [`MAX_PASTE_CHARS`] 个字符。
pub fn sanitize_clipboard(text: &str) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let piece: &str = match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                "\n"
            }
            '\n' => "\n",
            '\t' => "    ",
            c if c.is_control() => continue,
            _ => {
                if count >= MAX_PASTE_CHARS {
                    break;
                }
                out.push(c);
                count += 1;
                continue;
            }
        };
        for p in piece.chars() {
            if count >= MAX_PASTE_CHARS {
                return out;
            }
            out.push(p);
            count += 1;
        }
    }
    out
}

/// UTF-16 偏移对应的字符序号；偏移落在代理对中间时取该字符，超出末尾时取字符总数。
pub fn char_index_of_utf16(text: &str, utf16_offset: usize) -> usize {
    let mut units = 0usize;
    for (index, c) in text.chars().enumerate() {
        if units >= utf16_offset {
            return index;
        }
        units += c.len_utf16();
        if units > utf16_offset {
            return index;
        }
    }
    text.chars().count()
}

/// 字符序号对应的 UTF-16 偏移；序号超出末尾时取全长。
pub fn utf16_offset_of_char(text: &str, char_index: usize) -> usize {
    text.chars().take(char_index).map(char::len_utf16).sum()
}

/// 编辑缓冲：字符序列加光标，光标位置是字符序号，范围 `0..=len`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextBuffer {
    chars: Vec<char>,
    cursor: usize,
}

impl TextBuffer {
    /// 空缓冲。
    pub fn new() -> Self {
        Self::default()
    }

    /// 以已有文字建立，光标放在末尾；控制字符（换行除外）会被丢弃。
    pub fn from_text(text: &str) -> Self {
        let chars: Vec<char> = text
            .chars()
            .filter(|c| *c == '\n' || !c.is_control())
            .collect();
        let cursor = chars.len();
        Self { chars, cursor }
    }

    /// 全部文字。
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// 字符数。
    pub fn len(&self) -> usize {
        self.chars.len()
    }

    /// 没有任何字符。
    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// 内容为空或只有空白（含换行）。
    pub fn is_blank(&self) -> bool {
        self.chars.iter().all(|c| c.is_whitespace())
    }

    /// 光标位置。
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// 把光标放到 `index`，超出末尾时放到末尾。
    pub fn set_cursor(&mut self, index: usize) {
        self.cursor = index.min(self.chars.len());
    }

    /// 光标前的文字。
    pub fn before_cursor(&self) -> String {
        self.chars.iter().take(self.cursor).collect()
    }

    /// 光标后的文字。
    pub fn after_cursor(&self) -> String {
        self.chars.iter().skip(self.cursor).collect()
    }

    /// 在光标处插入一个字符；控制字符不插入，返回是否插入。
    pub fn insert_char(&mut self, c: char) -> bool {
        if c.is_control() {
            return false;
        }
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
        true
    }

    /// 在光标处插入文字，控制字符（换行除外）被忽略；返回插入的字符数。
    pub fn insert_str(&mut self, text: &str) -> usize {
        let mut inserted = 0;
        for c in text.chars() {
            let ok = if c == '\n' {
                self.insert_newline();
                true
            } else {
                self.insert_char(c)
            };
            if ok {
                inserted += 1;
            }
        }
        inserted
    }

    /// 插入换行。
    pub fn insert_newline(&mut self) {
        self.chars.insert(self.cursor, '\n');
        self.cursor += 1;
    }

    /// 插入 [`TAB_SPACES`] 个空格。
    pub fn insert_tab(&mut self) {
        for _ in 0..TAB_SPACES {
            self.insert_char(' ');
        }
    }

    /// 删除光标前一个字符；光标在开头时返回 `false`。
    pub fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        self.chars.remove(self.cursor);
        true
    }

    /// 删除光标后一个字符；光标在末尾时返回 `false`。
    pub fn delete(&mut self) -> bool {
        if self.cursor >= self.chars.len() {
            return false;
        }
        self.chars.remove(self.cursor);
        true
    }

    /// 光标左移一个字符。
    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// 光标右移一个字符。
    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.chars.len());
    }

    /// 光标所在行的起点（行首字符序号）。
    fn line_start(&self, index: usize) -> usize {
        self.chars
            .iter()
            .take(index)
            .rposition(|c| *c == '\n')
            .map_or(0, |p| p + 1)
    }

    /// 光标所在行的终点（换行符位置或总长）。
    fn line_end(&self, index: usize) -> usize {
        self.chars
            .iter()
            .skip(index)
            .position(|c| *c == '\n')
            .map_or(self.chars.len(), |p| index + p)
    }

    /// 光标移到行首。
    pub fn move_home(&mut self) {
        self.cursor = self.line_start(self.cursor);
    }

    /// 光标移到行尾。
    pub fn move_end(&mut self) {
        self.cursor = self.line_end(self.cursor);
    }

    /// 光标上移一行，列号保持，上一行较短时停在行尾；已在第一行时不动。
    pub fn move_up(&mut self) {
        let start = self.line_start(self.cursor);
        if start == 0 {
            return;
        }
        let column = self.cursor - start;
        let prev_end = start - 1;
        let prev_start = self.line_start(prev_end);
        self.cursor = prev_start + column.min(prev_end - prev_start);
    }

    /// 光标下移一行，列号保持，下一行较短时停在行尾；已在最后一行时不动。
    pub fn move_down(&mut self) {
        let end = self.line_end(self.cursor);
        if end >= self.chars.len() {
            return;
        }
        let column = self.cursor - self.line_start(self.cursor);
        let next_start = end + 1;
        let next_end = self.line_end(next_start);
        self.cursor = next_start + column.min(next_end - next_start);
    }

    /// 光标所在的（行号, 列号），都从 0 起。
    pub fn line_and_column(&self) -> (usize, usize) {
        let line = self
            .chars
            .iter()
            .take(self.cursor)
            .filter(|c| **c == '\n')
            .count();
        (line, self.cursor - self.line_start(self.cursor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str, cursor: usize) -> TextBuffer {
        let mut b = TextBuffer::from_text(text);
        while b.cursor() > cursor {
            b.move_left();
        }
        b
    }

    #[test]
    fn sanitize_normalizes_newlines_and_tabs() {
        assert_eq!(sanitize_clipboard("a\r\nb\rc\nd"), "a\nb\nc\nd");
        assert_eq!(sanitize_clipboard("a\tb"), "a    b");
        assert_eq!(sanitize_clipboard("\r\n\r\n"), "\n\n");
        assert_eq!(sanitize_clipboard("\n\r"), "\n\n");
    }

    #[test]
    fn sanitize_drops_other_controls_and_keeps_text() {
        assert_eq!(sanitize_clipboard("a\u{0}b\u{7}c\u{1b}[d\u{7f}"), "abc[d");
        assert_eq!(sanitize_clipboard("中文😀\u{200b}x"), "中文😀\u{200b}x");
        assert_eq!(sanitize_clipboard(""), "");
    }

    #[test]
    fn sanitize_truncates_by_scalar_without_splitting_pairs() {
        let long = "😀".repeat(MAX_PASTE_CHARS + 10);
        let out = sanitize_clipboard(&long);
        assert_eq!(out.chars().count(), MAX_PASTE_CHARS);
        assert!(out.chars().all(|c| c == '😀'));
        let exact = "a".repeat(MAX_PASTE_CHARS);
        assert_eq!(sanitize_clipboard(&exact), exact);
        let tabs = "\t".repeat(MAX_PASTE_CHARS);
        assert_eq!(sanitize_clipboard(&tabs).chars().count(), MAX_PASTE_CHARS);
        let lines = "\r\n".repeat(MAX_PASTE_CHARS + 5);
        assert_eq!(sanitize_clipboard(&lines).chars().count(), MAX_PASTE_CHARS);
    }

    #[test]
    fn utf16_offsets_round_trip_with_surrogates() {
        let text = "a😀b";
        assert_eq!(utf16_offset_of_char(text, 0), 0);
        assert_eq!(utf16_offset_of_char(text, 1), 1);
        assert_eq!(utf16_offset_of_char(text, 2), 3);
        assert_eq!(utf16_offset_of_char(text, 3), 4);
        assert_eq!(utf16_offset_of_char(text, 99), 4);
        assert_eq!(char_index_of_utf16(text, 0), 0);
        assert_eq!(char_index_of_utf16(text, 1), 1);
        assert_eq!(char_index_of_utf16(text, 2), 1);
        assert_eq!(char_index_of_utf16(text, 3), 2);
        assert_eq!(char_index_of_utf16(text, 4), 3);
        assert_eq!(char_index_of_utf16(text, 99), 3);
        assert_eq!(char_index_of_utf16("", 5), 0);
    }

    #[test]
    fn insert_and_delete_work_on_scalars() {
        let mut b = TextBuffer::new();
        assert!(b.is_empty() && b.is_blank());
        assert!(b.insert_char('中'));
        assert!(b.insert_char('😀'));
        assert!(b.insert_char('a'));
        assert_eq!(b.text(), "中😀a");
        assert_eq!((b.len(), b.cursor()), (3, 3));
        assert!(b.backspace());
        assert!(b.backspace());
        assert_eq!(b.text(), "中");
        b.move_left();
        assert!(b.delete());
        assert_eq!(b.text(), "");
        assert!(!b.backspace() && !b.delete());
    }

    #[test]
    fn insert_rejects_controls_but_newline_and_tab_are_explicit() {
        let mut b = TextBuffer::new();
        assert!(!b.insert_char('\n'));
        assert!(!b.insert_char('\t'));
        assert!(!b.insert_char('\u{7}'));
        b.insert_char('a');
        b.insert_newline();
        b.insert_char('b');
        b.insert_tab();
        assert_eq!(b.text(), "a\nb    ");
        assert_eq!(b.cursor(), 7);
    }

    #[test]
    fn insert_str_filters_controls_and_counts() {
        let mut b = TextBuffer::new();
        assert_eq!(b.insert_str("ab\u{1}c\nd"), 5);
        assert_eq!(b.text(), "abc\nd");
        b.move_home();
        assert_eq!(b.insert_str("中"), 1);
        assert_eq!(b.text(), "abc\n中d");
        assert_eq!(b.insert_str(""), 0);
    }

    #[test]
    fn insert_in_the_middle_moves_cursor_after_text() {
        let mut b = buffer("ac", 1);
        b.insert_char('b');
        assert_eq!((b.text().as_str(), b.cursor()), ("abc", 2));
        assert_eq!(b.before_cursor(), "ab");
        assert_eq!(b.after_cursor(), "c");
    }

    #[test]
    fn horizontal_moves_clamp() {
        let mut b = buffer("ab", 1);
        b.move_left();
        b.move_left();
        assert_eq!(b.cursor(), 0);
        b.move_right();
        b.move_right();
        b.move_right();
        assert_eq!(b.cursor(), 2);
    }

    #[test]
    fn home_and_end_stay_on_the_current_line() {
        let mut b = buffer("ab\ncde\nf", 4);
        b.move_home();
        assert_eq!(b.cursor(), 3);
        b.move_end();
        assert_eq!(b.cursor(), 6);
        b.move_end();
        assert_eq!(b.cursor(), 6);
        let mut first = buffer("ab\nc", 1);
        first.move_home();
        assert_eq!(first.cursor(), 0);
        let mut last = buffer("ab\ncd", 3);
        last.move_end();
        assert_eq!(last.cursor(), 5);
        let mut empty_line = buffer("a\n\nb", 2);
        empty_line.move_home();
        assert_eq!(empty_line.cursor(), 2);
        empty_line.move_end();
        assert_eq!(empty_line.cursor(), 2);
    }

    #[test]
    fn vertical_moves_keep_column_and_clamp() {
        let mut b = buffer("abcd\nxy\nmnopq", 3);
        b.move_down();
        assert_eq!(b.cursor(), 7);
        b.move_down();
        assert_eq!(b.line_and_column(), (2, 2));
        b.move_down();
        assert_eq!(b.line_and_column(), (2, 2));
        b.move_up();
        assert_eq!(b.line_and_column(), (1, 2));
        b.move_up();
        assert_eq!(b.line_and_column(), (0, 2));
        b.move_up();
        assert_eq!(b.line_and_column(), (0, 2));
    }

    #[test]
    fn vertical_moves_handle_blank_lines() {
        let mut b = buffer("ab\n\ncd", 2);
        b.move_down();
        assert_eq!(b.cursor(), 3);
        b.move_down();
        assert_eq!(b.cursor(), 4);
        b.move_up();
        assert_eq!(b.cursor(), 3);
        let mut single = buffer("abc", 1);
        single.move_down();
        single.move_up();
        assert_eq!(single.cursor(), 1);
    }

    #[test]
    fn backspace_joins_lines() {
        let mut b = buffer("a\nb", 2);
        assert!(b.backspace());
        assert_eq!(b.text(), "ab");
        assert_eq!(b.cursor(), 1);
    }

    #[test]
    fn blank_detection_counts_whitespace_and_newlines() {
        assert!(TextBuffer::from_text(" \n \u{3000}").is_blank());
        assert!(!TextBuffer::from_text(" a ").is_blank());
        assert!(TextBuffer::new().is_blank());
    }

    #[test]
    fn from_text_drops_controls_and_puts_cursor_at_end() {
        let b = TextBuffer::from_text("a\u{7}b\nc");
        assert_eq!(b.text(), "ab\nc");
        assert_eq!(b.cursor(), 4);
    }

    #[test]
    fn set_cursor_clamps_to_the_end() {
        let mut b = TextBuffer::from_text("abc");
        b.set_cursor(1);
        assert_eq!(b.cursor(), 1);
        b.set_cursor(99);
        assert_eq!(b.cursor(), 3);
        b.set_cursor(0);
        assert_eq!(b.cursor(), 0);
    }

    #[test]
    fn line_and_column_reports_position() {
        assert_eq!(buffer("ab\ncd", 4).line_and_column(), (1, 1));
        assert_eq!(buffer("ab\ncd", 0).line_and_column(), (0, 0));
        assert_eq!(buffer("ab\n", 3).line_and_column(), (1, 0));
    }
}
