//! Direct2D 绘制：冻结帧、遮罩、选框、套索轨迹、边缘辉光、手柄、尺寸标签、工具栏与放大镜。

use lenswin_core::geometry::{Point, Rect};
use lenswin_core::snip::ColorFormat;
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED,
    D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_FILL_MODE_WINDING,
    D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
    D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_BITMAP_PROPERTIES, D2D1_CAP_STYLE_ROUND,
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_GAMMA_2_2, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_LINE_JOIN_ROUND, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE,
    D2D1_QUADRATIC_BEZIER_SEGMENT, D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES,
    D2D1CreateFactory, ID2D1Bitmap, ID2D1BitmapBrush, ID2D1Brush, ID2D1Factory,
    ID2D1HwndRenderTarget, ID2D1LinearGradientBrush, ID2D1PathGeometry, ID2D1SolidColorBrush,
    ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS,
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::core::{BOOL, PCWSTR, w};

use crate::adjust;
use crate::error::{OverlayError, win32};
use crate::glow::{self, Edge, GlowFrame, Rgb};
use crate::hint::Hint;
use crate::input::View;
use crate::layout::{self, MagnifierLayout, ToolbarButton};
use crate::modes::Preview;
use crate::stroke::{self, PointF, Segment};

/// 选框描边宽度（像素，96 DPI 下）。
const OUTLINE_WIDTH: u32 = 2;
/// 遮罩透明度。
const DIM_ALPHA: f32 = 0.45;
/// 面板圆角半径（96 DPI 下）。
const CORNER_RADIUS: u32 = 4;
/// 边缘辉光的宽度（96 DPI 下）。
const GLOW_WIDTH: u32 = 72;
/// 辉光由外向内切成的层数，层数越多渐隐越平滑。
const GLOW_STRIPS: u16 = 18;
/// 辉光最外层的不透明度。
const GLOW_ALPHA: f32 = 0.9;
/// 辉光每条边的渐变色标数。
const GLOW_STOPS: u16 = 24;
/// 炫彩选框每条边的渐变色标数。
const OUTLINE_STOPS: u16 = 8;
/// 光晕线宽相对实线的倍数。
const HALO_SCALE: f32 = 3.0;
/// 光晕的不透明度。
const HALO_ALPHA: f32 = 0.3;
/// 轨迹抽稀的最小点距（96 DPI 下），过密的点会让曲线出现锯齿。
const TRAIL_MIN_GAP: u32 = 3;
/// 笔尖光点半径相对线宽的倍数。
const TIP_SCALE: f32 = 4.0;
/// 笔尖光点的最小半径（96 DPI 下）。
const TIP_MIN_RADIUS: u32 = 12;
/// 谷歌浅蓝 `#8AB4F8`，笔尖光点的过渡色。
const TIP_MID: D2D1_COLOR_F = color(0.541, 0.706, 0.973, 0.85);

/// 按优先级尝试的图标字体。
const ICON_FONTS: [PCWSTR; 2] = [w!("Segoe Fluent Icons"), w!("Segoe MDL2 Assets")];

const fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

/// 一次绘制的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawStatus {
    /// 已呈现。
    Done,
    /// 渲染目标失效（如显卡重置），调用方需丢弃后重建。
    RecreateTarget,
}

/// 工具栏按钮用图标还是文字。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ButtonStyle {
    /// 图标字体，值为字体在 `ICON_FONTS` 中的序号。
    Icon(usize),
    /// 系统没有图标字体，退回中文文字。
    Text,
}

/// Direct2D 与 DirectWrite 工厂，整个覆盖层会话共用。
pub struct Renderer {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    button_style: ButtonStyle,
}

impl Renderer {
    /// 创建单线程工厂，并探测可用的图标字体。
    pub fn new() -> Result<Self, OverlayError> {
        // SAFETY: 参数为合法的枚举值，无选项。
        let factory =
            unsafe { D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
                .map_err(win32("D2D1CreateFactory"))?;
        // SAFETY: 参数为合法的枚举值。
        let dwrite = unsafe { DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) }
            .map_err(win32("DWriteCreateFactory"))?;
        let button_style = icon_font_index(&dwrite).map_or(ButtonStyle::Text, ButtonStyle::Icon);
        Ok(Self {
            factory,
            dwrite,
            button_style,
        })
    }

    /// 为一个窗口创建渲染目标和设备相关资源，`bgra` 为该显示器的冻结帧。
    pub fn create_target(
        &self,
        hwnd: HWND,
        size: (u32, u32),
        bgra: &[u8],
        dpi: u32,
    ) -> Result<TargetResources, OverlayError> {
        let pitch = size
            .0
            .checked_mul(4)
            .ok_or_else(|| OverlayError::InvalidInput("帧行字节数溢出".to_string()))?;
        // 渲染目标 DPI 固定为 96，使 1 个 DIP 等于 1 个物理像素
        let target_props = D2D1_RENDER_TARGET_PROPERTIES {
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: D2D_SIZE_U {
                width: size.0,
                height: size.1,
            },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        // SAFETY: hwnd 是本线程创建的有效窗口，两个属性结构均已完整填写。
        let target = unsafe {
            self.factory
                .CreateHwndRenderTarget(&target_props, &hwnd_props)
        }
        .map_err(win32("CreateHwndRenderTarget"))?;

        let bitmap_props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        // SAFETY: 调用方保证 bgra 长度为宽×高×4，pitch 与之一致；调用期间数据有效且会被复制。
        let bitmap = unsafe {
            target.CreateBitmap(
                D2D_SIZE_U {
                    width: size.0,
                    height: size.1,
                },
                Some(bgra.as_ptr().cast()),
                pitch,
                &bitmap_props,
            )
        }
        .map_err(win32("CreateBitmap"))?;
        // 画刷变换为单位矩阵，位图左上角对齐窗口原点，填充任意形状都是 1:1 原图
        // SAFETY: bitmap 由同一目标创建，使用默认属性。
        let frame_brush = unsafe { target.CreateBitmapBrush(&bitmap, None, None) }
            .map_err(win32("CreateBitmapBrush"))?;

        let brush = |c: D2D1_COLOR_F| -> Result<ID2D1SolidColorBrush, OverlayError> {
            // SAFETY: c 是有效的颜色结构，目标刚刚创建。
            unsafe { target.CreateSolidColorBrush(&c, None) }
                .map_err(win32("CreateSolidColorBrush"))
        };
        let brushes = Brushes {
            dim: brush(color(0.0, 0.0, 0.0, DIM_ALPHA))?,
            trail: brush(color(1.0, 1.0, 1.0, 0.9))?,
            paint: brush(color(1.0, 1.0, 1.0, 1.0))?,
            accent: brush(color(0.30, 0.60, 1.0, 1.0))?,
            white: brush(color(1.0, 1.0, 1.0, 1.0))?,
            panel: brush(color(0.12, 0.12, 0.13, 0.92))?,
            hover: brush(color(1.0, 1.0, 1.0, 0.16))?,
            crosshair: brush(color(0.30, 0.60, 1.0, 0.45))?,
            muted: brush(color(1.0, 1.0, 1.0, 0.72))?,
            swatch: brush(color(0.0, 0.0, 0.0, 1.0))?,
        };

        let stroke_props = D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_ROUND,
            endCap: D2D1_CAP_STYLE_ROUND,
            lineJoin: D2D1_LINE_JOIN_ROUND,
            ..Default::default()
        };
        // SAFETY: stroke_props 已完整填写，无自定义虚线。
        let round_stroke = unsafe { self.factory.CreateStrokeStyle(&stroke_props, None) }
            .map_err(win32("CreateStrokeStyle"))?;

        let px = |v: u32| to_f32(i64::from(layout::scaled(v, dpi)));
        let fonts = Fonts {
            label: self.text_format(
                w!("Segoe UI"),
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                px(12),
                DWRITE_TEXT_ALIGNMENT_CENTER,
            )?,
            info: self.text_format(
                w!("Consolas"),
                DWRITE_FONT_WEIGHT_NORMAL,
                px(12),
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?,
            hint: self.text_format(
                w!("Microsoft YaHei UI"),
                DWRITE_FONT_WEIGHT_NORMAL,
                px(12),
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?,
            button: match self.button_style {
                ButtonStyle::Icon(index) => self.text_format(
                    ICON_FONTS
                        .get(index)
                        .copied()
                        .unwrap_or(w!("Segoe MDL2 Assets")),
                    DWRITE_FONT_WEIGHT_NORMAL,
                    px(16),
                    DWRITE_TEXT_ALIGNMENT_CENTER,
                )?,
                ButtonStyle::Text => self.text_format(
                    w!("Microsoft YaHei UI"),
                    DWRITE_FONT_WEIGHT_NORMAL,
                    px(12),
                    DWRITE_TEXT_ALIGNMENT_CENTER,
                )?,
            },
        };

        Ok(TargetResources {
            target,
            factory: self.factory.clone(),
            bitmap,
            frame_brush,
            brushes,
            round_stroke,
            fonts,
            dwrite: self.dwrite.clone(),
            button_style: self.button_style,
            dpi,
        })
    }

    fn text_format(
        &self,
        family: PCWSTR,
        weight: DWRITE_FONT_WEIGHT,
        size: f32,
        align: DWRITE_TEXT_ALIGNMENT,
    ) -> Result<IDWriteTextFormat, OverlayError> {
        // SAFETY: family 与区域名为静态字符串；字体不存在时 DirectWrite 自动回退。
        let format = unsafe {
            self.dwrite.CreateTextFormat(
                family,
                None::<&IDWriteFontCollection>,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("zh-CN"),
            )
        }
        .map_err(win32("CreateTextFormat"))?;
        // SAFETY: format 刚刚创建，参数为合法枚举值。
        unsafe { format.SetTextAlignment(align) }.map_err(win32("SetTextAlignment"))?;
        // SAFETY: 同上。
        unsafe { format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER) }
            .map_err(win32("SetParagraphAlignment"))?;
        Ok(format)
    }
}

/// 系统字体集中第一个存在的图标字体序号。
fn icon_font_index(dwrite: &IDWriteFactory) -> Option<usize> {
    let mut collection: Option<IDWriteFontCollection> = None;
    // SAFETY: collection 是有效的输出位置。
    unsafe { dwrite.GetSystemFontCollection(&mut collection, false) }.ok()?;
    let collection = collection?;
    ICON_FONTS.iter().position(|name| {
        let mut index = 0u32;
        let mut exists = BOOL::from(false);
        // SAFETY: name 为静态字符串，两个输出参数指向有效局部变量。
        let found = unsafe { collection.FindFamilyName(*name, &mut index, &mut exists) };
        found.is_ok() && exists.as_bool()
    })
}

struct Brushes {
    dim: ID2D1SolidColorBrush,
    trail: ID2D1SolidColorBrush,
    /// 颜色逐段变化的画刷，每次使用前设置颜色。
    paint: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    white: ID2D1SolidColorBrush,
    panel: ID2D1SolidColorBrush,
    hover: ID2D1SolidColorBrush,
    crosshair: ID2D1SolidColorBrush,
    muted: ID2D1SolidColorBrush,
    swatch: ID2D1SolidColorBrush,
}

struct Fonts {
    label: IDWriteTextFormat,
    info: IDWriteTextFormat,
    hint: IDWriteTextFormat,
    button: IDWriteTextFormat,
}

/// 单个窗口的渲染目标与资源，目标失效时整体丢弃重建。
pub struct TargetResources {
    target: ID2D1HwndRenderTarget,
    factory: ID2D1Factory,
    bitmap: ID2D1Bitmap,
    /// 冻结帧画刷，用来按套索形状还原亮度。
    frame_brush: ID2D1BitmapBrush,
    brushes: Brushes,
    round_stroke: ID2D1StrokeStyle,
    fonts: Fonts,
    dwrite: IDWriteFactory,
    button_style: ButtonStyle,
    dpi: u32,
}

/// 绘制所需的窗口信息。
pub struct Surface<'a> {
    /// 显示器在虚拟桌面上的左上角，用于把虚拟坐标转为窗口坐标。
    pub origin: Point,
    /// 窗口像素尺寸。
    pub size: (u32, u32),
    /// 该显示器的冻结帧，取色用。
    pub bgra: &'a [u8],
    /// 套索描边宽度。
    pub stroke_width: u32,
    /// 圈选过程中是否显示外接矩形。
    pub show_lasso_bounds: bool,
    /// 辉光与炫彩描边。
    pub effects: Effects,
}

/// 搜索覆盖层的装饰效果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Effects {
    /// 当前时刻的边缘辉光，`None` 表示不画。
    pub glow: Option<GlowFrame>,
    /// 遮罩不透明度比例，出现动画期间从 0 渐变到 1。
    pub dim: f32,
    /// 轨迹和选框是否用四色渐变。
    pub colorful: bool,
}

impl Surface<'_> {
    fn monitor(&self) -> Rect {
        Rect::new(self.origin.x, self.origin.y, self.size.0, self.size.1)
    }
}

impl TargetResources {
    /// 绘制一帧。
    pub fn draw(&self, surface: &Surface<'_>, view: &View<'_>) -> Result<DrawStatus, OverlayError> {
        let full = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: to_f32(i64::from(surface.size.0)),
            bottom: to_f32(i64::from(surface.size.1)),
        };
        let region = if shows_region(&view.preview, surface.show_lasso_bounds) {
            preview_region(&view.preview, surface.origin, surface.size).map(|r| rect_to_d2d(&r))
        } else {
            None
        };
        let target = &self.target;
        // SAFETY: 目标与资源均有效；BeginDraw/EndDraw 成对调用，期间所有指针参数都指向存活的局部数据。
        unsafe {
            target.BeginDraw();
            self.bitmap_into(&full, &full, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR);
            self.brushes
                .dim
                .SetOpacity(surface.effects.dim.clamp(0.0, 1.0));
            target.FillRectangle(&full, &self.brushes.dim);
            if let Some(r) = region {
                // 选区内还原为原图亮度，源矩形与目标矩形相同即 1:1 贴图
                self.bitmap_into(&r, &r, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR);
                if !surface.effects.colorful {
                    target.DrawRectangle(&r, &self.brushes.accent, self.px(OUTLINE_WIDTH), None);
                }
            }
        }
        if let (false, Preview::Lasso { trail, .. }) = (surface.show_lasso_bounds, view.preview) {
            self.draw_lasso_region(trail, surface.origin);
        }
        let phase = surface.effects.glow.map_or(0.0, |g| g.phase);
        if let (true, Some(r)) = (surface.effects.colorful, region) {
            self.draw_colorful_outline(&r, phase);
        }
        if let Some(frame) = surface.effects.glow {
            self.draw_glow(frame, surface);
        }
        if let Preview::Lasso { trail, .. } = view.preview {
            if surface.effects.colorful {
                self.draw_blue_trail(trail, surface);
            } else {
                self.draw_trail(trail, surface);
            }
        }
        if let (true, Preview::Rect(r)) = (view.handles, view.preview) {
            self.draw_handles(&r, surface.origin);
        }
        if let Some(r) = view.label {
            self.draw_label(&r, surface);
        }
        if let Some((bar, hovered)) = view.toolbar {
            self.draw_toolbar(&bar, hovered, surface.origin);
        }
        if !view.hints.is_empty() {
            self.draw_hints(
                &view.hints,
                &hint_obstacles(view, surface, self.dpi),
                surface,
            );
        }
        if let Some((cursor, format)) = view.magnifier {
            self.draw_magnifier(cursor, format, surface);
        }
        // SAFETY: 与上面的 BeginDraw 配对。
        match unsafe { target.EndDraw(None, None) } {
            Ok(()) => Ok(DrawStatus::Done),
            Err(e) if e.code() == D2DERR_RECREATE_TARGET => Ok(DrawStatus::RecreateTarget),
            Err(source) => Err(OverlayError::Win32 {
                api: "EndDraw",
                source,
            }),
        }
    }

    fn px(&self, v: u32) -> f32 {
        to_f32(i64::from(layout::scaled(v, self.dpi)))
    }

    /// 把冻结帧的 `src` 部分画到 `dst`；只能在 BeginDraw 与 EndDraw 之间调用。
    fn bitmap_into(
        &self,
        dst: &D2D_RECT_F,
        src: &D2D_RECT_F,
        mode: D2D1_BITMAP_INTERPOLATION_MODE,
    ) {
        // SAFETY: 位图与目标同源，两个矩形指向调用期间存活的数据。
        unsafe {
            self.target
                .DrawBitmap(&self.bitmap, Some(dst), 1.0, mode, Some(src));
        }
    }

    fn draw_trail(&self, trail: &[Point], surface: &Surface<'_>) {
        let width = to_f32(i64::from(surface.stroke_width));
        for pair in trail.windows(2) {
            if let [a, b] = pair {
                // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷与线型有效。
                unsafe {
                    self.target.DrawLine(
                        local_point(*a, surface.origin).point,
                        local_point(*b, surface.origin).point,
                        &self.brushes.trail,
                        width,
                        &self.round_stroke,
                    );
                }
            }
        }
    }

    /// 套索轨迹首尾相连围成的多边形（窗口坐标），点数不足三个时返回 `None`。
    fn lasso_geometry(
        &self,
        trail: &[Point],
        origin: Point,
    ) -> Result<Option<ID2D1PathGeometry>, OverlayError> {
        let [first, rest @ ..] = trail else {
            return Ok(None);
        };
        if rest.len() < 2 {
            return Ok(None);
        }
        // SAFETY: 工厂有效，无参数。
        let path =
            unsafe { self.factory.CreatePathGeometry() }.map_err(win32("CreatePathGeometry"))?;
        // SAFETY: path 刚创建，只打开一次。
        let sink = unsafe { path.Open() }.map_err(win32("ID2D1PathGeometry::Open"))?;
        // SAFETY: sink 处于打开状态，BeginFigure 与 EndFigure 成对调用。
        unsafe {
            // 轨迹自相交时按非零环绕填充，绕圈画出的小环不会被挖空
            sink.SetFillMode(D2D1_FILL_MODE_WINDING);
            sink.BeginFigure(local_point(*first, origin).point, D2D1_FIGURE_BEGIN_FILLED);
            for p in rest {
                sink.AddLine(local_point(*p, origin).point);
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        }
        // SAFETY: 与上面的 Open 配对。
        unsafe { sink.Close() }.map_err(win32("ID2D1GeometrySink::Close"))?;
        Ok(Some(path))
    }

    /// 只把套索围出的区域还原为原图亮度。
    fn draw_lasso_region(&self, trail: &[Point], origin: Point) {
        // 提亮只是视觉反馈，几何体创建失败时保持压暗，选区本身不受影响
        let Ok(Some(path)) = self.lasso_geometry(trail, origin) else {
            return;
        };
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，几何体与画刷有效。
        unsafe {
            self.target
                .FillGeometry(&path, &self.frame_brush, None::<&ID2D1Brush>);
        }
    }

    /// 按色标创建从 `from` 到 `to` 的线性渐变画刷；只能在 BeginDraw 与 EndDraw 之间调用。
    fn gradient_brush(
        &self,
        from: (f32, f32),
        to: (f32, f32),
        stops: &[(f32, Rgb)],
    ) -> Result<ID2D1LinearGradientBrush, OverlayError> {
        let stops: Vec<D2D1_GRADIENT_STOP> = stops
            .iter()
            .map(|(position, c)| D2D1_GRADIENT_STOP {
                position: *position,
                color: color(c.r, c.g, c.b, 1.0),
            })
            .collect();
        // SAFETY: stops 在调用期间有效，枚举参数合法。
        let collection = unsafe {
            self.target
                .CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
        }
        .map_err(win32("CreateGradientStopCollection"))?;
        let mut props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES::default();
        props.startPoint.X = from.0;
        props.startPoint.Y = from.1;
        props.endPoint.X = to.0;
        props.endPoint.Y = to.1;
        // SAFETY: props 指向存活的局部变量，色标集合由同一目标创建。
        unsafe {
            self.target
                .CreateLinearGradientBrush(&props, None, &collection)
        }
        .map_err(win32("CreateLinearGradientBrush"))
    }

    /// 屏幕四边由外向内渐隐的四色光带。
    fn draw_glow(&self, frame: GlowFrame, surface: &Surface<'_>) {
        let w = to_f32(i64::from(surface.size.0));
        let h = to_f32(i64::from(surface.size.1));
        let band = self.px(GLOW_WIDTH) * frame.spread.max(0.0);
        if band <= 0.0 {
            return;
        }
        let layers = f32::from(GLOW_STRIPS);
        for edge in Edge::ALL {
            let stops = glow::edge_stops(w, h, edge, frame.phase, 1.0, GLOW_STOPS);
            let (from, to) = edge.endpoints(w, h);
            // 辉光只是装饰，画刷创建失败时跳过这条边，不影响选区
            let Ok(brush) = self.gradient_brush(from, to, &stops) else {
                continue;
            };
            for i in 0..GLOW_STRIPS {
                let inner = band * f32::from(i) / layers;
                let outer = band * f32::from(i + 1) / layers;
                // 平方衰减比线性更接近柔和的光晕
                let depth = (f32::from(i) + 0.5) / layers;
                let falloff = (1.0 - depth) * (1.0 - depth);
                let (left, top, right, bottom) = edge.strip(w, h, inner, outer);
                let rect = D2D_RECT_F {
                    left,
                    top,
                    right,
                    bottom,
                };
                // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷由本目标创建。
                unsafe {
                    brush.SetOpacity(frame.intensity * GLOW_ALPHA * falloff);
                    self.target.FillRectangle(&rect, &brush);
                }
            }
        }
    }

    /// 四色渐变选框，先画半透明光晕再画实线。
    fn draw_colorful_outline(&self, r: &D2D_RECT_F, phase: f32) {
        let w = r.right - r.left;
        let h = r.bottom - r.top;
        let offset = |p: (f32, f32)| (p.0 + r.left, p.1 + r.top);
        // 炫彩只是装饰，画刷创建失败时跳过这条边，不影响选区
        let edges: Vec<_> = Edge::ALL
            .iter()
            .filter_map(|edge| {
                let stops = glow::edge_stops(w, h, *edge, phase, 1.0, OUTLINE_STOPS);
                let (from, to) = edge.endpoints(w, h);
                let (from, to) = (offset(from), offset(to));
                self.gradient_brush(from, to, &stops)
                    .ok()
                    .map(|brush| (from, to, brush))
            })
            .collect();
        let width = self.px(OUTLINE_WIDTH);
        for (line_width, opacity) in [(width * HALO_SCALE, HALO_ALPHA), (width, 1.0)] {
            for (from, to, brush) in &edges {
                // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷与线型有效。
                unsafe {
                    brush.SetOpacity(opacity);
                    self.target.DrawLine(
                        float_point(from.0, from.1).point,
                        float_point(to.0, to.1).point,
                        brush,
                        line_width,
                        &self.round_stroke,
                    );
                }
            }
        }
    }

    /// 平滑后的轨迹曲线（窗口坐标），点数不足两个时返回 `None`。
    fn trail_geometry(&self, points: &[PointF]) -> Result<Option<ID2D1PathGeometry>, OverlayError> {
        let Some((start, segments)) = stroke::smooth(points) else {
            return Ok(None);
        };
        // SAFETY: 工厂有效，无参数。
        let path =
            unsafe { self.factory.CreatePathGeometry() }.map_err(win32("CreatePathGeometry"))?;
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

    /// 谷歌蓝平滑轨迹加笔尖光点：整条曲线一次描边，半透明光晕不会在分段处叠出亮点。
    fn draw_blue_trail(&self, trail: &[Point], surface: &Surface<'_>) {
        let points: Vec<PointF> = trail
            .iter()
            .map(|p| {
                let local = local_point(*p, surface.origin).point;
                (local.X, local.Y)
            })
            .collect();
        let points = stroke::thin(&points, self.px(TRAIL_MIN_GAP));
        let width = to_f32(i64::from(surface.stroke_width));
        let blue = glow::google_blue();
        // 轨迹只是视觉反馈，几何体创建失败时不画曲线，选区本身不受影响
        if let Ok(Some(path)) = self.trail_geometry(&points) {
            for (line_width, alpha) in [(width * HALO_SCALE, HALO_ALPHA), (width, 1.0)] {
                // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，几何体、画刷与线型有效。
                unsafe {
                    self.brushes
                        .paint
                        .SetColor(&color(blue.r, blue.g, blue.b, alpha));
                    self.target.DrawGeometry(
                        &path,
                        &self.brushes.paint,
                        line_width,
                        &self.round_stroke,
                    );
                }
            }
        }
        if let Some(tip) = points.last() {
            let radius = (width * TIP_SCALE).max(self.px(TIP_MIN_RADIUS));
            self.draw_tip(*tip, radius, blue);
        }
    }

    /// 笔尖的发光圆点：中心近白，向外过渡到浅蓝再透明。
    fn draw_tip(&self, center: PointF, radius: f32, blue: Rgb) {
        let stops = [
            D2D1_GRADIENT_STOP {
                position: 0.0,
                color: color(1.0, 1.0, 1.0, 0.95),
            },
            D2D1_GRADIENT_STOP {
                position: 0.35,
                color: TIP_MID,
            },
            D2D1_GRADIENT_STOP {
                position: 1.0,
                color: color(blue.r, blue.g, blue.b, 0.0),
            },
        ];
        // SAFETY: stops 在调用期间有效，枚举参数合法。
        let Ok(collection) = (unsafe {
            self.target
                .CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
        }) else {
            // 光点只是装饰，创建失败时不画
            return;
        };
        let mut props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
            radiusX: radius,
            radiusY: radius,
            ..Default::default()
        };
        props.center.X = center.0;
        props.center.Y = center.1;
        // SAFETY: props 指向存活的局部变量，色标集合由同一目标创建。
        let Ok(brush) = (unsafe {
            self.target
                .CreateRadialGradientBrush(&props, None, &collection)
        }) else {
            // 光点只是装饰，创建失败时不画
            return;
        };
        let mut ellipse = float_point(center.0, center.1);
        ellipse.radiusX = radius;
        ellipse.radiusY = radius;
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，椭圆与画刷有效。
        unsafe { self.target.FillEllipse(&ellipse, &brush) };
    }

    fn draw_handles(&self, rect: &Rect, origin: Point) {
        let half = i64::from(layout::handle_size(self.dpi) / 2);
        let side = i64::from(layout::handle_size(self.dpi));
        for p in adjust::handle_points(rect) {
            let left = i64::from(p.x) - i64::from(origin.x) - half;
            let top = i64::from(p.y) - i64::from(origin.y) - half;
            let r = D2D_RECT_F {
                left: to_f32(left),
                top: to_f32(top),
                right: to_f32(left + side),
                bottom: to_f32(top + side),
            };
            // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷有效。
            unsafe {
                self.target.FillRectangle(&r, &self.brushes.white);
                self.target
                    .DrawRectangle(&r, &self.brushes.accent, 1.0, None);
            }
        }
    }

    /// 文本在给定格式下的宽度（像素），测量失败时按字符数估算。
    fn text_width(&self, text: &[u16], format: &IDWriteTextFormat) -> u32 {
        let estimate = u32::try_from(text.len())
            .unwrap_or(0)
            .saturating_mul(layout::scaled(8, self.dpi));
        // SAFETY: text 与 format 在调用期间有效。
        let Ok(layout) = (unsafe { self.dwrite.CreateTextLayout(text, format, 4096.0, 256.0) })
        else {
            return estimate;
        };
        let mut metrics = DWRITE_TEXT_METRICS::default();
        // SAFETY: metrics 是有效的输出位置。
        if unsafe { layout.GetMetrics(&mut metrics) }.is_err() {
            return estimate;
        }
        let width = metrics.widthIncludingTrailingWhitespace.ceil();
        if (0.0..65_535.0).contains(&width) {
            // 已检查范围，转换不会截断
            u32::from(f32_to_u16(width))
        } else {
            estimate
        }
    }

    fn draw_label(&self, selection: &Rect, surface: &Surface<'_>) {
        let text: Vec<u16> = format!("{} × {}", selection.width, selection.height)
            .encode_utf16()
            .collect();
        let width = self.text_width(&text, &self.fonts.label);
        let rect = layout::label_rect(selection, &surface.monitor(), width, self.dpi);
        let r = local_rect(&rect, surface.origin);
        self.panel(&r);
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，文本、格式与画刷有效。
        unsafe {
            self.target.DrawText(
                &text,
                &self.fonts.label,
                &r,
                &self.brushes.white,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    fn draw_toolbar(&self, bar: &Rect, hovered: Option<ToolbarButton>, origin: Point) {
        self.panel(&local_rect(bar, origin));
        for (index, button) in ToolbarButton::ALL.iter().enumerate() {
            let Some(cell) = layout::button_rect(bar, index, self.dpi) else {
                continue;
            };
            let r = local_rect(&cell, origin);
            if hovered == Some(*button) {
                self.rounded(&r, &self.brushes.hover);
            }
            let text: Vec<u16> = match self.button_style {
                ButtonStyle::Icon(_) => button.glyph().to_string().encode_utf16().collect(),
                ButtonStyle::Text => button.label().encode_utf16().collect(),
            };
            let brush = if *button == ToolbarButton::Copy {
                &self.brushes.accent
            } else {
                &self.brushes.white
            };
            // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，文本、格式与画刷有效。
            unsafe {
                self.target.DrawText(
                    &text,
                    &self.fonts.button,
                    &r,
                    brush,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
        }
    }

    fn draw_magnifier(&self, cursor: Point, format: ColorFormat, surface: &Surface<'_>) {
        let monitor = surface.monitor();
        let MagnifierLayout { zoom, info } = layout::magnifier_layout(cursor, &monitor, self.dpi);
        let zoom_local = local_rect(&zoom, surface.origin);
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷有效。
        unsafe { self.target.FillRectangle(&zoom_local, &self.brushes.panel) };
        if let Some((src, dst)) = layout::zoom_source(cursor, &monitor, &zoom) {
            self.bitmap_into(
                &local_rect(&dst, surface.origin),
                &local_rect(&src, surface.origin),
                D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
            );
        }
        self.draw_crosshair(&zoom_local, &zoom);
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷有效。
        unsafe {
            self.target
                .DrawRectangle(&zoom_local, &self.brushes.white, 1.0, None);
        }

        let info_local = local_rect(&info, surface.origin);
        // SAFETY: 同上。
        unsafe { self.target.FillRectangle(&info_local, &self.brushes.panel) };
        let pad = self.px(6);
        let rgb = layout::pixel_at(surface.bgra, &monitor, cursor);
        let color_text = rgb.map_or_else(|| "—".to_string(), |(r, g, b)| format.format(r, g, b));
        let line_h = (info_local.bottom - info_local.top) / 2.0;
        let swatch = self.px(12);
        let first = D2D_RECT_F {
            left: info_local.left + pad,
            top: info_local.top,
            right: info_local.right - pad,
            bottom: info_local.top + line_h,
        };
        let second = D2D_RECT_F {
            left: info_local.left + pad + swatch + pad,
            top: info_local.top + line_h,
            right: info_local.right - pad,
            bottom: info_local.bottom,
        };
        self.info_text(&format!("{}, {}", cursor.x, cursor.y), &first);
        if let Some((r, g, b)) = rgb {
            let c = color(
                f32::from(r) / 255.0,
                f32::from(g) / 255.0,
                f32::from(b) / 255.0,
                1.0,
            );
            let mid = info_local.top + line_h * 1.5;
            let sw = D2D_RECT_F {
                left: info_local.left + pad,
                top: mid - swatch / 2.0,
                right: info_local.left + pad + swatch,
                bottom: mid + swatch / 2.0,
            };
            // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，c 与画刷有效。
            unsafe {
                self.brushes.swatch.SetColor(&c);
                self.target.FillRectangle(&sw, &self.brushes.swatch);
                self.target
                    .DrawRectangle(&sw, &self.brushes.white, 1.0, None);
            }
        }
        self.info_text(&color_text, &second);
    }

    /// 放大图里的十字线与中心像素框。
    fn draw_crosshair(&self, zoom_local: &D2D_RECT_F, zoom: &Rect) {
        let samples = i64::from(layout::ZOOM_SAMPLES);
        let cell = i64::from(zoom.width) / samples.max(1);
        let center = to_f32(cell * (samples / 2));
        let cell_f = to_f32(cell);
        let left = zoom_local.left + center;
        let top = zoom_local.top + center;
        let vertical = D2D_RECT_F {
            left,
            top: zoom_local.top,
            right: left + cell_f,
            bottom: zoom_local.bottom,
        };
        let horizontal = D2D_RECT_F {
            left: zoom_local.left,
            top,
            right: zoom_local.right,
            bottom: top + cell_f,
        };
        let middle = D2D_RECT_F {
            left,
            top,
            right: left + cell_f,
            bottom: top + cell_f,
        };
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷有效。
        unsafe {
            self.target
                .FillRectangle(&vertical, &self.brushes.crosshair);
            self.target
                .FillRectangle(&horizontal, &self.brushes.crosshair);
            self.target
                .DrawRectangle(&middle, &self.brushes.white, 1.0, None);
        }
    }

    /// 屏幕角落的快捷键提示面板，避开光标、选区、工具栏和放大镜。
    fn draw_hints(&self, hints: &[Hint], obstacles: &[Rect], surface: &Surface<'_>) {
        let rows: Vec<(Vec<u16>, Vec<u16>)> = hints
            .iter()
            .map(|h| {
                (
                    h.keys.encode_utf16().collect(),
                    h.desc.encode_utf16().collect(),
                )
            })
            .collect();
        let key_width = rows
            .iter()
            .map(|(k, _)| self.text_width(k, &self.fonts.label))
            .max()
            .unwrap_or(0);
        let desc_width = rows
            .iter()
            .map(|(_, d)| self.text_width(d, &self.fonts.hint))
            .max()
            .unwrap_or(0);
        let size = layout::hint_panel_size(rows.len(), key_width, desc_width, self.dpi);
        let panel = layout::hint_panel_rect(&surface.monitor(), size, obstacles, self.dpi);
        let local = local_rect(&panel, surface.origin);
        self.panel(&local);
        let m = layout::hint_metrics(self.dpi);
        let (pad, row, gap) = (
            to_f32(i64::from(m.padding)),
            to_f32(i64::from(m.row)),
            to_f32(i64::from(m.gap)),
        );
        let key_col = to_f32(i64::from(key_width)) + gap * 2.0;
        let inset = self.px(3);
        let mut top = local.top + pad;
        for (keys, desc) in &rows {
            let keycap = D2D_RECT_F {
                left: local.left + pad,
                top: top + inset,
                right: local.left + pad + key_col,
                bottom: top + row - inset,
            };
            self.rounded(&keycap, &self.brushes.hover);
            let desc_rect = D2D_RECT_F {
                left: keycap.right + gap,
                top,
                right: local.right - pad,
                bottom: top + row,
            };
            // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，文本、格式与画刷有效。
            unsafe {
                self.target.DrawText(
                    keys,
                    &self.fonts.label,
                    &keycap,
                    &self.brushes.white,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
                self.target.DrawText(
                    desc,
                    &self.fonts.hint,
                    &desc_rect,
                    &self.brushes.muted,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
            top += row;
        }
    }

    fn info_text(&self, text: &str, rect: &D2D_RECT_F) {
        let wide: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，文本、格式与画刷有效。
        unsafe {
            self.target.DrawText(
                &wide,
                &self.fonts.info,
                rect,
                &self.brushes.white,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    fn panel(&self, rect: &D2D_RECT_F) {
        self.rounded(rect, &self.brushes.panel);
    }

    fn rounded(&self, rect: &D2D_RECT_F, brush: &ID2D1SolidColorBrush) {
        let radius = self.px(CORNER_RADIUS);
        let rounded = D2D1_ROUNDED_RECT {
            rect: *rect,
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷有效。
        unsafe { self.target.FillRoundedRectangle(&rounded, brush) };
    }
}

/// 提示面板需要避开的区域：光标周围、高亮或选区、工具栏与放大镜，均为虚拟桌面坐标。
fn hint_obstacles(view: &View<'_>, surface: &Surface<'_>, dpi: u32) -> Vec<Rect> {
    let mut out = Vec::with_capacity(5);
    if let Some(cursor) = view.cursor {
        out.push(layout::cursor_box(cursor, dpi));
    }
    if let Preview::Rect(r) = view.preview {
        out.push(r);
    }
    if let Some((bar, _)) = view.toolbar {
        out.push(bar);
    }
    if let Some((cursor, _)) = view.magnifier {
        let m = layout::magnifier_layout(cursor, &surface.monitor(), dpi);
        out.push(m.zoom);
        out.push(m.info);
    }
    out
}

/// 预览是否要画外接矩形；隐藏时圈选只提亮套索围出的区域。
fn shows_region(preview: &Preview<'_>, show_lasso_bounds: bool) -> bool {
    !matches!(preview, Preview::Lasso { .. }) || show_lasso_bounds
}

/// 预览中需要还原亮度并描边的区域（窗口坐标），已限制在窗口范围内。
fn preview_region(preview: &Preview<'_>, origin: Point, size: (u32, u32)) -> Option<Rect> {
    let bounds = match preview {
        Preview::None => return None,
        Preview::Rect(r) => *r,
        Preview::Lasso { bounds, .. } => (*bounds)?,
    };
    let window = Rect::new(origin.x, origin.y, size.0, size.1);
    let clipped = bounds.intersect(&window)?;
    Some(Rect::new(
        clipped.x.checked_sub(origin.x)?,
        clipped.y.checked_sub(origin.y)?,
        clipped.width,
        clipped.height,
    ))
}

fn rect_to_d2d(r: &Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: to_f32(i64::from(r.x)),
        top: to_f32(i64::from(r.y)),
        right: to_f32(r.right()),
        bottom: to_f32(r.bottom()),
    }
}

/// 虚拟桌面坐标下的矩形转为窗口坐标。
fn local_rect(r: &Rect, origin: Point) -> D2D_RECT_F {
    let dx = i64::from(origin.x);
    let dy = i64::from(origin.y);
    D2D_RECT_F {
        left: to_f32(i64::from(r.x) - dx),
        top: to_f32(i64::from(r.y) - dy),
        right: to_f32(r.right() - dx),
        bottom: to_f32(r.bottom() - dy),
    }
}

/// 窗口坐标下的点，取其 `.point` 传给 `DrawLine`。
///
/// `DrawLine` 的点类型是 `windows_numerics::Vector2`，该 crate 不在批准的依赖内，
/// 因此借 `D2D1_ELLIPSE` 的字段来得到同一类型。
fn local_point(p: Point, origin: Point) -> D2D1_ELLIPSE {
    let mut holder = D2D1_ELLIPSE::default();
    holder.point.X = to_f32(i64::from(p.x) - i64::from(origin.x));
    holder.point.Y = to_f32(i64::from(p.y) - i64::from(origin.y));
    holder
}

/// 窗口坐标下的浮点点，取其 `.point` 传给 `DrawLine`，原因同 [`local_point`]。
fn float_point(x: f32, y: f32) -> D2D1_ELLIPSE {
    let mut holder = D2D1_ELLIPSE::default();
    holder.point.X = x;
    holder.point.Y = y;
    holder
}

/// 坐标转 `f32`。显示器像素尺寸远小于 `i16` 范围，超出的值饱和到边界即可。
fn to_f32(value: i64) -> f32 {
    let clamped = value.clamp(i64::from(i16::MIN), i64::from(i16::MAX));
    f32::from(i16::try_from(clamped).unwrap_or(0))
}

/// 非负有限的 `f32` 转 `u16`，超出范围饱和。
fn f32_to_u16(value: f32) -> u16 {
    let mut lo = 0u16;
    let mut hi = u16::MAX;
    // 不允许 `as` 截断，用二分找出不大于 value 的最大整数
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if f32::from(mid) <= value {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// RGBA 转 BGRA，Direct2D 位图使用 BGRA 排列。
pub(crate) fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for px in out.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_rgba_to_bgra() {
        assert_eq!(
            rgba_to_bgra(&[1, 2, 3, 4, 10, 20, 30, 40]),
            vec![3, 2, 1, 4, 30, 20, 10, 40]
        );
    }

    #[test]
    fn region_is_translated_to_window_coordinates() {
        let preview = Preview::Rect(Rect::new(2020, 100, 300, 200));
        let r = preview_region(&preview, Point::new(1920, 0), (1920, 1080));
        assert_eq!(r, Some(Rect::new(100, 100, 300, 200)));
    }

    #[test]
    fn region_outside_window_is_none() {
        let preview = Preview::Rect(Rect::new(100, 100, 50, 50));
        assert_eq!(
            preview_region(&preview, Point::new(1920, 0), (1920, 1080)),
            None
        );
    }

    #[test]
    fn region_is_clipped_to_window() {
        let preview = Preview::Rect(Rect::new(-100, 50, 300, 100));
        let r = preview_region(&preview, Point::new(0, 0), (1920, 1080));
        assert_eq!(r, Some(Rect::new(0, 50, 200, 100)));
    }

    #[test]
    fn lasso_region_uses_expanded_bounds() {
        let trail = [Point::new(110, 110), Point::new(150, 150)];
        let preview = Preview::Lasso {
            trail: &trail,
            bounds: Some(Rect::new(98, 98, 64, 64)),
        };
        assert_eq!(
            preview_region(&preview, Point::new(0, 0), (1920, 1080)),
            Some(Rect::new(98, 98, 64, 64))
        );
        let no_bounds = Preview::Lasso {
            trail: &trail,
            bounds: None,
        };
        assert_eq!(
            preview_region(&no_bounds, Point::new(0, 0), (1920, 1080)),
            None
        );
    }

    #[test]
    fn lasso_bounds_follow_switch_but_rect_always_shows() {
        let trail = [Point::new(110, 110), Point::new(150, 150)];
        let lasso = Preview::Lasso {
            trail: &trail,
            bounds: Some(Rect::new(98, 98, 64, 64)),
        };
        let rect = Preview::Rect(Rect::new(0, 0, 10, 10));
        assert!(!shows_region(&lasso, false));
        assert!(shows_region(&lasso, true));
        assert!(shows_region(&rect, false));
        assert!(shows_region(&rect, true));
        assert!(shows_region(&Preview::None, false));
    }

    #[test]
    fn local_rect_translates_by_origin() {
        let r = local_rect(&Rect::new(2000, 10, 20, 30), Point::new(1920, 0));
        assert_eq!(
            (r.left, r.top, r.right, r.bottom),
            (80.0, 10.0, 100.0, 40.0)
        );
    }

    #[test]
    fn to_f32_saturates() {
        assert_eq!(to_f32(100), 100.0);
        assert_eq!(to_f32(-100), -100.0);
        assert_eq!(to_f32(i64::MAX), f32::from(i16::MAX));
        assert_eq!(to_f32(i64::MIN), f32::from(i16::MIN));
    }

    #[test]
    fn f32_to_u16_floors_and_saturates() {
        assert_eq!(f32_to_u16(0.0), 0);
        assert_eq!(f32_to_u16(41.7), 41);
        assert_eq!(f32_to_u16(70_000.0), u16::MAX);
    }
}
