//! 标注图元的 Direct2D 绘制：屏幕显示与离屏合成共用同一套代码，保证所见即所得。
//!
//! 渲染目标 DPI 固定为 96，1 个 DIP 等于 1 个物理像素，线宽直接取成图像素。

use lenswin_core::annotation::{
    HIGHLIGHTER_ALPHA, PixelOrder, Shape, arrow_head, mosaic_grid, number_label,
};
use lenswin_core::geometry::{Point, Rect};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_FIGURE_BEGIN_FILLED,
    D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
    D2D1_BITMAP_PROPERTIES, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_QUADRATIC_BEZIER_SEGMENT, ID2D1Brush,
    ID2D1Factory, ID2D1PathGeometry, ID2D1RenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER,
    IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::core::w;

use crate::error::{OverlayError, win32};
use crate::render::{color, float_point, to_f32};
use crate::stroke::{self, PointF, Segment};

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

fn channel(value: u8) -> f32 {
    f32::from(value) / 255.0
}

fn solid(
    c: lenswin_core::annotation::Color,
    alpha: f32,
) -> windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F {
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
    pub(crate) fn draw_shapes(&self, shapes: &[Shape], origin: Point) -> Result<(), OverlayError> {
        if shapes.is_empty() {
            return Ok(());
        }
        // SAFETY: 目标有效，颜色结构为局部变量。
        let brush = unsafe {
            self.target
                .CreateSolidColorBrush(&color(0.0, 0.0, 0.0, 1.0), None)
        }
        .map_err(win32("CreateSolidColorBrush"))?;
        for shape in shapes {
            self.draw_shape(&brush, shape, origin)?;
        }
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
