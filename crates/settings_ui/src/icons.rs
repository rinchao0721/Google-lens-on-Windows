//! 图标码点：Segoe Fluent Icons / Segoe MDL2 Assets 的私有区字符。

use crate::form::Page;

/// 设置齿轮。
pub const SETTINGS: char = '\u{E713}';
/// 键盘。
pub const KEYBOARD: char = '\u{E765}';
/// 搜索。
pub const SEARCH: char = '\u{E721}';
/// 相机。
pub const CAMERA: char = '\u{E722}';
/// 信息。
pub const INFO: char = '\u{E946}';
/// 电源。
pub const POWER: char = '\u{E7E8}';
/// 铃铛。
pub const RINGER: char = '\u{EA8F}';
/// 云。
pub const CLOUD: char = '\u{E753}';
/// 锁。
pub const LOCK: char = '\u{E72E}';
/// 文件夹。
pub const FOLDER: char = '\u{E8B7}';
/// 时钟。
pub const CLOCK: char = '\u{E916}';
/// 编辑。
pub const EDIT: char = '\u{E70F}';
/// 链接。
pub const LINK: char = '\u{E71B}';
/// 文档。
pub const DOCUMENT: char = '\u{E8A5}';

/// 导航项图标。
pub const fn page_icon(page: Page) -> char {
    match page {
        Page::General => SETTINGS,
        Page::Hotkeys => KEYBOARD,
        Page::SearchUpload => SEARCH,
        Page::Screenshot => CAMERA,
        Page::About => INFO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_has_a_distinct_private_use_icon() {
        let icons: Vec<char> = Page::ALL.iter().map(|p| page_icon(*p)).collect();
        for icon in &icons {
            assert!(('\u{E000}'..='\u{F8FF}').contains(icon));
        }
        let mut sorted = icons.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), icons.len());
    }
}
