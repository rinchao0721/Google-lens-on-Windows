//! Win32 句柄的 RAII 包装，在 `Drop` 中释放。

use windows::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, HBITMAP, HDC,
    HGDIOBJ, ReleaseDC, SelectObject,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS, RegCloseKey, RegCreateKeyExW,
    RegOpenKeyExW,
};
use windows::Win32::UI::WindowsAndMessaging::{CreatePopupMenu, DestroyMenu, HMENU};

use crate::error::{PlatformError, last_error, win32};
use crate::text::to_wide;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::core::{PCWSTR, PWSTR};

/// 显式丢弃 `Drop` 中无法处理的返回值。
pub(crate) fn discard<T>(_value: T) {}

/// 内核对象句柄，如互斥量。
pub struct OwnedHandle(HANDLE);

impl OwnedHandle {
    /// 接管一个有效句柄。
    pub(crate) fn new(handle: HANDLE) -> Self {
        Self(handle)
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // Drop 中无法上报错误，关闭失败只会泄漏一个句柄
        // SAFETY: 句柄由 new 接管，仅在此处关闭一次。
        discard(unsafe { CloseHandle(self.0) });
    }
}

/// 整个屏幕的设备上下文。
pub struct ScreenDc(HDC);

impl ScreenDc {
    /// 获取屏幕 DC。
    pub fn acquire() -> Result<Self, PlatformError> {
        // SAFETY: 传入 None 取整个屏幕的 DC，返回值在下面检查。
        let hdc = unsafe { GetDC(None) };
        if hdc.is_invalid() {
            return Err(last_error("GetDC"));
        }
        Ok(Self(hdc))
    }

    /// 原始 DC，借用期间不得释放。
    pub fn raw(&self) -> HDC {
        self.0
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        // Drop 中无法上报错误，释放失败只会泄漏一个 DC
        // SAFETY: DC 由 GetDC(None) 获得，仅在此处释放一次。
        discard(unsafe { ReleaseDC(None, self.0) });
    }
}

/// 内存设备上下文。
pub struct MemDc(HDC);

impl MemDc {
    /// 创建与屏幕兼容的内存 DC。
    pub fn create_compatible(screen: &ScreenDc) -> Result<Self, PlatformError> {
        // SAFETY: screen 持有有效 DC，返回值在下面检查。
        let hdc = unsafe { CreateCompatibleDC(Some(screen.raw())) };
        if hdc.is_invalid() {
            return Err(last_error("CreateCompatibleDC"));
        }
        Ok(Self(hdc))
    }

    /// 原始 DC，借用期间不得释放。
    pub fn raw(&self) -> HDC {
        self.0
    }
}

impl Drop for MemDc {
    fn drop(&mut self) {
        // Drop 中无法上报错误，删除失败只会泄漏一个 DC
        // SAFETY: DC 由 CreateCompatibleDC 创建，仅在此处删除一次。
        discard(unsafe { DeleteDC(self.0) });
    }
}

/// 位图。
pub struct Bitmap(HBITMAP);

impl Bitmap {
    /// 创建与屏幕兼容的位图，宽高须为正。
    pub fn create_compatible(
        screen: &ScreenDc,
        width: i32,
        height: i32,
    ) -> Result<Self, PlatformError> {
        // SAFETY: screen 持有有效 DC，返回值在下面检查。
        let bmp = unsafe { CreateCompatibleBitmap(screen.raw(), width, height) };
        if bmp.is_invalid() {
            return Err(last_error("CreateCompatibleBitmap"));
        }
        Ok(Self(bmp))
    }

    /// 原始位图句柄，借用期间不得释放。
    pub fn raw(&self) -> HBITMAP {
        self.0
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        // Drop 中无法上报错误，删除失败只会泄漏一个位图
        // SAFETY: 位图由 CreateCompatibleBitmap 创建，调用方保证已从所有 DC 中选出。
        discard(unsafe { DeleteObject(HGDIOBJ::from(self.0)) });
    }
}

/// 把位图选入内存 DC，离开作用域时还原。
pub struct SelectGuard<'a> {
    dc: &'a MemDc,
    previous: HGDIOBJ,
}

impl<'a> SelectGuard<'a> {
    /// 把位图选入内存 DC。
    pub fn select(dc: &'a MemDc, bitmap: &Bitmap) -> Result<Self, PlatformError> {
        // SAFETY: dc 与 bitmap 都持有有效句柄，返回值在下面检查。
        let previous = unsafe { SelectObject(dc.raw(), HGDIOBJ::from(bitmap.raw())) };
        if previous.is_invalid() {
            return Err(last_error("SelectObject"));
        }
        Ok(Self { dc, previous })
    }
}

impl Drop for SelectGuard<'_> {
    fn drop(&mut self) {
        // Drop 中无法上报错误，还原失败时位图随 DC 一并销毁
        // SAFETY: previous 来自同一个 DC 的上一次选择。
        discard(unsafe { SelectObject(self.dc.raw(), self.previous) });
    }
}

/// 弹出式菜单。
pub struct PopupMenu(HMENU);

impl PopupMenu {
    /// 创建空菜单。
    pub fn create() -> Result<Self, PlatformError> {
        // SAFETY: 无前置条件，失败时返回错误。
        let menu = unsafe { CreatePopupMenu() }.map_err(win32("CreatePopupMenu"))?;
        Ok(Self(menu))
    }

    /// 原始菜单句柄，借用期间不得销毁。
    pub fn raw(&self) -> HMENU {
        self.0
    }

    /// 菜单已作为子菜单交给父菜单后调用，父菜单销毁时会一并销毁，此处不再释放。
    pub(crate) fn into_parent_owned(self) {
        std::mem::forget(self);
    }
}

impl Drop for PopupMenu {
    fn drop(&mut self) {
        // Drop 中无法上报错误，销毁失败只会泄漏一个菜单
        // SAFETY: 菜单由 CreatePopupMenu 创建，仅在此处销毁一次。
        discard(unsafe { DestroyMenu(self.0) });
    }
}

/// 可移动全局内存块，所有权未交给系统时在 `Drop` 中释放。
pub struct GlobalMem {
    handle: Option<HGLOBAL>,
}

impl GlobalMem {
    /// 分配 `size` 字节的可移动内存。
    pub fn alloc(size: usize) -> Result<Self, PlatformError> {
        // SAFETY: 无前置条件，失败时返回错误。
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, size) }.map_err(win32("GlobalAlloc"))?;
        Ok(Self {
            handle: Some(handle),
        })
    }

    /// 原始句柄；已交出所有权时返回 `None`。
    pub fn raw(&self) -> Option<HGLOBAL> {
        self.handle
    }

    /// 所有权交给系统（如剪贴板）后调用，`Drop` 不再释放。
    pub fn release(&mut self) -> Option<HGLOBAL> {
        self.handle.take()
    }
}

impl Drop for GlobalMem {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // Drop 中无法上报错误，释放失败只会泄漏一块内存
            // SAFETY: 句柄由 GlobalAlloc 分配且所有权仍在本对象手中。
            discard(unsafe { GlobalFree(Some(handle)) });
        }
    }
}

/// 由 COM 分配、需要 `CoTaskMemFree` 释放的宽字符串。
pub struct CoTaskString(PWSTR);

impl CoTaskString {
    /// 接管一个由 COM 分配的字符串。
    ///
    /// # Safety
    /// `raw` 必须为空或来自 COM 分配器，且调用方此后不再释放它。
    pub unsafe fn from_raw(raw: PWSTR) -> Self {
        Self(raw)
    }

    /// 转为 Rust 字符串；为空或不是合法 UTF-16 时返回错误。
    pub fn to_string_lossless(&self) -> Result<String, PlatformError> {
        if self.0.is_null() {
            return Err(PlatformError::Invalid("系统返回了空字符串".to_string()));
        }
        // SAFETY: 指针非空，由 COM 返回的字符串以 NUL 结尾，生命周期由本对象管理。
        unsafe { self.0.to_string() }
            .map_err(|_| PlatformError::Invalid("系统返回的字符串不是合法 UTF-16".to_string()))
    }
}

impl Drop for CoTaskString {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: 字符串由 COM 分配器分配，所有权在本对象手中，仅释放一次。
            unsafe { CoTaskMemFree(Some(self.0.0.cast_const().cast())) };
        }
    }
}

/// `HKEY_CURRENT_USER` 下的注册表项。
pub struct RegKey(HKEY);

impl RegKey {
    /// 打开（不存在则创建）子项。
    pub fn create(subkey: &str, access: REG_SAM_FLAGS) -> Result<Self, PlatformError> {
        let wide = to_wide(subkey);
        let mut key = HKEY::default();
        // SAFETY: wide 以 NUL 结尾且在调用期间有效，key 是有效的输出变量。
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(wide.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                access,
                None,
                &mut key,
                None,
            )
        }
        .ok()
        .map_err(win32("RegCreateKeyExW"))?;
        Ok(Self(key))
    }

    /// 打开已存在的子项；子项不存在时返回 `Ok(None)`。
    pub fn open(subkey: &str, access: REG_SAM_FLAGS) -> Result<Option<Self>, PlatformError> {
        let wide = to_wide(subkey);
        let mut key = HKEY::default();
        // SAFETY: wide 以 NUL 结尾且在调用期间有效，key 是有效的输出变量。
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(wide.as_ptr()),
                None,
                access,
                &mut key,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        status.ok().map_err(win32("RegOpenKeyExW"))?;
        Ok(Some(Self(key)))
    }

    /// 原始句柄，借用期间不得关闭。
    pub fn raw(&self) -> HKEY {
        self.0
    }
}

impl Drop for RegKey {
    fn drop(&mut self) {
        // Drop 中无法上报错误，关闭失败只会泄漏一个句柄
        // SAFETY: 句柄由 RegCreateKeyExW / RegOpenKeyExW 取得，仅在此处关闭一次。
        discard(unsafe { RegCloseKey(self.0) });
    }
}
