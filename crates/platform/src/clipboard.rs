//! 剪贴板：把图像以 CF_DIB、文本以 CF_UNICODETEXT 写入。

use std::time::Duration;

use lenswin_core::frame::CroppedImage;
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};

use crate::error::{PlatformError, last_error, win32};
use crate::handle::{GlobalMem, discard};
use crate::window::MessageWindow;

/// 剪贴板被其它进程占用时的重试次数。
const OPEN_RETRIES: u32 = 10;
/// 重试间隔。
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(15);
/// 读取剪贴板文本时最多取的 UTF-16 单元数，防止异常大的内容被整块复制。
const MAX_READ_UNITS: usize = 1 << 18;
/// BITMAPINFOHEADER 大小。
const DIB_HEADER_LEN: usize = 40;

/// 剪贴板所有者：消息窗口句柄的数值，可跨线程传递。
///
/// 窗口销毁后再使用，`OpenClipboard` 会返回错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardOwner(usize);

impl ClipboardOwner {
    /// 以消息窗口作为所有者。
    pub fn of(window: &MessageWindow) -> Self {
        Self(window.hwnd().0.expose_provenance())
    }

    fn hwnd(self) -> HWND {
        HWND(std::ptr::with_exposed_provenance_mut(self.0))
    }
}

/// 打开期间持有剪贴板，离开作用域时关闭。
struct ClipboardGuard;

impl ClipboardGuard {
    fn open(owner: ClipboardOwner) -> Result<Self, PlatformError> {
        let mut last = None;
        for attempt in 0..OPEN_RETRIES {
            // SAFETY: 句柄无效时 OpenClipboard 只会返回错误。
            match unsafe { OpenClipboard(Some(owner.hwnd())) } {
                Ok(()) => return Ok(Self),
                Err(e) => last = Some(e),
            }
            if attempt + 1 < OPEN_RETRIES {
                std::thread::sleep(OPEN_RETRY_DELAY);
            }
        }
        match last {
            Some(source) => Err(PlatformError::Win32 {
                api: "OpenClipboard",
                source,
            }),
            None => Err(PlatformError::Invalid("无法打开剪贴板".to_string())),
        }
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // Drop 中无法上报错误，关闭失败时剪贴板会在进程退出时释放
        // SAFETY: 由 open 成功打开，仅在此处关闭一次。
        discard(unsafe { CloseClipboard() });
    }
}

/// 把图像写入剪贴板，会清空原有内容。
pub fn set_image(owner: ClipboardOwner, image: &CroppedImage) -> Result<(), PlatformError> {
    let dib = rgba_to_dib(image)?;
    set_data(owner, u32::from(CF_DIB.0), &dib)
}

/// 把文本写入剪贴板，会清空原有内容。
pub fn set_text(owner: ClipboardOwner, text: &str) -> Result<(), PlatformError> {
    set_data(owner, u32::from(CF_UNICODETEXT.0), &utf16_with_nul(text))
}

/// 读取剪贴板里的 Unicode 文本；没有文本格式时返回 `Ok(None)`。
pub fn get_text(owner: ClipboardOwner) -> Result<Option<String>, PlatformError> {
    let format = u32::from(CF_UNICODETEXT.0);
    let _clipboard = ClipboardGuard::open(owner)?;
    // SAFETY: 剪贴板已由当前线程打开。
    if unsafe { IsClipboardFormatAvailable(format) }.is_err() {
        return Ok(None);
    }
    // SAFETY: 剪贴板已打开；返回的句柄归系统所有，不可释放。
    let handle = unsafe { GetClipboardData(format) }.map_err(win32("GetClipboardData"))?;
    let global = HGLOBAL(handle.0);
    // SAFETY: global 来自 GetClipboardData，剪贴板仍处于打开状态。
    let bytes = unsafe { GlobalSize(global) };
    // SAFETY: 同上。
    let ptr = unsafe { GlobalLock(global) };
    if ptr.is_null() {
        return Err(last_error("GlobalLock"));
    }
    let units = (bytes / 2).min(MAX_READ_UNITS);
    // SAFETY: 锁定期间 ptr 指向至少 bytes 字节的可读内存，可移动内存按 8 字节以上对齐，units 不超过其一半。
    let text = decode_utf16_text(unsafe { std::slice::from_raw_parts(ptr.cast::<u16>(), units) });
    // 数据已复制，解锁结果不影响后续
    // SAFETY: 与上面的 GlobalLock 配对。
    discard(unsafe { GlobalUnlock(global) });
    Ok(Some(text))
}

/// 解码 UTF-16 文本：遇到 NUL 结束，非法单元以替换字符代替。
fn decode_utf16_text(units: &[u16]) -> String {
    let end = units.iter().position(|u| *u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(units.get(..end).unwrap_or_default())
}

/// 文本转为以 NUL 结尾的 UTF-16 小端字节。
fn utf16_with_nul(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// 把一块数据以指定格式写入剪贴板。
fn set_data(owner: ClipboardOwner, format: u32, bytes: &[u8]) -> Result<(), PlatformError> {
    // 先备好全部数据，打开并清空剪贴板之后不再有会丢失原内容的失败路径
    let mut mem = GlobalMem::alloc(bytes.len())?;
    let handle = mem
        .raw()
        .ok_or_else(|| PlatformError::Invalid("全局内存句柄已失效".to_string()))?;
    // SAFETY: handle 来自 GlobalAlloc 的可移动内存。
    let ptr = unsafe { GlobalLock(handle) };
    if ptr.is_null() {
        return Err(last_error("GlobalLock"));
    }
    // SAFETY: 内存块大小等于 bytes.len()，已锁定且与 bytes 不重叠。
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast::<u8>(), bytes.len()) };
    // 数据已写入，解锁结果不影响后续
    // SAFETY: 与上面的 GlobalLock 配对。
    discard(unsafe { GlobalUnlock(handle) });

    let _clipboard = ClipboardGuard::open(owner)?;
    // SAFETY: 剪贴板已由当前线程打开。
    unsafe { EmptyClipboard() }.map_err(win32("EmptyClipboard"))?;
    // SAFETY: 剪贴板已打开且已清空，handle 为可移动内存；成功后所有权归系统。
    unsafe { SetClipboardData(format, Some(HANDLE(handle.0))) }
        .map_err(win32("SetClipboardData"))?;
    mem.release();
    Ok(())
}

/// RGBA 转为自下而上的 32 位 BGRA DIB（头部加像素），不依赖任何系统调用。
fn rgba_to_dib(image: &CroppedImage) -> Result<Vec<u8>, PlatformError> {
    let overflow = || PlatformError::Invalid("图像尺寸溢出".to_string());
    let width = usize::try_from(image.width()).map_err(|_| overflow())?;
    let height = usize::try_from(image.height()).map_err(|_| overflow())?;
    let row_bytes = width.checked_mul(4).ok_or_else(overflow)?;
    let pixel_bytes = row_bytes.checked_mul(height).ok_or_else(overflow)?;
    let total = pixel_bytes
        .checked_add(DIB_HEADER_LEN)
        .ok_or_else(overflow)?;
    let width_i32 = i32::try_from(image.width()).map_err(|_| overflow())?;
    let height_i32 = i32::try_from(image.height()).map_err(|_| overflow())?;
    let size_image = u32::try_from(pixel_bytes).map_err(|_| overflow())?;

    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(
        &u32::try_from(DIB_HEADER_LEN)
            .map_err(|_| overflow())?
            .to_le_bytes(),
    );
    out.extend_from_slice(&width_i32.to_le_bytes());
    out.extend_from_slice(&height_i32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // 平面数
    out.extend_from_slice(&32u16.to_le_bytes()); // 位深
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&size_image.to_le_bytes());
    out.extend_from_slice(&[0u8; 16]); // 分辨率与调色板字段

    if row_bytes == 0 {
        return Err(PlatformError::Invalid("图像宽度为 0".to_string()));
    }
    for row in image.rgba().chunks_exact(row_bytes).rev() {
        for px in row.chunks_exact(4) {
            if let [r, g, b, a] = *px {
                out.extend_from_slice(&[b, g, r, a]);
            }
        }
    }
    if out.len() != total {
        return Err(PlatformError::Invalid("DIB 数据长度不符".to_string()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_stops_at_nul_and_replaces_bad_units() {
        let units: Vec<u16> = "中a😀".encode_utf16().chain([0, 0x41]).collect();
        assert_eq!(decode_utf16_text(&units), "中a😀");
        let bad = decode_utf16_text(&[0x41, 0xD800]);
        assert_eq!(bad, format!("A{}", char::REPLACEMENT_CHARACTER));
        assert_eq!(decode_utf16_text(&[]), "");
        assert_eq!(decode_utf16_text(&[0, 0x41]), "");
    }

    fn image_2x2() -> CroppedImage {
        // 第一行：红、绿；第二行：蓝、白
        let rgba = vec![
            255, 0, 0, 255, 0, 255, 0, 255, //
            0, 0, 255, 255, 255, 255, 255, 255,
        ];
        CroppedImage::new(2, 2, rgba).expect("构造图像")
    }

    #[test]
    fn text_is_nul_terminated_utf16() {
        assert_eq!(utf16_with_nul(""), vec![0, 0]);
        assert_eq!(utf16_with_nul("#A"), vec![b'#', 0, b'A', 0, 0, 0]);
        assert_eq!(utf16_with_nul("色"), vec![0x72, 0x82, 0, 0]);
    }

    #[test]
    fn dib_header_fields() {
        let dib = rgba_to_dib(&image_2x2()).expect("转换");
        assert_eq!(dib.len(), DIB_HEADER_LEN + 16);
        assert_eq!(dib.get(0..4), Some(&40u32.to_le_bytes()[..]));
        assert_eq!(dib.get(4..8), Some(&2i32.to_le_bytes()[..]));
        assert_eq!(dib.get(8..12), Some(&2i32.to_le_bytes()[..]));
        assert_eq!(dib.get(12..14), Some(&1u16.to_le_bytes()[..]));
        assert_eq!(dib.get(14..16), Some(&32u16.to_le_bytes()[..]));
        assert_eq!(dib.get(20..24), Some(&16u32.to_le_bytes()[..]));
    }

    #[test]
    fn dib_rows_are_bottom_up_bgra() {
        let dib = rgba_to_dib(&image_2x2()).expect("转换");
        let pixels = dib.get(DIB_HEADER_LEN..).expect("像素区");
        // 底行在前：蓝(BGRA = 255,0,0,255)、白
        assert_eq!(pixels.get(0..4), Some(&[255u8, 0, 0, 255][..]));
        assert_eq!(pixels.get(4..8), Some(&[255u8, 255, 255, 255][..]));
        // 顶行在后：红(BGRA = 0,0,255,255)、绿
        assert_eq!(pixels.get(8..12), Some(&[0u8, 0, 255, 255][..]));
        assert_eq!(pixels.get(12..16), Some(&[0u8, 255, 0, 255][..]));
    }
}
