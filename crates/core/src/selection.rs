//! 选区：模式、形状与构造规则。

use serde::{Deserialize, Serialize};

use crate::error::SelectionError;
use crate::geometry::{Point, Rect, lasso_bounds, rect_bounds};

/// 选区交互模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelectionMode {
    /// 矩形框选。
    Rect,
    /// 自由套索。
    #[default]
    Lasso,
}

impl SelectionMode {
    /// 全部取值，供界面生成选项。
    pub const ALL: [SelectionMode; 2] = [SelectionMode::Lasso, SelectionMode::Rect];

    /// 面向用户的中文名称。
    pub const fn label(self) -> &'static str {
        match self {
            SelectionMode::Rect => "矩形",
            SelectionMode::Lasso => "圈选",
        }
    }
}

/// 选区形状。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionShape {
    /// 矩形。
    Rect,
    /// 套索，保留原始轨迹以便后续做蒙版。
    Lasso(Vec<Point>),
}

/// 一次选区结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    shape: SelectionShape,
    bounds: Rect,
    monitor_index: usize,
}

impl Selection {
    /// 形状。
    pub fn shape(&self) -> &SelectionShape {
        &self.shape
    }

    /// 虚拟桌面物理像素坐标下的外接范围，保证非空。
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// 选区所在显示器的序号。
    pub fn monitor_index(&self) -> usize {
        self.monitor_index
    }

    /// 构造选区；范围为空或套索轨迹为空时返回错误。
    pub fn new(
        shape: SelectionShape,
        bounds: Rect,
        monitor_index: usize,
    ) -> Result<Self, SelectionError> {
        if bounds.is_empty() {
            return Err(SelectionError::EmptyBounds);
        }
        if matches!(&shape, SelectionShape::Lasso(points) if points.is_empty()) {
            return Err(SelectionError::EmptyLasso);
        }
        Ok(Self {
            shape,
            bounds,
            monitor_index,
        })
    }

    /// 由矩形拖拽的起止点构造，范围限制在所在显示器内。
    pub fn from_rect_drag(
        start: Point,
        end: Point,
        monitor: &Rect,
        monitor_index: usize,
    ) -> Result<Self, SelectionError> {
        let bounds = rect_bounds(start, end, monitor).ok_or(SelectionError::EmptyBounds)?;
        Self::new(SelectionShape::Rect, bounds, monitor_index)
    }

    /// 由套索轨迹构造：外接矩形外扩 `padding + stroke_width / 2` 后限制在显示器内。
    pub fn from_lasso(
        points: Vec<Point>,
        padding: u32,
        stroke_width: u32,
        monitor: &Rect,
        monitor_index: usize,
    ) -> Result<Self, SelectionError> {
        if points.is_empty() {
            return Err(SelectionError::EmptyLasso);
        }
        let bounds = lasso_bounds(&points, padding, stroke_width, monitor)
            .ok_or(SelectionError::EmptyBounds)?;
        Self::new(SelectionShape::Lasso(points), bounds, monitor_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor() -> Rect {
        Rect::new(0, 0, 1920, 1080)
    }

    #[test]
    fn mode_list_covers_every_variant_with_distinct_labels() {
        assert!(SelectionMode::ALL.contains(&SelectionMode::Rect));
        assert!(SelectionMode::ALL.contains(&SelectionMode::Lasso));
        assert_ne!(SelectionMode::Rect.label(), SelectionMode::Lasso.label());
    }

    #[test]
    fn new_rejects_empty_bounds() {
        let err = Selection::new(SelectionShape::Rect, Rect::new(0, 0, 0, 10), 0).unwrap_err();
        assert_eq!(err, SelectionError::EmptyBounds);
    }

    #[test]
    fn new_rejects_empty_lasso() {
        let err =
            Selection::new(SelectionShape::Lasso(vec![]), Rect::new(0, 0, 5, 5), 0).unwrap_err();
        assert_eq!(err, SelectionError::EmptyLasso);
    }

    #[test]
    fn rect_drag_is_clipped_to_monitor() {
        let sel = Selection::from_rect_drag(
            Point::new(1900, 1000),
            Point::new(2500, 1500),
            &monitor(),
            1,
        )
        .expect("框选");
        assert_eq!(sel.bounds(), Rect::new(1900, 1000, 20, 80));
        assert_eq!(sel.monitor_index(), 1);
        assert_eq!(sel.shape(), &SelectionShape::Rect);
    }

    #[test]
    fn rect_drag_zero_width_is_invalid() {
        let err = Selection::from_rect_drag(Point::new(5, 5), Point::new(5, 90), &monitor(), 0)
            .unwrap_err();
        assert_eq!(err, SelectionError::EmptyBounds);
    }

    #[test]
    fn lasso_keeps_trajectory_and_expands() {
        let pts = vec![Point::new(100, 100), Point::new(200, 150)];
        let sel = Selection::from_lasso(pts.clone(), 10, 4, &monitor(), 0).expect("套索");
        assert_eq!(sel.bounds(), Rect::new(88, 88, 124, 74));
        assert_eq!(sel.shape(), &SelectionShape::Lasso(pts));
    }

    #[test]
    fn lasso_empty_trajectory_is_invalid() {
        let err = Selection::from_lasso(vec![], 10, 4, &monitor(), 0).unwrap_err();
        assert_eq!(err, SelectionError::EmptyLasso);
    }

    #[test]
    fn lasso_fully_outside_monitor_is_invalid() {
        let pts = vec![Point::new(-500, -500), Point::new(-400, -400)];
        let err = Selection::from_lasso(pts, 10, 4, &monitor(), 0).unwrap_err();
        assert_eq!(err, SelectionError::EmptyBounds);
    }
}
