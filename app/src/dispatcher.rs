//! 热键、托盘事件到命令的路由。

use std::cell::RefCell;
use std::sync::mpsc::{Receiver, Sender, channel};

use lenswin_actions::ActionRegistry;
use lenswin_core::action::{ActionContext, ActionInput, ActionKind};
use lenswin_core::frame::{CroppedImage, Frame};
use lenswin_core::selection::{Selection, SelectionMode};
use lenswin_overlay::SelectRequest;
use lenswin_platform::capture::ScreenCapturer;
use lenswin_platform::capture::gdi::GdiCapturer;
use lenswin_platform::dialog;
use lenswin_platform::message_loop::LoopControl;
use lenswin_platform::monitor;
use lenswin_platform::notify::{self, NotifyLevel};
use lenswin_platform::paths::AppPaths;
use lenswin_platform::tray::TrayIcon;
use lenswin_platform::window::{TaskNotifier, TrayMouse, WindowEvent, discard_pending_hotkeys};
use lenswin_upload::privacy_summary;

use crate::config_file::{self, LoadedConfig};
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

/// 后台任务的结果，由工作线程投递、主线程取出提示。
struct TaskResult {
    label: &'static str,
    error: Option<String>,
}

/// 把窗口事件分发为具体行为。
pub struct Dispatcher<'a> {
    tray: &'a TrayIcon,
    paths: &'a AppPaths,
    config: RefCell<LoadedConfig>,
    registry: ActionRegistry,
    notifier: TaskNotifier,
    results_tx: Sender<TaskResult>,
    results_rx: Receiver<TaskResult>,
}

impl<'a> Dispatcher<'a> {
    /// 构造分发器。
    pub fn new(
        tray: &'a TrayIcon,
        paths: &'a AppPaths,
        config: LoadedConfig,
        registry: ActionRegistry,
        notifier: TaskNotifier,
    ) -> Self {
        let (results_tx, results_rx) = channel();
        Self {
            tray,
            paths,
            config: RefCell::new(config),
            registry,
            notifier,
            results_tx,
            results_rx,
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
            WindowEvent::TaskDone => {
                self.drain_results();
                LoopControl::Continue
            }
        }
    }

    fn show_menu(&self) -> LoopControl {
        let items = tray_menu::items(&self.config.borrow().config.hotkeys);
        match self.tray.show_menu(&items) {
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
                self.search();
                LoopControl::Continue
            }
            Command::ScreenshotCopy => {
                self.screenshot_copy();
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

    /// 圈选 → 隐私确认 → 在工作线程上传并打开搜索结果。
    fn search(&self) {
        let mode = self.config.borrow().config.search.selection_mode;
        let Some(image) = self.pick_image(Command::Search, mode) else {
            return;
        };
        if self.ensure_upload_acknowledged() {
            self.spawn(ActionKind::Search, "搜索", image);
        } else {
            log::info!("上传未获确认，已丢弃截图");
        }
    }

    /// 框选 → 复制到剪贴板；配置了保存目录时再到工作线程保存文件。
    fn screenshot_copy(&self) {
        let Some(image) = self.pick_image(Command::ScreenshotCopy, SelectionMode::Rect) else {
            return;
        };
        let save = !self
            .config
            .borrow()
            .config
            .screenshot
            .save_dir
            .trim()
            .is_empty();
        let spare = save.then(|| image.clone());
        match self.registry.get(ActionKind::Copy) {
            Some(copy) => match copy.run(&self.context(), ActionInput::Image(image)) {
                Ok(_) => log::info!("截图已复制到剪贴板"),
                Err(e) => {
                    log::error!("复制失败：{e}");
                    self.balloon(NotifyLevel::Error, "lenswin", &format!("复制失败：{e}"));
                }
            },
            None => log::error!("复制动作未注册"),
        }
        if let Some(image) = spare {
            self.spawn(ActionKind::Save, "保存", image);
        }
    }

    fn context(&self) -> ActionContext {
        ActionContext {
            config: self.config.borrow().config.clone(),
        }
    }

    /// 首次向某个图床上传前弹确认；确认后记入配置。
    fn ensure_upload_acknowledged(&self) -> bool {
        let config = self.config.borrow().config.clone();
        let summary = match privacy_summary(&config.upload) {
            Ok(summary) => summary,
            Err(e) => {
                log::error!("上传配置无效：{e}");
                self.balloon(NotifyLevel::Error, "lenswin", &format!("上传配置无效：{e}"));
                return false;
            }
        };
        if config.privacy.is_acknowledged(&summary.host) {
            return true;
        }
        let text = format!(
            "截图将上传到 {}（{}）。\n\n任何拿到链接的人都能看到这张图片。\n是否继续上传？",
            summary.host, summary.retention
        );
        match dialog::confirm("lenswin 隐私提示", &text) {
            Ok(true) => {
                self.config.borrow_mut().config.privacy.acknowledged_host = summary.host;
                self.persist_config();
                true
            }
            Ok(false) => false,
            Err(e) => {
                log::error!("显示隐私确认框失败：{e}");
                self.balloon(
                    NotifyLevel::Error,
                    "lenswin",
                    &format!("无法显示确认框：{e}"),
                );
                false
            }
        }
    }

    /// 把当前配置写回文件；失败只提示，本次运行仍按已确认处理。
    fn persist_config(&self) {
        let loaded = self.config.borrow().clone();
        let result = config_file::persist(&self.paths.config_file(), loaded.state, &loaded.config);
        if let Err(e) = result {
            log::warn!("保存配置失败：{e}");
            self.balloon(
                NotifyLevel::Warning,
                "lenswin",
                &format!("隐私确认未能保存，下次仍会询问：{e}"),
            );
        }
    }

    /// 在工作线程执行动作，结果经任务通知回到主线程。
    fn spawn(&self, kind: ActionKind, label: &'static str, image: CroppedImage) {
        let Some(action) = self.registry.get(kind) else {
            log::error!("动作 {kind:?} 未注册");
            return;
        };
        let ctx = self.context();
        let tx = self.results_tx.clone();
        let notifier = self.notifier;
        let spawned = std::thread::Builder::new()
            .name(format!("lenswin-{label}"))
            .spawn(move || {
                let error = action
                    .run(&ctx, ActionInput::Image(image))
                    .err()
                    .map(|e| e.to_string());
                if tx.send(TaskResult { label, error }).is_err() {
                    return;
                }
                if let Err(e) = notifier.notify() {
                    log::error!("投递任务结果失败：{e}");
                }
            });
        if let Err(e) = spawned {
            log::error!("创建工作线程失败：{e}");
            self.balloon(NotifyLevel::Error, "lenswin", &format!("{label}失败：{e}"));
        }
    }

    fn drain_results(&self) {
        while let Ok(result) = self.results_rx.try_recv() {
            match result.error {
                Some(message) => {
                    log::error!("{}失败：{message}", result.label);
                    self.balloon(
                        NotifyLevel::Error,
                        "lenswin",
                        &format!("{}失败：{message}", result.label),
                    );
                }
                None => log::info!("{}完成", result.label),
            }
        }
    }

    /// 冻结屏幕让用户选区，并裁出对应图像；取消或失败返回 `None`。
    fn pick_image(&self, command: Command, mode: SelectionMode) -> Option<CroppedImage> {
        let (selection, frames) = self.pick_region(command, mode)?;
        let Some(frame) = frames.get(selection.monitor_index()) else {
            log::error!(
                "选区所在显示器序号 {} 没有对应截屏",
                selection.monitor_index()
            );
            return None;
        };
        match frame.crop(&selection.bounds()) {
            Ok(image) => Some(image),
            Err(e) => {
                log::error!("裁剪失败：{e}");
                self.balloon(NotifyLevel::Error, "lenswin", &format!("裁剪失败：{e}"));
                None
            }
        }
    }

    fn pick_region(
        &self,
        command: Command,
        mode: SelectionMode,
    ) -> Option<(Selection, Vec<Frame>)> {
        log::info!("触发命令：{}", command.label());
        let monitors = match monitor::enumerate() {
            Ok(list) => list,
            Err(e) => {
                log::error!("枚举显示器失败：{e}");
                return None;
            }
        };
        let frames = match GdiCapturer.capture_all(&monitors) {
            Ok(frames) => frames,
            Err(e) => {
                log::error!("截屏失败：{e}");
                self.balloon(NotifyLevel::Error, "lenswin", &format!("截屏失败：{e}"));
                return None;
            }
        };
        let lasso = self.config.borrow().config.selection.lasso.clone();
        let request = SelectRequest {
            frames: &frames,
            monitors: &monitors,
            mode,
            lasso: &lasso,
        };
        let outcome = lenswin_overlay::select(&request);
        // 覆盖层期间积压的热键不应在结束后再次触发
        discard_pending_hotkeys();
        match outcome {
            Ok(Some(selection)) => {
                log::info!(
                    "选区完成：范围 {:?}，显示器 {}",
                    selection.bounds(),
                    selection.monitor_index()
                );
                Some((selection, frames))
            }
            Ok(None) => {
                log::info!("选区已取消");
                None
            }
            Err(e) => {
                log::error!("选区失败：{e}");
                self.balloon(NotifyLevel::Error, "lenswin", &format!("选区失败：{e}"));
                None
            }
        }
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
