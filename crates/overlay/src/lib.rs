//! 选区覆盖层：只返回选区，不触发上传或动作。
//!
//! 进程须声明 Per-Monitor DPI 感知，所有坐标均为虚拟桌面物理像素。

#![deny(unsafe_op_in_unsafe_fn)]

pub mod error;
pub mod input;
pub mod modes;

mod render;
mod window;

use lenswin_core::config::LassoConfig;
use lenswin_core::frame::Frame;
use lenswin_core::monitor::MonitorInfo;
use lenswin_core::selection::{Selection, SelectionMode};

pub use crate::error::OverlayError;
use crate::input::{Interaction, Outcome};
use crate::window::SessionInput;

/// 一次选区请求。
pub struct SelectRequest<'a> {
    /// 每个显示器的冻结帧，顺序与 `monitors` 一致。
    pub frames: &'a [Frame],
    /// 显示器列表。
    pub monitors: &'a [MonitorInfo],
    /// 选区模式。
    pub mode: SelectionMode,
    /// 套索参数。
    pub lasso: &'a LassoConfig,
}

/// 校验帧与显示器一一对应。
fn validate(request: &SelectRequest<'_>) -> Result<(), OverlayError> {
    if request.monitors.is_empty() || request.frames.is_empty() {
        return Err(OverlayError::InvalidInput(
            "没有可用的显示器画面".to_string(),
        ));
    }
    if request.frames.len() != request.monitors.len() {
        return Err(OverlayError::InvalidInput(
            "帧数量与显示器数量不一致".to_string(),
        ));
    }
    let aligned = request
        .frames
        .iter()
        .zip(request.monitors)
        .all(|(frame, monitor)| frame.region() == monitor.bounds);
    if !aligned {
        return Err(OverlayError::InvalidInput(
            "帧区域与显示器范围不一致".to_string(),
        ));
    }
    Ok(())
}

/// 冻结所有显示器并让用户选区。
///
/// 用户完成选区返回 `Some`，按 Esc 或点右键取消返回 `None`。
/// 必须在创建了消息循环的主线程调用；函数内部自带消息循环，返回前所有窗口已销毁。
pub fn select(request: &SelectRequest<'_>) -> Result<Option<Selection>, OverlayError> {
    validate(request)?;
    let tool = modes::tool_for(request.mode, request.lasso);
    let input = SessionInput {
        interaction: Interaction::new(tool, request.monitors.to_vec()),
        monitors: request.monitors.to_vec(),
        bgra_frames: request
            .frames
            .iter()
            .map(|f| render::rgba_to_bgra(f.rgba()))
            .collect(),
        stroke_width: request.lasso.stroke_width,
    };
    match window::run(input)? {
        Outcome::Selected(selection) => Ok(Some(selection)),
        Outcome::Cancelled => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lenswin_core::geometry::Rect;

    fn monitor(x: i32) -> MonitorInfo {
        MonitorInfo {
            bounds: Rect::new(x, 0, 2, 2),
            dpi: 96,
            primary: false,
        }
    }

    fn frame(x: i32) -> Frame {
        Frame::new(Rect::new(x, 0, 2, 2), vec![0; 16]).expect("构造帧")
    }

    fn request<'a>(
        frames: &'a [Frame],
        monitors: &'a [MonitorInfo],
        lasso: &'a LassoConfig,
    ) -> SelectRequest<'a> {
        SelectRequest {
            frames,
            monitors,
            mode: SelectionMode::Rect,
            lasso,
        }
    }

    #[test]
    fn rejects_empty_input() {
        let lasso = LassoConfig::default();
        assert!(validate(&request(&[], &[], &lasso)).is_err());
    }

    #[test]
    fn rejects_count_mismatch() {
        let lasso = LassoConfig::default();
        let frames = [frame(0)];
        let monitors = [monitor(0), monitor(2)];
        assert!(validate(&request(&frames, &monitors, &lasso)).is_err());
    }

    #[test]
    fn rejects_region_mismatch() {
        let lasso = LassoConfig::default();
        let frames = [frame(0), frame(5)];
        let monitors = [monitor(0), monitor(2)];
        assert!(validate(&request(&frames, &monitors, &lasso)).is_err());
    }

    #[test]
    fn accepts_aligned_input() {
        let lasso = LassoConfig::default();
        let frames = [frame(0), frame(2)];
        let monitors = [monitor(0), monitor(2)];
        assert!(validate(&request(&frames, &monitors, &lasso)).is_ok());
    }
}
