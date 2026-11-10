//! 热键、托盘事件到命令的路由。

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use lenswin_actions::ActionRegistry;
use lenswin_core::action::{ActionContext, ActionInput, ActionKind};
use lenswin_core::config::{Config, SEARCH_ENGINE_PRESETS, TrayLeftClick};
use lenswin_core::frame::{CroppedImage, Frame};
use lenswin_core::ipc::SETTINGS_ARG;
use lenswin_core::selection::{Selection, SelectionMode};
use lenswin_overlay::SelectRequest;
use lenswin_platform::autostart::{self, AutostartState};
use lenswin_platform::capture::ScreenCapturer;
use lenswin_platform::capture::gdi::GdiCapturer;
use lenswin_platform::dialog;
use lenswin_platform::message_loop::LoopControl;
use lenswin_platform::monitor;
use lenswin_platform::notify::{self, NotifyLevel};
use lenswin_platform::paths::AppPaths;
use lenswin_platform::shell;
use lenswin_platform::tray::TrayIcon;
use lenswin_platform::window::{TaskNotifier, TrayMouse, WindowEvent, discard_pending_triggers};
use lenswin_upload::privacy_summary;

use crate::hotkeys::{self, HotkeySet};
use crate::shared::AppShared;
use crate::tray_menu::{self, MenuAction};

/// 开机自启在注册表里的值名。
const AUTOSTART_NAME: &str = autostart::VALUE_NAME;

/// 托盘左键配置对应的命令；「无操作」返回 `None`。
fn left_click_command(action: TrayLeftClick) -> Option<Command> {
    match action {
        TrayLeftClick::Search => Some(Command::Search),
        TrayLeftClick::ScreenshotCopy => Some(Command::ScreenshotCopy),
        TrayLeftClick::Nothing => None,
    }
}

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
    shared: Rc<AppShared>,
    registry: ActionRegistry,
    notifier: TaskNotifier,
    exe: Option<PathBuf>,
    results_tx: Sender<TaskResult>,
    results_rx: Receiver<TaskResult>,
}

impl<'a> Dispatcher<'a> {
    /// 构造分发器；热键需随后调用 [`Dispatcher::install_hotkeys`] 注册。
    pub fn new(
        tray: &'a TrayIcon,
        paths: &'a AppPaths,
        shared: Rc<AppShared>,
        registry: ActionRegistry,
        notifier: TaskNotifier,
    ) -> Self {
        let (results_tx, results_rx) = channel();
        let exe = match std::env::current_exe() {
            Ok(path) => Some(path),
            Err(e) => {
                log::warn!("获取 exe 路径失败，开机自启不可用：{e}");
                None
            }
        };
        Self {
            tray,
            paths,
            shared,
            registry,
            notifier,
            exe,
            results_tx,
            results_rx,
        }
    }

    /// 按当前配置注册热键，失败项合并成一条气泡并指明配置文件位置。
    pub fn install_hotkeys(&self) {
        let config = self.shared.store.snapshot();
        let (set, failures) = HotkeySet::register(self.shared.window, &config.hotkeys);
        self.shared.hotkeys.replace(set);
        if !failures.is_empty() {
            let text = hotkeys::failure_summary(&failures, self.shared.store.path());
            self.balloon(NotifyLevel::Warning, "lenswin", &text);
        }
    }

    /// 自启项指向别的路径（如便携版被移动）时提示用户更新。
    pub fn check_autostart(&self) {
        if let AutostartState::StalePath(command) = self.autostart_state() {
            log::warn!("开机自启指向其他位置：{command}");
            self.balloon(
                NotifyLevel::Warning,
                "lenswin",
                "开机自启指向的位置已变化，可在托盘菜单里重新开启以更新",
            );
        }
    }

    fn autostart_state(&self) -> AutostartState {
        let Some(exe) = &self.exe else {
            return AutostartState::Disabled;
        };
        match autostart::state(AUTOSTART_NAME, exe) {
            Ok(state) => state,
            Err(e) => {
                log::warn!("读取开机自启状态失败：{e}");
                AutostartState::Disabled
            }
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
            WindowEvent::Tray(TrayMouse::LeftClick) => {
                let action = self.shared.store.snapshot().general.tray_left_click;
                match left_click_command(action) {
                    Some(command) => self.run(command),
                    None => LoopControl::Continue,
                }
            }
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
        let mut config = self.shared.store.snapshot();
        // 菜单显示实际生效的热键，而不是文件里可能被占用的写法
        config.hotkeys = self.shared.hotkeys.borrow().effective().clone();
        let menu = tray_menu::build(&config, &self.autostart_state());
        match self.tray.show_menu(&menu.items) {
            Ok(Some(id)) => match menu.action(id) {
                Some(action) => self.perform(action),
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

    fn perform(&self, action: MenuAction) -> LoopControl {
        match action {
            MenuAction::Run(command) => return self.run(command),
            MenuAction::SetSelectionMode(mode) => {
                self.apply_change(|c| c.search.selection_mode = mode);
            }
            MenuAction::SetSearchEngine(index) => match SEARCH_ENGINE_PRESETS.get(index) {
                Some(preset) => {
                    self.apply_change(|c| {
                        c.search.engine_url_template = preset.url_template.to_string();
                    });
                }
                None => log::warn!("未知的搜索引擎序号：{index}"),
            },
            MenuAction::SetExpiry(expiry) => {
                self.apply_change(|c| c.upload.litterbox.expiry = expiry);
            }
            MenuAction::ToggleAutostart => self.toggle_autostart(),
            MenuAction::OpenConfigDir => {
                let dir = self.paths.config_dir.clone();
                self.spawn_task("打开配置文件夹", move || {
                    shell::open_folder(&dir).err().map(|e| e.to_string())
                });
            }
            MenuAction::OpenSettings => self.open_settings(),
            MenuAction::ReloadConfig => self.reload_config(),
            MenuAction::Display => {}
        }
        LoopControl::Continue
    }

    /// 修改配置并交给 `ConfigStore`；被拒绝时提示原因。
    fn apply_change(&self, change: impl FnOnce(&mut Config)) {
        if let Err(e) = self.shared.store.update(change) {
            log::warn!("配置未生效：{e}");
            self.balloon(NotifyLevel::Warning, "lenswin", &format!("配置未生效：{e}"));
        }
    }

    /// 以 `--settings` 启动自身；设置窗口已打开时新进程会激活它后退出。
    fn open_settings(&self) {
        let Some(exe) = &self.exe else {
            self.balloon(
                NotifyLevel::Error,
                "lenswin",
                "无法确定程序路径，不能打开设置",
            );
            return;
        };
        if let Err(e) = std::process::Command::new(exe).arg(SETTINGS_ARG).spawn() {
            log::error!("启动设置进程失败：{e}");
            self.balloon(NotifyLevel::Error, "lenswin", &format!("无法打开设置：{e}"));
        }
    }

    fn toggle_autostart(&self) {
        let Some(exe) = &self.exe else {
            self.balloon(
                NotifyLevel::Error,
                "lenswin",
                "无法确定程序路径，不能设置开机自启",
            );
            return;
        };
        let result = match self.autostart_state() {
            AutostartState::Enabled => autostart::disable(AUTOSTART_NAME),
            AutostartState::Disabled | AutostartState::StalePath(_) => {
                autostart::enable(AUTOSTART_NAME, exe)
            }
        };
        if let Err(e) = result {
            log::error!("切换开机自启失败：{e}");
            self.balloon(
                NotifyLevel::Error,
                "lenswin",
                &format!("开机自启设置失败：{e}"),
            );
        }
    }

    /// 重新读取配置文件；热键变化时重注册，冲突则回滚旧热键。
    fn reload_config(&self) {
        let reloaded = match self.shared.store.reload() {
            Ok(reloaded) => reloaded,
            Err(e) => {
                log::warn!("重新加载配置失败：{e}");
                self.balloon(
                    NotifyLevel::Warning,
                    "lenswin",
                    &format!("配置未重新加载：{e}"),
                );
                return;
            }
        };
        let (report, effective) = {
            let mut set = self.shared.hotkeys.borrow_mut();
            let report = set.replace(self.shared.window, &reloaded.current.hotkeys);
            (report, set.effective().clone())
        };
        // 退回只发生在内存里，不写文件：用户手写的配置保持原样，下次重新加载会再次尝试
        match hotkeys::reload_summary(&report, &effective, self.shared.store.path()) {
            None => self.balloon(NotifyLevel::Info, "lenswin", "配置已重新加载"),
            Some(text) => self.balloon(NotifyLevel::Warning, "lenswin", &text),
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
        let mode = self.shared.store.snapshot().search.selection_mode;
        let Some(image) = self.pick_image(Command::Search, mode) else {
            return;
        };
        if self.ensure_upload_acknowledged() {
            self.balloon(NotifyLevel::Info, "lenswin", "正在上传…");
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
            .shared
            .store
            .snapshot()
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
            config: self.shared.store.snapshot(),
        }
    }

    /// 首次向某个图床上传前弹确认；确认后记入配置。
    fn ensure_upload_acknowledged(&self) -> bool {
        let config = self.shared.store.snapshot();
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
        let heading = format!("截图将上传到 {}", summary.host);
        let body = format!(
            "{}\n\n任何拿到链接的人都能看到这张图片。",
            summary.retention
        );
        match dialog::ask("lenswin 隐私提示", &heading, &body, "继续上传", "取消") {
            Ok(true) => {
                self.apply_change(|c| c.privacy.acknowledged_host = summary.host);
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

    /// 在工作线程执行动作，结果经任务通知回到主线程。
    fn spawn(&self, kind: ActionKind, label: &'static str, image: CroppedImage) {
        let Some(action) = self.registry.get(kind) else {
            log::error!("动作 {kind:?} 未注册");
            return;
        };
        let ctx = self.context();
        self.spawn_task(label, move || {
            action
                .run(&ctx, ActionInput::Image(image))
                .err()
                .map(|e| e.to_string())
        });
    }

    /// 在工作线程运行任务，返回的错误文字经任务通知回到主线程提示。
    fn spawn_task(
        &self,
        label: &'static str,
        job: impl FnOnce() -> Option<String> + Send + 'static,
    ) {
        let tx = self.results_tx.clone();
        let notifier = self.notifier;
        let spawned = std::thread::Builder::new()
            .name(format!("lenswin-{label}"))
            .spawn(move || {
                let error = job();
                if tx.send(TaskResult { label, error }).is_err() {
                    // 接收端随主线程销毁，只会发生在退出阶段，没有人再需要结果
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
        for message in self.shared.store.drain_failures() {
            self.balloon(
                NotifyLevel::Warning,
                "lenswin",
                &format!("配置未能保存，重启后会丢失：{message}"),
            );
        }
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
        let lasso = self.shared.store.snapshot().selection.lasso;
        let request = SelectRequest {
            frames: &frames,
            monitors: &monitors,
            mode,
            lasso: &lasso,
        };
        let outcome = lenswin_overlay::select(&request);
        // 覆盖层期间积压的热键和托盘左键不应在结束后再次触发
        discard_pending_triggers();
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
        // 关闭通知后只静音提示类，警告和错误仍要让用户看到
        if level == NotifyLevel::Info && !self.shared.store.snapshot().general.notifications {
            log::info!("通知已关闭，略过：{text}");
            return;
        }
        if let Err(e) = notify::balloon(self.tray, level, title, text) {
            log::error!("显示通知失败：{e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_click_maps_to_matching_command() {
        assert_eq!(
            left_click_command(TrayLeftClick::Search),
            Some(Command::Search)
        );
        assert_eq!(
            left_click_command(TrayLeftClick::ScreenshotCopy),
            Some(Command::ScreenshotCopy)
        );
        assert_eq!(left_click_command(TrayLeftClick::Nothing), None);
    }

    #[test]
    fn hotkey_ids_roundtrip() {
        assert_eq!(Command::from_hotkey_id(1), Some(Command::Search));
        assert_eq!(Command::from_hotkey_id(2), Some(Command::ScreenshotCopy));
        assert_eq!(Command::from_hotkey_id(3), None);
        assert_eq!(Command::About.hotkey_id(), None);
    }
}
