//! Direct2D 绘制：冻结帧、遮罩、选框与套索轨迹。

use lenswin_core::geometry::{Point, Rect};
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE;
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, D2D1_CAP_STYLE_ROUND,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_LINE_JOIN_ROUND,
    D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES, D2D1_STROKE_STYLE_PROPERTIES,
    D2D1CreateFactory, ID2D1Bitmap, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
    ID2D1StrokeStyle,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

use crate::error::{OverlayError, win32};
use crate::modes::Preview;

/// 选框描边宽度（像素）。
const OUTLINE_WIDTH: f32 = 2.0;
/// 遮罩透明度。
const DIM_ALPHA: f32 = 0.45;

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

/// Direct2D 工厂，整个覆盖层会话共用。
pub struct Renderer {
    factory: ID2D1Factory,
}

impl Renderer {
    /// 创建单线程工厂。
    pub fn new() -> Result<Self, OverlayError> {
        // SAFETY: 参数为合法的枚举值，无选项。
        let factory =
            unsafe { D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
                .map_err(win32("D2D1CreateFactory"))?;
        Ok(Self { factory })
    }

    /// 为一个窗口创建渲染目标和设备相关资源，`bgra` 为该显示器的冻结帧。
    pub fn create_target(
        &self,
        hwnd: HWND,
        size: (u32, u32),
        bgra: &[u8],
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

        let brush = |c: D2D1_COLOR_F| -> Result<ID2D1SolidColorBrush, OverlayError> {
            // SAFETY: c 是有效的颜色结构，目标刚刚创建。
            unsafe { target.CreateSolidColorBrush(&c, None) }
                .map_err(win32("CreateSolidColorBrush"))
        };
        let dim = brush(color(0.0, 0.0, 0.0, DIM_ALPHA))?;
        let trail = brush(color(1.0, 1.0, 1.0, 0.9))?;
        let accent = brush(color(0.30, 0.60, 1.0, 1.0))?;

        let stroke_props = D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_ROUND,
            endCap: D2D1_CAP_STYLE_ROUND,
            lineJoin: D2D1_LINE_JOIN_ROUND,
            ..Default::default()
        };
        // SAFETY: stroke_props 已完整填写，无自定义虚线。
        let round_stroke = unsafe { self.factory.CreateStrokeStyle(&stroke_props, None) }
            .map_err(win32("CreateStrokeStyle"))?;

        Ok(TargetResources {
            target,
            bitmap,
            dim,
            trail,
            accent,
            round_stroke,
        })
    }
}

/// 单个窗口的渲染目标与资源，目标失效时整体丢弃重建。
pub struct TargetResources {
    target: ID2D1HwndRenderTarget,
    bitmap: ID2D1Bitmap,
    dim: ID2D1SolidColorBrush,
    trail: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    round_stroke: ID2D1StrokeStyle,
}

impl TargetResources {
    /// 绘制一帧；`origin` 是该显示器在虚拟桌面上的左上角，用于把预览坐标转为窗口坐标。
    pub fn draw(
        &self,
        origin: Point,
        size: (u32, u32),
        preview: Preview<'_>,
        stroke_width: u32,
    ) -> Result<DrawStatus, OverlayError> {
        let full = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: to_f32(i64::from(size.0)),
            bottom: to_f32(i64::from(size.1)),
        };
        let region = preview_region(&preview, origin, size).map(|r| rect_to_d2d(&r));
        let target = &self.target;
        // SAFETY: 目标与资源均有效；BeginDraw/EndDraw 成对调用，期间所有指针参数都指向存活的局部数据。
        unsafe {
            target.BeginDraw();
            target.DrawBitmap(
                &self.bitmap,
                Some(&full),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
            target.FillRectangle(&full, &self.dim);
            if let Some(r) = region {
                // 选区内还原为原图亮度，源矩形与目标矩形相同即 1:1 贴图
                target.DrawBitmap(
                    &self.bitmap,
                    Some(&r),
                    1.0,
                    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                    Some(&r),
                );
                target.DrawRectangle(&r, &self.accent, OUTLINE_WIDTH, None);
            }
            if let Preview::Lasso { trail, .. } = preview {
                let width = to_f32(i64::from(stroke_width));
                for pair in trail.windows(2) {
                    if let [a, b] = pair {
                        target.DrawLine(
                            local_point(*a, origin).point,
                            local_point(*b, origin).point,
                            &self.trail,
                            width,
                            &self.round_stroke,
                        );
                    }
                }
            }
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

/// 坐标转 `f32`。显示器像素尺寸远小于 `i16` 范围，超出的值饱和到边界即可。
fn to_f32(value: i64) -> f32 {
    let clamped = value.clamp(i64::from(i16::MIN), i64::from(i16::MAX));
    f32::from(i16::try_from(clamped).unwrap_or(0))
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
    fn to_f32_saturates() {
        assert_eq!(to_f32(100), 100.0);
        assert_eq!(to_f32(-100), -100.0);
        assert_eq!(to_f32(i64::MAX), f32::from(i16::MAX));
        assert_eq!(to_f32(i64::MIN), f32::from(i16::MIN));
    }
}
