//! 快捷键字符串的纯函数解析，如 `Win+Shift+A`。

use std::fmt;
use std::str::FromStr;

use crate::error::HotkeyError;

/// 修饰键集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(u8);

impl Modifiers {
    /// 空集合。
    pub const NONE: Self = Self(0);
    /// Ctrl。
    pub const CTRL: Self = Self(1);
    /// Alt。
    pub const ALT: Self = Self(2);
    /// Shift。
    pub const SHIFT: Self = Self(4);
    /// Windows 键。
    pub const WIN: Self = Self(8);

    /// 是否包含 `other` 中的全部修饰键。
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// 并集。
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// 是否为空。
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// 具名主键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedKey {
    /// 空格。
    Space,
    /// 回车。
    Enter,
    /// Tab。
    Tab,
    /// Esc。
    Escape,
    /// 退格。
    Backspace,
    /// Insert。
    Insert,
    /// Delete。
    Delete,
    /// Home。
    Home,
    /// End。
    End,
    /// PageUp。
    PageUp,
    /// PageDown。
    PageDown,
    /// 上方向键。
    Up,
    /// 下方向键。
    Down,
    /// 左方向键。
    Left,
    /// 右方向键。
    Right,
    /// PrintScreen。
    PrintScreen,
}

impl NamedKey {
    /// 全部具名键，供界面生成下拉列表。
    pub const ALL: [NamedKey; 16] = [
        NamedKey::Space,
        NamedKey::Enter,
        NamedKey::Tab,
        NamedKey::Escape,
        NamedKey::Backspace,
        NamedKey::Insert,
        NamedKey::Delete,
        NamedKey::Home,
        NamedKey::End,
        NamedKey::PageUp,
        NamedKey::PageDown,
        NamedKey::Up,
        NamedKey::Down,
        NamedKey::Left,
        NamedKey::Right,
        NamedKey::PrintScreen,
    ];

    /// 规范名称。
    pub const fn name(self) -> &'static str {
        match self {
            NamedKey::Space => "Space",
            NamedKey::Enter => "Enter",
            NamedKey::Tab => "Tab",
            NamedKey::Escape => "Escape",
            NamedKey::Backspace => "Backspace",
            NamedKey::Insert => "Insert",
            NamedKey::Delete => "Delete",
            NamedKey::Home => "Home",
            NamedKey::End => "End",
            NamedKey::PageUp => "PageUp",
            NamedKey::PageDown => "PageDown",
            NamedKey::Up => "Up",
            NamedKey::Down => "Down",
            NamedKey::Left => "Left",
            NamedKey::Right => "Right",
            NamedKey::PrintScreen => "PrintScreen",
        }
    }

    /// Windows 虚拟键码。
    pub const fn vk_code(self) -> u16 {
        match self {
            NamedKey::Space => 0x20,
            NamedKey::Enter => 0x0D,
            NamedKey::Tab => 0x09,
            NamedKey::Escape => 0x1B,
            NamedKey::Backspace => 0x08,
            NamedKey::Insert => 0x2D,
            NamedKey::Delete => 0x2E,
            NamedKey::Home => 0x24,
            NamedKey::End => 0x23,
            NamedKey::PageUp => 0x21,
            NamedKey::PageDown => 0x22,
            NamedKey::Up => 0x26,
            NamedKey::Down => 0x28,
            NamedKey::Left => 0x25,
            NamedKey::Right => 0x27,
            NamedKey::PrintScreen => 0x2C,
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        let lower = if lower == "esc" {
            "escape"
        } else {
            lower.as_str()
        };
        Self::ALL
            .into_iter()
            .find(|k| k.name().eq_ignore_ascii_case(lower))
    }
}

/// 主键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// 大写字母 `A`–`Z`。
    Letter(char),
    /// 数字 0–9。
    Digit(u8),
    /// 功能键 F1–F24。
    Function(u8),
    /// 具名键。
    Named(NamedKey),
}

impl Key {
    /// Windows 虚拟键码；字段值不在合法范围内时返回 `None`。
    pub fn vk_code(self) -> Option<u16> {
        match self {
            Key::Letter(c) if c.is_ascii_uppercase() => u16::try_from(u32::from(c)).ok(),
            Key::Digit(d) if d <= 9 => Some(0x30 + u16::from(d)),
            Key::Function(n) if (1..=24).contains(&n) => Some(0x70 + u16::from(n) - 1),
            Key::Named(k) => Some(k.vk_code()),
            _ => None,
        }
    }

    fn parse(token: &str) -> Option<Self> {
        let mut chars = token.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if c.is_ascii_alphabetic() {
                return Some(Key::Letter(c.to_ascii_uppercase()));
            }
            if let Some(d) = c.to_digit(10).filter(|_| c.is_ascii_digit()) {
                return u8::try_from(d).ok().map(Key::Digit);
            }
        }
        if let Some(num) = token.strip_prefix(['F', 'f']) {
            if let Ok(n) = num.parse::<u8>() {
                if (1..=24).contains(&n) {
                    return Some(Key::Function(n));
                }
            }
            return None;
        }
        NamedKey::from_name(token).map(Key::Named)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Letter(c) => write!(f, "{c}"),
            Key::Digit(d) => write!(f, "{d}"),
            Key::Function(n) => write!(f, "F{n}"),
            Key::Named(k) => f.write_str(k.name()),
        }
    }
}

/// 解析后的快捷键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hotkey {
    modifiers: Modifiers,
    key: Key,
}

impl Hotkey {
    /// 构造快捷键；没有修饰键，或主键取值不合法时返回错误。
    pub fn new(modifiers: Modifiers, key: Key) -> Result<Self, HotkeyError> {
        if key.vk_code().is_none() {
            return Err(HotkeyError::UnknownKey(key.to_string()));
        }
        if modifiers.is_empty() {
            return Err(HotkeyError::NoModifier);
        }
        Ok(Self { modifiers, key })
    }

    /// 修饰键，至少一个。
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// 主键。
    pub fn key(&self) -> Key {
        self.key
    }
}

/// 全部可选主键，供界面生成下拉列表：字母、数字、F1–F24、具名键。
pub fn selectable_keys() -> Vec<Key> {
    let letters = ('A'..='Z').map(Key::Letter);
    let digits = (0..=9).map(Key::Digit);
    let functions = (1..=24).map(Key::Function);
    let named = NamedKey::ALL.into_iter().map(Key::Named);
    letters
        .chain(digits)
        .chain(functions)
        .chain(named)
        .collect()
}

/// 界面上的快捷键编辑状态：四个修饰键复选框加一个主键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyChoice {
    /// Ctrl。
    pub ctrl: bool,
    /// Alt。
    pub alt: bool,
    /// Shift。
    pub shift: bool,
    /// Windows 键。
    pub win: bool,
    /// 主键。
    pub key: Key,
}

impl HotkeyChoice {
    /// 由快捷键文字还原；文字无法解析时返回 `None`。
    pub fn parse(text: &str) -> Option<Self> {
        let hotkey: Hotkey = text.parse().ok()?;
        let m = hotkey.modifiers();
        Some(Self {
            ctrl: m.contains(Modifiers::CTRL),
            alt: m.contains(Modifiers::ALT),
            shift: m.contains(Modifiers::SHIFT),
            win: m.contains(Modifiers::WIN),
            key: hotkey.key(),
        })
    }

    /// 修饰键集合。
    pub fn modifiers(&self) -> Modifiers {
        [
            (self.ctrl, Modifiers::CTRL),
            (self.alt, Modifiers::ALT),
            (self.shift, Modifiers::SHIFT),
            (self.win, Modifiers::WIN),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .fold(Modifiers::NONE, |acc, (_, m)| acc.union(m))
    }

    /// 转成规范的快捷键文字；没有勾选任何修饰键时返回错误。
    pub fn to_text(&self) -> Result<String, HotkeyError> {
        Hotkey::new(self.modifiers(), self.key).map(|h| h.to_string())
    }
}

fn parse_modifier(token: &str) -> Option<Modifiers> {
    match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(Modifiers::CTRL),
        "alt" => Some(Modifiers::ALT),
        "shift" => Some(Modifiers::SHIFT),
        "win" | "windows" => Some(Modifiers::WIN),
        _ => None,
    }
}

impl FromStr for Hotkey {
    type Err = HotkeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(HotkeyError::Empty);
        }
        let tokens: Vec<&str> = trimmed.split('+').map(str::trim).collect();
        if tokens.iter().any(|t| t.is_empty()) {
            return Err(HotkeyError::EmptyToken);
        }
        let (key_token, modifier_tokens) = tokens.split_last().ok_or(HotkeyError::Empty)?;

        let mut modifiers = Modifiers::NONE;
        for token in modifier_tokens {
            let m = parse_modifier(token)
                .ok_or_else(|| HotkeyError::UnknownModifier((*token).to_string()))?;
            if modifiers.contains(m) {
                return Err(HotkeyError::DuplicateModifier((*token).to_string()));
            }
            modifiers = modifiers.union(m);
        }
        if parse_modifier(key_token).is_some() {
            return Err(HotkeyError::MissingKey);
        }
        let key = Key::parse(key_token)
            .ok_or_else(|| HotkeyError::UnknownKey((*key_token).to_string()))?;
        Hotkey::new(modifiers, key)
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = [
            (Modifiers::CTRL, "Ctrl"),
            (Modifiers::ALT, "Alt"),
            (Modifiers::SHIFT, "Shift"),
            (Modifiers::WIN, "Win"),
        ];
        for (m, name) in names {
            if self.modifiers.contains(m) {
                write!(f, "{name}+")?;
            }
        }
        write!(f, "{}", self.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Hotkey, HotkeyError> {
        s.parse()
    }

    #[test]
    fn parses_default_hotkeys() {
        let h = parse("Win+Shift+A").expect("默认搜索快捷键");
        assert_eq!(h.modifiers(), Modifiers::WIN.union(Modifiers::SHIFT));
        assert_eq!(h.key(), Key::Letter('A'));
        assert!(parse("Win+Shift+C").is_ok());
    }

    #[test]
    fn is_case_and_space_insensitive() {
        let a = parse("ctrl + ALT + f12").expect("解析");
        let b = parse("Ctrl+Alt+F12").expect("解析");
        assert_eq!(a, b);
        assert_eq!(a.key(), Key::Function(12));
    }

    #[test]
    fn new_enforces_invariants() {
        let key = Key::Letter('A');
        assert_eq!(
            Hotkey::new(Modifiers::NONE, key),
            Err(HotkeyError::NoModifier)
        );
        assert_eq!(
            Hotkey::new(Modifiers::CTRL, Key::Letter('a')),
            Err(HotkeyError::UnknownKey("a".into()))
        );
        assert_eq!(
            Hotkey::new(Modifiers::CTRL, Key::Function(25)),
            Err(HotkeyError::UnknownKey("F25".into()))
        );
        assert!(Hotkey::new(Modifiers::CTRL, key).is_ok());
    }

    #[test]
    fn modifier_order_does_not_matter() {
        assert_eq!(parse("Shift+Win+A"), parse("Win+Shift+A"));
    }

    #[test]
    fn display_roundtrips() {
        for s in [
            "Ctrl+Alt+Shift+Win+Z",
            "Alt+Space",
            "Ctrl+F24",
            "Win+5",
            "Ctrl+Escape",
        ] {
            let h = parse(s).expect("解析");
            assert_eq!(parse(&h.to_string()), Ok(h), "{s}");
        }
    }

    #[test]
    fn esc_alias_maps_to_escape() {
        assert_eq!(parse("Ctrl+Esc"), parse("Ctrl+Escape"));
    }

    #[test]
    fn rejects_invalid_forms() {
        assert_eq!(parse(""), Err(HotkeyError::Empty));
        assert_eq!(parse("   "), Err(HotkeyError::Empty));
        assert_eq!(parse("Ctrl++A"), Err(HotkeyError::EmptyToken));
        assert_eq!(parse("Ctrl+"), Err(HotkeyError::EmptyToken));
        assert_eq!(parse("Ctrl+Shift"), Err(HotkeyError::MissingKey));
        assert_eq!(parse("A"), Err(HotkeyError::NoModifier));
        assert_eq!(
            parse("Ctrl+Ctrl+A"),
            Err(HotkeyError::DuplicateModifier("Ctrl".into()))
        );
        assert_eq!(
            parse("Foo+A"),
            Err(HotkeyError::UnknownModifier("Foo".into()))
        );
        assert_eq!(
            parse("Ctrl+F25"),
            Err(HotkeyError::UnknownKey("F25".into()))
        );
        assert_eq!(parse("Ctrl+F0"), Err(HotkeyError::UnknownKey("F0".into())));
        assert_eq!(parse("Ctrl+AB"), Err(HotkeyError::UnknownKey("AB".into())));
        assert_eq!(parse("Ctrl+é"), Err(HotkeyError::UnknownKey("é".into())));
    }

    #[test]
    fn vk_codes() {
        assert_eq!(Key::Letter('A').vk_code(), Some(0x41));
        assert_eq!(Key::Digit(0).vk_code(), Some(0x30));
        assert_eq!(Key::Function(1).vk_code(), Some(0x70));
        assert_eq!(Key::Function(24).vk_code(), Some(0x87));
        assert_eq!(Key::Named(NamedKey::Space).vk_code(), Some(0x20));
        assert_eq!(Key::Letter('a').vk_code(), None);
        assert_eq!(Key::Digit(10).vk_code(), None);
        assert_eq!(Key::Function(0).vk_code(), None);
    }

    #[test]
    fn selectable_keys_are_unique_valid_and_parse_back() {
        let keys = selectable_keys();
        assert_eq!(keys.len(), 26 + 10 + 24 + NamedKey::ALL.len());
        for (i, key) in keys.iter().enumerate() {
            assert!(key.vk_code().is_some(), "{key}");
            assert_eq!(keys.iter().position(|k| k == key), Some(i), "{key}");
            let text = format!("Ctrl+{key}");
            assert_eq!(parse(&text).expect("解析").key(), *key, "{text}");
        }
    }

    #[test]
    fn choice_roundtrips_default_and_custom_hotkeys() {
        // 输出按 Ctrl、Alt、Shift、Win 的固定顺序，与输入顺序无关
        for text in [
            "Shift+Win+A",
            "Ctrl+Alt+F13",
            "Ctrl+Shift+PrintScreen",
            "Alt+0",
        ] {
            let choice = HotkeyChoice::parse(text).expect("解析");
            assert_eq!(choice.to_text().expect("生成"), text);
        }
        let default = HotkeyChoice::parse("Win+Shift+A").expect("解析");
        assert_eq!(default.to_text().expect("生成"), "Shift+Win+A");
        assert_eq!(HotkeyChoice::parse("bad"), None);
        assert_eq!(HotkeyChoice::parse(""), None);
    }

    #[test]
    fn choice_without_modifier_is_rejected() {
        let mut choice = HotkeyChoice::parse("Ctrl+A").expect("解析");
        choice.ctrl = false;
        assert_eq!(choice.to_text(), Err(HotkeyError::NoModifier));
        assert!(choice.modifiers().is_empty());
    }

    #[test]
    fn named_keys_all_parse_back() {
        for k in NamedKey::ALL {
            assert_eq!(NamedKey::from_name(k.name()), Some(k));
        }
    }
}
