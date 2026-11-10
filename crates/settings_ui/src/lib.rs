//! 设置窗口：只编辑并提交 Config。
//!
//! 在独立进程里运行；只读配置文件，修改通过 `WM_COPYDATA` 交给托盘进程，自己从不写配置。

#![forbid(unsafe_code)]

mod app;
mod client;
mod fonts;
mod form;
mod icons;
mod recorder;
mod theme;
mod widgets;

use eframe::egui;
use lenswin_core::ipc::{SETTINGS_MUTEX_NAME, SETTINGS_WINDOW_TITLE};
use lenswin_platform::error::PlatformError;
use lenswin_platform::focus::activate_window_titled;
use lenswin_platform::single_instance::{self, Acquired};
use thiserror::Error;

use crate::app::{SettingsApp, Startup};

/// 设置窗口启动失败的原因。
#[derive(Debug, Error)]
pub enum SettingsError {
    /// 系统调用失败。
    #[error("系统调用失败：{0}")]
    Platform(#[from] PlatformError),
    /// 图形窗口无法创建。
    #[error("设置窗口启动失败：{0}")]
    Window(String),
}

/// 窗口初始逻辑尺寸。
const WINDOW_SIZE: [f32; 2] = [820.0, 600.0];
/// 窗口图标：64x64 的 RGBA 原始像素，由 `assets/icon-64.rgba` 嵌入。
const WINDOW_ICON_RGBA: &[u8] = include_bytes!("../assets/icon-64.rgba");
/// 窗口图标边长。
const WINDOW_ICON_SIDE: u32 = 64;
/// 窗口最小逻辑尺寸。
const WINDOW_MIN_SIZE: [f32; 2] = [640.0, 460.0];

/// 运行设置窗口，直到用户关闭；已有设置窗口时激活它后直接返回。
pub fn run() -> Result<(), SettingsError> {
    let _instance = match single_instance::acquire(SETTINGS_MUTEX_NAME)? {
        Acquired::First(instance) => instance,
        Acquired::AlreadyRunning => {
            // 找不到窗口（如对方正在启动）时无事可做，直接退出
            activate_window_titled(SETTINGS_WINDOW_TITLE);
            return Ok(());
        }
    };
    let startup = Startup::collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(SETTINGS_WINDOW_TITLE)
            .with_icon(egui::IconData {
                rgba: WINDOW_ICON_RGBA.to_vec(),
                width: WINDOW_ICON_SIDE,
                height: WINDOW_ICON_SIDE,
            })
            .with_inner_size(WINDOW_SIZE)
            .with_min_inner_size(WINDOW_MIN_SIZE),
        ..Default::default()
    };
    eframe::run_native(
        SETTINGS_WINDOW_TITLE,
        options,
        Box::new(move |cc| {
            let fonts = fonts::install_fonts(&cc.egui_ctx);
            let kind = theme::resolve_system(cc.egui_ctx.system_theme());
            theme::install(&cc.egui_ctx, kind);
            Ok(Box::new(SettingsApp::new(startup, fonts, kind)))
        }),
    )
    .map_err(|e| SettingsError::Window(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_icon_has_exact_rgba_length() {
        let expected = usize::try_from(WINDOW_ICON_SIDE * WINDOW_ICON_SIDE * 4).expect("尺寸");
        assert_eq!(WINDOW_ICON_RGBA.len(), expected);
    }
}
