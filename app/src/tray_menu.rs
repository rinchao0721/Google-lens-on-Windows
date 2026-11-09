//! 根据配置生成托盘菜单。

use lenswin_core::config::HotkeysConfig;
use lenswin_platform::tray::MenuItem;

use crate::dispatcher::Command;

fn entry(command: Command, hotkey: Option<&str>) -> MenuItem {
    let label = match hotkey {
        // 制表符让快捷键文字在菜单中右对齐
        Some(text) => format!("{}\t{text}", command.label()),
        None => command.label().to_string(),
    };
    MenuItem::Entry {
        id: command.menu_id(),
        label,
    }
}

/// 当前版本的托盘菜单。
pub fn items(hotkeys: &HotkeysConfig) -> Vec<MenuItem> {
    vec![
        entry(Command::Search, Some(&hotkeys.search)),
        entry(Command::ScreenshotCopy, Some(&hotkeys.screenshot_copy)),
        MenuItem::Separator,
        entry(Command::About, None),
        entry(Command::Exit, None),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_contains_hotkeys_and_unique_ids() {
        let items = items(&HotkeysConfig::default());
        let mut ids: Vec<u16> = items
            .iter()
            .filter_map(|i| match i {
                MenuItem::Entry { id, .. } => Some(*id),
                MenuItem::Separator => None,
            })
            .collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
        assert!(ids.iter().all(|id| *id > 0));
        assert!(
            items.iter().any(
                |i| matches!(i, MenuItem::Entry { label, .. } if label.contains("Win+Shift+A"))
            )
        );
    }
}
