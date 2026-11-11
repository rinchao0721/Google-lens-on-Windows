//! 界面元素的尺寸与摆放：工具栏、尺寸标签、放大镜，纯几何，按显示器 DPI 缩放。

use lenswin_core::geometry::{Point, Rect};

/// 基准 DPI，以下常量都是该 DPI 下的像素值。
const BASE_DPI: u32 = 96;

/// 工具栏按钮宽。
const BUTTON_WIDTH: u32 = 36;
/// 工具栏按钮高。
const BUTTON_HEIGHT: u32 = 32;
/// 工具栏内边距。
const BAR_PADDING: u32 = 4;
/// 工具栏、标签与选区之间的间距。
const GAP: u32 = 6;
/// 手柄抓取半径。
const GRIP: u32 = 6;
/// 手柄绘制边长。
const HANDLE_SIZE: u32 = 7;
/// 尺寸标签高度。
const LABEL_HEIGHT: u32 = 22;
/// 放大镜采样边长（像素数，取奇数以便中心像素居中）。
pub const ZOOM_SAMPLES: u32 = 17;
/// 放大镜里每个像素的边长。
const ZOOM_CELL: u32 = 7;
/// 放大镜下方信息区高度。
const INFO_HEIGHT: u32 = 44;
/// 放大镜与光标的距离。
const CURSOR_OFFSET: u32 = 20;
/// 提示面板与屏幕边缘的距离。
const HINT_MARGIN: u32 = 16;
/// 提示面板内边距。
const HINT_PADDING: u32 = 10;
/// 提示面板每行高度。
const HINT_ROW: u32 = 24;
/// 按键列与说明列的间距，也是按键文字两侧的留白。
const HINT_GAP: u32 = 10;
/// 判断光标是否挡住面板时，光标周围视为占用的半径。
const HINT_CURSOR_RADIUS: u32 = 32;

/// 工具栏按钮，顺序即从左到右的摆放顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarButton {
    /// 以图搜图。
    Search,
    /// 保存。
    Save,
    /// 取消。
    Cancel,
    /// 复制，主操作。
    Copy,
}

impl ToolbarButton {
    /// 全部按钮。
    pub const ALL: [ToolbarButton; 4] = [
        ToolbarButton::Search,
        ToolbarButton::Save,
        ToolbarButton::Cancel,
        ToolbarButton::Copy,
    ];

    /// Segoe Fluent Icons / Segoe MDL2 Assets 中的图标码位。
    pub const fn glyph(self) -> char {
        match self {
            ToolbarButton::Search => '\u{E721}',
            ToolbarButton::Save => '\u{E74E}',
            ToolbarButton::Cancel => '\u{E711}',
            ToolbarButton::Copy => '\u{E8C8}',
        }
    }

    /// 图标字体不可用时显示的文字。
    pub const fn label(self) -> &'static str {
        match self {
            ToolbarButton::Search => "搜索",
            ToolbarButton::Save => "保存",
            ToolbarButton::Cancel => "取消",
            ToolbarButton::Copy => "复制",
        }
    }
}

/// 按 DPI 缩放像素值，至少为 1。
pub fn scaled(value: u32, dpi: u32) -> u32 {
    let dpi = if dpi == 0 { BASE_DPI } else { dpi };
    let v = u64::from(value) * u64::from(dpi) / u64::from(BASE_DPI);
    u32::try_from(v).unwrap_or(u32::MAX).max(1)
}

/// 手柄抓取半径。
pub fn grip(dpi: u32) -> u32 {
    scaled(GRIP, dpi)
}

/// 手柄绘制边长。
pub fn handle_size(dpi: u32) -> u32 {
    scaled(HANDLE_SIZE, dpi)
}

/// 工具栏范围：优先放在选区下方右对齐，放不下时放上方，再放不下时贴着选区内侧底部。
pub fn toolbar_rect(selection: &Rect, monitor: &Rect, dpi: u32) -> Rect {
    let count = u32::try_from(ToolbarButton::ALL.len()).unwrap_or(0);
    let pad = scaled(BAR_PADDING, dpi);
    let width = scaled(BUTTON_WIDTH, dpi)
        .saturating_mul(count)
        .saturating_add(pad.saturating_mul(2));
    let height = scaled(BUTTON_HEIGHT, dpi).saturating_add(pad.saturating_mul(2));
    let gap = i64::from(scaled(GAP, dpi));
    let (w, h) = (i64::from(width), i64::from(height));
    let (m_left, m_top, m_right, m_bottom) = edges(monitor);

    let below = selection.bottom() + gap;
    let above = i64::from(selection.y) - gap - h;
    let y = if below + h <= m_bottom {
        below
    } else if above >= m_top {
        above
    } else {
        selection.bottom() - gap - h
    };
    let x = selection.right() - w;
    place(x, y, w, h, (m_left, m_top, m_right, m_bottom))
}

/// 工具栏内第 `index` 个按钮的范围。
pub fn button_rect(bar: &Rect, index: usize, dpi: u32) -> Option<Rect> {
    let pad = i64::from(scaled(BAR_PADDING, dpi));
    let bw = i64::from(scaled(BUTTON_WIDTH, dpi));
    let bh = i64::from(scaled(BUTTON_HEIGHT, dpi));
    let i = i64::try_from(index).ok()?;
    let left = i64::from(bar.x) + pad + bw * i;
    let top = i64::from(bar.y) + pad;
    Rect::from_edges(left, top, left + bw, top + bh)
}

/// 点中的按钮。
pub fn button_at(bar: &Rect, p: Point, dpi: u32) -> Option<ToolbarButton> {
    ToolbarButton::ALL
        .iter()
        .enumerate()
        .find(|(i, _)| button_rect(bar, *i, dpi).is_some_and(|r| r.contains(p)))
        .map(|(_, b)| *b)
}

/// 尺寸标签的范围：放在选区左上角外侧，放不下时放进选区内侧。
pub fn label_rect(selection: &Rect, monitor: &Rect, text_width: u32, dpi: u32) -> Rect {
    let h = i64::from(scaled(LABEL_HEIGHT, dpi));
    let w = i64::from(text_width.saturating_add(scaled(12, dpi)));
    let gap = i64::from(scaled(GAP, dpi) / 2);
    let mon = edges(monitor);
    let above = i64::from(selection.y) - gap - h;
    let y = if above >= mon.1 {
        above
    } else {
        i64::from(selection.y) + gap
    };
    place(i64::from(selection.x), y, w, h, mon)
}

/// 放大镜的两部分：放大图与下方信息区。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MagnifierLayout {
    /// 放大图区域。
    pub zoom: Rect,
    /// 坐标与颜色信息区域。
    pub info: Rect,
}

/// 放大镜整体放在光标右下方，超出显示器时翻到另一侧。
pub fn magnifier_layout(cursor: Point, monitor: &Rect, dpi: u32) -> MagnifierLayout {
    let side = i64::from(scaled(ZOOM_SAMPLES * ZOOM_CELL, dpi));
    let info_h = i64::from(scaled(INFO_HEIGHT, dpi));
    let offset = i64::from(scaled(CURSOR_OFFSET, dpi));
    let total_h = side + info_h;
    let mon = edges(monitor);
    let (cx, cy) = (i64::from(cursor.x), i64::from(cursor.y));
    let x = if cx + offset + side <= mon.2 {
        cx + offset
    } else {
        cx - offset - side
    };
    let y = if cy + offset + total_h <= mon.3 {
        cy + offset
    } else {
        cy - offset - total_h
    };
    let whole = place(x, y, side, total_h, mon);
    let zoom = Rect::new(
        whole.x,
        whole.y,
        u32::try_from(side).unwrap_or(0),
        u32::try_from(side).unwrap_or(0),
    );
    let info_top = i64::from(whole.y) + side;
    let info = Rect::from_edges(
        i64::from(whole.x),
        info_top,
        i64::from(whole.x) + side,
        info_top + info_h,
    )
    .unwrap_or(zoom);
    MagnifierLayout { zoom, info }
}

/// 放大镜采样：以光标为中心的采样框与显示器求交后的源范围，以及它在放大图里对应的范围。
///
/// 两个范围都用虚拟桌面坐标；光标贴边时源范围变小，放大图里只填对应的一部分。
pub fn zoom_source(cursor: Point, monitor: &Rect, zoom: &Rect) -> Option<(Rect, Rect)> {
    let half = i64::from(ZOOM_SAMPLES / 2);
    let samples = i64::from(ZOOM_SAMPLES);
    let left = i64::from(cursor.x) - half;
    let top = i64::from(cursor.y) - half;
    let full = Rect::from_edges(left, top, left + samples, top + samples)?;
    let src = full.intersect(monitor)?;
    let cell = i64::from(zoom.width) / samples;
    let dst_left = i64::from(zoom.x) + (i64::from(src.x) - left) * cell;
    let dst_top = i64::from(zoom.y) + (i64::from(src.y) - top) * cell;
    let dst = Rect::from_edges(
        dst_left,
        dst_top,
        dst_left + i64::from(src.width) * cell,
        dst_top + i64::from(src.height) * cell,
    )?;
    Some((src, dst))
}

/// 读取冻结帧中某个虚拟桌面坐标的 RGB；`bgra` 是 `monitor` 对应的整屏 BGRA 数据。
pub fn pixel_at(bgra: &[u8], monitor: &Rect, p: Point) -> Option<(u8, u8, u8)> {
    if !monitor.contains(p) {
        return None;
    }
    let x = usize::try_from(i64::from(p.x) - i64::from(monitor.x)).ok()?;
    let y = usize::try_from(i64::from(p.y) - i64::from(monitor.y)).ok()?;
    let width = usize::try_from(monitor.width).ok()?;
    let offset = y.checked_mul(width)?.checked_add(x)?.checked_mul(4)?;
    match bgra.get(offset..offset.checked_add(3)?)? {
        [b, g, r] => Some((*r, *g, *b)),
        _ => None,
    }
}

/// 提示面板的尺寸参数（像素），按 DPI 缩放。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HintMetrics {
    /// 内边距。
    pub padding: u32,
    /// 行高。
    pub row: u32,
    /// 列间距与按键留白。
    pub gap: u32,
}

/// 提示面板的尺寸参数。
pub fn hint_metrics(dpi: u32) -> HintMetrics {
    HintMetrics {
        padding: scaled(HINT_PADDING, dpi),
        row: scaled(HINT_ROW, dpi),
        gap: scaled(HINT_GAP, dpi),
    }
}

/// 提示面板总尺寸：`key_width`、`desc_width` 为两列最宽文字的宽度。
pub fn hint_panel_size(rows: usize, key_width: u32, desc_width: u32, dpi: u32) -> (u32, u32) {
    let m = hint_metrics(dpi);
    let rows = u32::try_from(rows).unwrap_or(u32::MAX);
    let width = m
        .padding
        .saturating_mul(2)
        .saturating_add(key_width)
        .saturating_add(m.gap.saturating_mul(3))
        .saturating_add(desc_width);
    let height = m
        .padding
        .saturating_mul(2)
        .saturating_add(m.row.saturating_mul(rows));
    (width, height)
}

/// 光标周围视为占用的方框，用于让提示面板避让。
pub fn cursor_box(cursor: Point, dpi: u32) -> Rect {
    let r = i64::from(scaled(HINT_CURSOR_RADIUS, dpi));
    let (x, y) = (i64::from(cursor.x), i64::from(cursor.y));
    Rect::from_edges(x - r, y - r, x + r, y + r).unwrap_or_default()
}

/// 提示面板位置：依次尝试左下、右下、左上、右上角，取第一个不与 `avoid` 相交的；都相交时放左下。
pub fn hint_panel_rect(monitor: &Rect, size: (u32, u32), avoid: &[Rect], dpi: u32) -> Rect {
    let margin = i64::from(scaled(HINT_MARGIN, dpi));
    let (w, h) = (i64::from(size.0), i64::from(size.1));
    let mon = edges(monitor);
    let left = mon.0 + margin;
    let right = mon.2 - margin - w;
    let top = mon.1 + margin;
    let bottom = mon.3 - margin - h;
    let candidates = [(left, bottom), (right, bottom), (left, top), (right, top)];
    let rects: Vec<Rect> = candidates
        .iter()
        .map(|(x, y)| place(*x, *y, w, h, mon))
        .collect();
    rects
        .iter()
        .find(|r| avoid.iter().all(|a| r.intersect(a).is_none()))
        .or_else(|| rects.first())
        .copied()
        .unwrap_or_default()
}

/// 把 `(x, y, w, h)` 推回显示器范围内；比显示器还大时贴左上角。
fn place(x: i64, y: i64, w: i64, h: i64, mon: (i64, i64, i64, i64)) -> Rect {
    let (m_left, m_top, m_right, m_bottom) = mon;
    let x = x.min(m_right - w).max(m_left);
    let y = y.min(m_bottom - h).max(m_top);
    Rect::from_edges(x, y, x + w, y + h).unwrap_or_default()
}

fn edges(r: &Rect) -> (i64, i64, i64, i64) {
    (i64::from(r.x), i64::from(r.y), r.right(), r.bottom())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: Rect = Rect::new(0, 0, 1920, 1080);

    #[test]
    fn scaling_follows_dpi() {
        assert_eq!(scaled(36, 96), 36);
        assert_eq!(scaled(36, 144), 54);
        assert_eq!(scaled(0, 96), 1);
        assert_eq!(scaled(10, 0), 10);
    }

    #[test]
    fn toolbar_prefers_below_right_aligned() {
        let sel = Rect::new(100, 100, 400, 300);
        let bar = toolbar_rect(&sel, &MONITOR, 96);
        assert_eq!(bar, Rect::new(500 - 152, 406, 152, 40));
    }

    #[test]
    fn toolbar_moves_above_then_inside() {
        let near_bottom = Rect::new(100, 900, 400, 170);
        let bar = toolbar_rect(&near_bottom, &MONITOR, 96);
        assert_eq!(bar.y, 900 - 6 - 40);
        let full = Rect::new(0, 0, 1920, 1080);
        let bar = toolbar_rect(&full, &MONITOR, 96);
        assert_eq!(bar.y, 1080 - 6 - 40);
        assert_eq!(bar.x, 1920 - 152);
    }

    #[test]
    fn toolbar_is_clamped_horizontally() {
        let narrow = Rect::new(10, 100, 20, 20);
        let bar = toolbar_rect(&narrow, &MONITOR, 96);
        assert_eq!(bar.x, 0);
        let other = Rect::new(-1920, 0, 1920, 1080);
        let bar = toolbar_rect(&Rect::new(-1910, 10, 20, 20), &other, 96);
        assert_eq!(bar.x, -1920);
    }

    #[test]
    fn buttons_are_hit_in_order() {
        let bar = Rect::new(0, 0, 152, 40);
        assert_eq!(
            button_at(&bar, Point::new(5, 5), 96),
            Some(ToolbarButton::Search)
        );
        assert_eq!(
            button_at(&bar, Point::new(45, 20), 96),
            Some(ToolbarButton::Save)
        );
        assert_eq!(
            button_at(&bar, Point::new(147, 35), 96),
            Some(ToolbarButton::Copy)
        );
        assert_eq!(button_at(&bar, Point::new(1, 1), 96), None);
        assert_eq!(button_at(&bar, Point::new(200, 20), 96), None);
    }

    #[test]
    fn label_goes_above_or_inside() {
        let sel = Rect::new(100, 100, 50, 50);
        assert_eq!(
            label_rect(&sel, &MONITOR, 60, 96),
            Rect::new(100, 75, 72, 22)
        );
        let top = Rect::new(100, 5, 50, 50);
        assert_eq!(label_rect(&top, &MONITOR, 60, 96).y, 8);
    }

    #[test]
    fn magnifier_flips_near_edges() {
        let m = magnifier_layout(Point::new(100, 100), &MONITOR, 96);
        assert_eq!(m.zoom, Rect::new(120, 120, 119, 119));
        assert_eq!(m.info, Rect::new(120, 239, 119, 44));
        let m = magnifier_layout(Point::new(1900, 1070), &MONITOR, 96);
        assert_eq!(m.zoom.x, 1900 - 20 - 119);
        assert_eq!(m.zoom.y, 1070 - 20 - 163);
    }

    #[test]
    fn zoom_source_clips_at_monitor_edge() {
        let zoom = Rect::new(500, 500, 119, 119);
        let (src, dst) = zoom_source(Point::new(100, 100), &MONITOR, &zoom).expect("采样");
        assert_eq!(src, Rect::new(92, 92, 17, 17));
        assert_eq!(dst, zoom);
        let (src, dst) = zoom_source(Point::new(0, 0), &MONITOR, &zoom).expect("采样");
        assert_eq!(src, Rect::new(0, 0, 9, 9));
        assert_eq!(dst, Rect::new(500 + 8 * 7, 500 + 8 * 7, 63, 63));
    }

    #[test]
    fn hint_panel_size_adds_padding_and_rows() {
        assert_eq!(
            hint_panel_size(3, 50, 100, 96),
            (20 + 50 + 30 + 100, 20 + 72)
        );
        assert_eq!(hint_panel_size(0, 0, 0, 96), (50, 20));
    }

    #[test]
    fn hint_panel_avoids_cursor_and_selection() {
        let size = (200, 100);
        let free = hint_panel_rect(&MONITOR, size, &[], 96);
        assert_eq!(free, Rect::new(16, 1080 - 16 - 100, 200, 100));
        let cursor = cursor_box(Point::new(50, 1000), 96);
        let moved = hint_panel_rect(&MONITOR, size, &[cursor], 96);
        assert_eq!(moved, Rect::new(1920 - 16 - 200, 964, 200, 100));
        let both_bottom = [cursor, Rect::new(1700, 900, 100, 100)];
        assert_eq!(hint_panel_rect(&MONITOR, size, &both_bottom, 96).y, 16);
        let everywhere = [Rect::new(0, 0, 1920, 1080)];
        assert_eq!(hint_panel_rect(&MONITOR, size, &everywhere, 96), free);
    }

    #[test]
    fn hint_panel_respects_monitor_offset() {
        let other = Rect::new(-1920, 0, 1920, 1080);
        let r = hint_panel_rect(&other, (200, 100), &[], 96);
        assert_eq!(r.x, -1920 + 16);
    }

    #[test]
    fn pixel_lookup_handles_bounds() {
        let monitor = Rect::new(10, 10, 2, 1);
        let bgra = [1, 2, 3, 255, 4, 5, 6, 255];
        assert_eq!(
            pixel_at(&bgra, &monitor, Point::new(11, 10)),
            Some((6, 5, 4))
        );
        assert_eq!(
            pixel_at(&bgra, &monitor, Point::new(10, 10)),
            Some((3, 2, 1))
        );
        assert_eq!(pixel_at(&bgra, &monitor, Point::new(12, 10)), None);
        assert_eq!(pixel_at(&bgra[..5], &monitor, Point::new(11, 10)), None);
    }
}
