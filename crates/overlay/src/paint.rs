//! 标注图元的 Direct2D 绘制：屏幕显示与离屏合成共用同一套代码，保证所见即所得。
//!
//! 渲染目标 DPI 固定为 96，1 个 DIP 等于 1 个物理像素，线宽直接取成图像素。

use lenswin_core::annotation::{
    Color, HIGHLIGHTER_ALPHA, PixelOrder, Shape, TEXT_BACKGROUND_ALPHA, arrow_head, mosaic_grid,
    number_label,
};
use lenswin_core::geometry::{Point, Rect};
use lenswin_core::text_box::round_to_i32;
use lenswin_core::text_edit::{char_index_of_utf16, utf16_offset_of_char};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED,
    D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
    D2D1_BITMAP_PROPERTIES, D2D1_BRUSH_PROPERTIES, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_QUADRATIC_BEZIER_SEGMENT, D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_DEFAULT,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, ID2D1Brush, ID2D1Factory, ID2D1PathGeometry,
    ID2D1RenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FLOW_DIRECTION_RIGHT_TO_LEFT, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_HIT_TEST_METRICS, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_READING_DIRECTION_TOP_TO_BOTTOM, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_METRICS,
    DWRITE_TEXT_RANGE, DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP,
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
    IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::core::{BOOL, w};

use crate::annotate::{ApproxMeasure, TextLayout, TextMeasure, text_padding};
use crate::error::{OverlayError, win32};
use crate::render::{color, f32_to_u16, float_point, to_f32};
use crate::stroke::{self, PointF, Segment};

/// 文字字体，缺字时 DirectWrite 自动回退。
const TEXT_FONT: windows::core::PCWSTR = w!("Microsoft YaHei UI");
/// 文字排版区域的边长上限，不自动换行，只需要足够大。
const TEXT_LAYOUT_LIMIT: f32 = 16_384.0;
/// 底色圆角半径（像素）。
const TEXT_BACKGROUND_RADIUS: f32 = 4.0;

/// 序号文字大小相对圆直径的比例。
const NUMBER_FONT_SCALE: f32 = 0.5;

/// 马赛克取色用的冻结帧：显示器范围与整屏 BGRA 数据。
#[derive(Clone, Copy)]
pub(crate) struct FrameRef<'a> {
    /// 显示器在虚拟桌面上的范围。
    pub bounds: Rect,
    /// 整屏 BGRA 像素。
    pub bgra: &'a [u8],
}

/// 在某个渲染目标上绘制图元；只能在该目标的 BeginDraw 与 EndDraw 之间使用。
pub(crate) struct Painter<'a> {
    pub target: &'a ID2D1RenderTarget,
    pub factory: &'a ID2D1Factory,
    pub dwrite: &'a IDWriteFactory,
    /// 圆头圆角线型。
    pub round_stroke: &'a ID2D1StrokeStyle,
    /// 方头线型，荧光笔用。
    pub flat_stroke: &'a ID2D1StrokeStyle,
    /// 可供马赛克取色的冻结帧。
    pub frames: &'a [FrameRef<'a>],
}

/// 虚拟桌面坐标转目标坐标。
fn local(p: Point, origin: Point) -> PointF {
    (
        to_f32(i64::from(p.x) - i64::from(origin.x)),
        to_f32(i64::from(p.y) - i64::from(origin.y)),
    )
}

/// 两个对角点围出的矩形（目标坐标）。
fn box_rect(from: Point, to: Point, origin: Point) -> D2D_RECT_F {
    let (ax, ay) = local(from, origin);
    let (bx, by) = local(to, origin);
    D2D_RECT_F {
        left: ax.min(bx),
        top: ay.min(by),
        right: ax.max(bx),
        bottom: ay.max(by),
    }
}

/// 一段要绘制的文字：位置、内容与外观。
pub(crate) struct TextBlock<'a> {
    /// 未旋转的本地框（虚拟桌面坐标），底色模式下含内边距；旋转中心是框中心。
    pub bounds: Rect,
    /// 顺时针旋转角度（度）。
    pub rotation: i32,
    /// 换行长度。
    pub wrap: Option<u32>,
    /// 竖排：列从右往左排。
    pub vertical: bool,
    /// 内容。
    pub text: &'a str,
    /// 字号。
    pub size: u32,
    /// 文字颜色，底色模式下是底色。
    pub color: Color,
    /// 是否带底色。
    pub background: bool,
    /// 要加下划线的 UTF-16 范围（组字串）。
    pub underline: Option<(usize, usize)>,
}

/// 按字号和排版参数建立文字排版。
///
/// 竖排用自上而下的阅读方向加自右向左的流向；`width_hint` 是竖排时排版区域的宽度，
/// 列从这个宽度的右缘起排，不给时取一个足够大的值。
pub(crate) fn text_layout(
    dwrite: &IDWriteFactory,
    text: &str,
    size: u32,
    spec: TextLayout,
    width_hint: Option<u32>,
) -> Result<IDWriteTextLayout, OverlayError> {
    // SAFETY: 字体名与区域名为静态字符串；字体不存在时 DirectWrite 自动回退。
    let format = unsafe {
        dwrite.CreateTextFormat(
            TEXT_FONT,
            None::<&IDWriteFontCollection>,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            to_f32(i64::from(size)),
            w!("zh-CN"),
        )
    }
    .map_err(win32("CreateTextFormat"))?;
    let wrapping = if spec.wrap.is_some() {
        DWRITE_WORD_WRAPPING_WRAP
    } else {
        DWRITE_WORD_WRAPPING_NO_WRAP
    };
    // SAFETY: format 刚刚创建，参数为合法枚举值。
    unsafe { format.SetWordWrapping(wrapping) }.map_err(win32("SetWordWrapping"))?;
    if spec.vertical {
        // SAFETY: 同上。
        unsafe { format.SetReadingDirection(DWRITE_READING_DIRECTION_TOP_TO_BOTTOM) }
            .map_err(win32("SetReadingDirection"))?;
        // SAFETY: 同上。
        unsafe { format.SetFlowDirection(DWRITE_FLOW_DIRECTION_RIGHT_TO_LEFT) }
            .map_err(win32("SetFlowDirection"))?;
    }
    let limit = |v: Option<u32>| v.map_or(TEXT_LAYOUT_LIMIT, |v| to_f32(i64::from(v)));
    let (max_width, max_height) = if spec.vertical {
        (limit(width_hint), limit(spec.wrap))
    } else {
        (limit(spec.wrap), TEXT_LAYOUT_LIMIT)
    };
    let wide: Vec<u16> = text.encode_utf16().collect();
    // SAFETY: wide 与 format 在调用期间有效。
    unsafe { dwrite.CreateTextLayout(&wide, &format, max_width, max_height) }
        .map_err(win32("CreateTextLayout"))
}

/// 排版后文字区域的（左, 上, 宽, 高）；竖排时区域靠排版宽度的右缘，所以左边不一定是 0。
fn layout_extent(layout: &IDWriteTextLayout) -> Result<(f32, f32, f32, f32), OverlayError> {
    let mut metrics = DWRITE_TEXT_METRICS::default();
    // SAFETY: metrics 是有效的输出位置。
    unsafe { layout.GetMetrics(&mut metrics) }.map_err(win32("GetMetrics"))?;
    Ok((
        metrics.left,
        metrics.top,
        metrics.widthIncludingTrailingWhitespace,
        metrics.height,
    ))
}

/// 基于 DirectWrite 的文字度量，屏幕绘制与成图共用同一套排版，保证命中范围与所见一致。
pub(crate) struct DWriteMeasure {
    dwrite: IDWriteFactory,
}

fn px(value: f32) -> u32 {
    u32::from(f32_to_u16(value.max(0.0).ceil()))
}

impl DWriteMeasure {
    /// 创建共享的 DirectWrite 工厂。
    pub(crate) fn new() -> Result<Self, OverlayError> {
        // SAFETY: 参数为合法的枚举值。
        let dwrite = unsafe { DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) }
            .map_err(win32("DWriteCreateFactory"))?;
        Ok(Self { dwrite })
    }

    /// 复用已有的工厂。
    pub(crate) fn from_factory(dwrite: IDWriteFactory) -> Self {
        Self { dwrite }
    }

    fn try_measure(
        &self,
        text: &str,
        size: u32,
        spec: TextLayout,
    ) -> Result<(u32, u32), OverlayError> {
        // 空文字量不出行高，用一个空格代替，沿行方向的长度仍按 0 算
        let probe = if text.is_empty() { " " } else { text };
        let layout = text_layout(&self.dwrite, probe, size, spec, None)?;
        let (_, _, w, h) = layout_extent(&layout)?;
        let (w, h) = match (text.is_empty(), spec.vertical) {
            (true, false) => (0.0, h),
            (true, true) => (w, 0.0),
            (false, _) => (w, h),
        };
        Ok((px(w), px(h)))
    }

    /// 光标处字符的（左, 上, 宽, 高），排版坐标，未减去文字区域的偏移。
    /// 竖排自右向左流动时系统返回的是字符框的右缘，这里换成左缘。
    fn hit_position(
        layout: &IDWriteTextLayout,
        text: &str,
        char_index: usize,
        vertical: bool,
    ) -> Result<(f32, f32, f32, f32), OverlayError> {
        let position = u32::try_from(utf16_offset_of_char(text, char_index)).unwrap_or(u32::MAX);
        let (mut x, mut y) = (0.0f32, 0.0f32);
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        // SAFETY: 三个输出参数都指向有效的局部变量。
        unsafe { layout.HitTestTextPosition(position, false, &mut x, &mut y, &mut metrics) }
            .map_err(win32("HitTestTextPosition"))?;
        let left = if vertical { x - metrics.width } else { x };
        Ok((left, y, metrics.width, metrics.height))
    }

    fn try_caret(
        &self,
        text: &str,
        size: u32,
        spec: TextLayout,
        char_index: usize,
    ) -> Result<(i32, i32, u32), OverlayError> {
        let layout = text_layout(&self.dwrite, text, size, spec, None)?;
        let (left, top, _, _) = layout_extent(&layout)?;
        let (x, y, w, h) = Self::hit_position(&layout, text, char_index, spec.vertical)?;
        let thickness = if spec.vertical { w } else { h };
        Ok((
            round_to_i32(f64::from(x - left)),
            round_to_i32(f64::from(y - top)),
            px(thickness),
        ))
    }

    fn try_step(
        &self,
        text: &str,
        size: u32,
        spec: TextLayout,
        char_index: usize,
        forward: bool,
    ) -> Result<Option<usize>, OverlayError> {
        let layout = text_layout(&self.dwrite, text, size, spec, None)?;
        let (x, y, w, h) = Self::hit_position(&layout, text, char_index, spec.vertical)?;
        // 取下一行（列）正中的一点：横排向下，竖排向左（列从右往左排）
        let (tx, ty) = match (spec.vertical, forward) {
            (false, true) => (x + 1.0, y + h * 1.5),
            (false, false) => (x + 1.0, y - h * 0.5),
            (true, true) => (x - w * 0.5, y + 1.0),
            (true, false) => (x + w * 1.5, y + 1.0),
        };
        let (mut trailing, mut inside) = (BOOL::default(), BOOL::default());
        let mut hit = DWRITE_HIT_TEST_METRICS::default();
        // SAFETY: 三个输出参数都指向有效的局部变量。
        unsafe { layout.HitTestPoint(tx, ty, &mut trailing, &mut inside, &mut hit) }
            .map_err(win32("HitTestPoint"))?;
        let unit = hit.textPosition + if trailing.as_bool() { hit.length } else { 0 };
        let target = char_index_of_utf16(text, usize::try_from(unit).unwrap_or(usize::MAX));
        // 目标落在同一行（列）说明已经到头
        let (nx, ny, _, _) = Self::hit_position(&layout, text, target, spec.vertical)?;
        let moved = if spec.vertical {
            (nx - x).abs() >= 1.0
        } else {
            (ny - y).abs() >= 1.0
        };
        Ok((moved && target != char_index).then_some(target))
    }
}

impl TextMeasure for DWriteMeasure {
    fn measure(&self, text: &str, size: u32, layout: TextLayout) -> (u32, u32) {
        // 度量失败时退回近似值，命中范围略有偏差但不影响成图
        self.try_measure(text, size, layout)
            .unwrap_or_else(|_| ApproxMeasure.measure(text, size, layout))
    }

    fn caret(
        &self,
        text: &str,
        size: u32,
        layout: TextLayout,
        char_index: usize,
    ) -> (i32, i32, u32) {
        self.try_caret(text, size, layout, char_index)
            .unwrap_or_else(|_| ApproxMeasure.caret(text, size, layout, char_index))
    }

    fn step_block(
        &self,
        text: &str,
        size: u32,
        layout: TextLayout,
        char_index: usize,
        forward: bool,
    ) -> Option<usize> {
        // 没有排版就当作到头，调用方退回按逻辑行移动
        self.try_step(text, size, layout, char_index, forward)
            .ok()
            .flatten()
    }
}

/// 绕 `center` 顺时针旋转 `degrees` 度的变换，目标坐标；用画刷属性结构取得矩阵类型。
fn rotation_properties(center: (f32, f32), degrees: i32) -> D2D1_BRUSH_PROPERTIES {
    let radians = f32::from(i16::try_from(degrees.rem_euclid(360)).unwrap_or(0)).to_radians();
    let (s, c) = radians.sin_cos();
    let mut props = D2D1_BRUSH_PROPERTIES::default();
    props.transform.M11 = c;
    props.transform.M12 = s;
    props.transform.M21 = -s;
    props.transform.M22 = c;
    props.transform.M31 = center.0 - center.0 * c + center.1 * s;
    props.transform.M32 = center.1 - center.0 * s - center.1 * c;
    props
}

fn channel(value: u8) -> f32 {
    f32::from(value) / 255.0
}

fn solid(c: Color, alpha: f32) -> D2D1_COLOR_F {
    color(channel(c.r), channel(c.g), channel(c.b), alpha)
}

/// 平滑后的轨迹曲线，点数不足两个时返回 `None`。
pub(crate) fn smooth_path(
    factory: &ID2D1Factory,
    points: &[PointF],
) -> Result<Option<ID2D1PathGeometry>, OverlayError> {
    let Some((start, segments)) = stroke::smooth(points) else {
        return Ok(None);
    };
    // SAFETY: 工厂有效，无参数。
    let path = unsafe { factory.CreatePathGeometry() }.map_err(win32("CreatePathGeometry"))?;
    // SAFETY: path 刚创建，只打开一次。
    let sink = unsafe { path.Open() }.map_err(win32("ID2D1PathGeometry::Open"))?;
    // SAFETY: sink 处于打开状态，BeginFigure 与 EndFigure 成对调用，贝塞尔结构为局部变量。
    unsafe {
        sink.BeginFigure(
            float_point(start.0, start.1).point,
            D2D1_FIGURE_BEGIN_HOLLOW,
        );
        for segment in &segments {
            match segment {
                Segment::Quad { control, end } => {
                    let bezier = D2D1_QUADRATIC_BEZIER_SEGMENT {
                        point1: float_point(control.0, control.1).point,
                        point2: float_point(end.0, end.1).point,
                    };
                    sink.AddQuadraticBezier(&bezier);
                }
                Segment::Line { end } => sink.AddLine(float_point(end.0, end.1).point),
            }
        }
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
    }
    // SAFETY: 与上面的 Open 配对。
    unsafe { sink.Close() }.map_err(win32("ID2D1GeometrySink::Close"))?;
    Ok(Some(path))
}

/// 实心三角形路径。
fn triangle_path(
    factory: &ID2D1Factory,
    a: PointF,
    b: PointF,
    c: PointF,
) -> Result<ID2D1PathGeometry, OverlayError> {
    // SAFETY: 工厂有效，无参数。
    let path = unsafe { factory.CreatePathGeometry() }.map_err(win32("CreatePathGeometry"))?;
    // SAFETY: path 刚创建，只打开一次。
    let sink = unsafe { path.Open() }.map_err(win32("ID2D1PathGeometry::Open"))?;
    // SAFETY: sink 处于打开状态，BeginFigure 与 EndFigure 成对调用。
    unsafe {
        sink.BeginFigure(float_point(a.0, a.1).point, D2D1_FIGURE_BEGIN_FILLED);
        sink.AddLine(float_point(b.0, b.1).point);
        sink.AddLine(float_point(c.0, c.1).point);
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
    }
    // SAFETY: 与上面的 Open 配对。
    unsafe { sink.Close() }.map_err(win32("ID2D1GeometrySink::Close"))?;
    Ok(path)
}

impl Painter<'_> {
    /// 按顺序绘制一组图元，`origin` 是目标左上角对应的虚拟桌面坐标。
    pub(crate) fn draw_shapes<'s>(
        &self,
        shapes: impl IntoIterator<Item = &'s Shape>,
        origin: Point,
    ) -> Result<(), OverlayError> {
        let mut shapes = shapes.into_iter().peekable();
        if shapes.peek().is_none() {
            return Ok(());
        }
        let brush = self.new_brush()?;
        // 文字统一用灰度抗锯齿：缓存层是透明位图，ClearType 在上面会出彩边，屏幕与成图也要一致
        // SAFETY: 目标有效，参数为合法枚举值。
        unsafe {
            self.target
                .SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        }
        let drawn = shapes.try_for_each(|shape| self.draw_shape(&brush, shape, origin));
        // SAFETY: 同上；还原后工具栏等界面文字保持默认外观。
        unsafe {
            self.target
                .SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_DEFAULT);
        }
        drawn
    }

    /// 新建一支可改色的实心画刷。
    pub(crate) fn new_brush(&self) -> Result<ID2D1SolidColorBrush, OverlayError> {
        // SAFETY: 目标有效，颜色结构为局部变量。
        unsafe {
            self.target
                .CreateSolidColorBrush(&color(0.0, 0.0, 0.0, 1.0), None)
        }
        .map_err(win32("CreateSolidColorBrush"))
    }

    /// 设置绕 `center`（目标坐标）顺时针旋转 `degrees` 度的变换，画完后要调用 [`Painter::reset_transform`]。
    pub(crate) fn set_rotation(&self, center: (f32, f32), degrees: i32) {
        let props = rotation_properties(center, degrees);
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，矩阵为局部变量。
        unsafe { self.target.SetTransform(&props.transform) };
    }

    /// 恢复单位变换。
    pub(crate) fn reset_transform(&self) {
        let mut props = D2D1_BRUSH_PROPERTIES::default();
        props.transform.M11 = 1.0;
        props.transform.M22 = 1.0;
        // SAFETY: 同 set_rotation。
        unsafe { self.target.SetTransform(&props.transform) };
    }

    /// 绘制一段文字；底色模式先画 85% 不透明的圆角底色，文字用与序号一致的对比色。
    /// 整块（底色与文字）绕框中心旋转。调用方负责设置灰度抗锯齿。
    pub(crate) fn draw_text_block(
        &self,
        brush: &ID2D1SolidColorBrush,
        block: &TextBlock<'_>,
        origin: Point,
    ) -> Result<(), OverlayError> {
        let spec = TextLayout {
            wrap: block.wrap,
            vertical: block.vertical,
        };
        let pad_px = text_padding(block.size, block.background);
        let inner_width = block.bounds.width.saturating_sub(pad_px.saturating_mul(2));
        let layout = text_layout(self.dwrite, block.text, block.size, spec, Some(inner_width))?;
        if let Some((start, length)) = block.underline {
            let range = DWRITE_TEXT_RANGE {
                startPosition: u32::try_from(start).unwrap_or(0),
                length: u32::try_from(length).unwrap_or(0),
            };
            // SAFETY: layout 刚创建，range 为局部变量。
            unsafe { layout.SetUnderline(true, range) }.map_err(win32("SetUnderline"))?;
        }
        let pad = to_f32(i64::from(pad_px));
        let (left, top) = local(Point::new(block.bounds.x, block.bounds.y), origin);
        let (width, height) = (
            to_f32(i64::from(block.bounds.width)),
            to_f32(i64::from(block.bounds.height)),
        );
        self.set_rotation((left + width / 2.0, top + height / 2.0), block.rotation);
        let text_color = if block.background {
            let rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left,
                    top,
                    right: left + width,
                    bottom: top + height,
                },
                radiusX: TEXT_BACKGROUND_RADIUS,
                radiusY: TEXT_BACKGROUND_RADIUS,
            };
            // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷与圆角矩形有效。
            unsafe {
                brush.SetColor(&solid(block.color, TEXT_BACKGROUND_ALPHA));
                self.target.FillRoundedRectangle(&rect, brush);
            }
            block.color.label_color()
        } else {
            block.color
        };
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，排版与画刷有效。
        unsafe {
            brush.SetColor(&solid(text_color, 1.0));
            self.target.DrawTextLayout(
                float_point(left + pad, top + pad).point,
                &layout,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
        self.reset_transform();
        Ok(())
    }

    fn draw_shape(
        &self,
        brush: &ID2D1SolidColorBrush,
        shape: &Shape,
        origin: Point,
    ) -> Result<(), OverlayError> {
        match shape {
            Shape::Rectangle {
                from,
                to,
                color: c,
                width,
            } => {
                let rect = box_rect(*from, *to, origin);
                // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷有效，矩形为局部变量。
                unsafe {
                    brush.SetColor(&solid(*c, 1.0));
                    self.target
                        .DrawRectangle(&rect, brush, to_f32(i64::from(*width)), None);
                }
            }
            Shape::Ellipse {
                from,
                to,
                color: c,
                width,
            } => {
                let rect = box_rect(*from, *to, origin);
                let mut ellipse = float_point(
                    (rect.left + rect.right) / 2.0,
                    (rect.top + rect.bottom) / 2.0,
                );
                ellipse.radiusX = (rect.right - rect.left) / 2.0;
                ellipse.radiusY = (rect.bottom - rect.top) / 2.0;
                // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷有效，椭圆为局部变量。
                unsafe {
                    brush.SetColor(&solid(*c, 1.0));
                    self.target
                        .DrawEllipse(&ellipse, brush, to_f32(i64::from(*width)), None);
                }
            }
            Shape::Line {
                from,
                to,
                color: c,
                width,
            } => {
                let (a, b) = (local(*from, origin), local(*to, origin));
                // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷与线型有效。
                unsafe {
                    brush.SetColor(&solid(*c, 1.0));
                    self.target.DrawLine(
                        float_point(a.0, a.1).point,
                        float_point(b.0, b.1).point,
                        brush,
                        to_f32(i64::from(*width)),
                        self.round_stroke,
                    );
                }
            }
            Shape::Arrow {
                from,
                to,
                color: c,
                width,
            } => self.draw_arrow(brush, (*from, *to), *c, *width, origin)?,
            Shape::Pen {
                points,
                color: c,
                width,
            } => self.draw_trail(brush, points, solid(*c, 1.0), *width, false, origin)?,
            Shape::Highlighter {
                points,
                color: c,
                width,
            } => self.draw_trail(
                brush,
                points,
                solid(*c, HIGHLIGHTER_ALPHA),
                *width,
                true,
                origin,
            )?,
            Shape::Mosaic { from, to, block } => self.draw_mosaic(*from, *to, *block, origin)?,
            Shape::Text {
                text,
                color: c,
                size,
                background,
                bounds,
                rotation,
                wrap,
                vertical,
                ..
            } => self.draw_text_block(
                brush,
                &TextBlock {
                    bounds: *bounds,
                    rotation: *rotation,
                    wrap: *wrap,
                    vertical: *vertical,
                    text,
                    size: *size,
                    color: *c,
                    background: *background,
                    underline: None,
                },
                origin,
            )?,
            Shape::Number {
                center,
                value,
                style,
                color: c,
                size,
            } => {
                let label = number_label(*value, *style);
                self.draw_number(brush, *center, &label, *c, *size, origin)?;
            }
        }
        Ok(())
    }

    fn draw_arrow(
        &self,
        brush: &ID2D1SolidColorBrush,
        (from, to): (Point, Point),
        c: lenswin_core::annotation::Color,
        width: u32,
        origin: Point,
    ) -> Result<(), OverlayError> {
        let (a, tip) = (local(from, origin), local(to, origin));
        let Some(head) = arrow_head(from, to, width) else {
            return Ok(());
        };
        let at = |offset: (f32, f32)| (tip.0 + offset.0, tip.1 + offset.1);
        let shaft_end = at(head.shaft_end);
        let triangle = triangle_path(self.factory, tip, at(head.left), at(head.right))?;
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷、线型与几何体有效。
        unsafe {
            brush.SetColor(&solid(c, 1.0));
            self.target.DrawLine(
                float_point(a.0, a.1).point,
                float_point(shaft_end.0, shaft_end.1).point,
                brush,
                to_f32(i64::from(width)),
                self.round_stroke,
            );
            self.target
                .FillGeometry(&triangle, brush, None::<&ID2D1Brush>);
        }
        Ok(())
    }

    /// 整条轨迹作为一个几何体描边，半透明的荧光笔自身重叠处不会加深。
    fn draw_trail(
        &self,
        brush: &ID2D1SolidColorBrush,
        points: &[Point],
        paint: windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F,
        width: u32,
        flat_caps: bool,
        origin: Point,
    ) -> Result<(), OverlayError> {
        let local_points: Vec<PointF> = points.iter().map(|p| local(*p, origin)).collect();
        let Some(path) = smooth_path(self.factory, &local_points)? else {
            return Ok(());
        };
        let stroke = if flat_caps {
            self.flat_stroke
        } else {
            self.round_stroke
        };
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷、线型与几何体有效。
        unsafe {
            brush.SetColor(&paint);
            self.target
                .DrawGeometry(&path, brush, to_f32(i64::from(width)), stroke);
        }
        Ok(())
    }

    /// 马赛克：块色缩成一像素一块的小位图，最近邻放大后裁到区域内。
    fn draw_mosaic(
        &self,
        from: Point,
        to: Point,
        block: u32,
        origin: Point,
    ) -> Result<(), OverlayError> {
        let Some(area) = Rect::bounding(&[from, to]) else {
            return Ok(());
        };
        let Some(frame) = self
            .frames
            .iter()
            .find(|f| f.bounds.intersect(&area).is_some())
        else {
            return Ok(());
        };
        let Some(in_frame) = Rect::from_edges(
            i64::from(area.x) - i64::from(frame.bounds.x),
            i64::from(area.y) - i64::from(frame.bounds.y),
            area.right() - i64::from(frame.bounds.x),
            area.bottom() - i64::from(frame.bounds.y),
        ) else {
            return Ok(());
        };
        let Some(grid) = mosaic_grid(
            frame.bgra,
            (frame.bounds.width, frame.bounds.height),
            PixelOrder::Bgra,
            &in_frame,
            block,
        ) else {
            return Ok(());
        };
        let mut pixels = Vec::with_capacity(grid.colors.len().saturating_mul(4));
        for c in &grid.colors {
            pixels.extend_from_slice(&[c.b, c.g, c.r, 255]);
        }
        let pitch = grid
            .cols
            .checked_mul(4)
            .ok_or_else(|| OverlayError::InvalidInput("马赛克行字节数溢出".to_string()))?;
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        // SAFETY: pixels 长度为列数×行数×4，pitch 与之一致；调用期间数据有效且会被复制。
        let bitmap = unsafe {
            self.target.CreateBitmap(
                D2D_SIZE_U {
                    width: grid.cols,
                    height: grid.rows,
                },
                Some(pixels.as_ptr().cast()),
                pitch,
                &props,
            )
        }
        .map_err(win32("CreateBitmap"))?;
        // 区域在目标里的左上角；块从这里起排列
        let left = i64::from(frame.bounds.x) + i64::from(grid.area.x) - i64::from(origin.x);
        let top = i64::from(frame.bounds.y) + i64::from(grid.area.y) - i64::from(origin.y);
        let clip = D2D_RECT_F {
            left: to_f32(left),
            top: to_f32(top),
            right: to_f32(left + i64::from(grid.area.width)),
            bottom: to_f32(top + i64::from(grid.area.height)),
        };
        let dst = D2D_RECT_F {
            left: clip.left,
            top: clip.top,
            right: to_f32(left + i64::from(grid.cols) * i64::from(block)),
            bottom: to_f32(top + i64::from(grid.rows) * i64::from(block)),
        };
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，位图由本目标创建，矩形为局部变量；裁剪与弹出成对。
        unsafe {
            self.target
                .PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
            self.target.DrawBitmap(
                &bitmap,
                Some(&dst),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                None,
            );
            self.target.PopAxisAlignedClip();
        }
        Ok(())
    }

    /// 序号：实心圆加居中文字。
    fn draw_number(
        &self,
        brush: &ID2D1SolidColorBrush,
        center: Point,
        label: &str,
        c: lenswin_core::annotation::Color,
        size: u32,
        origin: Point,
    ) -> Result<(), OverlayError> {
        let (cx, cy) = local(center, origin);
        let diameter = to_f32(i64::from(size));
        let radius = diameter / 2.0;
        let mut circle = float_point(cx, cy);
        circle.radiusX = radius;
        circle.radiusY = radius;
        let text_rect = D2D_RECT_F {
            left: cx - radius,
            top: cy - radius,
            right: cx + radius,
            bottom: cy + radius,
        };
        let format = self.number_format(diameter * NUMBER_FONT_SCALE)?;
        let text: Vec<u16> = label.encode_utf16().collect();
        let label_color = c.label_color();
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷、格式与文本均有效。
        unsafe {
            brush.SetColor(&solid(c, 1.0));
            self.target.FillEllipse(&circle, brush);
            brush.SetColor(&solid(label_color, 1.0));
            self.target.DrawText(
                &text,
                &format,
                &text_rect,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }

    fn number_format(&self, font_size: f32) -> Result<IDWriteTextFormat, OverlayError> {
        // SAFETY: 字体名与区域名为静态字符串；字体不存在时 DirectWrite 自动回退。
        let format = unsafe {
            self.dwrite.CreateTextFormat(
                w!("Segoe UI"),
                None::<&IDWriteFontCollection>,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                font_size,
                w!("en-US"),
            )
        }
        .map_err(win32("CreateTextFormat"))?;
        // SAFETY: format 刚刚创建，参数为合法枚举值。
        unsafe { format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER) }
            .map_err(win32("SetTextAlignment"))?;
        // SAFETY: 同上。
        unsafe { format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER) }
            .map_err(win32("SetParagraphAlignment"))?;
        Ok(format)
    }
}
