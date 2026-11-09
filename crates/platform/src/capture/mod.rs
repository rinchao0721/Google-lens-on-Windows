//! 屏幕捕获抽象。

pub mod gdi;

use lenswin_core::error::FrameError;
use lenswin_core::frame::Frame;
use thiserror::Error;

use crate::error::PlatformError;

/// 捕获错误。
#[derive(Debug, Error)]
pub enum CaptureError {
    /// 系统调用失败。
    #[error(transparent)]
    Platform(#[from] PlatformError),
    /// 像素数据不合法。
    #[error("捕获结果不合法：{0}")]
    Frame(#[from] FrameError),
}

/// 屏幕捕获后端，MVP 用 GDI，录屏阶段再加 WGC。
pub trait ScreenCapturer {
    /// 一次性抓取所有显示器，每个显示器一帧，顺序与 [`crate::monitor::enumerate`] 一致。
    fn capture_all(&self) -> Result<Vec<Frame>, CaptureError>;
}
