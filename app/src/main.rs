//! lenswin 托盘程序入口：只做装配、路由和生命周期管理。

#![forbid(unsafe_code)]
#![windows_subsystem = "windows"]

mod config_file;
mod dispatcher;
mod logging;
mod tray_menu;

use anyhow::Context;
use lenswin_actions::ActionRegistry;
use lenswin_core::config::Config;
use lenswin_core::hotkey::Hotkey;
use lenswin_core::ipc::{TRAY_MUTEX_NAME, TRAY_WINDOW_CLASS};
use lenswin_platform::clipboard::ClipboardOwner;
use lenswin_platform::hotkey::{self, HotkeyGuard};
use lenswin_platform::notify::NotifyLevel;
use lenswin_platform::paths::{AppPaths, Mode};
use lenswin_platform::single_instance::{self, Acquired};
use lenswin_platform::tray::TrayIcon;
use lenswin_platform::window::MessageWindow;
use lenswin_platform::{dpi, message_loop};

use crate::dispatcher::{Command, Dispatcher};

fn main() {
    if let Err(err) = run() {
        // 日志可能尚未初始化，同时输出到日志和 stderr
        log::error!("程序异常退出：{err:#}");
        eprintln!("程序异常退出：{err:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    // 必须早于任何窗口创建，之后所有坐标才是物理像素
    let dpi_result = dpi::enable_per_monitor_v2();
    // 先抢互斥量再碰日志，避免第二个实例轮转或打开正被首个实例写入的日志
    let _instance =
        match single_instance::acquire(TRAY_MUTEX_NAME).context("创建单实例互斥量失败")? {
            Acquired::First(instance) => instance,
            Acquired::AlreadyRunning => {
                // 日志尚未初始化，通知失败也只能放弃
                match single_instance::notify_existing(TRAY_WINDOW_CLASS) {
                    Ok(()) | Err(_) => {}
                }
                return Ok(());
            }
        };

    let paths = AppPaths::detect().context("确定数据目录失败")?;
    logging::init(&paths.log_dir).context("初始化日志失败")?;
    log::info!(
        "lenswin {} 启动，模式：{}，配置目录：{}",
        env!("CARGO_PKG_VERSION"),
        paths.mode.label(),
        paths.config_dir.display()
    );
    if let Err(e) = dpi_result {
        log::debug!("设置 DPI 感知未生效（可能已由 manifest 声明）：{e}");
    }

    // 热更新与统一写入在 M5 由 ConfigStore 接管，此处只在启动时读取一次
    let loaded = config_file::load(&paths.config_file());
    let config = loaded.config.clone();
    let notice = loaded.notice.clone();
    let window = MessageWindow::create(TRAY_WINDOW_CLASS).context("创建消息窗口失败")?;
    let tray = TrayIcon::add(&window, "lenswin").context("添加托盘图标失败")?;
    let registry = ActionRegistry::new(ClipboardOwner::of(&window));
    let dispatcher = Dispatcher::new(&tray, &paths, loaded, registry, window.task_notifier());
    if let Some(text) = notice {
        log::warn!("{text}");
        dispatcher.balloon(NotifyLevel::Warning, "lenswin", &text);
    }

    let _hotkeys = register_hotkeys(&window, &config, &dispatcher);
    if paths.mode == Mode::PortableFallback {
        dispatcher.balloon(
            NotifyLevel::Warning,
            "lenswin",
            "便携目录不可写，本次改用安装版数据目录",
        );
    }

    message_loop::run(|event| dispatcher.handle(event)).context("消息循环异常")?;
    log::info!("lenswin 退出");
    Ok(())
}

/// 注册全部热键；单个失败只提示并跳过，不影响其它功能。
fn register_hotkeys(
    window: &MessageWindow,
    config: &Config,
    dispatcher: &Dispatcher<'_>,
) -> Vec<HotkeyGuard> {
    let entries = [
        (Command::Search, config.hotkeys.search.as_str()),
        (
            Command::ScreenshotCopy,
            config.hotkeys.screenshot_copy.as_str(),
        ),
    ];
    let mut guards = Vec::new();
    for (command, text) in entries {
        let Some(id) = command.hotkey_id() else {
            continue;
        };
        let registered = text
            .parse::<Hotkey>()
            .map_err(|e| e.to_string())
            .and_then(|h| hotkey::register(window, id, &h).map_err(|e| e.to_string()));
        match registered {
            Ok(guard) => {
                log::info!("已注册热键 {text}（{}）", command.label());
                guards.push(guard);
            }
            Err(reason) => {
                log::warn!("热键 {text}（{}）注册失败：{reason}", command.label());
                dispatcher.balloon(
                    NotifyLevel::Warning,
                    "lenswin",
                    &format!("{} 的快捷键 {text} 注册失败：{reason}", command.label()),
                );
            }
        }
    }
    guards
}
