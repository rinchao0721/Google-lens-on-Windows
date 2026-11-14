//! 选区覆盖层：只返回选区和用户选择的动作，不触发上传或动作；
//! 常驻选框会话通过回调把选区回投给调用方。
//!
//! 进程须声明 Per-Monitor DPI 感知，所有坐标均为虚拟桌面物理像素。

#![deny(unsafe_op_in_unsafe_fn)]

pub mod adjust;
pub mod annotate;
pub mod error;
pub mod glow;
pub mod hint;
pub mod input;
pub mod layout;
pub mod modes;
pub mod stroke;

mod paint;
mod render;
mod window;

use std::rc::Rc;

use lenswin_core::config::{AnnotateConfig, GlowMode, LassoConfig, ScreenshotConfig, SearchConfig};
use lenswin_core::frame::Frame;
use lenswin_core::geometry::Rect;
use lenswin_core::monitor::MonitorInfo;
use lenswin_core::selection::SelectionMode;
use lenswin_core::snip::{LiveEvent, OverlayFlow, OverlayOutput, OverlayResult};

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
    /// 标注工具的初始样式，只在截屏流程用到。
    pub annotate: &'a AnnotateConfig,
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
        OverlayFlow::Direct | OverlayFlow::Live => Visual {
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
/// 用户完成选区或取色时 `result` 为 `Some`，按 Esc 或右键取消为 `None`；
/// 标注样式有变化时无论是否取消都会带回新样式。
/// 截屏流程的动作结果直接携带合成好的成图（含标注）。
/// 必须在创建了消息循环的主线程调用，且线程已初始化 COM；函数内部自带消息循环，返回前所有窗口已销毁。
pub fn select(request: &SelectRequest<'_>) -> Result<OverlayOutput, OverlayError> {
    if request.flow == OverlayFlow::Live {
        return Err(OverlayError::InvalidInput(
            "常驻选框请使用 LiveSession".to_string(),
        ));
    }
    let input = session_input(request, request.flow)?;
    let run = window::run(input)?;
    let result = match run.outcome {
        Outcome::Selected(selection) => Some(OverlayResult::Selected(selection)),
        Outcome::Action(selection, action) => {
            let image = run
                .image
                .ok_or_else(|| OverlayError::InvalidInput("缺少合成好的成图".to_string()))?;
            Some(OverlayResult::Action(selection, action, image))
        }
        Outcome::Color(text) => Some(OverlayResult::Color(text)),
        Outcome::Cancelled => None,
    };
    Ok(OverlayOutput {
        result,
        annotate: run.annotate,
    })
}

/// 常驻选框会话：选区落定后保留可调整的选框，每次落定或调整回投一次选区。
///
/// 只能在主线程创建和使用，窗口由调用方的消息循环驱动；销毁时关闭全部覆盖层窗口。
/// 同一时间只能有一个覆盖层，已有会话时 [`select`] 和 [`LiveSession::begin`] 都会返回错误。
pub struct LiveSession {
    _windows: window::LiveWindows,
}

impl LiveSession {
    /// 冻结画面并显示覆盖层后立即返回；`request.flow` 被忽略，一律按常驻选框处理。
    ///
    /// `sink` 在窗口过程里调用，调用前已释放覆盖层内部状态；实现应只做入队等轻量操作，
    /// 不要在回调里销毁本会话。
    pub fn begin(
        request: &SelectRequest<'_>,
        sink: impl Fn(LiveEvent) + 'static,
    ) -> Result<Self, OverlayError> {
        let input = session_input(request, OverlayFlow::Live)?;
        let windows = window::open_live(input, Rc::new(sink))?;
        Ok(Self { _windows: windows })
    }
}

/// 校验请求并组装窗口层需要的会话输入。
fn session_input(
    request: &SelectRequest<'_>,
    flow: OverlayFlow,
) -> Result<SessionInput, OverlayError> {
    validate(request)?;
    let tool = modes::tool_for(request.mode, request.lasso);
    let snap = match request.mode {
        SelectionMode::Rect if request.screenshot.window_snap && !request.windows.is_empty() => {
            SnapState::On
        }
        _ => SnapState::Unavailable,
    };
    let options = Options {
        flow,
        magnifier: request.screenshot.magnifier,
        snap,
        windows: request.windows.to_vec(),
        history: request.history.to_vec(),
        hints: if request.screenshot.show_hints {
            HintState::Shown
        } else {
            HintState::Hidden
        },
        annotate: request.annotate.clone(),
    };
    Ok(SessionInput {
        interaction: Interaction::new(tool, request.monitors.to_vec(), options),
        monitors: request.monitors.to_vec(),
        bgra_frames: request
            .frames
            .iter()
            .map(|f| render::rgba_to_bgra(f.rgba()))
            .collect(),
        stroke_width: request.lasso.stroke_width,
        show_lasso_bounds: request.lasso.show_bounds,
        visual: visual_for(flow, request.search),
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
        annotate: &'a AnnotateConfig,
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
            annotate,
        }
    }

    #[test]
    fn rejects_empty_input() {
        let lasso = LassoConfig::default();
        let shot = ScreenshotConfig::default();
        let search = SearchConfig::default();
        let annotate = AnnotateConfig::default();
        assert!(validate(&request(&[], &[], &lasso, &shot, &search, &annotate)).is_err());
    }

    #[test]
    fn rejects_count_mismatch() {
        let lasso = LassoConfig::default();
        let shot = ScreenshotConfig::default();
        let search = SearchConfig::default();
        let annotate = AnnotateConfig::default();
        let frames = [frame(0)];
        let monitors = [monitor(0), monitor(2)];
        assert!(
            validate(&request(
                &frames, &monitors, &lasso, &shot, &search, &annotate
            ))
            .is_err()
        );
    }

    #[test]
    fn rejects_region_mismatch() {
        let lasso = LassoConfig::default();
        let shot = ScreenshotConfig::default();
        let search = SearchConfig::default();
        let annotate = AnnotateConfig::default();
        let frames = [frame(0), frame(5)];
        let monitors = [monitor(0), monitor(2)];
        assert!(
            validate(&request(
                &frames, &monitors, &lasso, &shot, &search, &annotate
            ))
            .is_err()
        );
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
        let annotate = AnnotateConfig::default();
        let frames = [frame(0), frame(2)];
        let monitors = [monitor(0), monitor(2)];
        assert!(
            validate(&request(
                &frames, &monitors, &lasso, &shot, &search, &annotate
            ))
            .is_ok()
        );
    }
}
