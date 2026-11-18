//! 贴图右键菜单：由 `Pin` 内部创建并用 RAII 管理 `HMENU`。

use lenswin_core::geometry::Point;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, HMENU, MF_CHECKED, MF_SEPARATOR, MF_STRING,
    MF_UNCHECKED, SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenu,
};
use windows::core::w;

use crate::error::{PinError, last_error, win32};

const CMD_COPY: usize = 1;
const CMD_SAVE: usize = 2;
const CMD_SEARCH: usize = 3;
const CMD_OCR: usize = 4;
const CMD_PASSTHROUGH: usize = 5;
const CMD_CLOSE: usize = 6;

/// 用户在贴图右键菜单中选中的项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuAction {
    Copy,
    Save,
    Search,
    Ocr,
    TogglePassthrough,
    Close,
}

/// 弹出式菜单的 RAII 包装，离开作用域时自动销毁。
struct PopupMenu(HMENU);

impl PopupMenu {
    fn create() -> Result<Self, PinError> {
        // SAFETY: 无前置条件，失败时由 map_err 转出。
        let menu = unsafe { CreatePopupMenu() }.map_err(win32("CreatePopupMenu"))?;
        Ok(Self(menu))
    }

    fn raw(&self) -> HMENU {
        self.0
    }
}

impl Drop for PopupMenu {
    fn drop(&mut self) {
        // Drop 中无法上报错误，销毁失败只会泄漏一个菜单句柄
        // SAFETY: 句柄由 CreatePopupMenu 创建，仅在此处释放一次。
        crate::discard(unsafe { DestroyMenu(self.0) });
    }
}

/// 在指定屏幕坐标弹出贴图右键菜单。
pub(crate) fn show_context_menu(
    hwnd: HWND,
    screen_pt: Point,
    passthrough: bool,
) -> Result<Option<MenuAction>, PinError> {
    let menu = PopupMenu::create()?;
    let pass_flags = MF_STRING
        | if passthrough {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };

    // SAFETY: menu.raw() 为有效菜单句柄，字符串均为静态宽字面量。
    unsafe {
        AppendMenuW(menu.raw(), MF_STRING, CMD_COPY, w!("复制")).map_err(win32("AppendMenuW"))?;
        AppendMenuW(menu.raw(), MF_STRING, CMD_SAVE, w!("保存")).map_err(win32("AppendMenuW"))?;
        AppendMenuW(menu.raw(), MF_STRING, CMD_SEARCH, w!("搜索")).map_err(win32("AppendMenuW"))?;
        AppendMenuW(menu.raw(), MF_STRING, CMD_OCR, w!("识别文字"))
            .map_err(win32("AppendMenuW"))?;
        AppendMenuW(menu.raw(), MF_SEPARATOR, 0, None).map_err(win32("AppendMenuW"))?;
        AppendMenuW(menu.raw(), pass_flags, CMD_PASSTHROUGH, w!("鼠标穿透"))
            .map_err(win32("AppendMenuW"))?;
        AppendMenuW(menu.raw(), MF_SEPARATOR, 0, None).map_err(win32("AppendMenuW"))?;
        AppendMenuW(menu.raw(), MF_STRING, CMD_CLOSE, w!("关闭")).map_err(win32("AppendMenuW"))?;
    }

    // TrackPopupMenu 前把宿主窗提前台，点击菜单外部时系统才能正常收起菜单
    // SAFETY: hwnd 为当前线程有效窗口，失败不阻止继续弹出。
    crate::discard(unsafe { SetForegroundWindow(hwnd) });

    // SAFETY: menu.raw() 与 hwnd 均有效，flags 含 TPM_RETURNCMD 直接返回命令 ID。
    let selected = unsafe {
        TrackPopupMenu(
            menu.raw(),
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            screen_pt.x,
            screen_pt.y,
            None,
            hwnd,
            None,
        )
    };
    if selected.0 == 0 {
        let err = last_error("TrackPopupMenu");
        // 用户点在菜单外部取消时返回 0 且线程错误码为 0
        if let PinError::Win32 { source, .. } = &err
            && source.code().0 != 0
        {
            return Err(err);
        }
        return Ok(None);
    }

    let action = usize::try_from(selected.0).ok().and_then(|id| match id {
        CMD_COPY => Some(MenuAction::Copy),
        CMD_SAVE => Some(MenuAction::Save),
        CMD_SEARCH => Some(MenuAction::Search),
        CMD_OCR => Some(MenuAction::Ocr),
        CMD_PASSTHROUGH => Some(MenuAction::TogglePassthrough),
        CMD_CLOSE => Some(MenuAction::Close),
        _ => None,
    });
    Ok(action)
}
