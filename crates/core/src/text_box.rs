//! 文字文本框的纯几何：旋转、本地坐标、手柄命中、缩放与换行长度，与平台无关。
//!
//! 文本框是未旋转的本地矩形 `bounds`，绕其中心顺时针旋转 `rotation` 度（屏幕坐标，y 向下）。

use crate::config::TEXT_SIZE_RANGE;
use crate::geometry::Rect;

/// 旋转手柄吸附的角度步长。
pub const ROTATION_SNAP_DEG: i32 = 15;

/// 屏幕上的浮点点。
pub type PointF = (f64, f64);

/// 四舍五入到 `i32`，超出范围时饱和，非有限值按 0 处理。
pub fn round_to_i32(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    let rounded = value.round();
    // 不允许 `as` 截断，二分找出不大于 rounded 的最大整数
    let (mut lo, mut hi) = (i64::from(i32::MIN), i64::from(i32::MAX));
    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        let probe = i32::try_from(mid).map_or(f64::INFINITY, f64::from);
        if probe <= rounded {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    i32::try_from(lo).unwrap_or(0)
}

/// 角度规范到 `0..360`。
pub fn normalize_degrees(degrees: i32) -> i32 {
    degrees.rem_euclid(360)
}

/// 角度吸附到 15° 的倍数，结果在 `0..360`。
pub fn snap_rotation(degrees: i32) -> i32 {
    let normalized = normalize_degrees(degrees);
    normalize_degrees((normalized + ROTATION_SNAP_DEG / 2) / ROTATION_SNAP_DEG * ROTATION_SNAP_DEG)
}

/// 由指针相对文本框中心的方向得到旋转角：指针离中心不足 1 像素时返回 `None`。
///
/// 横排时旋转手柄在框上方，竖排时在框右方，指针方向与手柄方向重合。
pub fn rotation_from_pointer(center: PointF, pointer: PointF, vertical: bool) -> Option<i32> {
    let (dx, dy) = (pointer.0 - center.0, pointer.1 - center.1);
    if dx.hypot(dy) < 1.0 {
        return None;
    }
    let angle = if vertical {
        dy.atan2(dx)
    } else {
        dx.atan2(-dy)
    };
    Some(normalize_degrees(round_to_i32(angle.to_degrees())))
}

/// 文本框的角。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxCorner {
    /// 左上。
    TopLeft,
    /// 右上。
    TopRight,
    /// 右下。
    BottomRight,
    /// 左下。
    BottomLeft,
}

impl BoxCorner {
    /// 全部四个角。
    pub const ALL: [BoxCorner; 4] = [
        BoxCorner::TopLeft,
        BoxCorner::TopRight,
        BoxCorner::BottomRight,
        BoxCorner::BottomLeft,
    ];

    /// 对角。
    pub const fn opposite(self) -> Self {
        match self {
            BoxCorner::TopLeft => BoxCorner::BottomRight,
            BoxCorner::TopRight => BoxCorner::BottomLeft,
            BoxCorner::BottomRight => BoxCorner::TopLeft,
            BoxCorner::BottomLeft => BoxCorner::TopRight,
        }
    }

    /// 角相对框中心的方向符号。
    const fn signs(self) -> (f64, f64) {
        match self {
            BoxCorner::TopLeft => (-1.0, -1.0),
            BoxCorner::TopRight => (1.0, -1.0),
            BoxCorner::BottomRight => (1.0, 1.0),
            BoxCorner::BottomLeft => (-1.0, 1.0),
        }
    }
}

/// 文本框的边。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxEdge {
    /// 左边。
    Left,
    /// 右边。
    Right,
    /// 上边。
    Top,
    /// 下边。
    Bottom,
}

impl BoxEdge {
    /// 拖动该边时固定不动的角：横排拖右边固定左上、拖左边固定右上；
    /// 竖排拖下边固定右上、拖上边固定右下（列从右往左排，右缘是起点）。
    pub const fn fixed_corner(self) -> BoxCorner {
        match self {
            BoxEdge::Right => BoxCorner::TopLeft,
            BoxEdge::Left | BoxEdge::Bottom => BoxCorner::TopRight,
            BoxEdge::Top => BoxCorner::BottomRight,
        }
    }
}

/// 文本框上可操作的部位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextHandle {
    /// 框内部。
    Body,
    /// 虚线框的边缘一带。
    Border,
    /// 角，等比缩放字号。
    Corner(BoxCorner),
    /// 换行方向上的边，设置换行长度。
    Edge(BoxEdge),
    /// 旋转手柄。
    Rotate,
}

/// 旋转后的文本框。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextBox {
    /// 未旋转的本地框。
    pub bounds: Rect,
    /// 顺时针旋转角度（度，`0..360`）。
    pub rotation: i32,
    /// 竖排：换行边是上下边，旋转手柄在右侧。
    pub vertical: bool,
}

fn sin_cos(rotation: i32) -> (f64, f64) {
    f64::from(normalize_degrees(rotation))
        .to_radians()
        .sin_cos()
}

impl TextBox {
    fn half(&self) -> (f64, f64) {
        (
            f64::from(self.bounds.width) / 2.0,
            f64::from(self.bounds.height) / 2.0,
        )
    }

    /// 旋转中心（屏幕坐标）。
    pub fn center(&self) -> PointF {
        let (hw, hh) = self.half();
        (f64::from(self.bounds.x) + hw, f64::from(self.bounds.y) + hh)
    }

    /// 屏幕点转为相对中心、未旋转的本地坐标。
    pub fn to_local(&self, p: PointF) -> PointF {
        let (cx, cy) = self.center();
        let (s, c) = sin_cos(self.rotation);
        let (rx, ry) = (p.0 - cx, p.1 - cy);
        (rx * c + ry * s, -rx * s + ry * c)
    }

    /// 相对中心的本地坐标转为屏幕点。
    pub fn to_screen(&self, local: PointF) -> PointF {
        let (cx, cy) = self.center();
        let (s, c) = sin_cos(self.rotation);
        (
            cx + local.0 * c - local.1 * s,
            cy + local.0 * s + local.1 * c,
        )
    }

    /// 某个角的屏幕坐标。
    pub fn corner(&self, corner: BoxCorner) -> PointF {
        let (hw, hh) = self.half();
        let (sx, sy) = corner.signs();
        self.to_screen((sx * hw, sy * hh))
    }

    /// 四个角的屏幕坐标，顺序为左上、右上、右下、左下。
    pub fn corners(&self) -> [PointF; 4] {
        BoxCorner::ALL.map(|c| self.corner(c))
    }

    /// 屏幕点是否落在旋转后的框内。
    pub fn contains(&self, p: PointF) -> bool {
        let (lx, ly) = self.to_local(p);
        let (hw, hh) = self.half();
        lx.abs() <= hw && ly.abs() <= hh
    }

    /// 旋转手柄的本地位置，距框边 `distance`。
    fn rotate_local(&self, distance: f64) -> PointF {
        let (hw, hh) = self.half();
        if self.vertical {
            (hw + distance, 0.0)
        } else {
            (0.0, -hh - distance)
        }
    }

    /// 旋转手柄的屏幕位置。
    pub fn rotate_handle(&self, distance: f64) -> PointF {
        self.to_screen(self.rotate_local(distance))
    }

    /// 换行边手柄的位置（边中点，屏幕坐标）：横排为左右边，竖排为上下边。
    pub fn edge_handles(&self) -> [(BoxEdge, PointF); 2] {
        let (hw, hh) = self.half();
        if self.vertical {
            [
                (BoxEdge::Top, self.to_screen((0.0, -hh))),
                (BoxEdge::Bottom, self.to_screen((0.0, hh))),
            ]
        } else {
            [
                (BoxEdge::Left, self.to_screen((-hw, 0.0))),
                (BoxEdge::Right, self.to_screen((hw, 0.0))),
            ]
        }
    }

    /// 命中测试：`tolerance` 为手柄与边缘的抓取半径，`rotate_distance` 为旋转手柄到框边的距离。
    /// 优先级：旋转手柄 > 角 > 边手柄 > 边缘 > 内部。
    pub fn hit(&self, p: PointF, tolerance: f64, rotate_distance: f64) -> Option<TextHandle> {
        let q = self.to_local(p);
        let near = |target: PointF| (q.0 - target.0).hypot(q.1 - target.1) <= tolerance;
        if near(self.rotate_local(rotate_distance)) {
            return Some(TextHandle::Rotate);
        }
        let (hw, hh) = self.half();
        for corner in BoxCorner::ALL {
            let (sx, sy) = corner.signs();
            if near((sx * hw, sy * hh)) {
                return Some(TextHandle::Corner(corner));
            }
        }
        for (edge, _) in self.edge_handles() {
            let (ex, ey) = match edge {
                BoxEdge::Left => (-hw, 0.0),
                BoxEdge::Right => (hw, 0.0),
                BoxEdge::Top => (0.0, -hh),
                BoxEdge::Bottom => (0.0, hh),
            };
            // 边手柄是沿边方向较长的短条
            let (along_x, along_y) = match edge {
                BoxEdge::Left | BoxEdge::Right => (tolerance, tolerance * 2.0),
                BoxEdge::Top | BoxEdge::Bottom => (tolerance * 2.0, tolerance),
            };
            if (q.0 - ex).abs() <= along_x && (q.1 - ey).abs() <= along_y {
                return Some(TextHandle::Edge(edge));
            }
        }
        let inside_outer = q.0.abs() <= hw + tolerance && q.1.abs() <= hh + tolerance;
        let inside_inner = q.0.abs() <= hw - tolerance && q.1.abs() <= hh - tolerance;
        if inside_outer && !inside_inner {
            return Some(TextHandle::Border);
        }
        (q.0.abs() <= hw && q.1.abs() <= hh).then_some(TextHandle::Body)
    }

    /// 本地矩形经旋转后的屏幕外接框；用于输入法候选窗排除区域。
    pub fn screen_bounds_of(&self, local: [PointF; 4]) -> Option<Rect> {
        let points = local.map(|l| self.to_screen(l));
        let xs = points.iter().map(|p| p.0);
        let ys = points.iter().map(|p| p.1);
        let min = |it: &mut dyn Iterator<Item = f64>| it.fold(f64::INFINITY, f64::min);
        let max = |it: &mut dyn Iterator<Item = f64>| it.fold(f64::NEG_INFINITY, f64::max);
        let (left, right) = (min(&mut xs.clone()), max(&mut xs.clone()));
        let (top, bottom) = (min(&mut ys.clone()), max(&mut ys.clone()));
        Rect::from_edges(
            i64::from(round_to_i32(left.floor())),
            i64::from(round_to_i32(top.floor())),
            i64::from(round_to_i32(right.ceil())),
            i64::from(round_to_i32(bottom.ceil())),
        )
    }
}

/// 文字默认的锚点角：横排为左上，竖排为右上（列从右往左排，右缘是起点）。
pub const fn anchor_corner(vertical: bool) -> BoxCorner {
    if vertical {
        BoxCorner::TopRight
    } else {
        BoxCorner::TopLeft
    }
}

/// 使 `corner` 角落在屏幕点 `anchor` 的本地框：给定框大小与旋转角，返回未旋转的 `bounds`。
pub fn bounds_from_anchor(
    anchor: PointF,
    corner: BoxCorner,
    size: (u32, u32),
    rotation: i32,
) -> Rect {
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    let (sx, sy) = corner.signs();
    let (off_x, off_y) = (sx * w / 2.0, sy * h / 2.0);
    let (s, c) = sin_cos(rotation);
    let cx = anchor.0 - (off_x * c - off_y * s);
    let cy = anchor.1 - (off_x * s + off_y * c);
    Rect::new(
        round_to_i32(cx - w / 2.0),
        round_to_i32(cy - h / 2.0),
        size.0,
        size.1,
    )
}

/// 文字字号的运行时上限：不超过所在显示器高度（物理像素）的 2 倍，且在配置范围内。
pub fn max_text_size(monitor_height: u32) -> u32 {
    monitor_height
        .saturating_mul(2)
        .clamp(*TEXT_SIZE_RANGE.start(), *TEXT_SIZE_RANGE.end())
}

/// 等比缩放后的字号：新字号 = 原字号 × 当前距离 / 原距离，取整并限制在 `12..=max`。
/// 原距离不足 1 像素时保持原字号（同样夹紧）。
pub fn scaled_size(size: u32, start_distance: f64, distance: f64, max: u32) -> u32 {
    let low = *TEXT_SIZE_RANGE.start();
    let high = max.clamp(low, *TEXT_SIZE_RANGE.end());
    if start_distance < 1.0 || !distance.is_finite() {
        return size.clamp(low, high);
    }
    let value = round_to_i32(f64::from(size) * distance / start_distance);
    u32::try_from(value.max(0)).map_or(low, |v| v.clamp(low, high))
}

/// 字号变化后换行长度同比缩放。
pub fn scaled_wrap(wrap: Option<u32>, old_size: u32, new_size: u32) -> Option<u32> {
    let wrap = wrap?;
    if old_size == 0 {
        return Some(wrap);
    }
    let value = u64::from(wrap) * u64::from(new_size) / u64::from(old_size);
    Some(u32::try_from(value).unwrap_or(u32::MAX).max(1))
}

/// 拖动换行边时的新换行长度（不含内边距）：以 `start` 框为参照，从固定的对边量到指针，
/// 至少一个字号；指针在对边另一侧时也取最小值。
pub fn wrap_length_from_pointer(
    start: &TextBox,
    edge: BoxEdge,
    pointer: PointF,
    padding: u32,
    size: u32,
) -> u32 {
    let (lx, ly) = start.to_local(pointer);
    let (hw, hh) = start.half();
    let total = match edge {
        BoxEdge::Right => lx + hw,
        BoxEdge::Left => hw - lx,
        BoxEdge::Bottom => ly + hh,
        BoxEdge::Top => hh - ly,
    };
    let inner = total - f64::from(padding) * 2.0;
    let value = round_to_i32(inner).max(0);
    u32::try_from(value).unwrap_or(0).max(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tb(x: i32, y: i32, w: u32, h: u32, rotation: i32, vertical: bool) -> TextBox {
        TextBox {
            bounds: Rect::new(x, y, w, h),
            rotation,
            vertical,
        }
    }

    fn near(a: PointF, b: PointF) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    #[test]
    fn rounding_saturates_and_handles_signs() {
        assert_eq!(round_to_i32(2.4), 2);
        assert_eq!(round_to_i32(2.5), 3);
        assert_eq!(round_to_i32(-2.5), -3);
        assert_eq!(round_to_i32(-0.4), 0);
        assert_eq!(round_to_i32(1e20), i32::MAX);
        assert_eq!(round_to_i32(-1e20), i32::MIN);
        assert_eq!(round_to_i32(f64::NAN), 0);
        assert_eq!(round_to_i32(f64::INFINITY), 0);
        assert_eq!(round_to_i32(123_456.0), 123_456);
    }

    #[test]
    fn degrees_normalize_and_snap_to_15() {
        assert_eq!(normalize_degrees(-90), 270);
        assert_eq!(normalize_degrees(360), 0);
        assert_eq!(normalize_degrees(725), 5);
        for (input, expected) in [
            (0, 0),
            (7, 0),
            (8, 15),
            (22, 15),
            (23, 30),
            (352, 345),
            (353, 0),
            (359, 0),
            (-5, 0),
            (-8, 345),
            (90, 90),
        ] {
            assert_eq!(snap_rotation(input), expected, "{input}");
        }
    }

    #[test]
    fn pointer_rotation_matches_handle_direction() {
        let c = (100.0, 100.0);
        // 横排：手柄在上方，指针在正上方时不旋转
        assert_eq!(rotation_from_pointer(c, (100.0, 40.0), false), Some(0));
        assert_eq!(rotation_from_pointer(c, (160.0, 100.0), false), Some(90));
        assert_eq!(rotation_from_pointer(c, (100.0, 160.0), false), Some(180));
        assert_eq!(rotation_from_pointer(c, (40.0, 100.0), false), Some(270));
        // 竖排：手柄在右侧
        assert_eq!(rotation_from_pointer(c, (160.0, 100.0), true), Some(0));
        assert_eq!(rotation_from_pointer(c, (100.0, 160.0), true), Some(90));
        assert_eq!(rotation_from_pointer(c, (40.0, 100.0), true), Some(180));
        assert_eq!(rotation_from_pointer(c, (100.0, 40.0), true), Some(270));
        assert_eq!(rotation_from_pointer(c, (100.5, 100.5), false), None);
        assert_eq!(rotation_from_pointer(c, (160.0, 40.0), false), Some(45));
    }

    #[test]
    fn local_and_screen_coordinates_round_trip() {
        for rotation in [0, 15, 45, 90, 137, 270, 359] {
            let b = tb(10, 20, 80, 40, rotation, false);
            for p in [(0.0, 0.0), (33.5, -12.25), (-40.0, 20.0)] {
                let back = b.to_local(b.to_screen(p));
                assert!(near(back, p), "{rotation}: {back:?}");
            }
        }
        let b = tb(10, 20, 80, 40, 0, false);
        assert!(near(b.center(), (50.0, 40.0)));
        assert!(near(b.to_screen((0.0, 0.0)), (50.0, 40.0)));
    }

    #[test]
    fn corners_rotate_about_the_center() {
        let b = tb(0, 0, 100, 50, 0, false);
        let [tl, tr, br, bl] = b.corners();
        assert!(near(tl, (0.0, 0.0)) && near(tr, (100.0, 0.0)));
        assert!(near(br, (100.0, 50.0)) && near(bl, (0.0, 50.0)));
        // 顺时针 90°：左上角转到右上方向
        let r = tb(0, 0, 100, 50, 90, false);
        assert!(near(r.corner(BoxCorner::TopLeft), (75.0, -25.0)));
        assert!(near(r.corner(BoxCorner::TopRight), (75.0, 75.0)));
        assert!(near(r.center(), (50.0, 25.0)));
    }

    #[test]
    fn contains_follows_rotation() {
        let b = tb(0, 0, 100, 20, 90, false);
        // 旋转后变成竖条，中心 (50, 10)
        assert!(b.contains((50.0, 55.0)));
        assert!(b.contains((50.0, -35.0)));
        assert!(!b.contains((95.0, 10.0)));
        assert!(!b.contains((50.0, 70.0)));
        let flat = tb(0, 0, 100, 20, 0, false);
        assert!(flat.contains((95.0, 10.0)) && !flat.contains((50.0, 55.0)));
    }

    #[test]
    fn hit_covers_body_corners_edges_border_and_rotate() {
        let b = tb(100, 100, 200, 60, 0, false);
        let (tol, dist) = (6.0, 24.0);
        assert_eq!(b.hit((200.0, 130.0), tol, dist), Some(TextHandle::Body));
        assert_eq!(
            b.hit((100.0, 100.0), tol, dist),
            Some(TextHandle::Corner(BoxCorner::TopLeft))
        );
        assert_eq!(
            b.hit((301.0, 161.0), tol, dist),
            Some(TextHandle::Corner(BoxCorner::BottomRight))
        );
        assert_eq!(
            b.hit((100.0, 130.0), tol, dist),
            Some(TextHandle::Edge(BoxEdge::Left))
        );
        assert_eq!(
            b.hit((300.0, 125.0), tol, dist),
            Some(TextHandle::Edge(BoxEdge::Right))
        );
        // 上边中点不是换行手柄，只属于边缘
        assert_eq!(b.hit((200.0, 100.0), tol, dist), Some(TextHandle::Border));
        assert_eq!(b.hit((200.0, 104.0), tol, dist), Some(TextHandle::Border));
        assert_eq!(
            b.hit((200.0, 100.0 - 24.0), tol, dist),
            Some(TextHandle::Rotate)
        );
        assert_eq!(b.hit((200.0, 20.0), tol, dist), None);
        assert_eq!(b.hit((350.0, 130.0), tol, dist), None);
    }

    #[test]
    fn hit_works_in_rotated_and_vertical_frames() {
        let b = tb(100, 100, 200, 60, 90, false);
        let (tol, dist) = (6.0, 24.0);
        // 顺时针 90°：旋转手柄在中心右侧 30 + 24 处
        let center = b.center();
        assert_eq!(
            b.hit((center.0 + 30.0 + 24.0, center.1), tol, dist),
            Some(TextHandle::Rotate)
        );
        assert_eq!(b.hit(center, tol, dist), Some(TextHandle::Body));
        let corner = b.corner(BoxCorner::TopRight);
        assert_eq!(
            b.hit(corner, tol, dist),
            Some(TextHandle::Corner(BoxCorner::TopRight))
        );

        let v = tb(100, 100, 60, 200, 0, true);
        assert_eq!(
            v.hit((130.0, 100.0), tol, dist),
            Some(TextHandle::Edge(BoxEdge::Top))
        );
        assert_eq!(
            v.hit((130.0, 300.0), tol, dist),
            Some(TextHandle::Edge(BoxEdge::Bottom))
        );
        // 竖排的左右边不是换行边
        assert_eq!(v.hit((100.0, 200.0), tol, dist), Some(TextHandle::Border));
        assert_eq!(
            v.hit((160.0 + 24.0, 200.0), tol, dist),
            Some(TextHandle::Rotate)
        );
    }

    #[test]
    fn handle_positions_match_hit_testing() {
        for (rotation, vertical) in [(0, false), (30, false), (200, true), (90, true)] {
            let b = tb(50, 60, 140, 70, rotation, vertical);
            for (edge, pos) in b.edge_handles() {
                assert_eq!(b.hit(pos, 6.0, 24.0), Some(TextHandle::Edge(edge)));
            }
            assert_eq!(
                b.hit(b.rotate_handle(24.0), 6.0, 24.0),
                Some(TextHandle::Rotate)
            );
            assert_eq!(b.hit(b.center(), 6.0, 24.0), Some(TextHandle::Body));
        }
    }

    #[test]
    fn anchor_keeps_the_chosen_corner_fixed() {
        for rotation in [0, 20, 90, 181, 300] {
            for corner in BoxCorner::ALL {
                let first = bounds_from_anchor((200.0, 150.0), corner, (120, 40), rotation);
                let b = TextBox {
                    bounds: first,
                    rotation,
                    vertical: false,
                };
                let got = b.corner(corner);
                assert!(
                    (got.0 - 200.0).abs() <= 1.0 && (got.1 - 150.0).abs() <= 1.0,
                    "{rotation} {corner:?}: {got:?}"
                );
                // 换成别的大小后同一个角仍在锚点
                let grown = bounds_from_anchor((200.0, 150.0), corner, (200, 90), rotation);
                let g = TextBox {
                    bounds: grown,
                    rotation,
                    vertical: false,
                }
                .corner(corner);
                assert!((g.0 - 200.0).abs() <= 1.0 && (g.1 - 150.0).abs() <= 1.0);
            }
        }
        // 不旋转时左上角锚点就是 bounds 的左上角
        assert_eq!(
            bounds_from_anchor((10.0, 20.0), BoxCorner::TopLeft, (30, 40), 0),
            Rect::new(10, 20, 30, 40)
        );
        assert_eq!(
            bounds_from_anchor((40.0, 20.0), BoxCorner::TopRight, (30, 40), 0),
            Rect::new(10, 20, 30, 40)
        );
    }

    #[test]
    fn anchor_corner_depends_on_direction() {
        assert_eq!(anchor_corner(false), BoxCorner::TopLeft);
        assert_eq!(anchor_corner(true), BoxCorner::TopRight);
        for c in BoxCorner::ALL {
            assert_eq!(c.opposite().opposite(), c);
            assert_ne!(c.opposite(), c);
        }
    }

    #[test]
    fn scaling_size_is_proportional_and_clamped() {
        assert_eq!(scaled_size(24, 100.0, 200.0, 96), 48);
        assert_eq!(scaled_size(24, 100.0, 50.0, 96), 12);
        assert_eq!(scaled_size(24, 100.0, 10.0, 96), 12);
        assert_eq!(scaled_size(24, 100.0, 1000.0, 96), 96);
        assert_eq!(scaled_size(24, 100.0, 125.0, 96), 30);
        assert_eq!(scaled_size(24, 0.0, 125.0, 96), 24);
        assert_eq!(scaled_size(24, 100.0, f64::NAN, 96), 24);
        assert_eq!(scaled_size(200, 100.0, 100.0, 96), 96);
        assert_eq!(scaled_size(24, 100.0, -50.0, 96), 12);
        // 上限放开后可以放得很大，但不超过传入的上限和配置上限
        assert_eq!(scaled_size(24, 100.0, 10_000.0, 2160), 2160);
        assert_eq!(scaled_size(24, 1.0, 1e9, 1_000_000), 16_384);
        // 上限低于下限时仍不低于 12
        assert_eq!(scaled_size(24, 100.0, 200.0, 5), 12);
    }

    #[test]
    fn max_text_size_is_twice_the_monitor_height_within_the_range() {
        assert_eq!(max_text_size(1080), 2160);
        assert_eq!(max_text_size(2160), 4320);
        assert_eq!(max_text_size(0), 12);
        assert_eq!(max_text_size(3), 12);
        assert_eq!(max_text_size(6), 12);
        assert_eq!(max_text_size(7), 14);
        assert_eq!(max_text_size(8192), 16_384);
        assert_eq!(max_text_size(100_000), 16_384);
        assert_eq!(max_text_size(u32::MAX), 16_384);
    }

    #[test]
    fn wrap_scales_with_size() {
        assert_eq!(scaled_wrap(None, 24, 48), None);
        assert_eq!(scaled_wrap(Some(200), 24, 48), Some(400));
        assert_eq!(scaled_wrap(Some(200), 24, 12), Some(100));
        assert_eq!(scaled_wrap(Some(1), 24, 12), Some(1));
        assert_eq!(scaled_wrap(Some(50), 0, 12), Some(50));
    }

    #[test]
    fn wrap_length_is_measured_from_the_fixed_edge() {
        let b = tb(100, 100, 200, 60, 0, false);
        // 右边拖到 x=400：从左边 100 量起共 300，去掉内边距 2×10
        assert_eq!(
            wrap_length_from_pointer(&b, BoxEdge::Right, (400.0, 130.0), 10, 24),
            280
        );
        assert_eq!(
            wrap_length_from_pointer(&b, BoxEdge::Left, (50.0, 130.0), 0, 24),
            250
        );
        // 过了对边：取一个字号
        assert_eq!(
            wrap_length_from_pointer(&b, BoxEdge::Right, (90.0, 130.0), 0, 24),
            24
        );
        // 旋转 90° 后右边朝下
        let r = tb(100, 100, 200, 60, 90, false);
        let c = r.center();
        assert_eq!(
            wrap_length_from_pointer(&r, BoxEdge::Right, (c.0, c.1 + 150.0), 0, 24),
            250
        );
        // 竖排：下边拖动量的是高度
        let v = tb(100, 100, 60, 200, 0, true);
        assert_eq!(
            wrap_length_from_pointer(&v, BoxEdge::Bottom, (130.0, 400.0), 0, 24),
            300
        );
        assert_eq!(
            wrap_length_from_pointer(&v, BoxEdge::Top, (130.0, 50.0), 0, 24),
            250
        );
    }

    #[test]
    fn screen_bounds_of_local_rect_is_axis_aligned() {
        let b = tb(0, 0, 100, 50, 90, false);
        let local = [(-1.0, -10.0), (1.0, -10.0), (1.0, 10.0), (-1.0, 10.0)];
        let r = b.screen_bounds_of(local).expect("外接框");
        // 竖条旋转 90° 后变成横条，围绕中心 (50, 25)
        assert_eq!(r, Rect::new(40, 24, 20, 2));
        let flat = tb(0, 0, 100, 50, 0, false);
        assert_eq!(flat.screen_bounds_of(local), Some(Rect::new(49, 15, 2, 20)));
    }
}
