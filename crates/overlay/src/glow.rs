//! 谷歌四色辉光与炫彩描边的颜色、动画和几何计算，与 Win32 无关。

use lenswin_core::config::GlowMode;

/// 谷歌官方四色，按蓝、红、黄、绿排列。
pub const GOOGLE_COLORS: [[u8; 3]; 4] = [
    [0x42, 0x85, 0xF4],
    [0xEA, 0x43, 0x35],
    [0xFB, 0xBC, 0x05],
    [0x34, 0xA8, 0x53],
];

/// `startup` 档动画结束后保留的辉光强度。
pub const REST_INTENSITY: f32 = 0.7;

/// 光带从边缘向内扩散到最宽所用时长（秒）。
const SPREAD_SECS: f32 = 0.6;
/// 扩散时的回弹幅度，越大冲过最宽处越多。
const SPREAD_OVERSHOOT: f32 = 1.70158;
/// 屏幕压暗淡入的时长（秒）。
const DIM_IN_SECS: f32 = 0.35;
/// `startup` 档满强度流动的时长（秒），之后开始减弱。
const FLOW_SECS: f32 = 1.0;
/// `startup` 档减弱到静态强度所用时长（秒）。
const SETTLE_SECS: f32 = 0.5;
/// `startup` 档整段动画转过的圈数，用缓出曲线让流动平滑停下。
const STARTUP_TURNS: f32 = 0.75;
/// `always` 档的流动速度（圈 / 秒）。
const ALWAYS_TURNS_PER_SEC: f32 = 0.12;

/// RGB 颜色，分量取 0–1。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb {
    /// 红。
    pub r: f32,
    /// 绿。
    pub g: f32,
    /// 蓝。
    pub b: f32,
}

impl Rgb {
    fn from_bytes([r, g, b]: [u8; 3]) -> Self {
        Self {
            r: f32::from(r) / 255.0,
            g: f32::from(g) / 255.0,
            b: f32::from(b) / 255.0,
        }
    }

    fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
        }
    }
}

fn google(index: usize) -> Rgb {
    let bytes = GOOGLE_COLORS
        .get(index % GOOGLE_COLORS.len())
        .copied()
        .unwrap_or([0, 0, 0]);
    Rgb::from_bytes(bytes)
}

/// 循环色带上位置 `t` 的颜色：每增加 1 依次走完蓝、红、黄、绿一圈，相邻颜色平滑过渡。
pub fn palette_at(t: f32) -> Rgb {
    if !t.is_finite() {
        return google(0);
    }
    let scaled = t.rem_euclid(1.0) * 4.0;
    // 浮点转整数会被 lint 拒绝，这里只有四段，直接比较
    let (index, fraction) = if scaled < 1.0 {
        (0, scaled)
    } else if scaled < 2.0 {
        (1, scaled - 1.0)
    } else if scaled < 3.0 {
        (2, scaled - 2.0)
    } else {
        (3, (scaled - 3.0).min(1.0))
    };
    google(index).lerp(google(index + 1), fraction)
}

/// 谷歌蓝。
pub fn google_blue() -> Rgb {
    google(0)
}

/// 某一时刻的辉光状态。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlowFrame {
    /// 整体强度，0–1。
    pub intensity: f32,
    /// 光带宽度相对设定值的比例，出现时从 0 回弹到 1，可能短暂超过 1。
    pub spread: f32,
    /// 色带沿边框流动的相位（圈），取值 0–1。
    pub phase: f32,
}

fn sanitize_secs(elapsed: f32) -> f32 {
    if elapsed.is_finite() {
        elapsed.max(0.0)
    } else {
        0.0
    }
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn ease_out(x: f32) -> f32 {
    let inv = 1.0 - x.clamp(0.0, 1.0);
    1.0 - inv * inv
}

/// 带回弹的缓出：先冲过终点再落回，起点 0、终点 1。
fn ease_out_back(x: f32) -> f32 {
    let t = x.clamp(0.0, 1.0) - 1.0;
    1.0 + (SPREAD_OVERSHOOT + 1.0) * t * t * t + SPREAD_OVERSHOOT * t * t
}

/// 覆盖层打开 `elapsed` 秒后遮罩的不透明度比例；不显示辉光时直接压暗，没有淡入。
pub fn dim_progress(mode: GlowMode, elapsed: f32) -> f32 {
    match mode {
        GlowMode::Off => 1.0,
        GlowMode::Startup | GlowMode::Always => ease_out(sanitize_secs(elapsed) / DIM_IN_SECS),
    }
}

/// 覆盖层打开 `elapsed` 秒后的辉光状态；`Off` 返回 `None`。
pub fn glow_frame(mode: GlowMode, elapsed: f32) -> Option<GlowFrame> {
    let t = sanitize_secs(elapsed);
    let spread = ease_out_back(t / SPREAD_SECS);
    match mode {
        GlowMode::Off => None,
        GlowMode::Always => Some(GlowFrame {
            intensity: 1.0,
            spread,
            phase: (t * ALWAYS_TURNS_PER_SEC).rem_euclid(1.0),
        }),
        GlowMode::Startup => {
            let total = FLOW_SECS + SETTLE_SECS;
            let settle = smoothstep((t - FLOW_SECS) / SETTLE_SECS);
            Some(GlowFrame {
                intensity: 1.0 - (1.0 - REST_INTENSITY) * settle,
                spread,
                phase: (STARTUP_TURNS * ease_out(t / total)).rem_euclid(1.0),
            })
        }
    }
}

/// 覆盖层打开 `elapsed` 秒后辉光是否还在变化，用于决定是否继续定时重绘。
pub fn is_animating(mode: GlowMode, elapsed: f32) -> bool {
    match mode {
        GlowMode::Off => false,
        GlowMode::Always => true,
        GlowMode::Startup => sanitize_secs(elapsed) < (FLOW_SECS + SETTLE_SECS).max(SPREAD_SECS),
    }
}

/// 矩形的一条边，顺时针从上边开始。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// 上边，从左到右。
    Top,
    /// 右边，从上到下。
    Right,
    /// 下边，从右到左。
    Bottom,
    /// 左边，从下到上。
    Left,
}

impl Edge {
    /// 顺时针排列的四条边。
    pub const ALL: [Edge; 4] = [Edge::Top, Edge::Right, Edge::Bottom, Edge::Left];

    /// 宽 `w`、高 `h` 的矩形上，这条边沿顺时针方向的起点和终点（相对左上角）。
    pub fn endpoints(self, w: f32, h: f32) -> ((f32, f32), (f32, f32)) {
        match self {
            Edge::Top => ((0.0, 0.0), (w, 0.0)),
            Edge::Right => ((w, 0.0), (w, h)),
            Edge::Bottom => ((w, h), (0.0, h)),
            Edge::Left => ((0.0, h), (0.0, 0.0)),
        }
    }

    /// 这条边在顺时针周长上的起止距离。
    fn span(self, w: f32, h: f32) -> (f32, f32) {
        match self {
            Edge::Top => (0.0, w),
            Edge::Right => (w, w + h),
            Edge::Bottom => (w + h, w + w + h),
            Edge::Left => (w + w + h, w + w + h + h),
        }
    }

    /// 从这条边向内 `inner`–`outer` 深度的一条光带（左、上、右、下，相对左上角）。
    ///
    /// 上下边的光带占满除角落已被更浅光带占用之外的宽度，左右边让出上下边的部分，
    /// 四条边各层光带拼起来既不重叠也不留缝，角落不会叠出更亮的方块。
    pub fn strip(self, w: f32, h: f32, inner: f32, outer: f32) -> (f32, f32, f32, f32) {
        match self {
            Edge::Top => (inner, inner, w - inner, outer),
            Edge::Bottom => (inner, h - outer, w - inner, h - inner),
            Edge::Left => (inner, outer, outer, h - outer),
            Edge::Right => (w - outer, outer, w - inner, h - outer),
        }
    }
}

/// 一条边上均匀取 `count` 个渐变色标（边上的相对位置 0–1，颜色）。
///
/// 整个矩形周长对应 `cycles` 圈四色，`phase` 让色带沿顺时针方向平移。
pub fn edge_stops(
    w: f32,
    h: f32,
    edge: Edge,
    phase: f32,
    cycles: f32,
    count: u16,
) -> Vec<(f32, Rgb)> {
    let perimeter = 2.0 * (w + h);
    let (start, end) = edge.span(w, h);
    let last = f32::from(count.saturating_sub(1).max(1));
    (0..count.max(2))
        .map(|i| {
            let s = (f32::from(i) / last).min(1.0);
            let t = if perimeter.is_finite() && perimeter > 0.0 {
                phase + cycles * (start + s * (end - start)) / perimeter
            } else {
                phase
            };
            (s, palette_at(t))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn same(a: Rgb, b: Rgb) -> bool {
        close(a.r, b.r) && close(a.g, b.g) && close(a.b, b.b)
    }

    #[test]
    fn palette_hits_each_google_color_at_quarters() {
        for (i, quarter) in [0.0, 0.25, 0.5, 0.75].into_iter().enumerate() {
            assert!(same(palette_at(quarter), google(i)), "{quarter}");
        }
    }

    #[test]
    fn palette_wraps_and_handles_negative_and_non_finite() {
        assert!(same(palette_at(1.0), palette_at(0.0)));
        assert!(same(palette_at(-0.25), palette_at(0.75)));
        assert!(same(palette_at(3.5), palette_at(0.5)));
        assert!(same(palette_at(f32::NAN), google(0)));
        assert!(same(palette_at(f32::INFINITY), google(0)));
    }

    #[test]
    fn palette_blends_between_neighbours() {
        let mid = palette_at(0.125);
        let blue = google(0);
        let red = google(1);
        assert!(close(mid.r, (blue.r + red.r) / 2.0));
        assert!(close(mid.b, (blue.b + red.b) / 2.0));
    }

    #[test]
    fn palette_closes_the_loop_from_green_to_blue() {
        let near_end = palette_at(0.875);
        let green = google(3);
        let blue = google(0);
        assert!(close(near_end.g, (green.g + blue.g) / 2.0));
    }

    #[test]
    fn spread_starts_at_zero_overshoots_then_settles_at_one() {
        let start = glow_frame(GlowMode::Startup, 0.0).expect("有辉光");
        assert!(close(start.spread, 0.0));
        let mut peak = 0.0f32;
        let mut t = 0.0;
        while t <= SPREAD_SECS {
            let f = glow_frame(GlowMode::Always, t).expect("有辉光");
            peak = peak.max(f.spread);
            t += 0.01;
        }
        assert!(peak > 1.0 && peak < 1.2, "{peak}");
        let done = glow_frame(GlowMode::Startup, SPREAD_SECS).expect("有辉光");
        assert!(close(done.spread, 1.0));
        let late = glow_frame(GlowMode::Always, 50.0).expect("有辉光");
        assert!(close(late.spread, 1.0));
    }

    #[test]
    fn dim_fades_in_only_with_glow() {
        assert!(close(dim_progress(GlowMode::Off, 0.0), 1.0));
        assert!(close(dim_progress(GlowMode::Startup, 0.0), 0.0));
        assert!(close(dim_progress(GlowMode::Always, DIM_IN_SECS), 1.0));
        assert!(close(dim_progress(GlowMode::Startup, 10.0), 1.0));
        assert!(close(dim_progress(GlowMode::Startup, f32::NAN), 0.0));
    }

    #[test]
    fn off_has_no_glow_and_never_animates() {
        assert_eq!(glow_frame(GlowMode::Off, 0.5), None);
        assert!(!is_animating(GlowMode::Off, 0.0));
    }

    #[test]
    fn startup_flows_then_rests() {
        let start = glow_frame(GlowMode::Startup, 0.0).expect("有辉光");
        assert!(close(start.intensity, 1.0));
        let full = glow_frame(GlowMode::Startup, 0.5).expect("有辉光");
        assert!(close(full.intensity, 1.0));
        let rest = glow_frame(GlowMode::Startup, 10.0).expect("有辉光");
        assert!(close(rest.intensity, REST_INTENSITY));
        let settled = glow_frame(GlowMode::Startup, FLOW_SECS + SETTLE_SECS).expect("有辉光");
        assert!(close(settled.intensity, REST_INTENSITY));
        assert!(close(settled.phase, rest.phase));
        assert!(is_animating(GlowMode::Startup, 1.0));
        assert!(!is_animating(GlowMode::Startup, FLOW_SECS + SETTLE_SECS));
    }

    #[test]
    fn startup_intensity_never_drops_below_rest_after_flow() {
        let mut t = FLOW_SECS;
        while t <= FLOW_SECS + SETTLE_SECS {
            let f = glow_frame(GlowMode::Startup, t).expect("有辉光");
            assert!(f.intensity >= REST_INTENSITY - 1e-4 && f.intensity <= 1.0 + 1e-4);
            t += 0.05;
        }
    }

    #[test]
    fn always_keeps_flowing_and_phase_stays_in_range() {
        let a = glow_frame(GlowMode::Always, 1.0).expect("有辉光");
        let b = glow_frame(GlowMode::Always, 2.0).expect("有辉光");
        assert!(close(a.intensity, 1.0));
        assert!(!close(a.phase, b.phase));
        let late = glow_frame(GlowMode::Always, 10_000.0).expect("有辉光");
        assert!((0.0..1.0).contains(&late.phase));
        assert!(is_animating(GlowMode::Always, 10_000.0));
    }

    #[test]
    fn negative_or_non_finite_elapsed_is_treated_as_zero() {
        assert_eq!(
            glow_frame(GlowMode::Startup, -3.0),
            glow_frame(GlowMode::Startup, 0.0)
        );
        assert_eq!(
            glow_frame(GlowMode::Always, f32::NAN),
            glow_frame(GlowMode::Always, 0.0)
        );
        assert!(is_animating(GlowMode::Startup, f32::NAN));
    }

    #[test]
    fn edge_stops_are_continuous_around_the_rectangle() {
        let (w, h) = (300.0, 100.0);
        for pair in Edge::ALL.windows(2) {
            if let [a, b] = pair {
                let end = edge_stops(w, h, *a, 0.2, 1.0, 8);
                let start = edge_stops(w, h, *b, 0.2, 1.0, 8);
                let (Some(last), Some(first)) = (end.last(), start.first()) else {
                    panic!("色标为空");
                };
                assert!(same(last.1, first.1));
            }
        }
        let top = edge_stops(w, h, Edge::Top, 0.2, 1.0, 8);
        let left = edge_stops(w, h, Edge::Left, 0.2, 1.0, 8);
        assert!(same(
            left.last().map(|s| s.1).expect("色标"),
            top.first().map(|s| s.1).expect("色标")
        ));
    }

    #[test]
    fn edge_stops_positions_span_zero_to_one() {
        let stops = edge_stops(100.0, 100.0, Edge::Right, 0.0, 1.0, 5);
        assert_eq!(stops.len(), 5);
        assert!(close(stops.first().map(|s| s.0).expect("色标"), 0.0));
        assert!(close(stops.last().map(|s| s.0).expect("色标"), 1.0));
        assert_eq!(edge_stops(100.0, 100.0, Edge::Top, 0.0, 1.0, 0).len(), 2);
    }

    #[test]
    fn edge_stops_tolerate_degenerate_rectangle() {
        let stops = edge_stops(0.0, 0.0, Edge::Top, 0.5, 1.0, 3);
        assert!(stops.iter().all(|s| same(s.1, palette_at(0.5))));
    }

    #[test]
    fn strips_tile_corner_without_overlap_or_gap() {
        let (w, h) = (100.0, 80.0);
        let bands = [(0.0, 4.0), (4.0, 8.0), (8.0, 12.0)];
        let covers =
            |r: (f32, f32, f32, f32), x: f32, y: f32| x >= r.0 && x < r.2 && y >= r.1 && y < r.3;
        for xi in 0..12u8 {
            for yi in 0..12u8 {
                let (x, y) = (f32::from(xi) + 0.5, f32::from(yi) + 0.5);
                let hits = bands
                    .iter()
                    .flat_map(|(a, b)| Edge::ALL.map(|e| e.strip(w, h, *a, *b)))
                    .filter(|r| covers(*r, x, y))
                    .count();
                assert_eq!(hits, 1, "({x}, {y})");
            }
        }
    }

    #[test]
    fn endpoints_run_clockwise() {
        assert_eq!(Edge::Top.endpoints(4.0, 2.0), ((0.0, 0.0), (4.0, 0.0)));
        assert_eq!(Edge::Bottom.endpoints(4.0, 2.0), ((4.0, 2.0), (0.0, 2.0)));
        assert_eq!(Edge::Left.endpoints(4.0, 2.0), ((0.0, 2.0), (0.0, 0.0)));
    }
}
