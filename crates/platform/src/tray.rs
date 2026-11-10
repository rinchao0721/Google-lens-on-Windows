//! 托盘图标与右键菜单。

use windows::Win32::Foundation::{GetLastError, HINSTANCE, HWND, POINT, SetLastError, WIN32_ERROR};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, GetCursorPos, GetSystemMetrics, HICON, IDI_APPLICATION, IMAGE_ICON, LR_SHARED,
    LoadIconW, LoadImageW, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, PostMessageW,
    SM_CXSMICON, SM_CYSMICON, SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenu, WM_NULL,
};
use windows::core::PCWSTR;

use crate::error::{PlatformError, last_error, win32};
use crate::handle::{PopupMenu, discard};
use crate::text::{copy_wide, to_wide};
use crate::window::{MessageWindow, WM_TRAY};

/// 托盘图标在窗口内的标识。
const TRAY_ICON_UID: u32 = 1;

/// 子菜单最多嵌套的层数。
const MAX_MENU_DEPTH: usize = 4;

/// 托盘菜单项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    /// 可点击项，`id` 必须大于 0 且在整棵菜单中唯一。
    Entry {
        /// 选中后返回的命令号。
        id: u16,
        /// 显示文字。
        label: String,
        /// 是否带勾选标记。
        checked: bool,
        /// 是否可点击；不可点击时灰显。
        enabled: bool,
    },
    /// 子菜单。
    Submenu {
        /// 显示文字。
        label: String,
        /// 子项。
        items: Vec<MenuItem>,
    },
    /// 分隔线。
    Separator,
}

impl MenuItem {
    /// 普通可点击项，未勾选。
    pub fn entry(id: u16, label: impl Into<String>) -> Self {
        MenuItem::Entry {
            id,
            label: label.into(),
            checked: false,
            enabled: true,
        }
    }
}

/// exe 内嵌图标的资源编号，与 `app/lenswin.rc` 一致。
const APP_ICON_RESOURCE_ID: usize = 1;

/// 加载 exe 内嵌的托盘尺寸图标；没有内嵌资源（如测试进程）时退回系统默认图标。
fn load_app_icon() -> Result<HICON, PlatformError> {
    // SAFETY: 传入 None 取当前模块句柄。
    let module = unsafe { GetModuleHandleW(None) }.map_err(win32("GetModuleHandleW"))?;
    // SAFETY: 无前置条件。
    let (width, height) = unsafe { (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON)) };
    // SAFETY: 资源编号用 MAKEINTRESOURCE 约定编码为指针值；LR_SHARED 返回的共享句柄无需销毁。
    let loaded = unsafe {
        LoadImageW(
            Some(HINSTANCE(module.0)),
            PCWSTR(std::ptr::without_provenance::<u16>(APP_ICON_RESOURCE_ID)),
            IMAGE_ICON,
            width,
            height,
            LR_SHARED,
        )
    };
    match loaded {
        Ok(handle) => Ok(HICON(handle.0)),
        // SAFETY: 传入 None 加载系统预置图标，返回共享句柄，无需销毁。
        Err(_) => unsafe { LoadIconW(None, IDI_APPLICATION) }.map_err(win32("LoadIconW")),
    }
}

/// 校验菜单：命令号必须非 0 且全局唯一，嵌套不能过深。
fn check_items(items: &[MenuItem], seen: &mut Vec<u16>, depth: usize) -> Result<(), PlatformError> {
    if depth > MAX_MENU_DEPTH {
        return Err(PlatformError::Invalid("菜单嵌套过深".to_string()));
    }
    for item in items {
        match item {
            MenuItem::Entry { id, .. } => {
                if *id == 0 {
                    return Err(PlatformError::Invalid("菜单项 id 不能为 0".to_string()));
                }
                if seen.contains(id) {
                    return Err(PlatformError::Invalid(format!("菜单项 id {id} 重复")));
                }
                seen.push(*id);
            }
            MenuItem::Submenu { items, .. } => check_items(items, seen, depth + 1)?,
            MenuItem::Separator => {}
        }
    }
    Ok(())
}

/// 把菜单项逐个加入 `menu`，子菜单递归创建。
fn append_items(menu: &PopupMenu, items: &[MenuItem]) -> Result<(), PlatformError> {
    for item in items {
        match item {
            MenuItem::Entry {
                id,
                label,
                checked,
                enabled,
            } => {
                let mut flags = MF_STRING;
                if *checked {
                    flags |= MF_CHECKED;
                }
                if !*enabled {
                    flags |= MF_GRAYED;
                }
                let wide = to_wide(label);
                // SAFETY: wide 在调用期间有效，菜单由 PopupMenu 持有。
                unsafe { AppendMenuW(menu.raw(), flags, usize::from(*id), PCWSTR(wide.as_ptr())) }
                    .map_err(win32("AppendMenuW"))?;
            }
            MenuItem::Submenu { label, items } => {
                let child = PopupMenu::create()?;
                append_items(&child, items)?;
                let wide = to_wide(label);
                // SAFETY: MF_POPUP 下第三个参数是子菜单句柄，child 在此之前保持有效。
                unsafe {
                    AppendMenuW(
                        menu.raw(),
                        MF_POPUP | MF_STRING,
                        child.raw().0.addr(),
                        PCWSTR(wide.as_ptr()),
                    )
                }
                .map_err(win32("AppendMenuW"))?;
                // 成功后子菜单归父菜单所有，随父菜单销毁
                child.into_parent_owned();
            }
            MenuItem::Separator => {
                // SAFETY: 分隔线不需要文字，菜单由 PopupMenu 持有。
                unsafe { AppendMenuW(menu.raw(), MF_SEPARATOR, 0, PCWSTR::null()) }
                    .map_err(win32("AppendMenuW"))?;
            }
        }
    }
    Ok(())
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
        let icon = load_app_icon()?;
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
        check_items(items, &mut Vec::new(), 0)?;
        let menu = PopupMenu::create()?;
        append_items(&menu, items)?;
        let mut cursor = POINT::default();
        // SAFETY: cursor 是有效的局部变量。
        unsafe { GetCursorPos(&mut cursor) }.map_err(win32("GetCursorPos"))?;
        // 不置前台的话点击菜单外部不会关闭菜单；失败也只是菜单关闭行为稍差
        // SAFETY: hwnd 是本进程的有效窗口。
        discard(unsafe { SetForegroundWindow(self.hwnd) });
        // 取消与失败都返回 0，只能靠错误码区分，先清零
        // SAFETY: 无前置条件。
        unsafe { SetLastError(WIN32_ERROR(0)) };
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
        if chosen.0 == 0 {
            // SAFETY: 无前置条件，紧接 TrackPopupMenu 读取其留下的错误码。
            let code = unsafe { GetLastError() };
            if code.0 != 0 {
                log::warn!("TrackPopupMenu 未能弹出菜单，错误码 {}", code.0);
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(items: Vec<MenuItem>) -> MenuItem {
        MenuItem::Submenu {
            label: "s".to_string(),
            items,
        }
    }

    #[test]
    fn valid_nested_menu_passes() {
        let items = vec![
            MenuItem::entry(1, "a"),
            MenuItem::Separator,
            sub(vec![MenuItem::entry(2, "b"), MenuItem::entry(3, "c")]),
        ];
        assert!(check_items(&items, &mut Vec::new(), 0).is_ok());
    }

    #[test]
    fn zero_and_duplicate_ids_are_rejected() {
        let zero = vec![MenuItem::entry(0, "a")];
        assert!(check_items(&zero, &mut Vec::new(), 0).is_err());
        let dup = vec![MenuItem::entry(1, "a"), sub(vec![MenuItem::entry(1, "b")])];
        assert!(check_items(&dup, &mut Vec::new(), 0).is_err());
    }

    #[test]
    fn too_deep_nesting_is_rejected() {
        let mut item = MenuItem::entry(1, "leaf");
        for _ in 0..=MAX_MENU_DEPTH {
            item = sub(vec![item]);
        }
        assert!(check_items(&[item], &mut Vec::new(), 0).is_err());
    }
}
