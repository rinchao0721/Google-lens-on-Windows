//! 热键、托盘事件到命令的路由。

use lenswin_core::config::Config;
use lenswin_core::selection::SelectionMode;
use lenswin_overlay::SelectRequest;
use lenswin_platform::capture::ScreenCapturer;
use lenswin_platform::capture::gdi::GdiCapturer;
use lenswin_platform::message_loop::LoopControl;
use lenswin_platform::monitor;
use lenswin_platform::notify::{self, NotifyLevel};
use lenswin_platform::paths::AppPaths;
use lenswin_platform::tray::TrayIcon;
use lenswin_platform::window::{TrayMouse, WindowEvent, discard_pending_hotkeys};

use crate::tray_menu;

/// 用户可触发的命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// 圈选搜索。
    Search,
    /// 截屏到剪贴板。
    ScreenshotCopy,
    /// 关于。
    About,
    /// 退出。
    Exit,
}

impl Command {
    /// 托盘菜单项命令号，必须大于 0 且互不相同。
    pub const fn menu_id(self) -> u16 {
        match self {
            Command::Search => 1,
            Command::ScreenshotCopy => 2,
            Command::About => 3,
            Command::Exit => 4,
        }
    }

    /// 由菜单命令号还原。
    pub fn from_menu_id(id: u16) -> Option<Self> {
        [
            Command::Search,
            Command::ScreenshotCopy,
            Command::About,
            Command::Exit,
        ]
        .into_iter()
        .find(|c| c.menu_id() == id)
    }

    /// 全局热键 id，仅可由热键触发的命令有值。
    pub const fn hotkey_id(self) -> Option<i32> {
        match self {
            Command::Search => Some(1),
            Command::ScreenshotCopy => Some(2),
            Command::About | Command::Exit => None,
        }
    }

    /// 由热键 id 还原。
    pub fn from_hotkey_id(id: i32) -> Option<Self> {
        [Command::Search, Command::ScreenshotCopy]
            .into_iter()
            .find(|c| c.hotkey_id() == Some(id))
    }

    /// 菜单与日志中的名称。
    pub const fn label(self) -> &'static str {
        match self {
            Command::Search => "圈选搜索",
            Command::ScreenshotCopy => "截屏到剪贴板",
            Command::About => "关于 lenswin",
            Command::Exit => "退出",
        }
    }
}

/// 把窗口事件分发为具体行为。
pub struct Dispatcher<'a> {
    tray: &'a TrayIcon,
    paths: &'a AppPaths,
    config: Config,
}

impl<'a> Dispatcher<'a> {
    /// 构造分发器。
    pub fn new(tray: &'a TrayIcon, paths: &'a AppPaths, config: Config) -> Self {
        Self {
            tray,
            paths,
            config,
        }
    }

    /// 处理一个事件，返回消息循环的走向。
    pub fn handle(&self, event: WindowEvent) -> LoopControl {
        match event {
            WindowEvent::Hotkey(id) => match Command::from_hotkey_id(id) {
                Some(command) => self.run(command),
                None => {
                    log::warn!("收到未知热键 id：{id}");
                    LoopControl::Continue
                }
            },
            WindowEvent::Tray(TrayMouse::RightClick) => self.show_menu(),
            WindowEvent::Tray(TrayMouse::LeftClick) => LoopControl::Continue,
            WindowEvent::TaskbarCreated => {
                if let Err(e) = self.tray.re_add() {
                    log::error!("资源管理器重启后重建托盘图标失败：{e}");
                }
                LoopControl::Continue
            }
            WindowEvent::SecondInstance => {
                log::info!("检测到第二个实例启动");
                self.balloon(NotifyLevel::Info, "lenswin", "lenswin 已在运行");
                LoopControl::Continue
            }
        }
    }

    fn show_menu(&self) -> LoopControl {
        match self.tray.show_menu(&tray_menu::items(&self.config.hotkeys)) {
            Ok(Some(id)) => match Command::from_menu_id(id) {
                Some(command) => self.run(command),
                None => {
                    log::warn!("收到未知菜单项 id：{id}");
                    LoopControl::Continue
                }
            },
            Ok(None) => LoopControl::Continue,
            Err(e) => {
                log::error!("显示托盘菜单失败：{e}");
                LoopControl::Continue
            }
        }
    }

    fn run(&self, command: Command) -> LoopControl {
        match command {
            Command::Search => {
                self.pick_region(command, self.config.search.selection_mode);
                LoopControl::Continue
            }
            Command::ScreenshotCopy => {
                self.pick_region(command, SelectionMode::Rect);
                LoopControl::Continue
            }
            Command::About => {
                let text = format!(
                    "版本 {} · {}",
                    env!("CARGO_PKG_VERSION"),
                    self.paths.mode.label()
                );
                self.balloon(NotifyLevel::Info, "lenswin", &text);
                LoopControl::Continue
            }
            Command::Exit => {
                log::info!("用户选择退出");
                LoopControl::Quit
            }
        }
    }

    /// 冻结屏幕并让用户选区；M4 起由 actions 接手后续动作，目前只记录选区结果。
    fn pick_region(&self, command: Command, mode: SelectionMode) {
        log::info!("触发命令：{}", command.label());
        let monitors = match monitor::enumerate() {
            Ok(list) => list,
            Err(e) => {
                log::error!("枚举显示器失败：{e}");
                return;
            }
        };
        let frames = match GdiCapturer.capture_all(&monitors) {
            Ok(frames) => frames,
            Err(e) => {
                log::error!("截屏失败：{e}");
                self.balloon(NotifyLevel::Error, "lenswin", &format!("截屏失败：{e}"));
                return;
            }
        };
        let request = SelectRequest {
            frames: &frames,
            monitors: &monitors,
            mode,
            lasso: &self.config.selection.lasso,
        };
        match lenswin_overlay::select(&request) {
            Ok(Some(selection)) => log::info!(
                "选区完成：范围 {:?}，显示器 {}",
                selection.bounds(),
                selection.monitor_index()
            ),
            Ok(None) => log::info!("选区已取消"),
            Err(e) => {
                log::error!("选区失败：{e}");
                self.balloon(NotifyLevel::Error, "lenswin", &format!("选区失败：{e}"));
            }
        }
        // 覆盖层期间积压的热键不应在结束后再次触发
        discard_pending_hotkeys();
    }

    /// 弹气泡；失败只记日志，不影响主流程。
    pub fn balloon(&self, level: NotifyLevel, title: &str, text: &str) {
        if let Err(e) = notify::balloon(self.tray, level, title, text) {
            log::error!("显示通知失败：{e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_roundtrip() {
        for c in [
            Command::Search,
            Command::ScreenshotCopy,
            Command::About,
            Command::Exit,
        ] {
            assert_eq!(Command::from_menu_id(c.menu_id()), Some(c));
        }
        assert_eq!(Command::from_menu_id(0), None);
        assert_eq!(Command::from_menu_id(99), None);
    }

    #[test]
    fn hotkey_ids_roundtrip() {
        assert_eq!(Command::from_hotkey_id(1), Some(Command::Search));
        assert_eq!(Command::from_hotkey_id(2), Some(Command::ScreenshotCopy));
        assert_eq!(Command::from_hotkey_id(3), None);
        assert_eq!(Command::About.hotkey_id(), None);
    }
}
