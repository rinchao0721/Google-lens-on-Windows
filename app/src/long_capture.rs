//! 长截图会话协调：主线程定时抓帧并驱动自动滚动，工作线程执行匹配与合成。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use lenswin_core::frame::CroppedImage;
use lenswin_core::geometry::{Point, Rect};
use lenswin_core::snip::{ScrollEvent, ToolbarAction};
use lenswin_core::stitch::{ScrollAxis, StitchStatus, Stitcher};
use lenswin_overlay::ScrollSession;
use lenswin_platform::capture;
use lenswin_platform::input::{self, WHEEL_NOTCH};
use lenswin_platform::window::{TaskNotifier, WindowHandle};

/// 长截图定时抓帧与自动滚动的 Win32 定时器 ID。
pub const SCROLL_TIMER_ID: usize = 1;

/// 长截图抓帧与自动滚动周期（毫秒）。
pub const SCROLL_INTERVAL_MS: u32 = 80;

/// 预览缩略图在滚动轴方向上保留的最大尾端像素数。
const PREVIEW_MAX_PX: u32 = 400;

/// 工作线程积压帧数上限；达到上限时跳过新帧抓取以避免队列堆积。
const MAX_PENDING_FRAMES: usize = 2;

/// 自动滚动时允许的连续无位移帧数上限。
const AUTO_SCROLL_NO_MOVE_LIMIT: u8 = 3;

/// 从覆盖层回调投递到主线程的长截图 UI 事件。
pub type ScrollMessage = (u64, ScrollEvent);

/// 工作线程返回给主线程的长截图处理消息。
pub enum WorkerReply {
    /// 单帧拼接结果与可选尾部预览图。
    FrameProcessed {
        /// 所属长截图会话序号。
        seq: u64,
        /// 拼接状态。
        status: StitchStatus,
        /// 状态变化时生成的新预览缩略图。
        preview: Option<CroppedImage>,
    },
    /// 用户确认结束动作后，后台合成好的成图。
    Finished {
        /// 所属长截图会话序号。
        seq: u64,
        /// 用户选择的后处理动作。
        action: ToolbarAction,
        /// 初始长截图选区（用于搜索锚点与贴图工作区定位）。
        bounds: Rect,
        /// 成图结果或失败说明。
        result: Result<CroppedImage, String>,
    },
}

enum WorkerCommand {
    Frame(CroppedImage),
    Finish(ToolbarAction),
}

/// 纯逻辑状态机：处理自动滚动开关、定时器节拍检查与拼接状态转换。
pub struct ScrollController {
    bounds: Rect,
    auto_scrolling: bool,
    axis: Option<ScrollAxis>,
    total_px: u32,
    reached_limit: bool,
    no_move_streak: u8,
    moved_once: bool,
    finishing: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum AutoScrollAction {
    Start {
        center: Point,
        axis: ScrollAxis,
        delta: i32,
        status: String,
    },
    Stop {
        status: String,
        reset_ui: bool,
    },
}

#[derive(Debug, PartialEq, Eq)]
struct TickAction {
    capture: bool,
    scroll: Option<(Point, ScrollAxis, i32)>,
    stop_auto_scroll_status: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct StitchUiUpdate {
    status: Option<String>,
    auto_scrolling: Option<bool>,
    preview: Option<(Option<CroppedImage>, Option<ScrollAxis>)>,
    stop_timer: bool,
}

impl ScrollController {
    /// 按初始选区创建长截图控制器。
    pub fn new(bounds: Rect) -> Self {
        Self {
            bounds,
            auto_scrolling: false,
            axis: None,
            total_px: 0,
            reached_limit: false,
            no_move_streak: 0,
            moved_once: false,
            finishing: false,
        }
    }

    fn idle_status(&self) -> String {
        if self.reached_limit {
            "已达到最大长度".to_string()
        } else if self.axis.is_some() {
            format!("已拼接 {} px", self.total_px)
        } else {
            "滚动以开始".to_string()
        }
    }

    fn on_auto_scroll(&mut self, enable: bool) -> AutoScrollAction {
        if !enable {
            self.auto_scrolling = false;
            self.no_move_streak = 0;
            return AutoScrollAction::Stop {
                status: self.idle_status(),
                reset_ui: false,
            };
        }
        if self.reached_limit || self.finishing {
            self.auto_scrolling = false;
            return AutoScrollAction::Stop {
                status: self.idle_status(),
                reset_ui: true,
            };
        }
        self.auto_scrolling = true;
        self.no_move_streak = 0;
        self.moved_once = false;
        let axis = self.axis.unwrap_or(ScrollAxis::Vertical);
        let status = if self.axis.is_some() {
            format!("已拼接 {} px", self.total_px)
        } else {
            "正在自动滚动...".to_string()
        };
        AutoScrollAction::Start {
            center: bounds_center(self.bounds),
            axis,
            delta: wheel_delta_for(axis),
            status,
        }
    }

    fn on_tick(&mut self, cursor: Option<Point>) -> TickAction {
        if self.reached_limit || self.finishing {
            return TickAction {
                capture: false,
                scroll: None,
                stop_auto_scroll_status: None,
            };
        }
        if !self.auto_scrolling {
            return TickAction {
                capture: true,
                scroll: None,
                stop_auto_scroll_status: None,
            };
        }
        if let Some(pos) = cursor
            && !self.bounds.contains(pos)
        {
            self.auto_scrolling = false;
            self.no_move_streak = 0;
            return TickAction {
                capture: true,
                scroll: None,
                stop_auto_scroll_status: Some(self.idle_status()),
            };
        }
        let target = cursor
            .filter(|pos| self.bounds.contains(*pos))
            .unwrap_or_else(|| bounds_center(self.bounds));
        let axis = self.axis.unwrap_or(ScrollAxis::Vertical);
        TickAction {
            capture: true,
            scroll: Some((target, axis, wheel_delta_for(axis))),
            stop_auto_scroll_status: None,
        }
    }

    fn on_stitch_status(
        &mut self,
        status: StitchStatus,
        preview: Option<CroppedImage>,
    ) -> StitchUiUpdate {
        match status {
            StitchStatus::FirstFrame => StitchUiUpdate {
                status: None,
                auto_scrolling: None,
                preview: Some((preview, self.axis)),
                stop_timer: false,
            },
            StitchStatus::Adopted {
                total_px,
                axis,
                reached_limit,
            } => {
                self.axis = Some(axis);
                self.total_px = total_px;
                self.reached_limit = reached_limit;
                self.no_move_streak = 0;
                if self.auto_scrolling {
                    self.moved_once = true;
                }
                if reached_limit {
                    let was_auto = self.auto_scrolling;
                    self.auto_scrolling = false;
                    StitchUiUpdate {
                        status: Some("已达到最大长度".to_string()),
                        auto_scrolling: was_auto.then_some(false),
                        preview: Some((preview, Some(axis))),
                        stop_timer: true,
                    }
                } else {
                    StitchUiUpdate {
                        status: Some(format!("已拼接 {total_px} px")),
                        auto_scrolling: None,
                        preview: Some((preview, Some(axis))),
                        stop_timer: false,
                    }
                }
            }
            StitchStatus::NoMove => {
                if !self.auto_scrolling {
                    return StitchUiUpdate::default();
                }
                self.no_move_streak = self.no_move_streak.saturating_add(1);
                if self.no_move_streak >= AUTO_SCROLL_NO_MOVE_LIMIT {
                    self.auto_scrolling = false;
                    self.no_move_streak = 0;
                    let text = if self.moved_once {
                        "已到底"
                    } else {
                        "目标窗口不响应自动滚动，请手动滚动"
                    };
                    StitchUiUpdate {
                        status: Some(text.to_string()),
                        auto_scrolling: Some(false),
                        preview: None,
                        stop_timer: false,
                    }
                } else {
                    StitchUiUpdate::default()
                }
            }
            StitchStatus::IgnoredBackward => {
                self.no_move_streak = 0;
                StitchUiUpdate::default()
            }
            StitchStatus::TooFast => {
                self.no_move_streak = 0;
                StitchUiUpdate {
                    status: Some("滚动太快，请往回滚一点".to_string()),
                    auto_scrolling: None,
                    preview: None,
                    stop_timer: false,
                }
            }
        }
    }

    fn begin_finish(&mut self) -> bool {
        if self.finishing {
            return false;
        }
        self.finishing = true;
        self.auto_scrolling = false;
        self.no_move_streak = 0;
        true
    }
}

/// 正在运行或等待最终合成的长截图会话状态。
pub enum LongCaptureState {
    /// 边框与控制面板打开，定时抓帧并接收拼接状态。
    Active(Box<ActiveLongCapture>),
    /// 控制面板已关闭，正在等待后台工作线程完成最终图像合成。
    Finishing {
        /// 会话序号。
        id: u64,
    },
}

impl LongCaptureState {
    /// 返回当前会话序号。
    pub fn id(&self) -> u64 {
        match self {
            LongCaptureState::Active(active) => active.id,
            LongCaptureState::Finishing { id } => *id,
        }
    }
}

/// 活跃长截图会话持有的覆盖层窗口、控制器与后台线程通道。
pub struct ActiveLongCapture {
    id: u64,
    window: WindowHandle,
    session: ScrollSession,
    controller: ScrollController,
    worker_tx: Sender<WorkerCommand>,
    pending_frames: Arc<AtomicUsize>,
}

impl ActiveLongCapture {
    /// 启动长截图边框控制面板、工作线程与 80 ms 抓帧定时器，并立即抓下首帧。
    pub fn start(
        id: u64,
        bounds: Rect,
        window: WindowHandle,
        notifier: TaskNotifier,
        scroll_tx: Sender<ScrollMessage>,
        worker_tx_reply: Sender<WorkerReply>,
    ) -> Result<Self, String> {
        let session = ScrollSession::begin(bounds, move |event| {
            if scroll_tx.send((id, event)).is_err() {
                return;
            }
            if let Err(e) = notifier.notify() {
                log::error!("投递长截图界面事件失败：{e}");
            }
        })
        .map_err(|e| format!("启动长截图界面失败：{e}"))?;

        let (cmd_tx, cmd_rx) = channel();
        let pending_frames = Arc::new(AtomicUsize::new(0));
        let worker_pending = Arc::clone(&pending_frames);
        std::thread::Builder::new()
            .name("lenswin-stitch".to_string())
            .spawn(move || {
                run_stitch_worker(
                    id,
                    bounds,
                    &cmd_rx,
                    &worker_tx_reply,
                    notifier,
                    &worker_pending,
                );
            })
            .map_err(|e| format!("创建长截图拼接线程失败：{e}"))?;

        if let Err(e) = window.start_timer(SCROLL_TIMER_ID, SCROLL_INTERVAL_MS) {
            return Err(format!("启动长截图定时器失败：{e}"));
        }

        let active = Self {
            id,
            window,
            session,
            controller: ScrollController::new(bounds),
            worker_tx: cmd_tx,
            pending_frames,
        };
        active.capture_and_enqueue();
        Ok(active)
    }

    /// 定时抓帧并在开启自动滚动时检查光标位置、投递滚轮消息。
    pub fn on_timer(&mut self) {
        let cursor = if self.controller.auto_scrolling {
            input::cursor_position().ok()
        } else {
            None
        };
        let action = self.controller.on_tick(cursor);
        if let Some(status) = action.stop_auto_scroll_status {
            log::info!("光标已移出长截图选区，停止自动滚动");
            self.session.set_auto_scrolling(false);
            self.session.set_status(&status);
        }
        if action.capture {
            self.capture_and_enqueue();
        }
        if let Some((point, axis, delta)) = action.scroll
            && let Err(e) = input::scroll_wheel(point, axis, delta)
        {
            log::warn!("发送滚轮事件失败：{e}");
            self.controller.auto_scrolling = false;
            self.session.set_auto_scrolling(false);
            self.session
                .set_status("目标窗口不响应自动滚动，请手动滚动");
        }
    }

    /// 处理用户点击自动滚动开关。
    pub fn on_auto_scroll(&mut self, enable: bool) {
        match self.controller.on_auto_scroll(enable) {
            AutoScrollAction::Start {
                center,
                axis,
                delta,
                status,
            } => {
                if let Err(e) = input::scroll_wheel(center, axis, delta) {
                    log::warn!("启动自动滚动失败：{e}");
                    self.controller.auto_scrolling = false;
                    self.session.set_auto_scrolling(false);
                    self.session
                        .set_status("目标窗口不响应自动滚动，请手动滚动");
                    return;
                }
                self.session.set_status(&status);
                // 重置定时器周期，让目标窗口有完整的一帧间隔完成重绘
                if let Err(e) = self.window.start_timer(SCROLL_TIMER_ID, SCROLL_INTERVAL_MS) {
                    log::warn!("重置长截图定时器失败：{e}");
                }
            }
            AutoScrollAction::Stop { status, reset_ui } => {
                if reset_ui {
                    self.session.set_auto_scrolling(false);
                }
                self.session.set_status(&status);
            }
        }
    }

    /// 处理后台拼接线程返回的单帧处理结果。
    pub fn on_frame_processed(&mut self, status: StitchStatus, preview: Option<CroppedImage>) {
        let update = self.controller.on_stitch_status(status, preview);
        if let Some((img, axis)) = update.preview {
            self.session.set_preview(img, axis);
        }
        if let Some(active) = update.auto_scrolling {
            self.session.set_auto_scrolling(active);
        }
        if let Some(text) = update.status {
            self.session.set_status(&text);
        }
        if update.stop_timer {
            self.window.stop_timer(SCROLL_TIMER_ID);
        }
    }

    /// 请求后台线程合成最终图像；返回 `true` 表示已投递，此时可释放本结构切入等待态。
    pub fn request_finish(&mut self, action: ToolbarAction) -> bool {
        if !self.controller.begin_finish() {
            return false;
        }
        self.window.stop_timer(SCROLL_TIMER_ID);
        if self.worker_tx.send(WorkerCommand::Finish(action)).is_err() {
            log::error!("长截图工作线程已意外退出");
            return false;
        }
        true
    }

    fn capture_and_enqueue(&self) {
        if self.pending_frames.load(Ordering::Acquire) >= MAX_PENDING_FRAMES {
            return;
        }
        let frame = match capture::capture_rect(self.controller.bounds) {
            Ok(img) => img,
            Err(e) => {
                log::warn!("长截图抓帧失败：{e}");
                return;
            }
        };
        self.pending_frames.fetch_add(1, Ordering::AcqRel);
        if self.worker_tx.send(WorkerCommand::Frame(frame)).is_err() {
            self.pending_frames.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl Drop for ActiveLongCapture {
    fn drop(&mut self) {
        self.window.stop_timer(SCROLL_TIMER_ID);
    }
}

fn bounds_center(bounds: Rect) -> Point {
    let half_w = i32::try_from(bounds.width / 2).unwrap_or(0);
    let half_h = i32::try_from(bounds.height / 2).unwrap_or(0);
    Point::new(
        bounds.x.saturating_add(half_w),
        bounds.y.saturating_add(half_h),
    )
}

const fn wheel_delta_for(axis: ScrollAxis) -> i32 {
    match axis {
        ScrollAxis::Vertical => -WHEEL_NOTCH,
        ScrollAxis::Horizontal => WHEEL_NOTCH,
    }
}

fn run_stitch_worker(
    seq: u64,
    bounds: Rect,
    rx: &Receiver<WorkerCommand>,
    reply_tx: &Sender<WorkerReply>,
    notifier: TaskNotifier,
    pending_frames: &AtomicUsize,
) {
    let mut stitcher = Stitcher::new();
    while let Ok(cmd) = rx.recv() {
        match cmd {
            WorkerCommand::Frame(frame) => {
                pending_frames.fetch_sub(1, Ordering::AcqRel);
                match stitcher.push(&frame) {
                    Ok(status) => {
                        let preview = match status {
                            StitchStatus::FirstFrame | StitchStatus::Adopted { .. } => {
                                stitcher.tail_preview(PREVIEW_MAX_PX).ok()
                            }
                            StitchStatus::NoMove
                            | StitchStatus::IgnoredBackward
                            | StitchStatus::TooFast => None,
                        };
                        if reply_tx
                            .send(WorkerReply::FrameProcessed {
                                seq,
                                status,
                                preview,
                            })
                            .is_err()
                        {
                            return;
                        }
                        if let Err(e) = notifier.notify() {
                            log::error!("投递长截图拼接结果失败：{e}");
                        }
                    }
                    Err(e) => {
                        log::warn!("拼接长截图帧失败：{e}");
                    }
                }
            }
            WorkerCommand::Finish(action) => {
                let result = stitcher
                    .compose()
                    .map_err(|e| format!("合成长截图失败：{e}"));
                if reply_tx
                    .send(WorkerReply::Finished {
                        seq,
                        action,
                        bounds,
                        result,
                    })
                    .is_err()
                {
                    return;
                }
                if let Err(e) = notifier.notify() {
                    log::error!("投递长截图最终结果失败：{e}");
                }
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32) -> CroppedImage {
        let len = usize::try_from(w)
            .unwrap_or(0)
            .saturating_mul(usize::try_from(h).unwrap_or(0))
            .saturating_mul(4);
        CroppedImage::new(w, h, vec![128u8; len]).expect("测试图应合法")
    }

    #[test]
    fn auto_scroll_stops_after_three_no_moves_with_distinct_messages() {
        let mut ctrl = ScrollController::new(Rect::new(100, 100, 400, 500));
        assert!(matches!(
            ctrl.on_auto_scroll(true),
            AutoScrollAction::Start { .. }
        ));

        // 从未滚动过 -> 提示目标窗口不响应
        assert_eq!(
            ctrl.on_stitch_status(StitchStatus::NoMove, None),
            StitchUiUpdate::default()
        );
        assert_eq!(
            ctrl.on_stitch_status(StitchStatus::NoMove, None),
            StitchUiUpdate::default()
        );
        let third = ctrl.on_stitch_status(StitchStatus::NoMove, None);
        assert_eq!(
            third.status.as_deref(),
            Some("目标窗口不响应自动滚动，请手动滚动")
        );
        assert_eq!(third.auto_scrolling, Some(false));
        assert!(!ctrl.is_auto_scrolling_for_test());

        // 曾经成功拼接后再遇 3 次无位移 -> 提示已到底
        assert!(matches!(
            ctrl.on_auto_scroll(true),
            AutoScrollAction::Start { .. }
        ));
        let adopted = ctrl.on_stitch_status(
            StitchStatus::Adopted {
                total_px: 800,
                axis: ScrollAxis::Vertical,
                reached_limit: false,
            },
            Some(img(100, 100)),
        );
        assert_eq!(adopted.status.as_deref(), Some("已拼接 800 px"));
        assert_eq!(
            ctrl.on_stitch_status(StitchStatus::NoMove, None),
            StitchUiUpdate::default()
        );
        assert_eq!(
            ctrl.on_stitch_status(StitchStatus::NoMove, None),
            StitchUiUpdate::default()
        );
        let bottom = ctrl.on_stitch_status(StitchStatus::NoMove, None);
        assert_eq!(bottom.status.as_deref(), Some("已到底"));
        assert_eq!(bottom.auto_scrolling, Some(false));
    }

    #[test]
    fn auto_scroll_stops_when_cursor_leaves_bounds() {
        let mut ctrl = ScrollController::new(Rect::new(10, 20, 200, 300));
        assert!(matches!(
            ctrl.on_auto_scroll(true),
            AutoScrollAction::Start {
                center,
                axis: ScrollAxis::Vertical,
                delta,
                ..
            } if center == Point::new(110, 170) && delta == -WHEEL_NOTCH
        ));

        // 光标仍在选区内：继续抓帧并滚轮
        let inside = ctrl.on_tick(Some(Point::new(50, 50)));
        assert!(inside.capture);
        assert_eq!(
            inside.scroll,
            Some((Point::new(50, 50), ScrollAxis::Vertical, -WHEEL_NOTCH))
        );
        assert!(inside.stop_auto_scroll_status.is_none());

        // 光标移出选区：停止自动滚动，本次仍保留抓帧但不发送滚轮
        let outside = ctrl.on_tick(Some(Point::new(5, 50)));
        assert!(outside.capture);
        assert!(outside.scroll.is_none());
        assert_eq!(
            outside.stop_auto_scroll_status.as_deref(),
            Some("滚动以开始")
        );
        assert!(!ctrl.is_auto_scrolling_for_test());
    }

    #[test]
    fn reached_limit_stops_auto_scroll_and_timer_and_rejects_restart() {
        let mut ctrl = ScrollController::new(Rect::new(0, 0, 300, 400));
        assert!(matches!(
            ctrl.on_auto_scroll(true),
            AutoScrollAction::Start { .. }
        ));
        let limit = ctrl.on_stitch_status(
            StitchStatus::Adopted {
                total_px: 30_000,
                axis: ScrollAxis::Horizontal,
                reached_limit: true,
            },
            Some(img(100, 100)),
        );
        assert_eq!(limit.status.as_deref(), Some("已达到最大长度"));
        assert_eq!(limit.auto_scrolling, Some(false));
        assert!(limit.stop_timer);

        // 达到上限后，重新点自动滚动被拒绝，定时器也不再抓帧
        assert_eq!(
            ctrl.on_auto_scroll(true),
            AutoScrollAction::Stop {
                status: "已达到最大长度".to_string(),
                reset_ui: true,
            }
        );
        let tick = ctrl.on_tick(Some(Point::new(50, 50)));
        assert!(!tick.capture);
        assert!(tick.scroll.is_none());
    }

    #[test]
    fn too_fast_resets_no_move_streak_and_reports_warning() {
        let mut ctrl = ScrollController::new(Rect::new(0, 0, 200, 200));
        assert!(matches!(
            ctrl.on_auto_scroll(true),
            AutoScrollAction::Start { .. }
        ));
        assert_eq!(
            ctrl.on_stitch_status(StitchStatus::NoMove, None),
            StitchUiUpdate::default()
        );
        assert_eq!(
            ctrl.on_stitch_status(StitchStatus::NoMove, None),
            StitchUiUpdate::default()
        );
        let warn = ctrl.on_stitch_status(StitchStatus::TooFast, None);
        assert_eq!(warn.status.as_deref(), Some("滚动太快，请往回滚一点"));
        // TooFast 已清空无位移计数，下一次 NoMove 不应立刻停止
        assert_eq!(
            ctrl.on_stitch_status(StitchStatus::NoMove, None),
            StitchUiUpdate::default()
        );
        assert!(ctrl.is_auto_scrolling_for_test());
    }

    impl ScrollController {
        fn is_auto_scrolling_for_test(&self) -> bool {
            self.auto_scrolling
        }
    }
}
