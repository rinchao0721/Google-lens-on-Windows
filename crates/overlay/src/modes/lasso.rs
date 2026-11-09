//! 自由套索圈选。

use lenswin_core::error::SelectionError;
use lenswin_core::geometry::{Point, Rect, lasso_bounds};
use lenswin_core::monitor::MonitorInfo;
use lenswin_core::selection::Selection;

use super::{Preview, SelectionTool};

/// 轨迹点数上限，超出后忽略新点，防止长时间拖拽耗尽内存。
const MAX_TRAIL_POINTS: usize = 50_000;

/// 原始轨迹外接矩形的宽高都小于该值（物理像素）时视为误点，外扩之前判断。
const MIN_TRAIL_SPAN: u32 = 4;

/// 轨迹是否过小：宽和高都不足阈值。
fn is_trail_too_small(points: &[Point]) -> bool {
    Rect::bounding(points).is_none_or(|r| r.width < MIN_TRAIL_SPAN && r.height < MIN_TRAIL_SPAN)
}

struct Drag {
    points: Vec<Point>,
    monitor_bounds: Rect,
    monitor_index: usize,
}

/// 记录拖拽轨迹，取外接矩形外扩后作为选区。
pub struct LassoTool {
    padding: u32,
    stroke_width: u32,
    drag: Option<Drag>,
}

impl LassoTool {
    /// 创建空闲状态的工具。
    pub fn new(padding: u32, stroke_width: u32) -> Self {
        Self {
            padding,
            stroke_width,
            drag: None,
        }
    }
}

fn push_point(points: &mut Vec<Point>, point: Point) {
    if points.len() < MAX_TRAIL_POINTS && points.last() != Some(&point) {
        points.push(point);
    }
}

impl SelectionTool for LassoTool {
    fn press(&mut self, point: Point, monitor: &MonitorInfo, monitor_index: usize) {
        self.drag = Some(Drag {
            points: vec![point],
            monitor_bounds: monitor.bounds,
            monitor_index,
        });
    }

    fn drag(&mut self, point: Point) {
        if let Some(drag) = self.drag.as_mut() {
            push_point(&mut drag.points, point);
        }
    }

    fn release(&mut self, point: Point) -> Result<Selection, SelectionError> {
        let mut drag = self.drag.take().ok_or(SelectionError::EmptyLasso)?;
        push_point(&mut drag.points, point);
        if is_trail_too_small(&drag.points) {
            return Err(SelectionError::EmptyBounds);
        }
        Selection::from_lasso(
            drag.points,
            self.padding,
            self.stroke_width,
            &drag.monitor_bounds,
            drag.monitor_index,
        )
    }

    fn preview(&self) -> Preview<'_> {
        match &self.drag {
            Some(d) => Preview::Lasso {
                trail: &d.points,
                bounds: if is_trail_too_small(&d.points) {
                    None
                } else {
                    lasso_bounds(
                        &d.points,
                        self.padding,
                        self.stroke_width,
                        &d.monitor_bounds,
                    )
                },
            },
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
    use lenswin_core::selection::SelectionShape;

    fn monitor(x: i32) -> MonitorInfo {
        MonitorInfo {
            bounds: Rect::new(x, 0, 1920, 1080),
            dpi: 96,
            primary: true,
        }
    }

    fn preview_bounds(tool: &LassoTool) -> Option<Rect> {
        match tool.preview() {
            Preview::Lasso { bounds, .. } => bounds,
            _ => None,
        }
    }

    #[test]
    fn release_expands_bounds_by_padding_and_half_stroke() {
        let mut tool = LassoTool::new(10, 4);
        tool.press(Point::new(100, 100), &monitor(0), 0);
        tool.drag(Point::new(200, 120));
        tool.drag(Point::new(150, 180));
        let sel = tool.release(Point::new(120, 150)).expect("选区");
        assert_eq!(sel.bounds(), Rect::new(88, 88, 124, 104));
        assert!(matches!(sel.shape(), SelectionShape::Lasso(p) if p.len() == 4));
    }

    #[test]
    fn preview_tracks_trail_and_bounds() {
        let mut tool = LassoTool::new(10, 4);
        tool.press(Point::new(100, 100), &monitor(0), 0);
        tool.drag(Point::new(200, 150));
        match tool.preview() {
            Preview::Lasso { trail, bounds } => {
                assert_eq!(trail.len(), 2);
                assert_eq!(bounds, Some(Rect::new(88, 88, 124, 74)));
            }
            other => panic!("期望套索预览，实际为 {other:?}"),
        }
    }

    #[test]
    fn consecutive_duplicate_points_are_dropped() {
        let mut tool = LassoTool::new(0, 1);
        tool.press(Point::new(5, 5), &monitor(0), 0);
        tool.drag(Point::new(5, 5));
        tool.drag(Point::new(6, 6));
        tool.drag(Point::new(6, 6));
        match tool.preview() {
            Preview::Lasso { trail, .. } => assert_eq!(trail.len(), 2),
            other => panic!("期望套索预览，实际为 {other:?}"),
        }
    }

    #[test]
    fn trail_length_is_capped() {
        let mut tool = LassoTool::new(0, 1);
        tool.press(Point::new(0, 0), &monitor(0), 0);
        let total = i32::try_from(MAX_TRAIL_POINTS).expect("上限可放入 i32") + 10;
        for i in 1..=total {
            tool.drag(Point::new(i % 1000, i / 1000));
        }
        match tool.preview() {
            Preview::Lasso { trail, .. } => assert_eq!(trail.len(), MAX_TRAIL_POINTS),
            other => panic!("期望套索预览，实际为 {other:?}"),
        }
    }

    #[test]
    fn selection_is_clipped_to_start_monitor() {
        let mut tool = LassoTool::new(10, 4);
        tool.press(Point::new(1900, 500), &monitor(0), 0);
        let sel = tool.release(Point::new(2500, 700)).expect("选区");
        assert_eq!(sel.bounds(), Rect::new(1888, 488, 32, 224));
    }

    #[test]
    fn negative_coordinate_monitor() {
        let mut tool = LassoTool::new(10, 4);
        tool.press(Point::new(-300, 100), &monitor(-1920), 2);
        let sel = tool.release(Point::new(-100, 200)).expect("选区");
        assert_eq!(sel.bounds(), Rect::new(-312, 88, 224, 124));
        assert_eq!(sel.monitor_index(), 2);
    }

    #[test]
    fn trail_outside_monitor_is_invalid() {
        let mut tool = LassoTool::new(10, 4);
        tool.press(Point::new(5000, 5000), &monitor(0), 0);
        assert_eq!(
            tool.release(Point::new(5100, 5100)).unwrap_err(),
            SelectionError::EmptyBounds
        );
    }

    #[test]
    fn single_click_without_margin_is_invalid() {
        let mut tool = LassoTool::new(0, 1);
        tool.press(Point::new(50, 50), &monitor(0), 0);
        assert!(tool.release(Point::new(50, 50)).is_err());
    }

    #[test]
    fn single_click_with_default_margin_is_invalid() {
        let mut tool = LassoTool::new(10, 4);
        tool.press(Point::new(50, 50), &monitor(0), 0);
        assert_eq!(preview_bounds(&tool), None);
        assert_eq!(
            tool.release(Point::new(50, 50)).unwrap_err(),
            SelectionError::EmptyBounds
        );
    }

    #[test]
    fn tiny_jitter_below_threshold_is_invalid() {
        let mut tool = LassoTool::new(10, 4);
        tool.press(Point::new(50, 50), &monitor(0), 0);
        tool.drag(Point::new(52, 51));
        tool.drag(Point::new(51, 53));
        assert_eq!(preview_bounds(&tool), None);
        assert_eq!(
            tool.release(Point::new(50, 52)).unwrap_err(),
            SelectionError::EmptyBounds
        );
    }

    #[test]
    fn trail_reaching_threshold_is_valid() {
        let mut tool = LassoTool::new(10, 4);
        tool.press(Point::new(50, 50), &monitor(0), 0);
        tool.drag(Point::new(
            50 + i32::try_from(MIN_TRAIL_SPAN).expect("阈值可放入 i32"),
            50,
        ));
        assert!(preview_bounds(&tool).is_some());
        let sel = tool.release(Point::new(50, 50)).expect("选区");
        assert_eq!(sel.bounds(), Rect::new(38, 38, 28, 24));
    }

    #[test]
    fn one_axis_beyond_threshold_is_valid() {
        let mut tool = LassoTool::new(10, 4);
        tool.press(Point::new(50, 50), &monitor(0), 0);
        let sel = tool.release(Point::new(150, 50)).expect("选区");
        assert_eq!(sel.bounds(), Rect::new(38, 38, 124, 24));
    }

    #[test]
    fn trail_just_below_threshold_is_invalid() {
        let mut tool = LassoTool::new(10, 4);
        tool.press(Point::new(50, 50), &monitor(0), 0);
        let below = i32::try_from(MIN_TRAIL_SPAN).expect("阈值可放入 i32") - 1;
        assert!(tool.release(Point::new(50 + below, 50 + below)).is_err());
    }

    #[test]
    fn release_without_press_is_invalid() {
        let mut tool = LassoTool::new(10, 4);
        assert_eq!(
            tool.release(Point::new(1, 1)).unwrap_err(),
            SelectionError::EmptyLasso
        );
    }
}
