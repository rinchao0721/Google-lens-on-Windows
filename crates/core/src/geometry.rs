//! 几何基础类型。坐标一律为虚拟桌面物理像素，允许负值。

/// 虚拟桌面上的点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Point {
    /// 横坐标。
    pub x: i32,
    /// 纵坐标。
    pub y: i32,
}

impl Point {
    /// 构造一个点。
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// 轴对齐矩形，左上角加宽高；右、下边界不含。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    /// 左边界。
    pub x: i32,
    /// 上边界。
    pub y: i32,
    /// 宽度。
    pub width: u32,
    /// 高度。
    pub height: u32,
}

impl Rect {
    /// 构造矩形。
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// 由四条边构造；右小于左、下小于上，或结果超出 `i32`/`u32` 范围时返回 `None`。
    pub fn from_edges(left: i64, top: i64, right: i64, bottom: i64) -> Option<Self> {
        let width = right.checked_sub(left)?;
        let height = bottom.checked_sub(top)?;
        Some(Self {
            x: i32::try_from(left).ok()?,
            y: i32::try_from(top).ok()?,
            width: u32::try_from(width).ok()?,
            height: u32::try_from(height).ok()?,
        })
    }

    /// 宽或高为 0。
    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// 右边界（不含）。
    pub fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    /// 下边界（不含）。
    pub fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    /// 点是否落在矩形内。
    pub fn contains(&self, p: Point) -> bool {
        i64::from(p.x) >= i64::from(self.x)
            && i64::from(p.x) < self.right()
            && i64::from(p.y) >= i64::from(self.y)
            && i64::from(p.y) < self.bottom()
    }

    /// 与另一个矩形求交；无交集或交集为空时返回 `None`。
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let left = i64::from(self.x).max(i64::from(other.x));
        let top = i64::from(self.y).max(i64::from(other.y));
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        Rect::from_edges(left, top, right, bottom).filter(|r| !r.is_empty())
    }

    /// 四周各外扩 `amount` 像素；结果超出坐标范围时返回 `None`。
    pub fn expand(&self, amount: u32) -> Option<Rect> {
        let a = i64::from(amount);
        Rect::from_edges(
            i64::from(self.x) - a,
            i64::from(self.y) - a,
            self.right() + a,
            self.bottom() + a,
        )
    }

    /// 点集的外接矩形，可能宽或高为 0；点集为空时返回 `None`。
    ///
    /// 采用半开区间：不包含最大 x、最大 y 所在的那一行和那一列像素。
    pub fn bounding(points: &[Point]) -> Option<Rect> {
        let first = points.first()?;
        let (mut min_x, mut max_x) = (first.x, first.x);
        let (mut min_y, mut max_y) = (first.y, first.y);
        for p in points {
            min_x = min_x.min(p.x);
            max_x = max_x.max(p.x);
            min_y = min_y.min(p.y);
            max_y = max_y.max(p.y);
        }
        Rect::from_edges(
            i64::from(min_x),
            i64::from(min_y),
            i64::from(max_x),
            i64::from(max_y),
        )
    }
}

/// 套索选区的有效范围：外接矩形外扩 `padding + stroke_width / 2`，再与显示器求交。
///
/// 轨迹为空或结果为空矩形时返回 `None`。
pub fn lasso_bounds(
    points: &[Point],
    padding: u32,
    stroke_width: u32,
    monitor: &Rect,
) -> Option<Rect> {
    let margin = padding.checked_add(stroke_width / 2)?;
    Rect::bounding(points)?.expand(margin)?.intersect(monitor)
}

/// 矩形框选的有效范围：两个对角点的外接矩形与显示器求交。
pub fn rect_bounds(start: Point, end: Point, monitor: &Rect) -> Option<Rect> {
    Rect::bounding(&[start, end])?.intersect(monitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor() -> Rect {
        Rect::new(0, 0, 1920, 1080)
    }

    #[test]
    fn bounding_of_points() {
        let pts = [Point::new(10, 20), Point::new(50, 5), Point::new(30, 90)];
        assert_eq!(Rect::bounding(&pts), Some(Rect::new(10, 5, 40, 85)));
    }

    #[test]
    fn bounding_empty_is_none() {
        assert_eq!(Rect::bounding(&[]), None);
    }

    #[test]
    fn bounding_single_point_is_zero_size() {
        let r = Rect::bounding(&[Point::new(3, 4)]).expect("单点应有外接矩形");
        assert!(r.is_empty());
    }

    #[test]
    fn bounding_negative_coordinates() {
        let pts = [Point::new(-100, -50), Point::new(-20, 10)];
        assert_eq!(Rect::bounding(&pts), Some(Rect::new(-100, -50, 80, 60)));
    }

    #[test]
    fn bounding_extreme_coordinates_do_not_overflow() {
        let pts = [
            Point::new(i32::MIN, i32::MIN),
            Point::new(i32::MAX, i32::MAX),
        ];
        let r = Rect::bounding(&pts).expect("宽度 u32::MAX 可表示");
        assert_eq!(r.width, u32::MAX);
        assert_eq!(r.height, u32::MAX);
    }

    #[test]
    fn expand_adds_margin_on_all_sides() {
        let r = Rect::new(10, 10, 20, 20).expand(5);
        assert_eq!(r, Some(Rect::new(5, 5, 30, 30)));
    }

    #[test]
    fn expand_overflow_is_none() {
        assert_eq!(Rect::new(i32::MIN, 0, 10, 10).expand(1), None);
    }

    #[test]
    fn intersect_overlap_and_disjoint() {
        let a = Rect::new(0, 0, 100, 100);
        assert_eq!(
            a.intersect(&Rect::new(50, 50, 100, 100)),
            Some(Rect::new(50, 50, 50, 50))
        );
        assert_eq!(a.intersect(&Rect::new(100, 0, 10, 10)), None);
        assert_eq!(a.intersect(&Rect::new(200, 200, 10, 10)), None);
    }

    #[test]
    fn intersect_negative_monitor() {
        let left_monitor = Rect::new(-1920, 0, 1920, 1080);
        let sel = Rect::new(-100, 100, 300, 50);
        assert_eq!(
            sel.intersect(&left_monitor),
            Some(Rect::new(-100, 100, 100, 50))
        );
    }

    #[test]
    fn contains_edges_are_half_open() {
        let r = Rect::new(0, 0, 10, 10);
        assert!(r.contains(Point::new(0, 0)));
        assert!(r.contains(Point::new(9, 9)));
        assert!(!r.contains(Point::new(10, 5)));
        assert!(!r.contains(Point::new(-1, 5)));
    }

    #[test]
    fn lasso_adds_padding_and_half_stroke() {
        let pts = [Point::new(100, 100), Point::new(200, 150)];
        let r = lasso_bounds(&pts, 10, 4, &monitor());
        assert_eq!(r, Some(Rect::new(88, 88, 124, 74)));
    }

    #[test]
    fn lasso_clipped_to_monitor() {
        let pts = [Point::new(2, 2), Point::new(50, 50)];
        let r = lasso_bounds(&pts, 10, 4, &monitor());
        assert_eq!(r, Some(Rect::new(0, 0, 62, 62)));
    }

    #[test]
    fn lasso_outside_monitor_is_none() {
        let pts = [Point::new(5000, 5000), Point::new(5100, 5100)];
        assert_eq!(lasso_bounds(&pts, 10, 4, &monitor()), None);
    }

    #[test]
    fn lasso_single_point_without_margin_is_none() {
        assert_eq!(lasso_bounds(&[Point::new(5, 5)], 0, 0, &monitor()), None);
        assert_eq!(lasso_bounds(&[Point::new(5, 5)], 0, 1, &monitor()), None);
    }

    #[test]
    fn lasso_empty_points_is_none() {
        assert_eq!(lasso_bounds(&[], 10, 4, &monitor()), None);
    }

    #[test]
    fn rect_bounds_any_drag_direction() {
        let m = monitor();
        let expect = Some(Rect::new(10, 20, 30, 40));
        assert_eq!(
            rect_bounds(Point::new(10, 20), Point::new(40, 60), &m),
            expect
        );
        assert_eq!(
            rect_bounds(Point::new(40, 60), Point::new(10, 20), &m),
            expect
        );
    }

    #[test]
    fn rect_bounds_zero_size_is_none() {
        let m = monitor();
        assert_eq!(
            rect_bounds(Point::new(10, 10), Point::new(10, 50), &m),
            None
        );
    }

    #[test]
    fn from_edges_rejects_inverted() {
        assert_eq!(Rect::from_edges(10, 0, 5, 10), None);
    }
}
