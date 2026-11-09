//! 鼠标键盘状态机：与 Win32 无关，窗口层只负责把消息翻译成这里的调用。

use lenswin_core::geometry::Point;
use lenswin_core::monitor::MonitorInfo;
use lenswin_core::selection::Selection;

use crate::modes::{Preview, SelectionTool};

/// 一次选区交互的结局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 得到有效选区。
    Selected(Selection),
    /// 用户取消。
    Cancelled,
}

/// 解码鼠标消息的 `lparam`：低 16 位为 x，高 16 位为 y，均为有符号数，
/// 鼠标被捕获时可能是负值或超出窗口。
pub fn point_from_lparam(lparam: isize) -> (i32, i32) {
    let signed = |bits: isize| {
        let word = u16::try_from(bits & 0xFFFF).unwrap_or(0);
        i32::from(i16::from_ne_bytes(word.to_ne_bytes()))
    };
    (signed(lparam), signed(lparam >> 16))
}

/// 窗口客户区坐标转虚拟桌面坐标；窗口左上角即显示器左上角。
fn to_virtual(monitor: &MonitorInfo, local: (i32, i32)) -> Option<Point> {
    Some(Point::new(
        monitor.bounds.x.checked_add(local.0)?,
        monitor.bounds.y.checked_add(local.1)?,
    ))
}

/// 交互状态：持有选区工具并记录结局。
pub struct Interaction {
    tool: Box<dyn SelectionTool>,
    monitors: Vec<MonitorInfo>,
    dragging: bool,
    outcome: Option<Outcome>,
}

impl Interaction {
    /// 创建交互，`monitors` 的顺序即窗口序号。
    pub fn new(tool: Box<dyn SelectionTool>, monitors: Vec<MonitorInfo>) -> Self {
        Self {
            tool,
            monitors,
            dragging: false,
            outcome: None,
        }
    }

    /// 在序号为 `window_index` 的显示器窗口上按下。
    pub fn press(&mut self, window_index: usize, local: (i32, i32)) {
        if self.dragging || self.outcome.is_some() {
            return;
        }
        let Some(monitor) = self.monitors.get(window_index) else {
            return;
        };
        let Some(point) = to_virtual(monitor, local) else {
            return;
        };
        self.tool.press(point, monitor, window_index);
        self.dragging = true;
    }

    /// 拖拽中移动；`window_index` 是收到消息的窗口，鼠标捕获期间始终是起始窗口。
    pub fn drag(&mut self, window_index: usize, local: (i32, i32)) {
        if !self.dragging {
            return;
        }
        if let Some(point) = self
            .monitors
            .get(window_index)
            .and_then(|m| to_virtual(m, local))
        {
            self.tool.drag(point);
        }
    }

    /// 松开；选区无效（如只点了一下）时回到空闲，覆盖层保持打开。
    pub fn release(&mut self, window_index: usize, local: (i32, i32)) {
        if !self.dragging {
            return;
        }
        self.dragging = false;
        let Some(point) = self
            .monitors
            .get(window_index)
            .and_then(|m| to_virtual(m, local))
        else {
            self.tool.reset();
            return;
        };
        if let Ok(selection) = self.tool.release(point) {
            self.outcome = Some(Outcome::Selected(selection));
        }
    }

    /// 取消并结束。
    pub fn cancel(&mut self) {
        self.tool.reset();
        self.dragging = false;
        if self.outcome.is_none() {
            self.outcome = Some(Outcome::Cancelled);
        }
    }

    /// 是否正在拖拽。
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// 已有结局。
    pub fn is_finished(&self) -> bool {
        self.outcome.is_some()
    }

    /// 取走结局。
    pub fn take_outcome(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }

    /// 当前预览。
    pub fn preview(&self) -> Preview<'_> {
        self.tool.preview()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modes::{LassoTool, RectTool};
    use lenswin_core::geometry::Rect;

    fn monitors() -> Vec<MonitorInfo> {
        vec![
            MonitorInfo {
                bounds: Rect::new(1920, 0, 1920, 1080),
                dpi: 120,
                primary: false,
            },
            MonitorInfo {
                bounds: Rect::new(0, 0, 1920, 1080),
                dpi: 96,
                primary: true,
            },
        ]
    }

    #[test]
    fn lparam_decodes_signed_coordinates() {
        assert_eq!(point_from_lparam(0x0064_00C8), (200, 100));
        // x = -5 (0xFFFB)，y = -1 (0xFFFF)
        let packed = isize::try_from(0xFFFF_FFFB_u32).expect("u32 可放入 isize");
        assert_eq!(point_from_lparam(packed), (-5, -1));
        assert_eq!(point_from_lparam(0), (0, 0));
    }

    #[test]
    fn rect_selection_on_secondary_monitor_uses_virtual_coordinates() {
        let mut it = Interaction::new(Box::new(RectTool::new()), monitors());
        it.press(0, (100, 200));
        assert!(it.is_dragging());
        it.drag(0, (300, 400));
        it.release(0, (500, 600));
        match it.take_outcome() {
            Some(Outcome::Selected(sel)) => {
                assert_eq!(sel.bounds(), Rect::new(2020, 200, 400, 400));
                assert_eq!(sel.monitor_index(), 0);
            }
            other => panic!("期望得到选区，实际为 {other:?}"),
        }
    }

    #[test]
    fn mouse_capture_allows_negative_local_coordinates() {
        let mut it = Interaction::new(Box::new(RectTool::new()), monitors());
        it.press(0, (50, 50));
        it.release(0, (-300, 200));
        match it.take_outcome() {
            Some(Outcome::Selected(sel)) => {
                // 超出起始显示器左边界的部分被截掉
                assert_eq!(sel.bounds(), Rect::new(1920, 50, 50, 150));
            }
            other => panic!("期望得到选区，实际为 {other:?}"),
        }
    }

    #[test]
    fn click_without_drag_keeps_overlay_open() {
        let mut it = Interaction::new(Box::new(RectTool::new()), monitors());
        it.press(1, (10, 10));
        it.release(1, (10, 10));
        assert!(!it.is_finished());
        assert!(!it.is_dragging());
        it.press(1, (10, 10));
        it.release(1, (200, 200));
        assert!(it.is_finished());
    }

    #[test]
    fn lasso_flow_on_primary_monitor() {
        let mut it = Interaction::new(Box::new(LassoTool::new(10, 4)), monitors());
        it.press(1, (100, 100));
        it.drag(1, (200, 100));
        it.drag(1, (200, 200));
        it.release(1, (100, 200));
        match it.take_outcome() {
            Some(Outcome::Selected(sel)) => {
                assert_eq!(sel.bounds(), Rect::new(88, 88, 124, 124));
                assert_eq!(sel.monitor_index(), 1);
            }
            other => panic!("期望得到选区，实际为 {other:?}"),
        }
    }

    #[test]
    fn cancel_ends_with_cancelled_and_stops_drag() {
        let mut it = Interaction::new(Box::new(RectTool::new()), monitors());
        it.press(0, (10, 10));
        it.cancel();
        assert!(!it.is_dragging());
        assert_eq!(it.take_outcome(), Some(Outcome::Cancelled));
    }

    #[test]
    fn events_after_finish_are_ignored() {
        let mut it = Interaction::new(Box::new(RectTool::new()), monitors());
        it.cancel();
        it.press(0, (10, 10));
        assert!(!it.is_dragging());
    }

    #[test]
    fn unknown_window_index_is_ignored() {
        let mut it = Interaction::new(Box::new(RectTool::new()), monitors());
        it.press(9, (10, 10));
        assert!(!it.is_dragging());
    }

    #[test]
    fn move_without_press_does_nothing() {
        let mut it = Interaction::new(Box::new(RectTool::new()), monitors());
        it.drag(0, (10, 10));
        it.release(0, (20, 20));
        assert!(!it.is_finished());
    }
}
