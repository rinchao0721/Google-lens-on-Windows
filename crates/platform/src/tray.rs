//! 托盘图标与右键菜单。

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, GetCursorPos, IDI_APPLICATION, LoadIconW, MF_SEPARATOR, MF_STRING, PostMessageW,
    SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_NULL,
};
use windows::core::PCWSTR;

use crate::error::{PlatformError, last_error, win32};
use crate::handle::{PopupMenu, discard};
use crate::text::{copy_wide, to_wide};
use crate::window::{MessageWindow, WM_TRAY};

/// 托盘图标在窗口内的标识。
const TRAY_ICON_UID: u32 = 1;

/// 托盘菜单项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    /// 可点击项，`id` 必须大于 0。
    Entry {
        /// 选中后返回的命令号。
        id: u16,
        /// 显示文字。
        label: String,
    },
    /// 分隔线。
    Separator,
}

/// 托盘图标，销毁时从系统托盘移除。
pub struct TrayIcon {
    hwnd: HWND,
    tooltip: String,
}

impl TrayIcon {
    /// 添加托盘图标，鼠标事件由 `window` 转成 [`crate::window::WindowEvent::Tray`]。
    pub fn add(window: &MessageWindow, tooltip: &str) -> Result<Self, PlatformError> {
        let tray = Self {
            hwnd: window.hwnd(),
            tooltip: tooltip.to_string(),
        };
        tray.shell_add()?;
        Ok(tray)
    }

    /// 资源管理器重启后重新添加图标。
    pub fn re_add(&self) -> Result<(), PlatformError> {
        self.shell_add()
    }

    fn shell_add(&self) -> Result<(), PlatformError> {
        // 暂用系统默认图标，应用图标在分发阶段嵌入资源后替换
        // SAFETY: 传入 None 加载系统预置图标，返回共享句柄，无需销毁。
        let icon = unsafe { LoadIconW(None, IDI_APPLICATION) }.map_err(win32("LoadIconW"))?;
        let mut nid = self.base_data();
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = icon;
        copy_wide(&mut nid.szTip, &self.tooltip);
        // SAFETY: nid 已按要求填充 cbSize、hWnd 与 uID。
        if !unsafe { Shell_NotifyIconW(NIM_ADD, &nid) }.as_bool() {
            return Err(last_error("Shell_NotifyIconW(NIM_ADD)"));
        }
        Ok(())
    }

    /// 带 cbSize、hWnd、uID 的基础数据，供添加、修改、通知共用。
    pub(crate) fn base_data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: u32::try_from(std::mem::size_of::<NOTIFYICONDATAW>()).unwrap_or(0),
            hWnd: self.hwnd,
            uID: TRAY_ICON_UID,
            ..Default::default()
        }
    }

    /// 在光标处弹出菜单，返回选中项的 `id`；点空白处取消时返回 `None`。
    pub fn show_menu(&self, items: &[MenuItem]) -> Result<Option<u16>, PlatformError> {
        let menu = PopupMenu::create()?;
        for item in items {
            match item {
                MenuItem::Entry { id, label } => {
                    if *id == 0 {
                        return Err(PlatformError::Invalid("菜单项 id 不能为 0".to_string()));
                    }
                    let wide = to_wide(label);
                    // SAFETY: wide 在调用期间有效，菜单由 PopupMenu 持有。
                    unsafe {
                        AppendMenuW(
                            menu.raw(),
                            MF_STRING,
                            usize::from(*id),
                            PCWSTR(wide.as_ptr()),
                        )
                    }
                    .map_err(win32("AppendMenuW"))?;
                }
                MenuItem::Separator => {
                    // SAFETY: 分隔线不需要文字，菜单由 PopupMenu 持有。
                    unsafe { AppendMenuW(menu.raw(), MF_SEPARATOR, 0, PCWSTR::null()) }
                        .map_err(win32("AppendMenuW"))?;
                }
            }
        }
        let mut cursor = POINT::default();
        // SAFETY: cursor 是有效的局部变量。
        unsafe { GetCursorPos(&mut cursor) }.map_err(win32("GetCursorPos"))?;
        // 不置前台的话点击菜单外部不会关闭菜单；失败也只是菜单关闭行为稍差
        // SAFETY: hwnd 是本进程的有效窗口。
        discard(unsafe { SetForegroundWindow(self.hwnd) });
        // SAFETY: 菜单与窗口均有效；TPM_RETURNCMD 下返回值即选中的命令号，0 表示取消或失败。
        let chosen = unsafe {
            TrackPopupMenu(
                menu.raw(),
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
                cursor.x,
                cursor.y,
                None,
                self.hwnd,
                None,
            )
        };
        // 托盘菜单的标准收尾，使菜单能正确关闭；失败无影响
        // SAFETY: hwnd 是本进程的有效窗口。
        discard(unsafe {
            PostMessageW(
                Some(self.hwnd),
                WM_NULL,
                Default::default(),
                Default::default(),
            )
        });
        Ok(u16::try_from(chosen.0).ok().filter(|id| *id != 0))
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        let nid = self.base_data();
        // Drop 中无法上报错误，移除失败时系统会在鼠标划过后清理残留图标
        // SAFETY: nid 已按要求填充 cbSize、hWnd 与 uID。
        discard(unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) });
    }
}
