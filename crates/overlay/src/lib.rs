//! 选区覆盖层：只返回选区和用户选择的动作，不触发上传或动作。
//!
//! 进程须声明 Per-Monitor DPI 感知，所有坐标均为虚拟桌面物理像素。

#![deny(unsafe_op_in_unsafe_fn)]

pub mod adjust;
pub mod error;
pub mod glow;
pub mod hint;
pub mod input;
pub mod layout;
pub mod modes;
pub mod stroke;

mod render;
mod window;

use lenswin_core::config::{GlowMode, LassoConfig, ScreenshotConfig, SearchConfig};
use lenswin_core::frame::Frame;
use lenswin_core::geometry::Rect;
use lenswin_core::monitor::MonitorInfo;
use lenswin_core::selection::SelectionMode;
use lenswin_core::snip::{OverlayFlow, OverlayResult};

pub use crate::error::OverlayError;
use crate::hint::HintState;
use crate::input::{Interaction, Options, Outcome, SnapState};
use crate::window::{SessionInput, Visual};

/// 一次选区请求。
pub struct SelectRequest<'a> {
    /// 每个显示器的冻结帧，顺序与 `monitors` 一致。
    pub frames: &'a [Frame],
    /// 显示器列表。
    pub monitors: &'a [MonitorInfo],
    /// 选区模式。
    pub mode: SelectionMode,
    /// 流程。
    pub flow: OverlayFlow,
    /// 套索参数。
    pub lasso: &'a LassoConfig,
    /// 放大镜、吸附等截屏选项。
    pub screenshot: &'a ScreenshotConfig,
    /// 辉光与炫彩描边，只在搜索流程生效。
    pub search: &'a SearchConfig,
    /// 冻结时刻的窗口范围，最上层在前；为空表示不吸附。
    pub windows: &'a [Rect],
    /// 历史选区，最新的在前。
    pub history: &'a [Rect],
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

/// 截屏流程保持原来的单色样式，辉光和炫彩只给搜索流程。
fn visual_for(flow: OverlayFlow, search: &SearchConfig) -> Visual {
    match flow {
        OverlayFlow::Direct => Visual {
            glow: search.glow,
            colorful: search.colorful_stroke,
        },
        OverlayFlow::Toolbar => Visual {
            glow: GlowMode::Off,
            colorful: false,
        },
    }
}

/// 冻结所有显示器并让用户选区。
///
/// 用户完成选区或取色返回 `Some`，按 Esc 或右键取消返回 `None`。
/// 必须在创建了消息循环的主线程调用；函数内部自带消息循环，返回前所有窗口已销毁。
pub fn select(request: &SelectRequest<'_>) -> Result<Option<OverlayResult>, OverlayError> {
    validate(request)?;
    let tool = modes::tool_for(request.mode, request.lasso);
    let snap = match request.mode {
        SelectionMode::Rect if request.screenshot.window_snap && !request.windows.is_empty() => {
            SnapState::On
        }
        _ => SnapState::Unavailable,
    };
    let options = Options {
        flow: request.flow,
        magnifier: request.screenshot.magnifier,
        snap,
        windows: request.windows.to_vec(),
        history: request.history.to_vec(),
        hints: if request.screenshot.show_hints {
            HintState::Shown
        } else {
            HintState::Hidden
        },
    };
    let input = SessionInput {
        interaction: Interaction::new(tool, request.monitors.to_vec(), options),
        monitors: request.monitors.to_vec(),
        bgra_frames: request
            .frames
            .iter()
            .map(|f| render::rgba_to_bgra(f.rgba()))
            .collect(),
        stroke_width: request.lasso.stroke_width,
        show_lasso_bounds: request.lasso.show_bounds,
        visual: visual_for(request.flow, request.search),
    };
    Ok(match window::run(input)? {
        Outcome::Selected(selection) => Some(OverlayResult::Selected(selection)),
        Outcome::Action(selection, action) => Some(OverlayResult::Action(selection, action)),
        Outcome::Color(text) => Some(OverlayResult::Color(text)),
        Outcome::Cancelled => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        screenshot: &'a ScreenshotConfig,
        search: &'a SearchConfig,
    ) -> SelectRequest<'a> {
        SelectRequest {
            frames,
            monitors,
            mode: SelectionMode::Rect,
            flow: OverlayFlow::Direct,
            lasso,
            screenshot,
            search,
            windows: &[],
            history: &[],
        }
    }

    #[test]
    fn rejects_empty_input() {
        let lasso = LassoConfig::default();
        let shot = ScreenshotConfig::default();
        let search = SearchConfig::default();
        assert!(validate(&request(&[], &[], &lasso, &shot, &search)).is_err());
    }

    #[test]
    fn rejects_count_mismatch() {
        let lasso = LassoConfig::default();
        let shot = ScreenshotConfig::default();
        let search = SearchConfig::default();
        let frames = [frame(0)];
        let monitors = [monitor(0), monitor(2)];
        assert!(validate(&request(&frames, &monitors, &lasso, &shot, &search)).is_err());
    }

    #[test]
    fn rejects_region_mismatch() {
        let lasso = LassoConfig::default();
        let shot = ScreenshotConfig::default();
        let search = SearchConfig::default();
        let frames = [frame(0), frame(5)];
        let monitors = [monitor(0), monitor(2)];
        assert!(validate(&request(&frames, &monitors, &lasso, &shot, &search)).is_err());
    }

    #[test]
    fn screenshot_flow_keeps_plain_style() {
        let search = SearchConfig {
            glow: GlowMode::Always,
            colorful_stroke: true,
            ..SearchConfig::default()
        };
        assert_eq!(
            visual_for(OverlayFlow::Toolbar, &search),
            Visual {
                glow: GlowMode::Off,
                colorful: false
            }
        );
        assert_eq!(
            visual_for(OverlayFlow::Direct, &search),
            Visual {
                glow: GlowMode::Always,
                colorful: true
            }
        );
    }

    #[test]
    fn accepts_aligned_input() {
        let lasso = LassoConfig::default();
        let shot = ScreenshotConfig::default();
        let search = SearchConfig::default();
        let frames = [frame(0), frame(2)];
        let monitors = [monitor(0), monitor(2)];
        assert!(validate(&request(&frames, &monitors, &lasso, &shot, &search)).is_ok());
    }
}
