//! 显示器信息，由平台层枚举、覆盖层等上层消费。

use crate::geometry::{Point, Rect};

/// 单个显示器，坐标为虚拟桌面物理像素。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorInfo {
    /// 显示器在虚拟桌面上的范围，可能出现负坐标。
    pub bounds: Rect,
    /// 有效 DPI，100% 缩放为 96。
    pub dpi: u32,
    /// 是否为主显示器。
    pub primary: bool,
}

/// 所有显示器的并集外接矩形；列表为空或结果越界时返回 `None`。
pub fn virtual_bounds(monitors: &[MonitorInfo]) -> Option<Rect> {
    let first = monitors.first()?;
    let mut left = i64::from(first.bounds.x);
    let mut top = i64::from(first.bounds.y);
    let mut right = first.bounds.right();
    let mut bottom = first.bounds.bottom();
    for m in monitors {
        left = left.min(i64::from(m.bounds.x));
        top = top.min(i64::from(m.bounds.y));
        right = right.max(m.bounds.right());
        bottom = bottom.max(m.bounds.bottom());
    }
    Rect::from_edges(left, top, right, bottom)
}

/// 包含该点的显示器序号。
pub fn index_at(monitors: &[MonitorInfo], point: Point) -> Option<usize> {
    monitors.iter().position(|m| m.bounds.contains(point))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(x: i32, y: i32, w: u32, h: u32, primary: bool) -> MonitorInfo {
        MonitorInfo {
            bounds: Rect::new(x, y, w, h),
            dpi: 96,
            primary,
        }
    }

    #[test]
    fn virtual_bounds_spans_negative_origin() {
        let list = [
            mon(0, 0, 1920, 1080, true),
            mon(-1280, 100, 1280, 1024, false),
        ];
        assert_eq!(virtual_bounds(&list), Some(Rect::new(-1280, 0, 3200, 1124)));
    }

    #[test]
    fn virtual_bounds_empty_is_none() {
        assert_eq!(virtual_bounds(&[]), None);
    }

    #[test]
    fn index_at_finds_monitor() {
        let list = [
            mon(0, 0, 1920, 1080, true),
            mon(-1280, 0, 1280, 1024, false),
        ];
        assert_eq!(index_at(&list, Point::new(10, 10)), Some(0));
        assert_eq!(index_at(&list, Point::new(-1, 5)), Some(1));
        assert_eq!(index_at(&list, Point::new(5000, 5)), None);
        assert_eq!(index_at(&list, Point::new(1920, 5)), None);
    }
}
