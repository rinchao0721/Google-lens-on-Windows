//! 套索轨迹平滑：抽稀过密的点，再用中点二次贝塞尔连成光滑曲线，与 Win32 无关。

/// 平面上的浮点点（窗口坐标）。
pub type PointF = (f32, f32);

/// 平滑路径中的一段。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    /// 以 `control` 为控制点画到 `end` 的二次贝塞尔曲线。
    Quad {
        /// 控制点。
        control: PointF,
        /// 终点。
        end: PointF,
    },
    /// 直线画到 `end`。
    Line {
        /// 终点。
        end: PointF,
    },
}

fn distance(a: PointF, b: PointF) -> f32 {
    (b.0 - a.0).hypot(b.1 - a.1)
}

fn midpoint(a: PointF, b: PointF) -> PointF {
    ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
}

/// 去掉与上一个保留点距离小于 `min_gap` 的点，首尾两点始终保留。
///
/// 鼠标坐标落在整数像素上，慢速拖动时相邻点只差一两个像素，直接连线会出现锯齿。
pub fn thin(points: &[PointF], min_gap: f32) -> Vec<PointF> {
    let gap = if min_gap.is_finite() {
        min_gap.max(0.0)
    } else {
        0.0
    };
    let mut out: Vec<PointF> = Vec::with_capacity(points.len());
    for p in points {
        match out.last() {
            Some(last) if distance(*last, *p) < gap => {}
            _ => out.push(*p),
        }
    }
    if let (Some(&last), Some(&kept)) = (points.last(), out.last())
        && last != kept
    {
        // 末点离上一个保留点再近也要留下，保证曲线停在笔尖
        out.push(last);
    }
    out
}

/// 把折线转为光滑路径：以各中间点为控制点，相邻两点的中点为分段终点，最后直线连到末点。
///
/// 返回起点和分段；少于两个点时返回 `None`。
pub fn smooth(points: &[PointF]) -> Option<(PointF, Vec<Segment>)> {
    let [first, .., last] = points else {
        return None;
    };
    let mut segments = Vec::with_capacity(points.len());
    for pair in points.windows(2).skip(1) {
        if let [control, next] = pair {
            segments.push(Segment::Quad {
                control: *control,
                end: midpoint(*control, *next),
            });
        }
    }
    segments.push(Segment::Line { end: *last });
    Some((*first, segments))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thin_drops_close_points_but_keeps_ends() {
        let points = [(0.0, 0.0), (1.0, 0.0), (5.0, 0.0), (6.0, 0.0), (6.5, 0.0)];
        assert_eq!(thin(&points, 3.0), vec![(0.0, 0.0), (5.0, 0.0), (6.5, 0.0)]);
    }

    #[test]
    fn thin_keeps_everything_with_zero_or_bad_gap() {
        let points = [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)];
        assert_eq!(thin(&points, 0.0), points.to_vec());
        assert_eq!(thin(&points, f32::NAN), points.to_vec());
        assert_eq!(thin(&points, -4.0), points.to_vec());
    }

    #[test]
    fn thin_handles_empty_and_single_point() {
        assert!(thin(&[], 2.0).is_empty());
        assert_eq!(thin(&[(3.0, 4.0)], 2.0), vec![(3.0, 4.0)]);
        assert_eq!(
            thin(&[(0.0, 0.0), (0.5, 0.0)], 2.0),
            vec![(0.0, 0.0), (0.5, 0.0)]
        );
    }

    #[test]
    fn smooth_needs_at_least_two_points() {
        assert_eq!(smooth(&[]), None);
        assert_eq!(smooth(&[(1.0, 1.0)]), None);
    }

    #[test]
    fn smooth_two_points_is_a_straight_line() {
        let (start, segments) = smooth(&[(0.0, 0.0), (10.0, 0.0)]).expect("路径");
        assert_eq!(start, (0.0, 0.0));
        assert_eq!(segments, vec![Segment::Line { end: (10.0, 0.0) }]);
    }

    #[test]
    fn smooth_uses_midpoints_between_controls() {
        let points = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let (start, segments) = smooth(&points).expect("路径");
        assert_eq!(start, (0.0, 0.0));
        assert_eq!(
            segments,
            vec![
                Segment::Quad {
                    control: (10.0, 0.0),
                    end: (10.0, 5.0)
                },
                Segment::Quad {
                    control: (10.0, 10.0),
                    end: (5.0, 10.0)
                },
                Segment::Line { end: (0.0, 10.0) },
            ]
        );
    }
}
