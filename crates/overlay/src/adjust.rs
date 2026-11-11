//! 选区确定后的二次调整：手柄命中、拖拽改形与键盘微调，纯几何，不接触 Win32。

use lenswin_core::geometry::{Point, Rect};

/// 选区上可抓取的部位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    /// 上边。
    N,
    /// 下边。
    S,
    /// 右边。
    E,
    /// 左边。
    W,
    /// 右上角。
    NE,
    /// 左上角。
    NW,
    /// 右下角。
    SE,
    /// 左下角。
    SW,
    /// 选区内部，整体移动。
    Body,
}

/// 键盘微调方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// 向左。
    Left,
    /// 向右。
    Right,
    /// 向上。
    Up,
    /// 向下。
    Down,
}

impl Direction {
    /// 单位位移。
    pub const fn delta(self) -> (i32, i32) {
        match self {
            Direction::Left => (-1, 0),
            Direction::Right => (1, 0),
            Direction::Up => (0, -1),
            Direction::Down => (0, 1),
        }
    }

    /// 该方向一侧的边。
    const fn edge(self) -> Handle {
        match self {
            Direction::Left => Handle::W,
            Direction::Right => Handle::E,
            Direction::Up => Handle::N,
            Direction::Down => Handle::S,
        }
    }
}

/// 键盘微调方式，由修饰键决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeMode {
    /// 整体移动 1 像素。
    Move,
    /// 该方向的边向外扩 1 像素。
    Grow,
    /// 该方向的边向内收 1 像素。
    Shrink,
}

/// 命中测试；`tolerance` 是边和角的抓取半径。点在选区外且不在抓取范围内时返回 `None`。
pub fn hit_test(rect: &Rect, p: Point, tolerance: u32) -> Option<Handle> {
    let t = i64::from(tolerance);
    let (x, y) = (i64::from(p.x), i64::from(p.y));
    let (left, top, right, bottom) = edges(rect);
    if x < left - t || x > right + t || y < top - t || y > bottom + t {
        return None;
    }
    let near_left = (x - left).abs() <= t;
    let near_right = (x - right).abs() <= t;
    let near_top = (y - top).abs() <= t;
    let near_bottom = (y - bottom).abs() <= t;
    let handle = match (near_left, near_right, near_top, near_bottom) {
        (true, _, true, _) => Handle::NW,
        (_, true, true, _) => Handle::NE,
        (true, _, _, true) => Handle::SW,
        (_, true, _, true) => Handle::SE,
        (true, _, _, _) => Handle::W,
        (_, true, _, _) => Handle::E,
        (_, _, true, _) => Handle::N,
        (_, _, _, true) => Handle::S,
        _ if rect.contains(p) => Handle::Body,
        _ => return None,
    };
    Some(handle)
}

/// 从抓取时的选区 `start` 出发，按位移 `(dx, dy)` 拖动 `handle`，结果限制在 `monitor` 内。
///
/// 拖过对边时自动翻转；结果为空时返回 `None`，调用方保留原选区。
pub fn drag(start: &Rect, handle: Handle, dx: i64, dy: i64, monitor: &Rect) -> Option<Rect> {
    if handle == Handle::Body {
        return translate(start, dx, dy, monitor);
    }
    let (mut left, mut top, mut right, mut bottom) = edges(start);
    match handle {
        Handle::N => top += dy,
        Handle::S => bottom += dy,
        Handle::E => right += dx,
        Handle::W => left += dx,
        Handle::NE => {
            top += dy;
            right += dx;
        }
        Handle::NW => {
            top += dy;
            left += dx;
        }
        Handle::SE => {
            bottom += dy;
            right += dx;
        }
        Handle::SW => {
            bottom += dy;
            left += dx;
        }
        Handle::Body => {}
    }
    let rect = Rect::from_edges(
        left.min(right),
        top.min(bottom),
        left.max(right),
        top.max(bottom),
    )?;
    rect.intersect(monitor)
}

/// 键盘微调一步；结果无效（如收缩到 0）时返回 `None`。
pub fn nudge(rect: &Rect, direction: Direction, mode: NudgeMode, monitor: &Rect) -> Option<Rect> {
    let (dx, dy) = direction.delta();
    let (dx, dy) = (i64::from(dx), i64::from(dy));
    match mode {
        NudgeMode::Move => translate(rect, dx, dy, monitor),
        NudgeMode::Grow => drag(rect, direction.edge(), dx, dy, monitor),
        NudgeMode::Shrink => {
            if (dx != 0 && rect.width <= 1) || (dy != 0 && rect.height <= 1) {
                return None;
            }
            drag(rect, direction.edge(), -dx, -dy, monitor)
        }
    }
}

/// 整体平移，碰到显示器边缘时停住，尺寸不变。
fn translate(rect: &Rect, dx: i64, dy: i64, monitor: &Rect) -> Option<Rect> {
    let (left, top, right, bottom) = edges(rect);
    let (m_left, m_top, m_right, m_bottom) = edges(monitor);
    let dx = dx.clamp(m_left - left, (m_right - right).max(m_left - left));
    let dy = dy.clamp(m_top - top, (m_bottom - bottom).max(m_top - top));
    Rect::from_edges(left + dx, top + dy, right + dx, bottom + dy)?.intersect(monitor)
}

/// 八个手柄的中心点，供绘制使用。
pub fn handle_points(rect: &Rect) -> [Point; 8] {
    let (left, top, right, bottom) = edges(rect);
    let mid_x = left + (right - left) / 2;
    let mid_y = top + (bottom - top) / 2;
    let p = |x: i64, y: i64| {
        Point::new(
            i32::try_from(x).unwrap_or(i32::MAX),
            i32::try_from(y).unwrap_or(i32::MAX),
        )
    };
    [
        p(left, top),
        p(mid_x, top),
        p(right, top),
        p(right, mid_y),
        p(right, bottom),
        p(mid_x, bottom),
        p(left, bottom),
        p(left, mid_y),
    ]
}

fn edges(r: &Rect) -> (i64, i64, i64, i64) {
    (i64::from(r.x), i64::from(r.y), r.right(), r.bottom())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: Rect = Rect::new(0, 0, 1920, 1080);

    #[test]
    fn hit_test_finds_corners_edges_and_body() {
        let r = Rect::new(100, 100, 200, 100);
        assert_eq!(hit_test(&r, Point::new(100, 100), 5), Some(Handle::NW));
        assert_eq!(hit_test(&r, Point::new(303, 98), 5), Some(Handle::NE));
        assert_eq!(hit_test(&r, Point::new(97, 200), 5), Some(Handle::SW));
        assert_eq!(hit_test(&r, Point::new(300, 200), 5), Some(Handle::SE));
        assert_eq!(hit_test(&r, Point::new(200, 102), 5), Some(Handle::N));
        assert_eq!(hit_test(&r, Point::new(200, 199), 5), Some(Handle::S));
        assert_eq!(hit_test(&r, Point::new(102, 150), 5), Some(Handle::W));
        assert_eq!(hit_test(&r, Point::new(299, 150), 5), Some(Handle::E));
        assert_eq!(hit_test(&r, Point::new(200, 150), 5), Some(Handle::Body));
        assert_eq!(hit_test(&r, Point::new(50, 150), 5), None);
        assert_eq!(hit_test(&r, Point::new(200, 206), 5), None);
    }

    #[test]
    fn drag_corner_and_flip_past_opposite_edge() {
        let r = Rect::new(100, 100, 200, 100);
        assert_eq!(
            drag(&r, Handle::SE, 50, 20, &MONITOR),
            Some(Rect::new(100, 100, 250, 120))
        );
        // 左边拖过右边后翻转
        assert_eq!(
            drag(&r, Handle::W, 300, 0, &MONITOR),
            Some(Rect::new(300, 100, 100, 100))
        );
    }

    #[test]
    fn drag_is_clipped_to_monitor_and_rejects_empty() {
        let r = Rect::new(100, 100, 200, 100);
        assert_eq!(
            drag(&r, Handle::NW, -500, -500, &MONITOR),
            Some(Rect::new(0, 0, 300, 200))
        );
        assert_eq!(drag(&r, Handle::E, -200, 0, &MONITOR), None);
    }

    #[test]
    fn body_drag_stops_at_monitor_edges() {
        let r = Rect::new(100, 100, 200, 100);
        assert_eq!(
            drag(&r, Handle::Body, -1000, 5000, &MONITOR),
            Some(Rect::new(0, 980, 200, 100))
        );
        let other = Rect::new(1920, 0, 1920, 1080);
        let r2 = Rect::new(2000, 10, 50, 50);
        assert_eq!(
            drag(&r2, Handle::Body, -500, 0, &other),
            Some(Rect::new(1920, 10, 50, 50))
        );
    }

    #[test]
    fn nudge_moves_grows_and_shrinks() {
        let r = Rect::new(10, 10, 20, 20);
        assert_eq!(
            nudge(&r, Direction::Right, NudgeMode::Move, &MONITOR),
            Some(Rect::new(11, 10, 20, 20))
        );
        assert_eq!(
            nudge(&r, Direction::Left, NudgeMode::Grow, &MONITOR),
            Some(Rect::new(9, 10, 21, 20))
        );
        assert_eq!(
            nudge(&r, Direction::Down, NudgeMode::Shrink, &MONITOR),
            Some(Rect::new(10, 10, 20, 19))
        );
        assert_eq!(
            nudge(&r, Direction::Up, NudgeMode::Shrink, &MONITOR),
            Some(Rect::new(10, 11, 20, 19))
        );
    }

    #[test]
    fn nudge_respects_limits() {
        let edge = Rect::new(0, 0, 20, 20);
        assert_eq!(
            nudge(&edge, Direction::Left, NudgeMode::Move, &MONITOR),
            Some(edge)
        );
        assert_eq!(
            nudge(&edge, Direction::Up, NudgeMode::Grow, &MONITOR),
            Some(edge)
        );
        let thin = Rect::new(5, 5, 1, 10);
        assert_eq!(
            nudge(&thin, Direction::Right, NudgeMode::Shrink, &MONITOR),
            None
        );
    }

    #[test]
    fn handle_points_cover_corners_and_midpoints() {
        let pts = handle_points(&Rect::new(0, 0, 10, 20));
        assert_eq!(pts.first(), Some(&Point::new(0, 0)));
        assert_eq!(pts.get(2), Some(&Point::new(10, 0)));
        assert_eq!(pts.get(4), Some(&Point::new(10, 20)));
        assert_eq!(pts.get(7), Some(&Point::new(0, 10)));
    }
}
