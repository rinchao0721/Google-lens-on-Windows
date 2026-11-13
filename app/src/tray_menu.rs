//! 根据配置生成托盘菜单，并记录菜单项与动作的对应关系。

use lenswin_core::config::{
    Config, LitterboxExpiry, SEARCH_ENGINE_PRESETS, SearchBackend, UploadProvider, preset_index_of,
};
use lenswin_core::selection::SelectionMode;
use lenswin_platform::autostart::AutostartState;
use lenswin_platform::tray::MenuItem;

use crate::dispatcher::Command;

/// 菜单项被点击后要做的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    /// 执行命令。
    Run(Command),
    /// 切换选区模式。
    SetSelectionMode(SelectionMode),
    /// 切换搜索方式。
    SetBackend(SearchBackend),
    /// 切换为第 n 个预置搜索引擎。
    SetSearchEngine(usize),
    /// 切换图片保留时间。
    SetExpiry(LitterboxExpiry),
    /// 开启或关闭开机自启（路径过期时为更新）。
    ToggleAutostart,
    /// 打开设置窗口。
    OpenSettings,
    /// 打开配置文件夹。
    OpenConfigDir,
    /// 重新加载配置文件。
    ReloadConfig,
    /// 只用于展示状态的灰显项，点不到。
    Display,
}

/// 菜单与动作表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    /// 交给托盘显示的菜单项。
    pub items: Vec<MenuItem>,
    actions: Vec<(u16, MenuAction)>,
}

impl Menu {
    /// 由被点击的命令号还原动作。
    pub fn action(&self, id: u16) -> Option<MenuAction> {
        self.actions
            .iter()
            .find(|(candidate, _)| *candidate == id)
            .map(|(_, action)| *action)
    }
}

/// 逐项分配命令号并登记动作。
#[derive(Default)]
struct Builder {
    actions: Vec<(u16, MenuAction)>,
}

impl Builder {
    fn add(&mut self, action: MenuAction, label: String, checked: bool, enabled: bool) -> MenuItem {
        // 菜单项远少于 u16 上限；万一溢出就取 0，托盘会因 id 非法拒绝显示而不是误触发
        let id = u16::try_from(self.actions.len() + 1).unwrap_or(0);
        self.actions.push((id, action));
        MenuItem::Entry {
            id,
            label,
            checked,
            enabled,
        }
    }

    fn plain(&mut self, action: MenuAction, label: &str) -> MenuItem {
        self.add(action, label.to_string(), false, true)
    }

    fn command(&mut self, command: Command, hotkey: Option<&str>) -> MenuItem {
        let label = match hotkey {
            // 制表符让快捷键文字在菜单中右对齐
            Some(text) => format!("{}\t{text}", command.label()),
            None => command.label().to_string(),
        };
        self.add(MenuAction::Run(command), label, false, true)
    }
}

fn selection_mode_menu(builder: &mut Builder, config: &Config) -> MenuItem {
    let items = SelectionMode::ALL
        .iter()
        .map(|mode| {
            builder.add(
                MenuAction::SetSelectionMode(*mode),
                mode.label().to_string(),
                config.search.selection_mode == *mode,
                true,
            )
        })
        .collect();
    MenuItem::Submenu {
        label: "选区模式".to_string(),
        items,
    }
}

fn backend_menu(builder: &mut Builder, config: &Config) -> MenuItem {
    let items = SearchBackend::ALL
        .iter()
        .map(|backend| {
            builder.add(
                MenuAction::SetBackend(*backend),
                backend.label().to_string(),
                config.search.backend == *backend,
                true,
            )
        })
        .collect();
    MenuItem::Submenu {
        label: "搜索方式".to_string(),
        items,
    }
}

fn uses_image_host(config: &Config) -> bool {
    config.search.backend == SearchBackend::ImageHost
}

fn search_engine_menu(builder: &mut Builder, config: &Config) -> MenuItem {
    let current = preset_index_of(&config.search.engine_url_template);
    let enabled = uses_image_host(config);
    let mut items: Vec<MenuItem> = SEARCH_ENGINE_PRESETS
        .iter()
        .enumerate()
        .map(|(index, preset)| {
            builder.add(
                MenuAction::SetSearchEngine(index),
                preset.name.to_string(),
                current == Some(index),
                enabled,
            )
        })
        .collect();
    // 自定义模板只能在配置文件里改，这里仅显示当前是否在用
    items.push(builder.add(
        MenuAction::Display,
        "自定义（在配置文件中设置）".to_string(),
        current.is_none(),
        false,
    ));
    MenuItem::Submenu {
        label: "搜索引擎".to_string(),
        items,
    }
}

fn expiry_menu(builder: &mut Builder, config: &Config) -> MenuItem {
    // 保留时间只对 litterbox 有意义
    let enabled = uses_image_host(config) && config.upload.provider == UploadProvider::Litterbox;
    let items = LitterboxExpiry::ALL
        .iter()
        .map(|expiry| {
            builder.add(
                MenuAction::SetExpiry(*expiry),
                expiry.label().to_string(),
                config.upload.litterbox.expiry == *expiry,
                enabled,
            )
        })
        .collect();
    MenuItem::Submenu {
        label: "图片保留时间".to_string(),
        items,
    }
}

fn autostart_item(builder: &mut Builder, state: &AutostartState) -> MenuItem {
    match state {
        AutostartState::Enabled => builder.add(
            MenuAction::ToggleAutostart,
            "开机自启".to_string(),
            true,
            true,
        ),
        AutostartState::Disabled => builder.add(
            MenuAction::ToggleAutostart,
            "开机自启".to_string(),
            false,
            true,
        ),
        AutostartState::StalePath(_) => builder.add(
            MenuAction::ToggleAutostart,
            "开机自启（路径已变化，点击更新）".to_string(),
            false,
            true,
        ),
    }
}

/// 当前版本的托盘菜单。
pub fn build(config: &Config, autostart: &AutostartState) -> Menu {
    let mut b = Builder::default();
    let hotkeys = &config.hotkeys;
    let items = vec![
        b.command(Command::Search, Some(&hotkeys.search)),
        b.command(Command::ScreenshotCopy, Some(&hotkeys.screenshot_copy)),
        MenuItem::Separator,
        selection_mode_menu(&mut b, config),
        backend_menu(&mut b, config),
        search_engine_menu(&mut b, config),
        expiry_menu(&mut b, config),
        MenuItem::Separator,
        autostart_item(&mut b, autostart),
        b.plain(MenuAction::OpenSettings, "设置…"),
        b.plain(MenuAction::OpenConfigDir, "打开配置文件夹"),
        b.plain(MenuAction::ReloadConfig, "重新加载配置"),
        MenuItem::Separator,
        b.command(Command::CheckUpdate, None),
        b.command(Command::About, None),
        b.command(Command::Exit, None),
    ];
    Menu {
        items,
        actions: b.actions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries<'a>(items: &'a [MenuItem], out: &mut Vec<&'a MenuItem>) {
        for item in items {
            match item {
                MenuItem::Entry { .. } => out.push(item),
                MenuItem::Submenu { items, .. } => entries(items, out),
                MenuItem::Separator => {}
            }
        }
    }

    fn flat(menu: &Menu) -> Vec<&MenuItem> {
        let mut out = Vec::new();
        entries(&menu.items, &mut out);
        out
    }

    fn label_of(item: &MenuItem) -> &str {
        match item {
            MenuItem::Entry { label, .. } => label,
            MenuItem::Submenu { label, .. } => label,
            MenuItem::Separator => "",
        }
    }

    fn checked_labels(menu: &Menu) -> Vec<String> {
        flat(menu)
            .into_iter()
            .filter_map(|item| match item {
                MenuItem::Entry { label, checked, .. } if *checked => Some(label.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn ids_are_unique_positive_and_all_resolve() {
        let menu = build(&Config::default(), &AutostartState::Disabled);
        let mut ids = Vec::new();
        for item in flat(&menu) {
            if let MenuItem::Entry { id, .. } = item {
                assert!(*id > 0);
                assert!(menu.action(*id).is_some());
                ids.push(*id);
            }
        }
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
        assert_eq!(menu.action(0), None);
        assert_eq!(menu.action(9999), None);
    }

    #[test]
    fn default_config_checks_current_choices() {
        let menu = build(&Config::default(), &AutostartState::Disabled);
        let checked = checked_labels(&menu);
        assert!(checked.contains(&"圈选".to_string()));
        assert!(checked.contains(&"Google Lens".to_string()));
        assert!(checked.contains(&"1 小时".to_string()));
        assert!(!checked.contains(&"开机自启".to_string()));
    }

    #[test]
    fn menu_contains_hotkey_text() {
        let menu = build(&Config::default(), &AutostartState::Disabled);
        assert!(
            flat(&menu)
                .iter()
                .any(|i| label_of(i).contains("Win+Shift+A"))
        );
    }

    #[test]
    fn custom_template_checks_custom_entry_only() {
        let mut config = Config::default();
        config.search.engine_url_template = "https://example.com/?u={url}".to_string();
        let checked = checked_labels(&build(&config, &AutostartState::Disabled));
        assert!(checked.iter().any(|l| l.starts_with("自定义")));
        assert!(!checked.contains(&"Google Lens".to_string()));
    }

    fn enabled_of(menu: &Menu, pick: impl Fn(MenuAction) -> bool) -> Vec<bool> {
        flat(menu)
            .into_iter()
            .filter_map(|item| match item {
                MenuItem::Entry { id, enabled, .. } => {
                    menu.action(*id).filter(|a| pick(*a)).map(|_| *enabled)
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn backend_menu_checks_current_and_gates_image_host_options() {
        let embedded = build(&Config::default(), &AutostartState::Disabled);
        assert!(checked_labels(&embedded).contains(&SearchBackend::Embedded.label().to_string()));
        let engines = |a| matches!(a, MenuAction::SetSearchEngine(_) | MenuAction::SetExpiry(_));
        assert!(enabled_of(&embedded, engines).iter().all(|e| !e));
        let mut config = Config::default();
        config.search.backend = SearchBackend::ImageHost;
        let host = build(&config, &AutostartState::Disabled);
        assert!(checked_labels(&host).contains(&SearchBackend::ImageHost.label().to_string()));
        assert!(enabled_of(&host, engines).iter().all(|e| *e));
        let backends = enabled_of(&host, |a| matches!(a, MenuAction::SetBackend(_)));
        assert_eq!(backends, vec![true, true]);
    }

    #[test]
    fn expiry_is_disabled_for_custom_provider() {
        let mut config = Config::default();
        config.search.backend = SearchBackend::ImageHost;
        config.upload.provider = UploadProvider::Custom;
        let menu = build(&config, &AutostartState::Disabled);
        for item in flat(&menu) {
            if let MenuItem::Entry { id, enabled, .. } = item {
                if matches!(menu.action(*id), Some(MenuAction::SetExpiry(_))) {
                    assert!(!enabled);
                }
            }
        }
    }

    #[test]
    fn autostart_states_render_differently() {
        let on = build(&Config::default(), &AutostartState::Enabled);
        assert!(checked_labels(&on).contains(&"开机自启".to_string()));
        let stale = build(
            &Config::default(),
            &AutostartState::StalePath("x".to_string()),
        );
        assert!(
            flat(&stale)
                .iter()
                .any(|i| label_of(i).contains("路径已变化"))
        );
        assert!(
            !checked_labels(&stale)
                .iter()
                .any(|l| l.contains("开机自启"))
        );
    }
}
