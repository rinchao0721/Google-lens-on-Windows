//! lenswin 托盘程序入口：只做装配、路由和生命周期管理。

#![forbid(unsafe_code)]
#![windows_subsystem = "windows"]

mod config_file;
mod config_store;
mod dispatcher;
mod hotkeys;
mod logging;
mod shared;
mod tray_menu;

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Context;
use lenswin_actions::ActionRegistry;
use lenswin_core::ipc::{SETTINGS_ARG, TRAY_MUTEX_NAME, TRAY_WINDOW_CLASS};
use lenswin_platform::clipboard::ClipboardOwner;
use lenswin_platform::dialog;
use lenswin_platform::message_loop;
use lenswin_platform::notify::NotifyLevel;
use lenswin_platform::paths::{AppPaths, Mode};
use lenswin_platform::single_instance::{self, Acquired};
use lenswin_platform::tray::TrayIcon;
use lenswin_platform::window::{MessageWindow, clear_copydata_handler, set_copydata_handler};

use crate::config_store::ConfigStore;
use crate::dispatcher::Dispatcher;
use crate::hotkeys::HotkeySet;
use crate::shared::AppShared;

fn main() {
    // 设置窗口在独立进程里运行，不碰托盘进程的日志、互斥量和配置写入
    if std::env::args_os().any(|arg| arg == SETTINGS_ARG) {
        if let Err(err) = lenswin_settings_ui::run() {
            // 该进程没有日志也没有控制台，用对话框报告
            match dialog::error("lenswin 设置", &err.to_string()) {
                Ok(()) | Err(_) => {}
            }
            std::process::exit(1);
        }
        return;
    }
    if let Err(err) = run() {
        // 日志可能尚未初始化，同时输出到日志和 stderr
        log::error!("程序异常退出：{err:#}");
        eprintln!("程序异常退出：{err:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    // DPI 感知（PerMonitorV2）由内嵌 manifest 声明，坐标均为物理像素
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

    let window = MessageWindow::create(TRAY_WINDOW_CLASS).context("创建消息窗口失败")?;
    let notifier = window.task_notifier();
    let (store, notice) = ConfigStore::open(paths.config_file(), move || {
        if let Err(e) = notifier.notify() {
            log::error!("通知主线程配置写盘结果失败：{e}");
        }
    })
    .map_err(anyhow::Error::msg)
    .context("启动配置中心失败")?;
    let tray = TrayIcon::add(&window, "lenswin").context("添加托盘图标失败")?;
    let registry = ActionRegistry::new(ClipboardOwner::of(&window));
    let shared = Rc::new(AppShared {
        store,
        hotkeys: RefCell::new(HotkeySet::default()),
        window: window.handle(),
    });
    let callback_shared = Rc::clone(&shared);
    set_copydata_handler(move |kind, payload| callback_shared.on_message(kind, payload));
    let dispatcher = Dispatcher::new(&tray, &paths, shared, registry, window.task_notifier());
    if let Some(text) = notice {
        log::warn!("{text}");
        dispatcher.balloon(NotifyLevel::Warning, "lenswin", &text);
    }

    dispatcher.install_hotkeys();
    dispatcher.check_autostart();
    if paths.mode == Mode::PortableFallback {
        dispatcher.balloon(
            NotifyLevel::Warning,
            "lenswin",
            "便携目录不可写，本次改用安装版数据目录",
        );
    }

    let loop_result = message_loop::run(|event| dispatcher.handle(event));
    // 回调持有配置中心的引用，必须先撤销，退出时才会等待排队中的写盘完成
    clear_copydata_handler();
    loop_result.context("消息循环异常")?;
    log::info!("lenswin 退出");
    Ok(())
}
