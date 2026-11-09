//! 屏幕捕获抽象。

pub mod gdi;

use lenswin_core::error::FrameError;
use lenswin_core::frame::Frame;
use lenswin_core::monitor::MonitorInfo;
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
    /// 按调用方给定的显示器列表抓取，每个显示器一帧，顺序与列表一致。
    ///
    /// 列表由调用方枚举一次后传入，保证帧与显示器来自同一份拓扑。
    fn capture_all(&self, monitors: &[MonitorInfo]) -> Result<Vec<Frame>, CaptureError>;
}
