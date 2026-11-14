//! Direct2D 绘制：冻结帧、遮罩、选框、套索轨迹、边缘辉光、手柄、尺寸标签、工具栏与放大镜。

use std::cell::RefCell;

use lenswin_core::annotation::{PALETTE, Shape};
use lenswin_core::frame::CroppedImage;
use lenswin_core::geometry::{Point, Rect};
use lenswin_core::snip::ColorFormat;
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F,
    D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW,
    D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_FILL_MODE_WINDING, D2D1_GRADIENT_STOP,
    D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1GaussianBlur, D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_INTERPOLATION_MODE,
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
    D2D1_BITMAP_PROPERTIES, D2D1_CAP_STYLE_FLAT, D2D1_CAP_STYLE_ROUND,
    D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE,
    D2D1_EXTEND_MODE_CLAMP, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_GAMMA_2_2,
    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_LINE_JOIN_ROUND, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES,
    D2D1_PRESENT_OPTIONS_NONE, D2D1_PROPERTY_TYPE_FLOAT, D2D1_QUADRATIC_BEZIER_SEGMENT,
    D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES, D2D1_RENDER_TARGET_PROPERTIES, D2D1_ROUNDED_RECT,
    D2D1_STROKE_STYLE_PROPERTIES, D2D1CreateFactory, ID2D1Bitmap, ID2D1BitmapBrush,
    ID2D1BitmapRenderTarget, ID2D1Brush, ID2D1DeviceContext, ID2D1Factory, ID2D1HwndRenderTarget,
    ID2D1LinearGradientBrush, ID2D1PathGeometry, ID2D1RenderTarget, ID2D1SolidColorBrush,
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
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmap, IWICImagingFactory,
    WICBitmapCacheOnLoad, WICBitmapLockRead, WICRect,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::core::{BOOL, Interface, PCWSTR, w};

use crate::adjust;
use crate::annotate::StyleView;
use crate::error::{OverlayError, win32};
use crate::glow::{self, Edge, GlowFrame, Rgb};
use crate::hint::Hint;
use crate::input::{Annotations, FrameStyle, ToolbarView, View};
use crate::layout::{self, Glyph, Hit, MagnifierLayout, ToolbarButton};
use crate::modes::Preview;
use crate::paint::{self, FrameRef, Painter};
use crate::stroke::{self, PointF};

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
/// 角标的角点、第一段方向、第二段方向（窗口坐标，方向为单位向量）。
type Corner = ((f32, f32), (f32, f32), (f32, f32));

/// 样式条里细、中、粗三个预设圆点的直径（96 DPI 下）。
const PRESET_DOT_SIZES: [u32; 3] = [6, 10, 14];
/// 常驻选框角标线宽（96 DPI 下）。
const CORNER_LINE: u32 = 6;
/// 常驻选框外侧光晕的宽度（96 DPI 下）。
const BOX_GLOW_WIDTH: u32 = 28;
/// 模糊前四色描边的线宽相对光晕宽度的比例。
const BOX_GLOW_SOURCE: f32 = 0.6;
/// 模糊前四色描边的不透明度。
const BOX_GLOW_ALPHA: f32 = 0.95;
/// 谷歌浅蓝 `#8AB4F8`，笔尖光点的过渡色。
const TIP_MID: D2D1_COLOR_F = color(0.541, 0.706, 0.973, 0.85);

/// 按优先级尝试的图标字体。
const ICON_FONTS: [PCWSTR; 2] = [w!("Segoe Fluent Icons"), w!("Segoe MDL2 Assets")];

pub(crate) const fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
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
            active: brush(color(0.30, 0.60, 1.0, 0.40))?,
            disabled: brush(color(1.0, 1.0, 1.0, 0.30))?,
            separator: brush(color(1.0, 1.0, 1.0, 0.25))?,
        };

        let round_stroke = self.round_stroke_style()?;
        let flat_stroke = self.flat_stroke_style()?;

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
            glyph_text: self.text_format(
                w!("Segoe UI"),
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                px(15),
                DWRITE_TEXT_ALIGNMENT_CENTER,
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
            flat_stroke,
            layer: RefCell::new(None),
            fonts,
            dwrite: self.dwrite.clone(),
            button_style: self.button_style,
            dpi,
        })
    }

    /// 圆头圆角线型。
    fn round_stroke_style(&self) -> Result<ID2D1StrokeStyle, OverlayError> {
        let props = D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_ROUND,
            endCap: D2D1_CAP_STYLE_ROUND,
            lineJoin: D2D1_LINE_JOIN_ROUND,
            ..Default::default()
        };
        // SAFETY: props 已完整填写，无自定义虚线。
        unsafe { self.factory.CreateStrokeStyle(&props, None) }.map_err(win32("CreateStrokeStyle"))
    }

    /// 离屏合成成图：从冻结帧裁出 `area`，再用与屏幕相同的绘制函数叠上标注。
    pub fn compose(
        &self,
        frame: FrameRef<'_>,
        area: &Rect,
        shapes: &[Shape],
    ) -> Result<CroppedImage, OverlayError> {
        let area = area
            .intersect(&frame.bounds)
            .ok_or_else(|| OverlayError::InvalidInput("选区不在显示器范围内".to_string()))?;
        let (width, height) = (area.width, area.height);
        let base = crop_bgra(&frame, &area)?;
        let pitch = width
            .checked_mul(4)
            .ok_or_else(|| OverlayError::InvalidInput("成图行字节数溢出".to_string()))?;
        let _com = ComGuard::enter();
        // SAFETY: CLSID 为静态常量；ComGuard 保证调用线程已初始化 COM。
        let wic: IWICImagingFactory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
                .map_err(win32("CoCreateInstance(WICImagingFactory)"))?;
        // SAFETY: 尺寸非零，像素格式为静态 GUID。
        let wic_bitmap = unsafe {
            wic.CreateBitmap(
                width,
                height,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapCacheOnLoad,
            )
        }
        .map_err(win32("IWICImagingFactory::CreateBitmap"))?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        // SAFETY: wic_bitmap 有效，属性结构为局部变量。
        let target = unsafe {
            self.factory
                .CreateWicBitmapRenderTarget(&wic_bitmap, &props)
        }
        .map_err(win32("CreateWicBitmapRenderTarget"))?;
        let round_stroke = self.round_stroke_style()?;
        let flat_stroke = self.flat_stroke_style()?;
        let frames = [frame];
        // SAFETY: target 刚创建，BeginDraw/EndDraw 成对调用，出错时也会结束绘制。
        unsafe { target.BeginDraw() };
        let drawn = (|| -> Result<(), OverlayError> {
            let bitmap_props = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: 96.0,
                dpiY: 96.0,
            };
            // SAFETY: base 长度为宽×高×4，pitch 与之一致；调用期间数据有效且会被复制。
            let source = unsafe {
                target.CreateBitmap(
                    D2D_SIZE_U { width, height },
                    Some(base.as_ptr().cast()),
                    pitch,
                    &bitmap_props,
                )
            }
            .map_err(win32("CreateBitmap"))?;
            let whole = D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: to_f32(i64::from(width)),
                bottom: to_f32(i64::from(height)),
            };
            // SAFETY: 位图由同一目标创建，矩形为局部变量；1:1 贴图。
            unsafe {
                target.DrawBitmap(
                    &source,
                    Some(&whole),
                    1.0,
                    D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    None,
                );
            }
            Painter {
                target: &target,
                factory: &self.factory,
                dwrite: &self.dwrite,
                round_stroke: &round_stroke,
                flat_stroke: &flat_stroke,
                frames: &frames,
            }
            .draw_shapes(shapes, Point::new(area.x, area.y))
        })();
        // SAFETY: 与上面的 BeginDraw 配对。
        let ended = unsafe { target.EndDraw(None, None) }.map_err(win32("EndDraw"));
        drawn?;
        ended?;
        read_back(&wic_bitmap, width, height)
    }

    /// 方头线型，荧光笔用；端点为方形，转角仍然圆滑。
    fn flat_stroke_style(&self) -> Result<ID2D1StrokeStyle, OverlayError> {
        let props = D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_FLAT,
            endCap: D2D1_CAP_STYLE_FLAT,
            lineJoin: D2D1_LINE_JOIN_ROUND,
            ..Default::default()
        };
        // SAFETY: props 已完整填写，无自定义虚线。
        unsafe { self.factory.CreateStrokeStyle(&props, None) }.map_err(win32("CreateStrokeStyle"))
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

/// 当前线程的 COM 初始化：已按其它模式初始化时沿用，成功初始化的才在离开时反初始化。
struct ComGuard {
    initialized: bool,
}

impl ComGuard {
    fn enter() -> Self {
        // SAFETY: 无前置条件；成功（含 S_FALSE）时必须与 CoUninitialize 配对。
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Self {
            initialized: hr.is_ok(),
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: 与 enter 中成功的 CoInitializeEx 配对，仅调用一次。
            unsafe { CoUninitialize() };
        }
    }
}

/// 从整屏 BGRA 里裁出 `area`（必须在 `frame` 范围内）。
fn crop_bgra(frame: &FrameRef<'_>, area: &Rect) -> Result<Vec<u8>, OverlayError> {
    let overflow = || OverlayError::InvalidInput("裁剪范围超出冻结帧".to_string());
    let stride = usize::try_from(frame.bounds.width).map_err(|_| overflow())?;
    let off_x =
        usize::try_from(i64::from(area.x) - i64::from(frame.bounds.x)).map_err(|_| overflow())?;
    let off_y =
        usize::try_from(i64::from(area.y) - i64::from(frame.bounds.y)).map_err(|_| overflow())?;
    let rows = usize::try_from(area.height).map_err(|_| overflow())?;
    let row_bytes = usize::try_from(area.width)
        .map_err(|_| overflow())?
        .checked_mul(4)
        .ok_or_else(overflow)?;
    let mut out = Vec::with_capacity(row_bytes.checked_mul(rows).ok_or_else(overflow)?);
    for row in 0..rows {
        let start = off_y
            .checked_add(row)
            .and_then(|y| y.checked_mul(stride))
            .and_then(|px| px.checked_add(off_x))
            .and_then(|px| px.checked_mul(4))
            .ok_or_else(overflow)?;
        let end = start.checked_add(row_bytes).ok_or_else(overflow)?;
        out.extend_from_slice(frame.bgra.get(start..end).ok_or_else(overflow)?);
    }
    Ok(out)
}

/// 读回 WIC 位图，BGRA 转 RGBA；冻结帧不透明，alpha 一律置为 255。
fn read_back(bitmap: &IWICBitmap, width: u32, height: u32) -> Result<CroppedImage, OverlayError> {
    let invalid = |what: &str| OverlayError::InvalidInput(format!("读回成图失败：{what}"));
    let rect = WICRect {
        X: 0,
        Y: 0,
        Width: i32::try_from(width).map_err(|_| invalid("宽度过大"))?,
        Height: i32::try_from(height).map_err(|_| invalid("高度过大"))?,
    };
    let lock_flags = u32::try_from(WICBitmapLockRead.0).map_err(|_| invalid("锁定标志无效"))?;
    // SAFETY: rect 在位图范围内；锁在本函数结束时释放。
    let lock = unsafe { bitmap.Lock(&rect, lock_flags) }.map_err(win32("IWICBitmap::Lock"))?;
    // SAFETY: lock 有效。
    let stride = unsafe { lock.GetStride() }.map_err(win32("IWICBitmapLock::GetStride"))?;
    let mut size = 0u32;
    let mut data = std::ptr::null_mut();
    // SAFETY: 两个输出参数都指向有效局部变量。
    unsafe { lock.GetDataPointer(&mut size, &mut data) }
        .map_err(win32("IWICBitmapLock::GetDataPointer"))?;
    if data.is_null() {
        return Err(invalid("没有像素数据"));
    }
    let len = usize::try_from(size).map_err(|_| invalid("缓冲区过大"))?;
    // SAFETY: 锁有效期间 data 指向 size 字节的可读内存，下面只在 lock 存活时使用。
    let pixels = unsafe { std::slice::from_raw_parts(data, len) };
    let stride = usize::try_from(stride).map_err(|_| invalid("行距过大"))?;
    let row_bytes = usize::try_from(width)
        .map_err(|_| invalid("宽度过大"))?
        .checked_mul(4)
        .ok_or_else(|| invalid("行字节数溢出"))?;
    let rows = usize::try_from(height).map_err(|_| invalid("高度过大"))?;
    let mut rgba = Vec::with_capacity(
        row_bytes
            .checked_mul(rows)
            .ok_or_else(|| invalid("尺寸溢出"))?,
    );
    for row in 0..rows {
        let start = row.checked_mul(stride).ok_or_else(|| invalid("偏移溢出"))?;
        let end = start
            .checked_add(row_bytes)
            .ok_or_else(|| invalid("偏移溢出"))?;
        let line = pixels.get(start..end).ok_or_else(|| invalid("像素不足"))?;
        for px in line.chunks_exact(4) {
            if let [b, g, r, _] = px {
                rgba.extend_from_slice(&[*r, *g, *b, 255]);
            }
        }
    }
    CroppedImage::new(width, height, rgba).map_err(|e| invalid(&e.to_string()))
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
    /// 选中的工具与样式按钮底色。
    active: ID2D1SolidColorBrush,
    /// 不可用按钮的图标。
    disabled: ID2D1SolidColorBrush,
    /// 工具栏分隔线。
    separator: ID2D1SolidColorBrush,
}

struct Fonts {
    label: IDWriteTextFormat,
    info: IDWriteTextFormat,
    hint: IDWriteTextFormat,
    button: IDWriteTextFormat,
    /// 工具栏上不依赖图标字体的固定文字，如序号按钮的「1」。
    glyph_text: IDWriteTextFormat,
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
    flat_stroke: ID2D1StrokeStyle,
    /// 已完成标注的缓存层，只在新增、撤销、重做时重建。
    layer: RefCell<Option<LayerCache>>,
    fonts: Fonts,
    dwrite: IDWriteFactory,
    button_style: ButtonStyle,
    dpi: u32,
}

/// 与显示器同尺寸的离屏层，里面是已完成的标注。
struct LayerCache {
    bitmap: ID2D1Bitmap,
    revision: u64,
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
        let dim = surface.effects.dim.clamp(0.0, 1.0);
        // SAFETY: 目标与资源均有效；BeginDraw/EndDraw 成对调用，期间所有指针参数都指向存活的局部数据。
        unsafe {
            target.BeginDraw();
            self.bitmap_into(&full, &full, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR);
            self.brushes.dim.SetOpacity(dim);
            target.FillRectangle(&full, &self.brushes.dim);
        }
        // 选区外的标注以遮罩后的亮度留作参照，成图会把它们裁掉
        self.draw_layer(&view.annotations, surface, &full, 1.0 - DIM_ALPHA * dim);
        if let Some(r) = region {
            // 选区内还原为原图亮度，源矩形与目标矩形相同即 1:1 贴图
            self.bitmap_into(&r, &r, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR);
            self.draw_annotations(&view.annotations, surface, &r);
            if !surface.effects.colorful && view.frame != FrameStyle::Corners {
                // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷有效。
                unsafe {
                    target.DrawRectangle(&r, &self.brushes.accent, self.px(OUTLINE_WIDTH), None);
                }
            }
        }
        if let (false, Preview::Lasso { trail, .. }) = (surface.show_lasso_bounds, view.preview) {
            self.draw_lasso_region(trail, surface.origin);
        }
        let phase = surface.effects.glow.map_or(0.0, |g| g.phase);
        if let (true, Some(r)) = (surface.effects.colorful, region) {
            match view.frame {
                FrameStyle::Corners => self.draw_box_glow(&r, phase),
                FrameStyle::Plain | FrameStyle::Handles => {
                    let width = self.px(OUTLINE_WIDTH);
                    let passes = [(width * HALO_SCALE, HALO_ALPHA), (width, 1.0)];
                    self.draw_colorful_outline(&r, phase, &passes);
                }
            }
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
        match (view.frame, view.preview) {
            (FrameStyle::Handles, Preview::Rect(r)) => self.draw_handles(&r, surface.origin),
            (FrameStyle::Corners, Preview::Rect(r)) => {
                self.draw_corners(&r, surface.origin, surface.effects.colorful);
            }
            _ => {}
        }
        if let Some(r) = view.label {
            self.draw_label(&r, surface);
        }
        if let Some(toolbar) = &view.toolbar {
            self.draw_toolbar(toolbar, surface.origin);
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
        gradient_brush_on(&self.target, from, to, stops)
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

    /// 四色渐变选框，按 `passes` 依次描边，每项为线宽与不透明度。
    fn draw_colorful_outline(&self, r: &D2D_RECT_F, phase: f32, passes: &[(f32, f32)]) {
        colorful_outline_on(&self.target, &self.round_stroke, r, phase, passes);
    }

    /// 平滑后的轨迹曲线（窗口坐标），点数不足两个时返回 `None`。
    fn trail_geometry(&self, points: &[PointF]) -> Result<Option<ID2D1PathGeometry>, OverlayError> {
        paint::smooth_path(&self.factory, points)
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

    /// 常驻选框外侧的四色光晕：一圈圈向外扩、逐层变淡，不压进选区。
    fn draw_box_glow(&self, r: &D2D_RECT_F, phase: f32) {
        // 柔光只是装饰，模糊失败时不画，选框照常可用
        if self.soft_glow(r, phase).is_err() {
            return;
        }
        // 模糊会向内渗，把选区还原成原图，光只留在外侧
        self.bitmap_into(r, r, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR);
    }

    /// 在离屏位图上画出四色描边，高斯模糊后贴回选框周围。
    fn soft_glow(&self, r: &D2D_RECT_F, phase: f32) -> windows::core::Result<()> {
        let band = self.px(BOX_GLOW_WIDTH);
        // 模糊半径约三倍标准差，四周留足边距，光晕不会被位图边缘截断
        let pad = band * 2.0;
        let (rw, rh) = (r.right - r.left, r.bottom - r.top);
        if rw <= 0.0 || rh <= 0.0 {
            return Ok(());
        }
        let size = D2D_SIZE_F {
            width: rw + pad * 2.0,
            height: rh + pad * 2.0,
        };
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，size 为有效局部变量；兼容目标与本目标共享设备资源。
        let layer = unsafe {
            self.target.CreateCompatibleRenderTarget(
                Some(&size),
                None,
                None,
                D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE,
            )
        }?;
        let source_width = band * BOX_GLOW_SOURCE;
        let reach = source_width / 2.0;
        let ring = D2D_RECT_F {
            left: pad - reach,
            top: pad - reach,
            right: pad + rw + reach,
            bottom: pad + rh + reach,
        };
        // SAFETY: layer 刚创建，BeginDraw/EndDraw 成对调用，期间只在 layer 上绘制。
        unsafe {
            layer.BeginDraw();
            layer.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
        }
        colorful_outline_on(
            &layer,
            &self.round_stroke,
            &ring,
            phase,
            &[(source_width, BOX_GLOW_ALPHA)],
        );
        // SAFETY: 与上面的 BeginDraw 配对。
        unsafe { layer.EndDraw(None, None) }?;
        // SAFETY: layer 已结束绘制，位图内容有效。
        let bitmap = unsafe { layer.GetBitmap() }?;
        let context: ID2D1DeviceContext = self.target.cast()?;
        // SAFETY: 系统内置的高斯模糊效果，CLSID 为静态常量。
        let blur = unsafe { context.CreateEffect(&CLSID_D2D1GaussianBlur) }?;
        let sigma = band / 3.0;
        let property = u32::try_from(D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0).unwrap_or(0);
        // SAFETY: 输入位图由同一设备创建；属性索引与类型匹配，数据为 f32 的原始字节。
        unsafe {
            blur.SetInput(0, &bitmap, true);
            blur.SetValue(property, D2D1_PROPERTY_TYPE_FLOAT, &sigma.to_ne_bytes())?;
        }
        // SAFETY: 效果已配置完成。
        let output = unsafe { blur.GetOutput() }?;
        let offset = float_point(r.left - pad, r.top - pad).point;
        // SAFETY: 在本目标的 BeginDraw 与 EndDraw 之间调用，图像由同一设备创建。
        unsafe {
            context.DrawImage(
                &output,
                Some(&offset),
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
        }
        Ok(())
    }

    /// 常驻选框的四角圆角折线，描在选框外侧，不压住选区内容。
    fn draw_corners(&self, rect: &Rect, origin: Point, colorful: bool) {
        let (arm, radius) = layout::corner_arm(rect.width, rect.height, self.dpi);
        if arm == 0 {
            return;
        }
        let line = self.px(CORNER_LINE);
        let half = line / 2.0;
        let r = local_rect(rect, origin);
        let (l, t, rt, b) = (r.left - half, r.top - half, r.right + half, r.bottom + half);
        let arm = to_f32(i64::from(arm)) + half;
        let radius = to_f32(i64::from(radius)) + half;
        // 每个角：角点、第一段方向、第二段方向
        let corners = [
            ((l, t), (0.0, 1.0), (1.0, 0.0)),
            ((rt, t), (-1.0, 0.0), (0.0, 1.0)),
            ((rt, b), (0.0, -1.0), (-1.0, 0.0)),
            ((l, b), (1.0, 0.0), (0.0, -1.0)),
        ];
        // 角标只是视觉提示，几何体创建失败时不画，选框仍可调整
        let Ok(path) = self.corner_geometry(&corners, arm, radius) else {
            return;
        };
        // 没有炫彩光晕衬底时垫一层浅阴影，亮色画面上白色角标也看得清
        let shadow = (line + self.px(2), color(0.0, 0.0, 0.0, 0.25));
        let white = (line, color(1.0, 1.0, 1.0, 1.0));
        let passes: &[(f32, D2D1_COLOR_F)] = if colorful { &[white] } else { &[shadow, white] };
        for &(width, brush_color) in passes {
            // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，几何体、画刷与线型有效。
            unsafe {
                self.brushes.paint.SetColor(&brush_color);
                self.target
                    .DrawGeometry(&path, &self.brushes.paint, width, &self.round_stroke);
            }
        }
    }

    /// 四个角各一段「直线 → 二次贝塞尔圆角 → 直线」的空心路径。
    fn corner_geometry(
        &self,
        corners: &[Corner],
        arm: f32,
        radius: f32,
    ) -> Result<ID2D1PathGeometry, OverlayError> {
        // SAFETY: 工厂有效，无参数。
        let path =
            unsafe { self.factory.CreatePathGeometry() }.map_err(win32("CreatePathGeometry"))?;
        // SAFETY: path 刚创建，只打开一次。
        let sink = unsafe { path.Open() }.map_err(win32("ID2D1PathGeometry::Open"))?;
        let at = |c: (f32, f32), d: (f32, f32), len: f32| {
            float_point(c.0 + d.0 * len, c.1 + d.1 * len).point
        };
        for &(corner, first, second) in corners {
            let bezier = D2D1_QUADRATIC_BEZIER_SEGMENT {
                point1: float_point(corner.0, corner.1).point,
                point2: at(corner, second, radius),
            };
            // SAFETY: sink 处于打开状态，BeginFigure 与 EndFigure 成对调用，贝塞尔结构为局部变量。
            unsafe {
                sink.BeginFigure(at(corner, first, arm), D2D1_FIGURE_BEGIN_HOLLOW);
                sink.AddLine(at(corner, first, radius));
                sink.AddQuadraticBezier(&bezier);
                sink.AddLine(at(corner, second, arm));
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
            }
        }
        // SAFETY: 与上面的 Open 配对。
        unsafe { sink.Close() }.map_err(win32("ID2D1GeometrySink::Close"))?;
        Ok(path)
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

    /// 工具栏：分组按钮、分隔线，以及选中工具后的样式条。
    fn draw_toolbar(&self, toolbar: &ToolbarView, origin: Point) {
        let layout = &toolbar.layout;
        self.panel(&local_rect(&layout.bar, origin));
        for (button, cell) in &layout.buttons {
            let r = local_rect(cell, origin);
            let enabled = match button {
                ToolbarButton::Undo => toolbar.can_undo,
                ToolbarButton::Redo => toolbar.can_redo,
                _ => true,
            };
            if matches!(button, ToolbarButton::Tool(tool) if toolbar.active == Some(*tool)) {
                self.rounded(&r, &self.brushes.active);
            } else if enabled && toolbar.hovered == Some(Hit::Button(*button)) {
                self.rounded(&r, &self.brushes.hover);
            }
            let brush = if !enabled {
                &self.brushes.disabled
            } else if *button == ToolbarButton::Copy {
                &self.brushes.accent
            } else {
                &self.brushes.white
            };
            self.draw_button_glyph(*button, &r, brush);
        }
        for sep in &layout.separators {
            let r = local_rect(sep, origin);
            // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷有效。
            unsafe { self.target.FillRectangle(&r, &self.brushes.separator) };
        }
        if let (Some(strip), Some(style)) = (&layout.strip, toolbar.style) {
            self.draw_strip(strip, &style, toolbar.hovered, origin);
        }
    }

    fn draw_button_glyph(
        &self,
        button: ToolbarButton,
        rect: &D2D_RECT_F,
        brush: &ID2D1SolidColorBrush,
    ) {
        let (text, format): (Vec<u16>, &IDWriteTextFormat) =
            match (self.button_style, button.glyph()) {
                (_, Glyph::Text(t)) => (t.encode_utf16().collect(), &self.fonts.glyph_text),
                (ButtonStyle::Icon(_), Glyph::Icon(c)) => {
                    (c.to_string().encode_utf16().collect(), &self.fonts.button)
                }
                (ButtonStyle::Text, Glyph::Icon(_)) => {
                    (button.label().encode_utf16().collect(), &self.fonts.button)
                }
            };
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，文本、格式与画刷有效。
        unsafe {
            self.target.DrawText(
                &text,
                format,
                rect,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    /// 样式条：8 个色块、粗细数值框，序号工具多一组「1 / A」。
    fn draw_strip(
        &self,
        strip: &layout::StripLayout,
        style: &StyleView,
        hovered: Option<Hit>,
        origin: Point,
    ) {
        self.panel(&local_rect(&strip.rect, origin));
        for (index, cell) in strip.swatches.iter().enumerate() {
            let Some(c) = PALETTE.get(index) else {
                continue;
            };
            let r = local_rect(cell, origin);
            let fill = color(
                f32::from(c.r) / 255.0,
                f32::from(c.g) / 255.0,
                f32::from(c.b) / 255.0,
                1.0,
            );
            // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷有效。
            unsafe {
                self.brushes.swatch.SetColor(&fill);
                self.target.FillRectangle(&r, &self.brushes.swatch);
                // 细边让黑色和白色色块在深色底上都看得清
                self.target
                    .DrawRectangle(&r, &self.brushes.muted, 1.0, None);
            }
            if style.swatch == Some(index) {
                let ring = self.px(2);
                let outer = D2D_RECT_F {
                    left: r.left - ring,
                    top: r.top - ring,
                    right: r.right + ring,
                    bottom: r.bottom + ring,
                };
                // SAFETY: 同上。
                unsafe {
                    self.target
                        .DrawRectangle(&outer, &self.brushes.white, 2.0, None);
                }
            } else if hovered == Some(Hit::Swatch(index)) {
                // SAFETY: 同上。
                unsafe {
                    self.target
                        .DrawRectangle(&r, &self.brushes.white, 2.0, None)
                };
            }
        }
        for (index, cell) in strip.presets.iter().enumerate() {
            let r = local_rect(cell, origin);
            if style.preset == Some(index) {
                self.rounded(&r, &self.brushes.active);
            } else if hovered == Some(Hit::Preset(index)) {
                self.rounded(&r, &self.brushes.hover);
            }
            // 圆点固定画成细、中、粗三档视觉大小，不按实际粗细画
            let diameter = self.px(PRESET_DOT_SIZES.get(index).copied().unwrap_or(10));
            let mut dot = float_point((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
            dot.radiusX = diameter / 2.0;
            dot.radiusY = diameter / 2.0;
            // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，椭圆为局部变量，画刷有效。
            unsafe { self.target.FillEllipse(&dot, &self.brushes.white) };
        }
        let value = local_rect(&strip.value, origin);
        let text: Vec<u16> = style.size.to_string().encode_utf16().collect();
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，文本、格式与画刷有效。
        unsafe {
            self.target.DrawText(
                &text,
                &self.fonts.hint,
                &value,
                &self.brushes.muted,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        for (number_style, cell) in &strip.number_styles {
            let r = local_rect(cell, origin);
            if style.number_style == Some(*number_style) {
                self.rounded(&r, &self.brushes.active);
            } else if hovered == Some(Hit::NumberStyle(*number_style)) {
                self.rounded(&r, &self.brushes.hover);
            }
            self.chip_text(number_style.mark(), &r);
        }
    }

    fn chip_text(&self, text: &str, rect: &D2D_RECT_F) {
        let wide: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，文本、格式与画刷有效。
        unsafe {
            self.target.DrawText(
                &wide,
                &self.fonts.label,
                rect,
                &self.brushes.white,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    /// 缓存层的版本与标注一致；不一致时重画。
    fn ensure_layer(
        &self,
        annotations: &Annotations<'_>,
        surface: &Surface<'_>,
    ) -> Result<(), OverlayError> {
        let mut cache = self
            .layer
            .try_borrow_mut()
            .map_err(|_| OverlayError::InvalidInput("标注缓存正被占用".to_string()))?;
        if cache
            .as_ref()
            .is_some_and(|c| c.revision == annotations.revision)
        {
            return Ok(());
        }
        let size = D2D_SIZE_F {
            width: to_f32(i64::from(surface.size.0)),
            height: to_f32(i64::from(surface.size.1)),
        };
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，size 为有效局部变量；兼容目标与本目标共享设备资源。
        let layer: ID2D1BitmapRenderTarget = unsafe {
            self.target.CreateCompatibleRenderTarget(
                Some(&size),
                None,
                None,
                D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE,
            )
        }
        .map_err(win32("CreateCompatibleRenderTarget"))?;
        // SAFETY: layer 刚创建，BeginDraw/EndDraw 成对调用，期间只在 layer 上绘制。
        unsafe {
            layer.BeginDraw();
            layer.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
        }
        let frames = [FrameRef {
            bounds: surface.monitor(),
            bgra: surface.bgra,
        }];
        let painter = Painter {
            target: &layer,
            factory: &self.factory,
            dwrite: &self.dwrite,
            round_stroke: &self.round_stroke,
            flat_stroke: &self.flat_stroke,
            frames: &frames,
        };
        let drawn = painter.draw_shapes(annotations.shapes, surface.origin);
        // SAFETY: 与上面的 BeginDraw 配对，出错时也要结束绘制。
        let ended = unsafe { layer.EndDraw(None, None) }.map_err(win32("EndDraw"));
        drawn?;
        ended?;
        // SAFETY: layer 已结束绘制，位图内容有效。
        let bitmap = unsafe { layer.GetBitmap() }.map_err(win32("GetBitmap"))?;
        *cache = Some(LayerCache {
            bitmap,
            revision: annotations.revision,
        });
        Ok(())
    }

    /// 把已完成的标注缓存层整体贴到窗口上。
    fn draw_layer(
        &self,
        annotations: &Annotations<'_>,
        surface: &Surface<'_>,
        dst: &D2D_RECT_F,
        opacity: f32,
    ) {
        if annotations.shapes.is_empty() {
            return;
        }
        // 标注只是叠加层，缓存失败时本帧不画，选区和成图不受影响
        if self.ensure_layer(annotations, surface).is_err() {
            return;
        }
        let Ok(cache) = self.layer.try_borrow() else {
            return;
        };
        if let Some(cache) = cache.as_ref() {
            // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，位图由本目标的兼容目标创建。
            unsafe {
                self.target.DrawBitmap(
                    &cache.bitmap,
                    Some(dst),
                    opacity.clamp(0.0, 1.0),
                    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                    None,
                );
            }
        }
    }

    /// 选区内的标注：缓存层加正在画的图元，裁在选区范围内。
    fn draw_annotations(
        &self,
        annotations: &Annotations<'_>,
        surface: &Surface<'_>,
        clip: &D2D_RECT_F,
    ) {
        if annotations.shapes.is_empty() && annotations.draft.is_none() {
            return;
        }
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，与下面的弹出成对。
        unsafe {
            self.target
                .PushAxisAlignedClip(clip, D2D1_ANTIALIAS_MODE_ALIASED);
        }
        let full = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: to_f32(i64::from(surface.size.0)),
            bottom: to_f32(i64::from(surface.size.1)),
        };
        self.draw_layer(annotations, surface, &full, 1.0);
        if let Some(draft) = &annotations.draft {
            let frames = [FrameRef {
                bounds: surface.monitor(),
                bgra: surface.bgra,
            }];
            let painter = Painter {
                target: &self.target,
                factory: &self.factory,
                dwrite: &self.dwrite,
                round_stroke: &self.round_stroke,
                flat_stroke: &self.flat_stroke,
                frames: &frames,
            };
            // 预览只是临时反馈，失败时这一帧不画，松手后图元仍会正常加入
            painter
                .draw_shapes(std::slice::from_ref(draft), surface.origin)
                .unwrap_or(());
        }
        // SAFETY: 与上面的 PushAxisAlignedClip 配对。
        unsafe { self.target.PopAxisAlignedClip() };
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
    if let Some(toolbar) = &view.toolbar {
        out.push(toolbar.layout.bar);
        if let Some(strip) = &toolbar.layout.strip {
            out.push(strip.rect);
        }
    }
    if let Some((cursor, _)) = view.magnifier {
        let m = layout::magnifier_layout(cursor, &surface.monitor(), dpi);
        out.push(m.zoom);
        out.push(m.info);
    }
    out
}

/// 在任意渲染目标上按色标创建线性渐变画刷；只能在该目标的 BeginDraw 与 EndDraw 之间调用。
fn gradient_brush_on(
    target: &ID2D1RenderTarget,
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
        target.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
    }
    .map_err(win32("CreateGradientStopCollection"))?;
    let mut props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES::default();
    props.startPoint.X = from.0;
    props.startPoint.Y = from.1;
    props.endPoint.X = to.0;
    props.endPoint.Y = to.1;
    // SAFETY: props 指向存活的局部变量，色标集合由同一目标创建。
    unsafe { target.CreateLinearGradientBrush(&props, None, &collection) }
        .map_err(win32("CreateLinearGradientBrush"))
}

/// 在任意渲染目标上画四色渐变矩形边框，按 `passes` 依次描边，每项为线宽与不透明度。
fn colorful_outline_on(
    target: &ID2D1RenderTarget,
    stroke: &ID2D1StrokeStyle,
    r: &D2D_RECT_F,
    phase: f32,
    passes: &[(f32, f32)],
) {
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
            gradient_brush_on(target, from, to, &stops)
                .ok()
                .map(|brush| (from, to, brush))
        })
        .collect();
    for &(line_width, opacity) in passes {
        for (from, to, brush) in &edges {
            // SAFETY: 在该目标的 BeginDraw 与 EndDraw 之间调用，画刷与线型有效。
            unsafe {
                brush.SetOpacity(opacity);
                target.DrawLine(
                    float_point(from.0, from.1).point,
                    float_point(to.0, to.1).point,
                    brush,
                    line_width,
                    stroke,
                );
            }
        }
    }
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
pub(crate) fn float_point(x: f32, y: f32) -> D2D1_ELLIPSE {
    let mut holder = D2D1_ELLIPSE::default();
    holder.point.X = x;
    holder.point.Y = y;
    holder
}

/// 坐标转 `f32`。显示器像素尺寸远小于 `i16` 范围，超出的值饱和到边界即可。
pub(crate) fn to_f32(value: i64) -> f32 {
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

    // 以下测试在内存里走真实的 D2D + WIC 合成路径，不需要窗口。

    use lenswin_core::annotation::{Color, NumberStyle, Shape};

    const FRAME: Rect = Rect::new(100, 50, 40, 30);

    /// 冻结帧：R 随 x 变化、G 随 y 变化、B 固定，BGRA 排列。
    fn gradient_frame() -> Vec<u8> {
        let mut out = Vec::new();
        for y in 0..FRAME.height {
            for x in 0..FRAME.width {
                let (x, y) = (u8::try_from(x).expect("小"), u8::try_from(y).expect("小"));
                out.extend_from_slice(&[40, y * 6, x * 6, 0]);
            }
        }
        out
    }

    fn grey_frame() -> Vec<u8> {
        [200u8, 200, 200, 0].repeat(usize::try_from(FRAME.width * FRAME.height).expect("小"))
    }

    fn compose(bgra: &[u8], area: &Rect, shapes: &[Shape]) -> CroppedImage {
        let renderer = Renderer::new().expect("创建渲染器");
        let frame = FrameRef {
            bounds: FRAME,
            bgra,
        };
        renderer.compose(frame, area, shapes).expect("合成")
    }

    fn pixel(image: &CroppedImage, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let index = usize::try_from(y * image.width() + x).expect("小") * 4;
        match image.rgba().get(index..index + 4) {
            Some([r, g, b, a]) => (*r, *g, *b, *a),
            _ => panic!("像素越界：({x}, {y})"),
        }
    }

    fn near(actual: u8, expected: u8, tolerance: u8) -> bool {
        actual.abs_diff(expected) <= tolerance
    }

    const RED: Color = Color::rgb(0xFF, 0x3B, 0x30);

    #[test]
    fn compose_without_shapes_is_an_exact_crop() {
        let image = compose(&gradient_frame(), &Rect::new(110, 60, 10, 8), &[]);
        assert_eq!((image.width(), image.height()), (10, 8));
        for y in 0..8u32 {
            for x in 0..10u32 {
                let fx = u8::try_from(x + 10).expect("小");
                let fy = u8::try_from(y + 10).expect("小");
                assert_eq!(pixel(&image, x, y), (fx * 6, fy * 6, 40, 255), "({x}, {y})");
            }
        }
    }

    #[test]
    fn compose_clips_area_to_the_frame() {
        let image = compose(&grey_frame(), &Rect::new(130, 70, 100, 100), &[]);
        assert_eq!((image.width(), image.height()), (10, 10));
    }

    #[test]
    fn rectangle_outline_is_drawn_in_selection_coordinates() {
        let rect = Shape::Rectangle {
            from: Point::new(105, 55),
            to: Point::new(125, 75),
            color: RED,
            width: 2,
        };
        let image = compose(&grey_frame(), &FRAME, &[rect]);
        // 线宽 2 以边为中心，恰好盖住 x=4、5 两列
        assert_eq!(pixel(&image, 4, 15), (0xFF, 0x3B, 0x30, 255));
        assert_eq!(pixel(&image, 5, 15), (0xFF, 0x3B, 0x30, 255));
        assert_eq!(pixel(&image, 15, 15), (200, 200, 200, 255));
        assert_eq!(pixel(&image, 0, 0), (200, 200, 200, 255));
        assert_eq!(pixel(&image, 30, 25), (200, 200, 200, 255));
    }

    #[test]
    fn shapes_outside_the_selection_are_cropped_away() {
        let rect = Shape::Rectangle {
            from: Point::new(105, 55),
            to: Point::new(125, 75),
            color: RED,
            width: 2,
        };
        // 选区只取右下角，与矩形没有重叠
        let image = compose(&grey_frame(), &Rect::new(128, 78, 10, 2), &[rect]);
        for x in 0..10 {
            for y in 0..2 {
                assert_eq!(pixel(&image, x, y), (200, 200, 200, 255));
            }
        }
        // 选区左移到 x=102：矩形左边（屏幕 x=105）落在选区内 x=3
        let image = compose(
            &grey_frame(),
            &Rect::new(102, 60, 10, 10),
            &[Shape::Rectangle {
                from: Point::new(105, 55),
                to: Point::new(125, 75),
                color: RED,
                width: 2,
            }],
        );
        assert_eq!(pixel(&image, 2, 5), (0xFF, 0x3B, 0x30, 255));
        assert_eq!(pixel(&image, 3, 5), (0xFF, 0x3B, 0x30, 255));
        assert_eq!(pixel(&image, 8, 5), (200, 200, 200, 255));
    }

    #[test]
    fn line_and_arrow_head_are_drawn() {
        let arrow = Shape::Arrow {
            from: Point::new(105, 65),
            to: Point::new(135, 65),
            color: RED,
            width: 3,
        };
        let image = compose(&grey_frame(), &FRAME, &[arrow]);
        // 杆身：y=15 附近线宽 3
        assert_eq!(pixel(&image, 15, 15), (0xFF, 0x3B, 0x30, 255));
        // 头部比杆身宽，杆身覆盖不到的 y=11 只可能是箭头
        assert_eq!(pixel(&image, 25, 11), (0xFF, 0x3B, 0x30, 255));
        assert_eq!(pixel(&image, 15, 11), (200, 200, 200, 255));
        let line = Shape::Line {
            from: Point::new(105, 55),
            to: Point::new(105, 75),
            color: RED,
            width: 4,
        };
        let image = compose(&grey_frame(), &FRAME, &[line]);
        assert_eq!(pixel(&image, 4, 15), (0xFF, 0x3B, 0x30, 255));
        assert_eq!(pixel(&image, 6, 15), (0xFF, 0x3B, 0x30, 255));
        assert_eq!(pixel(&image, 8, 15), (200, 200, 200, 255));
    }

    #[test]
    fn highlighter_is_translucent_and_overlap_does_not_darken() {
        let there_and_back = Shape::Highlighter {
            points: vec![
                Point::new(105, 65),
                Point::new(135, 65),
                Point::new(105, 65),
            ],
            color: Color::rgb(0xFF, 0xCC, 0x00),
            width: 10,
        };
        let image = compose(&grey_frame(), &FRAME, &[there_and_back]);
        // 0.6 × 200 + 0.4 × 通道值
        let (r, g, b, a) = pixel(&image, 20, 15);
        assert!(
            near(r, 222, 3) && near(g, 202, 3) && near(b, 120, 3),
            "{r} {g} {b}"
        );
        assert_eq!(a, 255);
        assert_eq!(pixel(&image, 20, 2), (200, 200, 200, 255));
    }

    #[test]
    fn pen_stroke_is_opaque() {
        let pen = Shape::Pen {
            points: vec![Point::new(105, 65), Point::new(125, 65)],
            color: RED,
            width: 4,
        };
        let image = compose(&grey_frame(), &FRAME, &[pen]);
        assert_eq!(pixel(&image, 15, 15), (0xFF, 0x3B, 0x30, 255));
        assert_eq!(pixel(&image, 15, 20), (200, 200, 200, 255));
    }

    #[test]
    fn mosaic_fills_blocks_with_the_average_of_the_original() {
        let bgra = gradient_frame();
        let mosaic = Shape::Mosaic {
            from: Point::new(100, 50),
            to: Point::new(140, 80),
            block: 10,
        };
        let image = compose(&bgra, &FRAME, &[mosaic]);
        // 每个 10×10 块内部颜色一致
        for (bx, by) in [(0u32, 0u32), (10, 10), (30, 20)] {
            let first = pixel(&image, bx, by);
            assert_eq!(pixel(&image, bx + 9, by + 9), first);
            assert_eq!(pixel(&image, bx + 4, by + 5), first);
        }
        // 第一块：x∈0..10 的 R 平均为 (0+…+9)/10 × 6 = 27，y 平均 27
        let (r, g, b, _) = pixel(&image, 3, 3);
        assert!(near(r, 27, 1) && near(g, 27, 1) && b == 40, "{r} {g} {b}");
        assert_ne!(pixel(&image, 0, 0), pixel(&image, 10, 0));
    }

    #[test]
    fn mosaic_samples_the_frame_not_other_shapes() {
        let rect = Shape::Rectangle {
            from: Point::new(100, 50),
            to: Point::new(140, 80),
            color: RED,
            width: 30,
        };
        let mosaic = Shape::Mosaic {
            from: Point::new(110, 60),
            to: Point::new(130, 70),
            block: 20,
        };
        let image = compose(&grey_frame(), &FRAME, &[rect, mosaic]);
        // 马赛克按冻结帧取色，是灰色，盖在红色矩形之上
        assert_eq!(pixel(&image, 15, 15), (200, 200, 200, 255));
    }

    #[test]
    fn mosaic_partial_edge_block_stays_inside_the_area() {
        let mosaic = Shape::Mosaic {
            from: Point::new(100, 50),
            to: Point::new(125, 75),
            block: 10,
        };
        let image = compose(&gradient_frame(), &FRAME, &[mosaic]);
        // 区域 25×25，最后一块只有 5 像素宽，区域之外保持原图
        assert_eq!(pixel(&image, 24, 0), pixel(&image, 20, 0));
        assert_eq!(pixel(&image, 26, 0), (26 * 6, 0, 40, 255));
        assert_eq!(pixel(&image, 0, 26), (0, 26 * 6, 40, 255));
    }

    #[test]
    fn number_badge_is_a_filled_circle() {
        let badge = Shape::Number {
            center: Point::new(120, 65),
            value: 1,
            style: NumberStyle::Number,
            color: Color::rgb(0x00, 0x7A, 0xFF),
            size: 20,
        };
        let image = compose(&grey_frame(), &FRAME, &[badge]);
        // 圆心在 (20, 15)，半径 10：偏离文字的位置是纯色
        assert_eq!(pixel(&image, 26, 15), (0x00, 0x7A, 0xFF, 255));
        assert_eq!(pixel(&image, 20, 8), (0x00, 0x7A, 0xFF, 255));
        // 圆外与外接正方形的角保持原图
        assert_eq!(pixel(&image, 11, 6), (200, 200, 200, 255));
        assert_eq!(pixel(&image, 30, 25), (200, 200, 200, 255));
    }

    #[test]
    fn shapes_are_painted_in_insertion_order() {
        let red = Shape::Rectangle {
            from: Point::new(105, 55),
            to: Point::new(125, 75),
            color: RED,
            width: 6,
        };
        let blue = Shape::Rectangle {
            from: Point::new(105, 55),
            to: Point::new(125, 75),
            color: Color::rgb(0, 0, 255),
            width: 6,
        };
        let a = compose(&grey_frame(), &FRAME, &[red.clone(), blue.clone()]);
        let b = compose(&grey_frame(), &FRAME, &[blue, red]);
        assert_eq!(pixel(&a, 5, 15), (0, 0, 255, 255));
        assert_eq!(pixel(&b, 5, 15), (0xFF, 0x3B, 0x30, 255));
    }

    #[test]
    fn compose_rejects_area_outside_the_frame() {
        let renderer = Renderer::new().expect("创建渲染器");
        let bgra = grey_frame();
        let frame = FrameRef {
            bounds: FRAME,
            bgra: &bgra,
        };
        assert!(
            renderer
                .compose(frame, &Rect::new(0, 0, 5, 5), &[])
                .is_err()
        );
    }

    #[test]
    fn crop_bgra_rejects_out_of_range_area() {
        let bgra = grey_frame();
        let frame = FrameRef {
            bounds: FRAME,
            bgra: &bgra,
        };
        assert!(crop_bgra(&frame, &Rect::new(130, 70, 20, 20)).is_err());
        assert!(crop_bgra(&frame, &Rect::new(90, 50, 5, 5)).is_err());
        assert_eq!(
            crop_bgra(&frame, &Rect::new(100, 50, 2, 2))
                .map(|v| v.len())
                .ok(),
            Some(16)
        );
    }
}
