//! 矩形框选。

use lenswin_core::error::SelectionError;
use lenswin_core::geometry::{Point, Rect, rect_bounds};
use lenswin_core::monitor::MonitorInfo;
use lenswin_core::selection::Selection;

use super::{Preview, SelectionTool};

struct Drag {
    start: Point,
    current: Point,
    monitor_bounds: Rect,
    monitor_index: usize,
}

/// 拖出一个矩形。
#[derive(Default)]
pub struct RectTool {
    drag: Option<Drag>,
}

impl RectTool {
    /// 创建空闲状态的工具。
    pub fn new() -> Self {
        Self::default()
    }
}

impl SelectionTool for RectTool {
    fn press(&mut self, point: Point, monitor: &MonitorInfo, monitor_index: usize) {
        self.drag = Some(Drag {
            start: point,
            current: point,
            monitor_bounds: monitor.bounds,
            monitor_index,
        });
    }

    fn drag(&mut self, point: Point) {
        if let Some(drag) = self.drag.as_mut() {
            drag.current = point;
        }
    }

    fn release(&mut self, point: Point) -> Result<Selection, SelectionError> {
        let drag = self.drag.take().ok_or(SelectionError::EmptyBounds)?;
        Selection::from_rect_drag(drag.start, point, &drag.monitor_bounds, drag.monitor_index)
    }

    fn preview(&self) -> Preview<'_> {
        match &self.drag {
            Some(d) => rect_bounds(d.start, d.current, &d.monitor_bounds)
                .map_or(Preview::None, Preview::Rect),
            None => Preview::None,
        }
    }

    fn reset(&mut self) {
        self.drag = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(x: i32) -> MonitorInfo {
        MonitorInfo {
            bounds: Rect::new(x, 0, 1920, 1080),
            dpi: 96,
            primary: false,
        }
    }

    #[test]
    fn drag_and_release_yield_clipped_selection() {
        let mut tool = RectTool::new();
        tool.press(Point::new(2000, 100), &monitor(1920), 1);
        tool.drag(Point::new(2200, 300));
        assert_eq!(
            tool.preview(),
            Preview::Rect(Rect::new(2000, 100, 200, 200))
        );
        let sel = tool.release(Point::new(9999, 400)).expect("选区");
        assert_eq!(sel.bounds(), Rect::new(2000, 100, 1840, 300));
        assert_eq!(sel.monitor_index(), 1);
        assert_eq!(tool.preview(), Preview::None);
    }

    #[test]
    fn reverse_drag_into_negative_coordinates() {
        let mut tool = RectTool::new();
        tool.press(Point::new(-100, 500), &monitor(-1920), 0);
        let sel = tool.release(Point::new(-400, 200)).expect("选区");
        assert_eq!(sel.bounds(), Rect::new(-400, 200, 300, 300));
    }

    #[test]
    fn click_without_movement_is_invalid() {
        let mut tool = RectTool::new();
        tool.press(Point::new(50, 50), &monitor(0), 0);
        assert_eq!(
            tool.release(Point::new(50, 50)).unwrap_err(),
            SelectionError::EmptyBounds
        );
    }

    #[test]
    fn release_without_press_is_invalid() {
        let mut tool = RectTool::new();
        assert!(tool.release(Point::new(1, 1)).is_err());
    }

    #[test]
    fn drag_outside_monitor_is_invalid() {
        let mut tool = RectTool::new();
        tool.press(Point::new(10, 10), &monitor(0), 0);
        assert!(tool.release(Point::new(10, -500)).is_err());
    }

    #[test]
    fn reset_clears_state() {
        let mut tool = RectTool::new();
        tool.press(Point::new(0, 0), &monitor(0), 0);
        tool.reset();
        assert_eq!(tool.preview(), Preview::None);
        assert!(tool.release(Point::new(100, 100)).is_err());
    }
}
