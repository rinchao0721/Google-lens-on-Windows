//! 基于 GDI BitBlt 的屏幕捕获。

use lenswin_core::frame::Frame;
use lenswin_core::monitor::MonitorInfo;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, DIB_RGB_COLORS, GetDIBits, RGBQUAD,
    SRCCOPY,
};

use super::{CaptureError, ScreenCapturer};
use crate::error::{PlatformError, last_error};
use crate::handle::{Bitmap, MemDc, ScreenDc, SelectGuard};

/// GDI 捕获器。
#[derive(Debug, Default, Clone, Copy)]
pub struct GdiCapturer;

impl ScreenCapturer for GdiCapturer {
    fn capture_all(&self, monitors: &[MonitorInfo]) -> Result<Vec<Frame>, CaptureError> {
        let screen = ScreenDc::acquire()?;
        monitors
            .iter()
            .map(|m| capture_monitor(&screen, m))
            .collect()
    }
}

fn capture_monitor(screen: &ScreenDc, monitor: &MonitorInfo) -> Result<Frame, CaptureError> {
    let width = i32::try_from(monitor.bounds.width)
        .map_err(|_| PlatformError::Invalid("显示器宽度超出范围".to_string()))?;
    let height = i32::try_from(monitor.bounds.height)
        .map_err(|_| PlatformError::Invalid("显示器高度超出范围".to_string()))?;
    let byte_len = usize::try_from(monitor.bounds.width)
        .ok()
        .zip(usize::try_from(monitor.bounds.height).ok())
        .and_then(|(w, h)| w.checked_mul(h))
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| PlatformError::Invalid("帧缓冲区大小溢出".to_string()))?;

    let mem = MemDc::create_compatible(screen)?;
    let bitmap = Bitmap::create_compatible(screen, width, height)?;
    {
        let _selected = SelectGuard::select(&mem, &bitmap)?;
        // CAPTUREBLT 才能抓到分层窗口（如部分浮层和菜单）
        // SAFETY: 两个 DC 与位图均有效，宽高为正且已限制在 i32 范围内。
        unsafe {
            BitBlt(
                mem.raw(),
                0,
                0,
                width,
                height,
                Some(screen.raw()),
                monitor.bounds.x,
                monitor.bounds.y,
                SRCCOPY | CAPTUREBLT,
            )
        }
        .map_err(|source| PlatformError::Win32 {
            api: "BitBlt",
            source,
        })?;
    }

    let mut rgba = vec![0u8; byte_len];
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: u32::try_from(std::mem::size_of::<BITMAPINFOHEADER>())
                .map_err(|_| PlatformError::Invalid("头部大小超出范围".to_string()))?,
            biWidth: width,
            // 负高度表示自上而下
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        bmiColors: [RGBQUAD::default()],
    };
    // GetDIBits 要求位图此时不在任何 DC 中，上面的 SelectGuard 已在块结束时还原
    // SAFETY: rgba 长度恰为宽×高×4，与 info 描述一致；位图有效且未被选入 DC。
    let copied = unsafe {
        GetDIBits(
            screen.raw(),
            bitmap.raw(),
            0,
            monitor.bounds.height,
            Some(rgba.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    if copied == 0 {
        return Err(last_error("GetDIBits").into());
    }
    // 返回值是实际复制的扫描行数，部分复制时剩余行为黑色，必须视为失败
    if copied != height {
        return Err(
            PlatformError::Invalid(format!("GetDIBits 只复制了 {copied}/{height} 行")).into(),
        );
    }
    bgra_to_opaque_rgba(&mut rgba);
    Ok(Frame::new(monitor.bounds, rgba)?)
}

/// BGRA 转 RGBA；屏幕 BitBlt 的 alpha 通道不可靠，一律置为不透明。
fn bgra_to_opaque_rgba(buf: &mut [u8]) {
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
        if let Some(a) = px.get_mut(3) {
            *a = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_bgra_and_forces_opaque() {
        let mut buf = [1u8, 2, 3, 0, 10, 20, 30, 7];
        bgra_to_opaque_rgba(&mut buf);
        assert_eq!(buf, [3, 2, 1, 255, 30, 20, 10, 255]);
    }

    #[test]
    fn conversion_ignores_trailing_partial_pixel() {
        let mut buf = [1u8, 2, 3, 4, 9, 9];
        bgra_to_opaque_rgba(&mut buf);
        assert_eq!(buf, [3, 2, 1, 255, 9, 9]);
    }
}
