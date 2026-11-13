//! 热键、托盘事件到命令的路由。

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use lenswin_actions::{
    ActionRegistry, check_update, default_file_name, prepare_lens_request, save_to,
};
use lenswin_core::action::{ActionContext, ActionInput, ActionKind};
use lenswin_core::config::{Config, SEARCH_ENGINE_PRESETS, SearchBackend, TrayLeftClick};
use lenswin_core::frame::{CroppedImage, Frame};
use lenswin_core::geometry::Rect;
use lenswin_core::ipc::SETTINGS_ARG;
use lenswin_core::lens::LensRequest;
use lenswin_core::monitor::MonitorInfo;
use lenswin_core::selection::{Selection, SelectionMode, SelectionShape};
use lenswin_core::snip::{LiveEvent, OverlayFlow, OverlayResult, ToolbarAction, push_history};
use lenswin_core::update::is_release_page_url;
use lenswin_overlay::{LiveSession, SelectRequest};
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
use lenswin_platform::window_snapshot;
use lenswin_popup::{PopupEvent, PopupHost};
use lenswin_upload::privacy_summary;

use crate::hotkeys::{self, HotkeySet};
use crate::shared::AppShared;
use crate::tray_menu::{self, MenuAction};
use crate::update_check::{
    AUTO_CHECK_DELAY, FinishGuard, Prompt, UpdateMessage, UpdateTrigger, admit, prompt_for,
};

/// 开机自启在注册表里的值名。
const AUTOSTART_NAME: &str = autostart::VALUE_NAME;
/// 微软官方的 WebView2 Runtime 下载页。
const WEBVIEW2_DOWNLOAD_URL: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";

/// 内置搜索弹窗及其回投的事件；弹窗初始化失败时 `host` 为空，搜索改走图床。
pub struct PopupLink<'a> {
    /// 弹窗。
    pub host: Option<&'a PopupHost>,
    /// 弹窗事件，由弹窗回调发送、主线程取出。
    pub events: Receiver<PopupEvent>,
}

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
    /// 检查更新。
    CheckUpdate,
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
            Command::CheckUpdate | Command::About | Command::Exit => None,
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
            Command::CheckUpdate => "检查更新",
            Command::About => "关于 lenswin",
            Command::Exit => "退出",
        }
    }
}

/// 工作线程准备好的直传请求，或失败原因。
type LensResult = Result<LensRequest, String>;

/// 常驻选框事件，附带会话序号，用来丢弃已结束会话残留在队列里的事件。
type LiveMessage = (u64, LiveEvent);

/// 冻结时刻的屏幕、窗口快照与历史选区。
struct Frozen {
    monitors: Vec<MonitorInfo>,
    frames: Vec<Frame>,
    windows: Vec<Rect>,
    history: Vec<Rect>,
}

/// 正在进行的常驻选框搜索；冻结帧随会话保存，每次调整都从中裁剪。
struct LiveState {
    id: u64,
    _session: LiveSession,
    frames: Vec<Frame>,
    /// 最近一次搜索的选框，会话结束时记入历史。
    last: Option<Rect>,
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
    popup: PopupLink<'a>,
    lens_tx: Sender<(u64, LensResult)>,
    lens_rx: Receiver<(u64, LensResult)>,
    /// 最近一次内置弹窗搜索的序号。
    lens_seq: Cell<u64>,
    /// 本次运行内的历史选区，最新的在前，不落盘。
    history: RefCell<Vec<Rect>>,
    update_tx: Sender<UpdateMessage>,
    update_rx: Receiver<UpdateMessage>,
    /// 进行中检查的提示方式，`None` 表示空闲；同一时间只跑一个检查。
    update_running: Cell<Option<UpdateTrigger>>,
    live: RefCell<Option<LiveState>>,
    /// 最近一次常驻选框会话的序号。
    live_seq: Cell<u64>,
    live_tx: Sender<LiveMessage>,
    live_rx: Receiver<LiveMessage>,
}

impl<'a> Dispatcher<'a> {
    /// 构造分发器；热键需随后调用 [`Dispatcher::install_hotkeys`] 注册。
    pub fn new(
        tray: &'a TrayIcon,
        paths: &'a AppPaths,
        shared: Rc<AppShared>,
        registry: ActionRegistry,
        notifier: TaskNotifier,
        popup: PopupLink<'a>,
    ) -> Self {
        let (results_tx, results_rx) = channel();
        let (lens_tx, lens_rx) = channel();
        let (update_tx, update_rx) = channel();
        let (live_tx, live_rx) = channel();
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
            popup,
            lens_tx,
            lens_rx,
            lens_seq: Cell::new(0),
            history: RefCell::new(Vec::new()),
            update_tx,
            update_rx,
            update_running: Cell::new(None),
            live: RefCell::new(None),
            live_seq: Cell::new(0),
            live_tx,
            live_rx,
        }
    }

    /// 延迟一段时间后做一次自动检查；到点时再确认开关，期间被关掉就不发请求。
    pub fn schedule_update_check(&self) {
        if !self.shared.store.snapshot().update.auto_check {
            log::info!("自动检查更新已关闭");
            return;
        }
        let tx = self.update_tx.clone();
        let notifier = self.notifier;
        let spawned = std::thread::Builder::new()
            .name("lenswin-update-timer".to_string())
            .spawn(move || {
                std::thread::sleep(AUTO_CHECK_DELAY);
                if tx.send(UpdateMessage::AutoDue).is_err() {
                    // 主线程已退出，无需检查
                    return;
                }
                if let Err(e) = notifier.notify() {
                    log::error!("投递自动检查更新失败：{e}");
                }
            });
        if let Err(e) = spawned {
            log::error!("创建自动检查更新线程失败：{e}");
        }
    }

    /// 在工作线程检查更新；已有检查在进行时不再新开，手动请求会让这次结果照常提示。
    fn start_update_check(&self, trigger: UpdateTrigger) {
        let admission = admit(self.update_running.get(), trigger);
        self.update_running.set(admission.running);
        if !admission.spawn {
            log::info!("检查更新正在进行，本次结果按 {trigger:?} 提示");
            return;
        }
        let include_prerelease = self.shared.store.snapshot().update.include_prerelease;
        let guard = FinishGuard::new(self.update_tx.clone(), self.notifier);
        let spawned = std::thread::Builder::new()
            .name("lenswin-update".to_string())
            .spawn(move || {
                let result = check_update(include_prerelease).map_err(|e| e.to_string());
                guard.finish(result);
            });
        if let Err(e) = spawned {
            log::error!("创建检查更新线程失败：{e}");
            let running = self.update_running.take();
            if running == Some(UpdateTrigger::Manual) {
                self.show_update_prompt(Prompt::Failed(format!("无法创建检查线程：{e}")));
            }
        }
    }

    fn on_update_message(&self, message: UpdateMessage) {
        match message {
            UpdateMessage::AutoDue => {
                if self.shared.store.snapshot().update.auto_check {
                    self.start_update_check(UpdateTrigger::Auto);
                } else {
                    log::info!("自动检查更新已在等待期间关闭");
                }
            }
            UpdateMessage::Finished(result) => {
                // 线程创建失败时守卫也会补发一条，那时状态已复位，直接丢弃
                let Some(trigger) = self.update_running.take() else {
                    log::debug!("丢弃无对应检查的更新结果");
                    return;
                };
                match &result {
                    Ok(check) => log::info!("检查更新完成（{trigger:?}）：{check:?}"),
                    Err(message) => log::warn!("检查更新失败（{trigger:?}）：{message}"),
                }
                if let Some(prompt) = prompt_for(trigger, &result) {
                    self.show_update_prompt(prompt);
                }
            }
            UpdateMessage::OpenFailed(message) => {
                log::error!("打开更新页面失败：{message}");
                if let Err(e) = dialog::error("lenswin", &format!("无法打开下载页：{message}"))
                {
                    log::error!("显示错误框失败：{e}");
                }
            }
        }
    }

    // 气泡在部分系统上会被勿扰模式拦下，检查更新的结果一律用对话框
    fn show_update_prompt(&self, prompt: Prompt) {
        let current = env!("CARGO_PKG_VERSION");
        let shown = match prompt {
            Prompt::Upgrade {
                version,
                page_url: Some(url),
            } => dialog::ask(
                "lenswin",
                "发现新版本",
                &format!("lenswin {version} 已发布，当前版本 {current}"),
                "前往下载",
                "稍后",
            )
            .map(|go| {
                if go {
                    self.open_release_page(url);
                }
            }),
            Prompt::Upgrade {
                version,
                page_url: None,
            } => dialog::info(
                "lenswin",
                &format!("lenswin {version} 已发布，当前版本 {current}"),
            ),
            Prompt::UpToDate => dialog::info("lenswin", &format!("已是最新版本 {current}")),
            Prompt::Failed(message) => {
                dialog::error("lenswin", &format!("检查更新失败：{message}"))
            }
        };
        if let Err(e) = shown {
            log::error!("显示检查更新结果失败：{e}");
        }
        // 对话框模态期间按下的热键已积压，作答后不能连串触发
        discard_pending_triggers();
    }

    /// 后台打开 Release 页；失败用对话框提示，气泡可能被系统拦下。
    fn open_release_page(&self, url: String) {
        if !is_release_page_url(&url) {
            log::warn!("拒绝打开不受信任的地址");
            return;
        }
        let tx = self.update_tx.clone();
        let notifier = self.notifier;
        let spawned = std::thread::Builder::new()
            .name("lenswin-open-release".to_string())
            .spawn(move || {
                let Err(e) = shell::open_url(&url) else {
                    return;
                };
                if tx.send(UpdateMessage::OpenFailed(e.to_string())).is_err() {
                    // 主线程已退出，无人提示
                    return;
                }
                if let Err(e) = notifier.notify() {
                    log::error!("投递打开失败结果失败：{e}");
                }
            });
        if let Err(e) = spawned {
            log::error!("创建打开下载页线程失败：{e}");
            if let Err(e) = dialog::error("lenswin", &format!("无法打开下载页：{e}")) {
                log::error!("显示错误框失败：{e}");
            }
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
        self.end_live();
        match action {
            MenuAction::Run(command) => return self.run(command),
            MenuAction::SetSelectionMode(mode) => {
                self.apply_change(|c| c.search.selection_mode = mode);
            }
            MenuAction::SetBackend(backend) => {
                self.apply_change(|c| c.search.backend = backend);
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
        self.end_live();
        match command {
            Command::Search => {
                self.search();
                LoopControl::Continue
            }
            Command::ScreenshotCopy => {
                self.screenshot_copy();
                LoopControl::Continue
            }
            Command::CheckUpdate => {
                self.start_update_check(UpdateTrigger::Manual);
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

    /// 圈选 → 按搜索方式交给内置弹窗或图床；内置弹窗可用时保留选框反复搜索。
    fn search(&self) {
        let config = self.shared.store.snapshot();
        let mode = config.search.selection_mode;
        if config.search.backend == SearchBackend::Embedded && self.embedded_available() {
            self.live_search(mode);
            return;
        }
        let Some((result, frames)) = self.pick(Command::Search, mode, OverlayFlow::Direct) else {
            return;
        };
        match result {
            OverlayResult::Selected(selection) | OverlayResult::Action(selection, _) => {
                if let Some(image) = self.crop(&selection, &frames) {
                    self.search_image(image, &selection.bounds());
                }
            }
            OverlayResult::Color(text) => self.copy_color(&text),
        }
    }

    /// 框选并调整 → 工具栏选择复制、保存或搜索；也可以直接取色。
    fn screenshot_copy(&self) {
        let Some((result, frames)) = self.pick(
            Command::ScreenshotCopy,
            SelectionMode::Rect,
            OverlayFlow::Toolbar,
        ) else {
            return;
        };
        let (selection, action) = match result {
            OverlayResult::Action(selection, action) => (selection, action),
            OverlayResult::Selected(selection) => (selection, ToolbarAction::Copy),
            OverlayResult::Color(text) => {
                self.copy_color(&text);
                return;
            }
        };
        let Some(image) = self.crop(&selection, &frames) else {
            return;
        };
        match action {
            ToolbarAction::Copy => self.copy_image(image),
            ToolbarAction::Save => self.save_image(image),
            ToolbarAction::Search => self.search_image(image, &selection.bounds()),
        }
    }

    fn search_image(&self, image: CroppedImage, anchor: &Rect) {
        match self.shared.store.snapshot().search.backend {
            SearchBackend::Embedded => self.embedded_search(image, anchor),
            SearchBackend::ImageHost => self.upload_and_search(image),
        }
    }

    /// 内置弹窗已初始化且装有 WebView2 Runtime。
    fn embedded_available(&self) -> bool {
        self.popup.host.is_some() && PopupHost::runtime_version().is_some()
    }

    /// 常驻选框搜索：先确认隐私再冻结，全屏覆盖层会挡住确认框。
    fn live_search(&self, mode: SelectionMode) {
        log::info!("触发命令：{}（常驻选框）", Command::Search.label());
        if !self.ensure_lens_acknowledged() {
            log::info!("内置弹窗搜索未获确认，不冻结屏幕");
            return;
        }
        // 收起上一次的弹窗，免得被冻结进截图
        if let Some(popup) = self.popup.host {
            popup.hide();
        }
        let Some(frozen) = self.freeze(mode) else {
            return;
        };
        let config = self.shared.store.snapshot();
        let request = SelectRequest {
            frames: &frozen.frames,
            monitors: &frozen.monitors,
            mode,
            flow: OverlayFlow::Live,
            lasso: &config.selection.lasso,
            screenshot: &config.screenshot,
            search: &config.search,
            windows: &frozen.windows,
            history: &frozen.history,
        };
        let id = self.live_seq.get().wrapping_add(1);
        self.live_seq.set(id);
        let tx = self.live_tx.clone();
        let notifier = self.notifier;
        // 回调在覆盖层窗口过程里执行，只入队，由 drain_results 统一处理，避免重入
        let sink = move |event: LiveEvent| {
            if tx.send((id, event)).is_err() {
                // 接收端随分发器销毁，只会发生在退出阶段
                return;
            }
            if let Err(e) = notifier.notify() {
                log::error!("投递选框事件失败：{e}");
            }
        };
        match LiveSession::begin(&request, sink) {
            Ok(session) => {
                self.live.replace(Some(LiveState {
                    id,
                    _session: session,
                    frames: frozen.frames,
                    last: None,
                }));
            }
            Err(e) => {
                log::error!("选区失败：{e}");
                self.balloon(NotifyLevel::Error, "lenswin", &format!("选区失败：{e}"));
            }
        }
    }

    fn on_live_message(&self, (id, event): LiveMessage) {
        let current = self.live.borrow().as_ref().map(|state| state.id);
        if current != Some(id) {
            log::debug!("丢弃已结束会话 #{id} 的选框事件");
            return;
        }
        match event {
            LiveEvent::Committed(selection) => self.on_live_commit(&selection),
            LiveEvent::Closed => {
                log::info!("常驻选框已退出");
                self.end_live();
            }
            LiveEvent::Failed(message) => {
                log::error!("选区失败：{message}");
                self.end_live();
                self.balloon(
                    NotifyLevel::Error,
                    "lenswin",
                    &format!("选区失败：{message}"),
                );
            }
        }
    }

    /// 选框落定或调整后：裁剪冻结帧，弹窗跟到选框旁重新搜索。
    fn on_live_commit(&self, selection: &Selection) {
        let bounds = selection.bounds();
        log::info!(
            "选框落定：范围 {bounds:?}，显示器 {}",
            selection.monitor_index()
        );
        let image = {
            let mut live = self.live.borrow_mut();
            let Some(state) = live.as_mut() else {
                return;
            };
            state.last = Some(bounds);
            let Some(frame) = state.frames.get(selection.monitor_index()) else {
                log::error!(
                    "选区所在显示器序号 {} 没有对应截屏",
                    selection.monitor_index()
                );
                return;
            };
            frame.crop(&bounds)
        };
        match image {
            Ok(image) => {
                if !self.show_and_submit(image, &bounds) {
                    self.end_live();
                }
            }
            Err(e) => {
                log::error!("裁剪失败：{e}");
                self.balloon(NotifyLevel::Error, "lenswin", &format!("裁剪失败：{e}"));
            }
        }
    }

    /// 结束常驻选框：销毁覆盖层、收起弹窗，最后一个选框记入历史；没有会话时什么也不做。
    fn end_live(&self) {
        let Some(state) = self.live.borrow_mut().take() else {
            return;
        };
        if let Some(last) = state.last {
            let capacity = self.shared.store.snapshot().screenshot.history_size;
            let capacity = usize::try_from(capacity).unwrap_or(0);
            push_history(&mut self.history.borrow_mut(), last, capacity);
        }
        drop(state);
        if let Some(popup) = self.popup.host {
            popup.hide();
        }
    }

    /// 弹出内置弹窗，工作线程准备好请求后由弹窗直接提交给 Lens。
    fn embedded_search(&self, image: CroppedImage, anchor: &Rect) {
        let Some(popup) = self.popup.host else {
            self.fallback_to_image_host(image, "内置弹窗不可用");
            return;
        };
        if PopupHost::runtime_version().is_none() {
            log::warn!("未检测到 WebView2 Runtime，下载地址：{WEBVIEW2_DOWNLOAD_URL}");
            let reason = format!("未安装 WebView2 Runtime（下载：{WEBVIEW2_DOWNLOAD_URL}）");
            self.fallback_to_image_host(image, &reason);
            return;
        }
        if !self.ensure_lens_acknowledged() {
            log::info!("内置弹窗搜索未获确认，已丢弃截图");
            return;
        }
        if !self.popup_shown(popup, anchor) {
            return;
        }
        self.prepare_lens(image);
    }

    /// 在选框旁显示弹窗并准备请求；弹窗不可用或显示失败返回 `false`。
    fn show_and_submit(&self, image: CroppedImage, anchor: &Rect) -> bool {
        let Some(popup) = self.popup.host else {
            return false;
        };
        if !self.popup_shown(popup, anchor) {
            return false;
        }
        self.prepare_lens(image);
        true
    }

    /// 显示弹窗；失败时写日志并提示，返回 `false`。
    fn popup_shown(&self, popup: &PopupHost, anchor: &Rect) -> bool {
        match popup.show(anchor) {
            Ok(()) => true,
            Err(e) => {
                log::error!("显示搜索弹窗失败：{e}");
                self.balloon(
                    NotifyLevel::Error,
                    "lenswin",
                    &format!("无法显示搜索弹窗：{e}"),
                );
                false
            }
        }
    }

    /// 工作线程缩小、编码并组装直传请求，结果按序号回投，过期的丢弃。
    fn prepare_lens(&self, image: CroppedImage) {
        let config = self.shared.store.snapshot();
        let tx = self.lens_tx.clone();
        let seq = self.lens_seq.get().wrapping_add(1);
        self.lens_seq.set(seq);
        self.spawn_task("搜索", move || {
            let prepared = prepare_lens_request(&config, image).map_err(|e| e.to_string());
            let error = prepared.as_ref().err().cloned();
            if tx.send((seq, prepared)).is_err() {
                // 接收端随主线程销毁，只会发生在退出阶段
                return None;
            }
            error
        });
    }

    /// 内置弹窗不可用时改走图床；图床未确认过隐私则只提示，不再追加确认框。
    fn fallback_to_image_host(&self, image: CroppedImage, reason: &str) {
        let config = self.shared.store.snapshot();
        let acknowledged = privacy_summary(&config.upload)
            .map(|summary| config.privacy.is_acknowledged(&summary.host))
            .unwrap_or(false);
        if acknowledged {
            self.balloon(
                NotifyLevel::Warning,
                "lenswin",
                &format!("{reason}，本次改用图床方式"),
            );
            self.spawn(ActionKind::Search, "搜索", image);
        } else {
            self.balloon(
                NotifyLevel::Warning,
                "lenswin",
                &format!("{reason}，可在设置里改用图床方式"),
            );
        }
    }

    /// 首次使用内置弹窗前弹确认；确认后记入配置。
    fn ensure_lens_acknowledged(&self) -> bool {
        if self.shared.store.snapshot().privacy.lens_acknowledged {
            return true;
        }
        let body = "截图会直接提交到 lens.google.com，受 Google 隐私政策约束。\n\n\
                    弹窗会在本机保存 Google 的 Cookie，用于保持登录和语言等设置。";
        match dialog::ask(
            "lenswin 隐私提示",
            "截图将直接发送给 Google",
            body,
            "继续",
            "取消",
        ) {
            Ok(true) => {
                self.apply_change(|c| c.privacy.lens_acknowledged = true);
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

    fn on_popup_event(&self, event: PopupEvent) {
        match event {
            PopupEvent::OpenExternal(url) => {
                self.spawn_task("打开链接", move || {
                    shell::open_url(&url).err().map(|e| e.to_string())
                });
            }
            PopupEvent::CopyLink(url) => match self.registry.copy_text(&url) {
                Ok(()) => self.balloon(NotifyLevel::Info, "lenswin", "链接已复制"),
                Err(e) => {
                    log::error!("复制链接失败：{e}");
                    self.balloon(NotifyLevel::Error, "lenswin", &format!("复制链接失败：{e}"));
                }
            },
            PopupEvent::LoadFailed(message) => {
                self.balloon(
                    NotifyLevel::Error,
                    "lenswin",
                    &format!("内置弹窗：{message}"),
                );
            }
            PopupEvent::Closed => {
                if self.live.borrow().is_some() {
                    log::info!("弹窗已关闭，结束常驻选框");
                }
                self.end_live();
            }
        }
    }

    fn upload_and_search(&self, image: CroppedImage) {
        if self.ensure_upload_acknowledged() {
            self.balloon(NotifyLevel::Info, "lenswin", "正在上传…");
            self.spawn(ActionKind::Search, "搜索", image);
        } else {
            log::info!("上传未获确认，已丢弃截图");
        }
    }

    /// 复制到剪贴板；配置了保存目录时再到工作线程保存文件。
    fn copy_image(&self, image: CroppedImage) {
        let spare = self.has_save_dir().then(|| image.clone());
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

    /// 配置了保存目录时直接存入；否则弹另存为对话框，在工作线程写文件。
    fn save_image(&self, image: CroppedImage) {
        if self.has_save_dir() {
            self.spawn(ActionKind::Save, "保存", image);
            return;
        }
        let path = match dialog::save_png_path(&default_file_name()) {
            Ok(Some(path)) => path,
            Ok(None) => {
                log::info!("另存为已取消");
                return;
            }
            Err(e) => {
                log::error!("显示另存为对话框失败：{e}");
                self.balloon(
                    NotifyLevel::Error,
                    "lenswin",
                    &format!("无法显示另存为：{e}"),
                );
                return;
            }
        };
        self.spawn_task("保存", move || {
            save_to(&path, &image).err().map(|e| e.to_string())
        });
    }

    fn copy_color(&self, text: &str) {
        match self.registry.copy_text(text) {
            Ok(()) => log::info!("颜色值已复制：{text}"),
            Err(e) => {
                log::error!("复制颜色失败：{e}");
                self.balloon(NotifyLevel::Error, "lenswin", &format!("复制颜色失败：{e}"));
            }
        }
    }

    fn has_save_dir(&self) -> bool {
        !self
            .shared
            .store
            .snapshot()
            .screenshot
            .save_dir
            .trim()
            .is_empty()
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
        while let Ok((seq, prepared)) = self.lens_rx.try_recv() {
            // 慢的旧请求晚到时不能覆盖新的搜索
            if seq != self.lens_seq.get() {
                log::debug!("丢弃过期的搜索请求 #{seq}");
                continue;
            }
            if let Some(popup) = self.popup.host {
                match prepared {
                    Ok(request) => popup.submit(request),
                    Err(message) => popup.show_error(&message),
                }
            }
        }
        while let Ok(message) = self.live_rx.try_recv() {
            self.on_live_message(message);
        }
        while let Ok(event) = self.popup.events.try_recv() {
            self.on_popup_event(event);
        }
        while let Ok(message) = self.update_rx.try_recv() {
            self.on_update_message(message);
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

    /// 裁出选区对应的图像；失败时提示并返回 `None`。
    fn crop(&self, selection: &Selection, frames: &[Frame]) -> Option<CroppedImage> {
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

    /// 冻结屏幕让用户选区；取消或失败返回 `None`。
    fn pick(
        &self,
        command: Command,
        mode: SelectionMode,
        flow: OverlayFlow,
    ) -> Option<(OverlayResult, Vec<Frame>)> {
        log::info!("触发命令：{}", command.label());
        // 搜索前收起上一次的弹窗，免得被冻结进截图；截屏流程保留，用户可能要截它
        if command == Command::Search
            && let Some(popup) = self.popup.host
        {
            popup.hide();
        }
        let frozen = self.freeze(mode)?;
        let config = self.shared.store.snapshot();
        let request = SelectRequest {
            frames: &frozen.frames,
            monitors: &frozen.monitors,
            mode,
            flow,
            lasso: &config.selection.lasso,
            screenshot: &config.screenshot,
            search: &config.search,
            windows: &frozen.windows,
            history: &frozen.history,
        };
        let outcome = lenswin_overlay::select(&request);
        // 覆盖层期间积压的热键和托盘左键不应在结束后再次触发
        discard_pending_triggers();
        match outcome {
            Ok(Some(result)) => {
                self.remember(&result, config.screenshot.history_size);
                Some((result, frozen.frames))
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

    /// 枚举显示器并截下所有屏幕，同时拍下窗口快照与历史选区；失败时提示并返回 `None`。
    fn freeze(&self, mode: SelectionMode) -> Option<Frozen> {
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
        let config = self.shared.store.snapshot();
        Some(Frozen {
            windows: self.window_snapshot(mode, config.screenshot.window_snap),
            history: self.history.borrow().clone(),
            monitors,
            frames,
        })
    }

    /// 矩形模式且开启吸附时拍下窗口快照；失败只是不能吸附。
    fn window_snapshot(&self, mode: SelectionMode, enabled: bool) -> Vec<Rect> {
        if mode != SelectionMode::Rect || !enabled {
            return Vec::new();
        }
        match window_snapshot::visible_windows() {
            Ok(list) => list,
            Err(e) => {
                log::warn!("获取窗口快照失败，本次不吸附窗口：{e}");
                Vec::new()
            }
        }
    }

    /// 把矩形选区记入历史。
    fn remember(&self, result: &OverlayResult, capacity: u32) {
        let selection = match result {
            OverlayResult::Selected(s) | OverlayResult::Action(s, _) => s,
            OverlayResult::Color(_) => return,
        };
        log::info!(
            "选区完成：范围 {:?}，显示器 {}",
            selection.bounds(),
            selection.monitor_index()
        );
        if matches!(selection.shape(), SelectionShape::Rect) {
            let capacity = usize::try_from(capacity).unwrap_or(0);
            push_history(&mut self.history.borrow_mut(), selection.bounds(), capacity);
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
        assert_eq!(Command::CheckUpdate.hotkey_id(), None);
    }
}
