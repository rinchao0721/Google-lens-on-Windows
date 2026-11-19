//! Direct2D 贴图绘制：1:1 物理像素位图、旋转缩放矩阵与 1px 实线/虚线边框。

use lenswin_core::frame::CroppedImage;
use lenswin_core::geometry::Rect;
use lenswin_core::lens::downscale;
use lenswin_core::pin::Rotation;
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
    D2D1_BITMAP_PROPERTIES, D2D1_BRUSH_PROPERTIES, D2D1_CAP_STYLE_FLAT, D2D1_DASH_STYLE_DASH,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES, D2D1_STROKE_STYLE_PROPERTIES,
    D2D1CreateFactory, ID2D1Bitmap, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
    ID2D1StrokeStyle,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

use crate::error::{PinError, win32};
use crate::state::PinState;

const BORDER_WIDTH: f32 = 1.0;

const fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

/// 贴图窗口共用的 Direct2D 工厂与线型。
pub(crate) struct Renderer {
    factory: ID2D1Factory,
    dash_stroke: ID2D1StrokeStyle,
}

/// 单个贴图窗口的设备相关资源。
pub(crate) struct TargetResources {
    target: ID2D1HwndRenderTarget,
    bitmap: ID2D1Bitmap,
    bitmap_size: (u32, u32),
    accent: ID2D1SolidColorBrush,
    unfocused: ID2D1SolidColorBrush,
    target_size: (u32, u32),
}

impl Renderer {
    pub(crate) fn new() -> Result<Self, PinError> {
        // SAFETY: 单线程工厂枚举参数有效，无选项。
        let factory =
            unsafe { D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
                .map_err(win32("D2D1CreateFactory"))?;
        let dash_props = D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_FLAT,
            endCap: D2D1_CAP_STYLE_FLAT,
            dashCap: D2D1_CAP_STYLE_FLAT,
            dashStyle: D2D1_DASH_STYLE_DASH,
            ..Default::default()
        };
        // SAFETY: dash_props 已完整填写，使用内置虚线样式无需自定义数组。
        let dash_stroke = unsafe { factory.CreateStrokeStyle(&dash_props, None) }
            .map_err(win32("CreateStrokeStyle"))?;
        Ok(Self {
            factory,
            dash_stroke,
        })
    }

    pub(crate) fn create_target(
        &self,
        hwnd: HWND,
        win_size: (u32, u32),
        image: &CroppedImage,
    ) -> Result<TargetResources, PinError> {
        // 固定 96 DPI，确保 1 个 DIP 永远等于 1 个物理像素
        let target_props = D2D1_RENDER_TARGET_PROPERTIES {
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: D2D_SIZE_U {
                width: win_size.0.max(1),
                height: win_size.1.max(1),
            },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        // SAFETY: hwnd 为当前线程有效窗口，渲染属性已完整填写。
        let target = unsafe {
            self.factory
                .CreateHwndRenderTarget(&target_props, &hwnd_props)
        }
        .map_err(win32("CreateHwndRenderTarget"))?;

        // SAFETY: target 刚创建成功，GetMaximumBitmapSize 只读设备纹理边长上限。
        let max_side = unsafe { target.GetMaximumBitmapSize() }.max(1);
        let (bitmap_size, pbgra) = display_image_data(image, max_side)?;
        let pitch = bitmap_size
            .0
            .checked_mul(4)
            .ok_or_else(|| PinError::InvalidInput("贴图行字节数溢出".to_string()))?;

        let bitmap_props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        // SAFETY: pbgra 长度等于 bitmap_size.0 * bitmap_size.1 * 4，pitch 与之匹配且调用期间有效。
        let bitmap = unsafe {
            target.CreateBitmap(
                D2D_SIZE_U {
                    width: bitmap_size.0,
                    height: bitmap_size.1,
                },
                Some(pbgra.as_ptr().cast()),
                pitch,
                &bitmap_props,
            )
        }
        .map_err(win32("CreateBitmap"))?;

        // 强调色与覆盖层保持一致；无焦点时用半透明浅灰
        // SAFETY: target 刚刚创建，颜色结构为局部变量。
        let accent = unsafe { target.CreateSolidColorBrush(&color(0.30, 0.60, 1.0, 1.0), None) }
            .map_err(win32("CreateSolidColorBrush"))?;
        // SAFETY: 同上。
        let unfocused =
            unsafe { target.CreateSolidColorBrush(&color(0.75, 0.75, 0.75, 0.75), None) }
                .map_err(win32("CreateSolidColorBrush"))?;

        Ok(TargetResources {
            target,
            bitmap,
            bitmap_size,
            accent,
            unfocused,
            target_size: (win_size.0.max(1), win_size.1.max(1)),
        })
    }

    pub(crate) fn draw(
        &self,
        res: &mut TargetResources,
        state: &PinState,
    ) -> Result<bool, PinError> {
        let rect = state.rect();
        let win_w = rect.width.max(1);
        let win_h = rect.height.max(1);
        if res.target_size != (win_w, win_h) {
            let new_size = D2D_SIZE_U {
                width: win_w,
                height: win_h,
            };
            // SAFETY: target 有效，尺寸至少为 1x1。
            unsafe { res.target.Resize(&new_size) }.map_err(win32("Resize"))?;
            res.target_size = (win_w, win_h);
        }

        let (bmp_w, bmp_h) = res.bitmap_size;
        let transform = bitmap_transform(res.bitmap_size, &rect, state.rotation());
        let identity = D2D1_BRUSH_PROPERTIES::default().transform;
        let interp = if state.zoom_permille() == 1000 && res.bitmap_size == (win_w, win_h) {
            D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR
        } else {
            D2D1_BITMAP_INTERPOLATION_MODE_LINEAR
        };

        let w = u32_to_f32(win_w);
        let h = u32_to_f32(win_h);
        let src_rect = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: u32_to_f32(bmp_w),
            bottom: u32_to_f32(bmp_h),
        };
        let border_rect = D2D_RECT_F {
            left: 0.5,
            top: 0.5,
            right: (w - 0.5).max(0.5),
            bottom: (h - 0.5).max(0.5),
        };
        let border_brush = if state.is_focused() {
            &res.accent
        } else {
            &res.unfocused
        };
        let stroke_style = state.is_passthrough().then_some(&self.dash_stroke);

        // SAFETY: 所有 Direct2D 资源来自同一 target，且 BeginDraw 与 EndDraw 成对调用。
        unsafe {
            res.target.BeginDraw();
            res.target.SetTransform(&identity);
            res.target.Clear(Some(&color(0.0, 0.0, 0.0, 1.0)));
            res.target.SetTransform(&transform.transform);
            res.target
                .DrawBitmap(&res.bitmap, Some(&src_rect), 1.0, interp, None);
            res.target.SetTransform(&identity);
            res.target
                .DrawRectangle(&border_rect, border_brush, BORDER_WIDTH, stroke_style);
        }

        // SAFETY: 与 BeginDraw 成对调用。
        match unsafe { res.target.EndDraw(None, None) } {
            Ok(()) => Ok(false),
            Err(e) if e.code() == D2DERR_RECREATE_TARGET => Ok(true),
            Err(e) => Err(win32("EndDraw")(e)),
        }
    }
}

/// 生成用于 Direct2D 显示位图的尺寸和预乘 BGRA 缓冲；超过硬件纹理上限时等比缩小，不影响原图导出。
fn display_image_data(
    image: &CroppedImage,
    max_side: u32,
) -> Result<((u32, u32), Vec<u8>), PinError> {
    if image.width() <= max_side && image.height() <= max_side {
        let pbgra = rgba_to_premultiplied_bgra(image.rgba());
        return Ok(((image.width(), image.height()), pbgra));
    }
    let scaled = downscale(image.clone(), max_side)
        .map_err(|e| PinError::InvalidInput(format!("缩小超长贴图失败：{e}")))?;
    let pbgra = rgba_to_premultiplied_bgra(scaled.rgba());
    Ok(((scaled.width(), scaled.height()), pbgra))
}

/// RGBA 转预乘 Alpha 的 BGRA（Direct2D 位图格式）。
pub(crate) fn rgba_to_premultiplied_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for &[r, g, b, a] in rgba.as_chunks::<4>().0 {
        match a {
            255 => out.extend_from_slice(&[b, g, r, 255]),
            0 => out.extend_from_slice(&[0, 0, 0, 0]),
            alpha => {
                let pb = premul(b, alpha);
                let pg = premul(g, alpha);
                let pr = premul(r, alpha);
                out.extend_from_slice(&[pb, pg, pr, alpha]);
            }
        }
    }
    out
}

fn premul(channel: u8, alpha: u8) -> u8 {
    let val = (u16::from(channel) * u16::from(alpha) + 127) / 255;
    u8::try_from(val).unwrap_or(255)
}

/// `u32` 转 `f32`，拆成高低 16 位相加，避免 `as`。
fn u32_to_f32(value: u32) -> f32 {
    let high = u16::try_from(value >> 16).unwrap_or(u16::MAX);
    let low = u16::try_from(value & 0xFFFF).unwrap_or(u16::MAX);
    f32::from(high) * 65_536.0 + f32::from(low)
}

/// 生成把原图绕中心旋转并缩放到当前窗口客户区的仿射矩阵，借 `D2D1_BRUSH_PROPERTIES` 构造矩阵类型。
fn bitmap_transform(
    img_size: (u32, u32),
    win_rect: &Rect,
    rotation: Rotation,
) -> D2D1_BRUSH_PROPERTIES {
    let (rot_w, rot_h) = rotation.dimensions(img_size.0, img_size.1);
    let sx = u32_to_f32(win_rect.width.max(1)) / u32_to_f32(rot_w.max(1));
    let sy = u32_to_f32(win_rect.height.max(1)) / u32_to_f32(rot_h.max(1));
    let (c, s) = match rotation {
        Rotation::Deg0 => (1.0f32, 0.0f32),
        Rotation::Deg90 => (0.0f32, 1.0f32),
        Rotation::Deg180 => (-1.0f32, 0.0f32),
        Rotation::Deg270 => (0.0f32, -1.0f32),
    };
    let cx_src = u32_to_f32(img_size.0) / 2.0;
    let cy_src = u32_to_f32(img_size.1) / 2.0;
    let cx_dst = u32_to_f32(win_rect.width.max(1)) / 2.0;
    let cy_dst = u32_to_f32(win_rect.height.max(1)) / 2.0;

    let mut props = D2D1_BRUSH_PROPERTIES::default();
    props.transform.M11 = sx * c;
    props.transform.M12 = sy * s;
    props.transform.M21 = -sx * s;
    props.transform.M22 = sy * c;
    props.transform.M31 = cx_dst + sx * (-cx_src * c + cy_src * s);
    props.transform.M32 = cy_dst + sy * (-cx_src * s - cy_src * c);
    props
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u32_to_f32_matches_value() {
        assert_eq!(u32_to_f32(0), 0.0);
        assert_eq!(u32_to_f32(81_920), 81_920.0);
        assert_eq!(u32_to_f32(16_777_216), 16_777_216.0);
    }

    #[test]
    fn premul_bgra_handles_opaque_transparent_and_half() {
        let rgba = [200, 100, 50, 255, 200, 100, 50, 0, 200, 100, 50, 128];
        let pbgra = rgba_to_premultiplied_bgra(&rgba);
        assert_eq!(pbgra, vec![50, 100, 200, 255, 0, 0, 0, 0, 25, 50, 100, 128]);
    }

    #[test]
    fn rotation_matrix_maps_corners_accurately() {
        let map = |m: &D2D1_BRUSH_PROPERTIES, x: f32, y: f32| -> (f32, f32) {
            let t = m.transform;
            (x * t.M11 + y * t.M21 + t.M31, x * t.M12 + y * t.M22 + t.M32)
        };
        let m90 = bitmap_transform((300, 100), &Rect::new(0, 0, 200, 600), Rotation::Deg90);
        // 原图左下角 (0, 100) 旋转 90° 后应落在窗口左上角 (0, 0)
        assert_eq!(map(&m90, 0.0, 100.0), (0.0, 0.0));
        // 原图左上角 (0, 0) 旋转 90° 后应落在窗口右上角 (200, 0)
        assert_eq!(map(&m90, 0.0, 0.0), (200.0, 0.0));
    }

    #[test]
    fn display_image_data_downscales_when_exceeding_max_side() {
        let rgba = vec![255u8; 100 * 2000 * 4];
        let img = CroppedImage::new(100, 2000, rgba).expect("构造长图");
        let ((w, h), buf) = display_image_data(&img, 1000).expect("生成显示位图数据");
        assert_eq!((w, h), (50, 1000));
        assert_eq!(buf.len(), 50 * 1000 * 4);
    }
}
