//! 界面元素的尺寸与摆放：工具栏、尺寸标签、放大镜，纯几何，按显示器 DPI 缩放。

use lenswin_core::annotation::{NumberStyle, PALETTE, Tool};
use lenswin_core::geometry::{Point, Rect};

/// 基准 DPI，以下常量都是该 DPI 下的像素值。
const BASE_DPI: u32 = 96;

/// 工具栏按钮宽。
const BUTTON_WIDTH: u32 = 36;
/// 工具栏按钮高。
const BUTTON_HEIGHT: u32 = 32;
/// 工具栏内边距。
const BAR_PADDING: u32 = 4;
/// 工具栏里分组之间的分隔宽度。
const SEPARATOR_WIDTH: u32 = 9;
/// 样式条色块边长。
const SWATCH_SIZE: u32 = 22;
/// 样式条控件间距。
const SWATCH_GAP: u32 = 6;
/// 样式条当前粗细数值的文字宽度。
const VALUE_WIDTH: u32 = 36;
/// 样式条编号风格按钮宽度。
const STYLE_CHIP_WIDTH: u32 = 32;
/// 样式条与工具栏的间距。
const STRIP_GAP: u32 = 4;
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

/// 工具栏按钮。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarButton {
    /// 选中标注工具。
    Tool(Tool),
    /// 撤销。
    Undo,
    /// 重做。
    Redo,
    /// 以图搜图。
    Search,
    /// 保存。
    Save,
    /// 取消。
    Cancel,
    /// 复制，主操作。
    Copy,
}

/// 按钮上显示的内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    /// 图标字体里的码位。
    Icon(char),
    /// 固定文字，不依赖图标字体。
    Text(&'static str),
}

impl ToolbarButton {
    /// 标注工具组。
    pub const TOOLS: [ToolbarButton; 9] = [
        ToolbarButton::Tool(Tool::Rectangle),
        ToolbarButton::Tool(Tool::Ellipse),
        ToolbarButton::Tool(Tool::Line),
        ToolbarButton::Tool(Tool::Arrow),
        ToolbarButton::Tool(Tool::Pen),
        ToolbarButton::Tool(Tool::Highlighter),
        ToolbarButton::Tool(Tool::Mosaic),
        ToolbarButton::Tool(Tool::Number),
        ToolbarButton::Tool(Tool::Text),
    ];
    /// 撤销重做组。
    pub const HISTORY: [ToolbarButton; 2] = [ToolbarButton::Undo, ToolbarButton::Redo];
    /// 动作组。
    pub const ACTIONS: [ToolbarButton; 4] = [
        ToolbarButton::Search,
        ToolbarButton::Save,
        ToolbarButton::Cancel,
        ToolbarButton::Copy,
    ];

    /// Segoe Fluent Icons / Segoe MDL2 Assets 中的图标码位；序号用数字字符。
    pub const fn glyph(self) -> Glyph {
        match self {
            ToolbarButton::Tool(Tool::Rectangle) => Glyph::Icon('\u{E739}'),
            ToolbarButton::Tool(Tool::Ellipse) => Glyph::Icon('\u{EA3A}'),
            ToolbarButton::Tool(Tool::Line) => Glyph::Icon('\u{E738}'),
            ToolbarButton::Tool(Tool::Arrow) => Glyph::Icon('\u{E72A}'),
            ToolbarButton::Tool(Tool::Pen) => Glyph::Icon('\u{E70F}'),
            ToolbarButton::Tool(Tool::Highlighter) => Glyph::Icon('\u{E7E6}'),
            ToolbarButton::Tool(Tool::Mosaic) => Glyph::Icon('\u{E80A}'),
            ToolbarButton::Tool(Tool::Number) => Glyph::Text("1"),
            ToolbarButton::Tool(Tool::Text) => Glyph::Icon('\u{E8D2}'),
            ToolbarButton::Undo => Glyph::Icon('\u{E7A7}'),
            ToolbarButton::Redo => Glyph::Icon('\u{E7A6}'),
            ToolbarButton::Search => Glyph::Icon('\u{E721}'),
            ToolbarButton::Save => Glyph::Icon('\u{E74E}'),
            ToolbarButton::Cancel => Glyph::Icon('\u{E711}'),
            ToolbarButton::Copy => Glyph::Icon('\u{E8C8}'),
        }
    }

    /// 图标字体不可用时显示的文字。
    pub const fn label(self) -> &'static str {
        match self {
            ToolbarButton::Tool(Tool::Rectangle) => "矩",
            ToolbarButton::Tool(Tool::Ellipse) => "椭",
            ToolbarButton::Tool(Tool::Line) => "线",
            ToolbarButton::Tool(Tool::Arrow) => "箭",
            ToolbarButton::Tool(Tool::Pen) => "笔",
            ToolbarButton::Tool(Tool::Highlighter) => "荧",
            ToolbarButton::Tool(Tool::Mosaic) => "马",
            ToolbarButton::Tool(Tool::Number) => "序",
            ToolbarButton::Tool(Tool::Text) => "字",
            ToolbarButton::Undo => "撤",
            ToolbarButton::Redo => "重",
            ToolbarButton::Search => "搜索",
            ToolbarButton::Save => "保存",
            ToolbarButton::Cancel => "取消",
            ToolbarButton::Copy => "复制",
        }
    }
}

/// 行内的一项：按钮或分隔线。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowItem {
    Button(ToolbarButton),
    Separator,
}

/// 样式条需要显示哪些控件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StripSpec {
    /// 8 个色块。
    pub has_color: bool,
    /// 序号的「1 / A」切换。
    pub number_style: bool,
    /// 文字的「纯文字 / 底色」切换。
    pub appearance: bool,
}

/// 样式条内各控件的范围。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripLayout {
    /// 整条范围。
    pub rect: Rect,
    /// 色块，顺序与色板一致；没有颜色的工具为空。
    pub swatches: Vec<Rect>,
    /// 「细 / 中 / 粗」三个预设圆点的点击范围。
    pub presets: [Rect; 3],
    /// 当前粗细数值的文字区域，只显示不响应点击。
    pub value: Rect,
    /// 编号风格切换，只有序号工具有。
    pub number_styles: Vec<(NumberStyle, Rect)>,
    /// 外观切换（是否带底色），只有文字工具有。
    pub appearance: Vec<(bool, Rect)>,
}

/// 点中的控件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// 工具栏按钮。
    Button(ToolbarButton),
    /// 第几个色块。
    Swatch(usize),
    /// 第几档粗细预设。
    Preset(usize),
    /// 编号风格。
    NumberStyle(NumberStyle),
    /// 文字外观：`true` 为底色。
    Background(bool),
}

/// 工具栏与样式条的完整布局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolbarLayout {
    /// 工具栏范围。
    pub bar: Rect,
    /// 按钮及其范围。
    pub buttons: Vec<(ToolbarButton, Rect)>,
    /// 分隔线。
    pub separators: Vec<Rect>,
    /// 样式条，没有选中工具时为 `None`。
    pub strip: Option<StripLayout>,
}

impl ToolbarLayout {
    /// 点中的控件。
    pub fn hit(&self, p: Point) -> Option<Hit> {
        if let Some((button, _)) = self.buttons.iter().find(|(_, r)| r.contains(p)) {
            return Some(Hit::Button(*button));
        }
        let strip = self.strip.as_ref()?;
        if let Some(i) = strip.swatches.iter().position(|r| r.contains(p)) {
            return Some(Hit::Swatch(i));
        }
        if let Some((style, _)) = strip.number_styles.iter().find(|(_, r)| r.contains(p)) {
            return Some(Hit::NumberStyle(*style));
        }
        if let Some((background, _)) = strip.appearance.iter().find(|(_, r)| r.contains(p)) {
            return Some(Hit::Background(*background));
        }
        strip
            .presets
            .iter()
            .position(|r| r.contains(p))
            .map(Hit::Preset)
    }

    /// 点是否落在工具栏或样式条上，按下时据此拦截，避免在其下方画图。
    pub fn covers(&self, p: Point) -> bool {
        self.bar.contains(p) || self.strip.as_ref().is_some_and(|s| s.rect.contains(p))
    }

    /// 某个按钮的范围。
    pub fn button_rect(&self, button: ToolbarButton) -> Option<Rect> {
        self.buttons
            .iter()
            .find(|(b, _)| *b == button)
            .map(|(_, r)| *r)
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

/// 常驻选框角标的臂长（96 DPI 下）。
const CORNER_ARM: u32 = 36;
/// 常驻选框角标的圆角半径（96 DPI 下）。
const CORNER_ARC: u32 = 18;

/// 常驻选框四角折线的臂长与圆角半径（像素）。
/// 选框较小时臂长缩到短边的三分之一，同一条边上的两段之间始终留有空隙；为 0 表示不画。
pub fn corner_arm(width: u32, height: u32, dpi: u32) -> (u32, u32) {
    let short = width.min(height);
    let arm = scaled(CORNER_ARM, dpi).min(short / 3);
    let radius = scaled(CORNER_ARC, dpi).min(arm);
    (arm, radius)
}

/// 行内容：单行为全部分组，放不下时第一行工具与撤销重做、第二行动作按钮。
fn toolbar_rows(single_row_fits: bool) -> Vec<Vec<RowItem>> {
    let buttons = |list: &[ToolbarButton]| -> Vec<RowItem> {
        list.iter().copied().map(RowItem::Button).collect()
    };
    let mut first = buttons(&ToolbarButton::TOOLS);
    first.push(RowItem::Separator);
    first.extend(buttons(&ToolbarButton::HISTORY));
    let actions = buttons(&ToolbarButton::ACTIONS);
    if single_row_fits {
        first.push(RowItem::Separator);
        first.extend(actions);
        vec![first]
    } else {
        vec![first, actions]
    }
}

fn row_width(row: &[RowItem], dpi: u32) -> i64 {
    let pad = i64::from(scaled(BAR_PADDING, dpi));
    let bw = i64::from(scaled(BUTTON_WIDTH, dpi));
    let sep = i64::from(scaled(SEPARATOR_WIDTH, dpi));
    row.iter().fold(pad * 2, |acc, item| match item {
        RowItem::Button(_) => acc + bw,
        RowItem::Separator => acc + sep,
    })
}

/// 工具栏与样式条的布局。
///
/// 工具栏优先放在选区下方右对齐，放不下时放上方，再放不下时贴着选区内侧底部；
/// 单行宽度超过显示器时退为两行。样式条放在工具栏远离选区的一侧，放不下时放另一侧。
pub fn toolbar_layout(
    selection: &Rect,
    monitor: &Rect,
    dpi: u32,
    strip: Option<StripSpec>,
) -> ToolbarLayout {
    let pad = i64::from(scaled(BAR_PADDING, dpi));
    let bw = i64::from(scaled(BUTTON_WIDTH, dpi));
    let bh = i64::from(scaled(BUTTON_HEIGHT, dpi));
    let sep = i64::from(scaled(SEPARATOR_WIDTH, dpi));
    let gap = i64::from(scaled(GAP, dpi));
    let mon = edges(monitor);

    let single = toolbar_rows(true);
    let fits = single
        .first()
        .is_some_and(|row| row_width(row, dpi) <= i64::from(monitor.width));
    let rows = toolbar_rows(fits);
    let width = rows.iter().map(|r| row_width(r, dpi)).max().unwrap_or(0);
    let row_count = i64::try_from(rows.len()).unwrap_or(1);
    let height = pad * 2 + bh * row_count;

    let below = selection.bottom() + gap;
    let above = i64::from(selection.y) - gap - height;
    let y = if below + height <= mon.3 {
        below
    } else if above >= mon.1 {
        above
    } else {
        selection.bottom() - gap - height
    };
    let bar = place(selection.right() - width, y, width, height, mon);

    let mut buttons = Vec::new();
    let mut separators = Vec::new();
    let line = i64::from(scaled(1, dpi));
    for (row_index, row) in rows.iter().enumerate() {
        let top = i64::from(bar.y) + pad + bh * i64::try_from(row_index).unwrap_or(0);
        let mut left = i64::from(bar.x) + pad;
        for item in row {
            match item {
                RowItem::Button(button) => {
                    if let Some(r) = Rect::from_edges(left, top, left + bw, top + bh) {
                        buttons.push((*button, r));
                    }
                    left += bw;
                }
                RowItem::Separator => {
                    let x = left + (sep - line) / 2;
                    if let Some(r) = Rect::from_edges(x, top + bh / 4, x + line, top + bh * 3 / 4) {
                        separators.push(r);
                    }
                    left += sep;
                }
            }
        }
    }
    let strip = strip.and_then(|spec| strip_layout(&bar, selection, monitor, dpi, spec));
    ToolbarLayout {
        bar,
        buttons,
        separators,
        strip,
    }
}

/// 样式条：色块、粗细框、序号风格切换，横向排成一行。
fn strip_layout(
    bar: &Rect,
    selection: &Rect,
    monitor: &Rect,
    dpi: u32,
    spec: StripSpec,
) -> Option<StripLayout> {
    let pad = i64::from(scaled(BAR_PADDING, dpi));
    let swatch = i64::from(scaled(SWATCH_SIZE, dpi));
    let gap = i64::from(scaled(SWATCH_GAP, dpi));
    let value_w = i64::from(scaled(VALUE_WIDTH, dpi));
    let style_w = i64::from(scaled(STYLE_CHIP_WIDTH, dpi));
    let height = i64::from(scaled(BUTTON_HEIGHT, dpi)) + pad * 2;
    let swatch_count = i64::try_from(PALETTE.len()).unwrap_or(0);

    // 三个预设圆点之间留间距，其后接数值文字
    let mut width = pad * 2 + swatch * 3 + gap * 3 + value_w;
    if spec.has_color {
        width += swatch_count * (swatch + gap);
    }
    if spec.number_style || spec.appearance {
        width += gap + style_w * 2;
    }

    let mon = edges(monitor);
    let spacing = i64::from(scaled(STRIP_GAP, dpi));
    let bar_mid = i64::from(bar.y) * 2 + i64::from(bar.height);
    let sel_mid = i64::from(selection.y) * 2 + i64::from(selection.height);
    let below = bar.bottom() + spacing;
    let above = i64::from(bar.y) - spacing - height;
    let fits_below = below + height <= mon.3;
    let fits_above = above >= mon.1;
    let y = if bar_mid >= sel_mid {
        if fits_below || !fits_above {
            below
        } else {
            above
        }
    } else if fits_above || !fits_below {
        above
    } else {
        below
    };
    let rect = place(i64::from(bar.x), y, width, height, mon);

    let mid = i64::from(rect.y) + height / 2;
    let mut left = i64::from(rect.x) + pad;
    let mut swatches = Vec::new();
    if spec.has_color {
        for _ in 0..PALETTE.len() {
            let r = Rect::from_edges(
                left,
                mid - swatch / 2,
                left + swatch,
                mid - swatch / 2 + swatch,
            )?;
            swatches.push(r);
            left += swatch + gap;
        }
    }
    let chip_h = i64::from(scaled(BUTTON_HEIGHT, dpi)) * 3 / 4;
    let mut presets = [Rect::default(); 3];
    for slot in &mut presets {
        *slot = Rect::from_edges(
            left,
            mid - chip_h / 2,
            left + swatch,
            mid - chip_h / 2 + chip_h,
        )?;
        left += swatch + gap;
    }
    let value = Rect::from_edges(
        left,
        mid - chip_h / 2,
        left + value_w,
        mid - chip_h / 2 + chip_h,
    )?;
    left += value_w;
    let mut number_styles = Vec::new();
    if spec.number_style {
        left += gap;
        for style in NumberStyle::ALL {
            let r = Rect::from_edges(
                left,
                mid - chip_h / 2,
                left + style_w,
                mid - chip_h / 2 + chip_h,
            )?;
            number_styles.push((style, r));
            left += style_w;
        }
    }
    let mut appearance = Vec::new();
    if spec.appearance {
        left += gap;
        for background in [false, true] {
            let r = Rect::from_edges(
                left,
                mid - chip_h / 2,
                left + style_w,
                mid - chip_h / 2 + chip_h,
            )?;
            appearance.push((background, r));
            left += style_w;
        }
    }
    Some(StripLayout {
        rect,
        swatches,
        presets,
        value,
        number_styles,
        appearance,
    })
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
    fn corner_arm_scales_and_shrinks_for_small_boxes() {
        assert_eq!(corner_arm(400, 300, 96), (36, 18));
        assert_eq!(corner_arm(400, 300, 192), (72, 36));
        // 短边 30：臂长 10，两段之间留 10 像素
        assert_eq!(corner_arm(30, 500, 96), (10, 10));
        assert_eq!(corner_arm(2, 2, 96), (0, 0));
        assert_eq!(corner_arm(0, 100, 96), (0, 0));
    }

    const FULL_STRIP: StripSpec = StripSpec {
        has_color: true,
        number_style: true,
        appearance: false,
    };

    fn bar_of(sel: &Rect, monitor: &Rect) -> ToolbarLayout {
        toolbar_layout(sel, monitor, 96, None)
    }

    #[test]
    fn toolbar_single_row_below_right_aligned() {
        let sel = Rect::new(800, 100, 600, 300);
        let layout = bar_of(&sel, &MONITOR);
        // 15 个按钮 × 36 + 2 条分隔 × 9 + 内边距 8
        assert_eq!(layout.bar, Rect::new(834, 406, 566, 40));
        assert_eq!(layout.buttons.len(), 15);
        assert_eq!(layout.separators.len(), 2);
        assert_eq!(layout.strip, None);
        assert_eq!(
            layout.button_rect(ToolbarButton::Tool(Tool::Rectangle)),
            Some(Rect::new(838, 410, 36, 32))
        );
        assert_eq!(
            layout.button_rect(ToolbarButton::Undo),
            Some(Rect::new(838 + 9 * 36 + 9, 410, 36, 32))
        );
        let copy = layout.button_rect(ToolbarButton::Copy).expect("复制");
        assert_eq!(copy.right(), 1400 - 4);
    }

    #[test]
    fn toolbar_buttons_keep_order_and_never_overlap() {
        let layout = bar_of(&Rect::new(800, 100, 600, 300), &MONITOR);
        let order: Vec<_> = layout.buttons.iter().map(|(b, _)| *b).collect();
        let expected: Vec<_> = ToolbarButton::TOOLS
            .iter()
            .chain(&ToolbarButton::HISTORY)
            .chain(&ToolbarButton::ACTIONS)
            .copied()
            .collect();
        assert_eq!(order, expected);
        for pair in layout.buttons.windows(2) {
            if let [(_, a), (_, b)] = pair {
                assert!(a.right() <= i64::from(b.x));
            }
        }
        for sep in &layout.separators {
            assert!(
                layout
                    .buttons
                    .iter()
                    .all(|(_, r)| r.intersect(sep).is_none())
            );
            assert!(layout.bar.intersect(sep).is_some());
        }
    }

    #[test]
    fn toolbar_moves_above_then_inside() {
        let near_bottom = Rect::new(800, 900, 600, 170);
        let bar = bar_of(&near_bottom, &MONITOR).bar;
        assert_eq!(bar.y, 900 - 6 - 40);
        let full = Rect::new(0, 0, 1920, 1080);
        let bar = bar_of(&full, &MONITOR).bar;
        assert_eq!(bar.y, 1080 - 6 - 40);
        assert_eq!(bar.x, 1920 - 566);
    }

    #[test]
    fn toolbar_is_clamped_horizontally() {
        let narrow = Rect::new(10, 100, 20, 20);
        assert_eq!(bar_of(&narrow, &MONITOR).bar.x, 0);
        let other = Rect::new(-1920, 0, 1920, 1080);
        let bar = bar_of(&Rect::new(-1910, 10, 20, 20), &other).bar;
        assert_eq!(bar.x, -1920);
    }

    #[test]
    fn toolbar_wraps_to_two_rows_on_narrow_monitor() {
        let monitor = Rect::new(0, 0, 420, 800);
        let layout = bar_of(&Rect::new(100, 100, 200, 200), &monitor);
        // 第一行 11 个按钮 + 1 条分隔 + 内边距
        assert_eq!(layout.bar, Rect::new(0, 306, 413, 72));
        assert_eq!(layout.separators.len(), 1);
        assert_eq!(layout.buttons.len(), 15);
        assert_eq!(
            layout.button_rect(ToolbarButton::Search),
            Some(Rect::new(4, 306 + 4 + 32, 36, 32))
        );
        assert_eq!(
            layout.button_rect(ToolbarButton::Tool(Tool::Rectangle)),
            Some(Rect::new(4, 310, 36, 32))
        );
        for (_, r) in &layout.buttons {
            assert_eq!(layout.bar.intersect(r), Some(*r));
        }
    }

    #[test]
    fn dpi_scales_toolbar() {
        let layout = toolbar_layout(&Rect::new(1000, 100, 800, 300), &MONITOR, 144, None);
        assert_eq!(layout.bar.height, 60);
        assert_eq!(
            layout.button_rect(ToolbarButton::Undo).map(|r| r.width),
            Some(54)
        );
    }

    #[test]
    fn hit_finds_buttons_and_ignores_gaps() {
        let layout = bar_of(&Rect::new(800, 100, 600, 300), &MONITOR);
        assert_eq!(
            layout.hit(Point::new(842, 420)),
            Some(Hit::Button(ToolbarButton::Tool(Tool::Rectangle)))
        );
        let copy = layout.button_rect(ToolbarButton::Copy).expect("复制");
        assert_eq!(
            layout.hit(Point::new(copy.x + 1, copy.y + 1)),
            Some(Hit::Button(ToolbarButton::Copy))
        );
        // 内边距和分隔线不属于任何按钮，但仍在工具栏内
        assert_eq!(layout.hit(Point::new(835, 407)), None);
        assert!(layout.covers(Point::new(835, 407)));
        assert!(!layout.covers(Point::new(833, 407)));
        assert!(!layout.covers(Point::new(835, 446)));
    }

    #[test]
    fn strip_sits_beyond_the_bar_and_flips_when_cramped() {
        let below = toolbar_layout(
            &Rect::new(800, 100, 600, 300),
            &MONITOR,
            96,
            Some(FULL_STRIP),
        );
        let strip = below.strip.expect("样式条");
        assert_eq!(strip.rect.x, below.bar.x);
        assert_eq!(strip.rect.y, 446 + 4);
        assert_eq!(strip.swatches.len(), 8);
        assert_eq!(strip.number_styles.len(), 2);
        // 内边距 + 8 个色块 + 3 个圆点（含间距）+ 数值文字 + 间距 + 两个风格按钮
        assert_eq!(strip.rect.width, 8 + 8 * 28 + 3 * 28 + 36 + 6 + 64);

        // 工具栏被挤到选区上方：样式条也在更上方
        let bottom = toolbar_layout(
            &Rect::new(800, 900, 600, 150),
            &MONITOR,
            96,
            Some(FULL_STRIP),
        );
        assert_eq!(bottom.bar.y, 854);
        assert_eq!(bottom.strip.map(|s| s.rect.y), Some(854 - 4 - 40));

        // 工具栏在选区内侧底部，下方没有空间：样式条放到上方
        let full = toolbar_layout(&Rect::new(0, 0, 1920, 1080), &MONITOR, 96, Some(FULL_STRIP));
        assert_eq!(full.bar.y, 1034);
        assert_eq!(full.strip.map(|s| s.rect.y), Some(1034 - 4 - 40));
    }

    #[test]
    fn text_strip_has_appearance_switch_and_hits_it() {
        let spec = StripSpec {
            has_color: true,
            number_style: false,
            appearance: true,
        };
        let layout = toolbar_layout(&Rect::new(800, 100, 600, 300), &MONITOR, 96, Some(spec));
        let strip = layout.strip.clone().expect("样式条");
        assert!(strip.number_styles.is_empty());
        assert_eq!(strip.appearance.len(), 2);
        // 与序号工具的「1 / A」占同样宽度
        assert_eq!(strip.rect.width, 8 + 8 * 28 + 3 * 28 + 36 + 6 + 64);
        for (background, rect) in &strip.appearance {
            let p = Point::new(rect.x + 3, rect.y + 3);
            assert_eq!(layout.hit(p), Some(Hit::Background(*background)));
            assert!(layout.covers(p));
        }
        let plain = strip.appearance.first().map(|(b, _)| *b);
        let styled = strip.appearance.get(1).map(|(b, _)| *b);
        assert_eq!((plain, styled), (Some(false), Some(true)));
    }

    #[test]
    fn strip_content_depends_on_tool() {
        let sel = Rect::new(800, 100, 600, 300);
        let plain = StripSpec {
            has_color: true,
            number_style: false,
            appearance: false,
        };
        let strip = toolbar_layout(&sel, &MONITOR, 96, Some(plain))
            .strip
            .expect("样式条");
        assert_eq!(strip.swatches.len(), 8);
        assert!(strip.number_styles.is_empty());
        let mosaic = StripSpec {
            has_color: false,
            number_style: false,
            appearance: false,
        };
        let strip = toolbar_layout(&sel, &MONITOR, 96, Some(mosaic))
            .strip
            .expect("样式条");
        assert!(strip.swatches.is_empty());
        assert_eq!(strip.presets.len(), 3);
        assert_eq!(strip.rect.width, 8 + 3 * 28 + 36);
    }

    #[test]
    fn strip_hits_swatches_size_and_number_styles() {
        let layout = toolbar_layout(
            &Rect::new(800, 100, 600, 300),
            &MONITOR,
            96,
            Some(FULL_STRIP),
        );
        let strip = layout.strip.clone().expect("样式条");
        let center = |r: &Rect| Point::new(r.x + 3, r.y + 3);
        let swatch = strip.swatches.get(3).expect("色块");
        assert_eq!(layout.hit(center(swatch)), Some(Hit::Swatch(3)));
        for (i, dot) in strip.presets.iter().enumerate() {
            assert_eq!(layout.hit(center(dot)), Some(Hit::Preset(i)));
            assert!(layout.covers(center(dot)));
        }
        // 数值文字只显示，点击不响应，但仍在样式条内
        assert_eq!(layout.hit(center(&strip.value)), None);
        assert!(layout.covers(center(&strip.value)));
        let (style, rect) = strip.number_styles.get(1).expect("风格");
        assert_eq!(layout.hit(center(rect)), Some(Hit::NumberStyle(*style)));
        assert!(layout.covers(center(swatch)));
        assert!(
            strip
                .swatches
                .windows(2)
                .all(|w| matches!(w, [a, b] if a.right() <= i64::from(b.x)))
        );
    }

    #[test]
    fn strip_stays_inside_monitor() {
        let monitor = Rect::new(0, 0, 500, 300);
        let layout = toolbar_layout(&Rect::new(400, 100, 90, 90), &monitor, 96, Some(FULL_STRIP));
        let strip = layout.strip.expect("样式条");
        assert_eq!(monitor.intersect(&strip.rect), Some(strip.rect));
        for r in strip
            .swatches
            .iter()
            .chain(&strip.presets)
            .chain([&strip.value])
        {
            assert_eq!(strip.rect.intersect(r), Some(*r));
        }
    }

    #[test]
    fn every_button_has_a_glyph_and_label() {
        let all = ToolbarButton::TOOLS
            .iter()
            .chain(&ToolbarButton::HISTORY)
            .chain(&ToolbarButton::ACTIONS);
        for button in all {
            assert!(!button.label().is_empty());
            match button.glyph() {
                Glyph::Icon(c) => assert!(c as u32 >= 0xE000),
                Glyph::Text(t) => assert!(!t.is_empty()),
            }
        }
        for tool in Tool::ALL {
            assert_eq!(
                ToolbarButton::Tool(tool).label().chars().count(),
                1,
                "{tool:?} 的退化文字应为单字"
            );
        }
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
