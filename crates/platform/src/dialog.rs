//! 模态确认对话框。

use windows::Win32::Foundation::{HWND, LPARAM, S_OK, WPARAM};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Controls::{
    TASKDIALOG_BUTTON, TASKDIALOG_NOTIFICATIONS, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0,
    TD_INFORMATION_ICON, TDF_ALLOW_DIALOG_CANCELLATION, TDN_CREATED,
};
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_TOPMOST, IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MB_TOPMOST,
    MB_YESNO, MessageBoxW, SWP_NOMOVE, SWP_NOSIZE, SetForegroundWindow, SetWindowPos,
};
use windows::core::{BOOL, HRESULT, PCWSTR, s, w};

use crate::error::{PlatformError, last_error, win32};
use crate::handle::discard;
use crate::text::to_wide;

/// 弹出「是 / 否」确认框，选「是」返回 `true`。
///
/// 对话框置顶并抢前台，调用线程在用户作答前阻塞。
pub fn confirm(title: &str, text: &str) -> Result<bool, PlatformError> {
    let title_wide = to_wide(title);
    let text_wide = to_wide(text);
    // SAFETY: 两个字符串都以 NUL 结尾且在调用期间有效，无所有者窗口。
    let result = unsafe {
        MessageBoxW(
            None,
            PCWSTR(text_wide.as_ptr()),
            PCWSTR(title_wide.as_ptr()),
            MB_YESNO | MB_ICONINFORMATION | MB_TOPMOST | MB_SETFOREGROUND,
        )
    };
    if result.0 == 0 {
        return Err(last_error("MessageBoxW"));
    }
    Ok(result == IDYES)
}

/// 弹出只有「确定」的错误提示框，供没有日志和控制台的进程报告致命错误。
pub fn error(title: &str, text: &str) -> Result<(), PlatformError> {
    let title_wide = to_wide(title);
    let text_wide = to_wide(text);
    // SAFETY: 两个字符串都以 NUL 结尾且在调用期间有效，无所有者窗口。
    let result = unsafe {
        MessageBoxW(
            None,
            PCWSTR(text_wide.as_ptr()),
            PCWSTR(title_wide.as_ptr()),
            MB_OK | MB_ICONERROR | MB_TOPMOST | MB_SETFOREGROUND,
        )
    };
    if result.0 == 0 {
        return Err(last_error("MessageBoxW"));
    }
    Ok(())
}

/// 任务对话框里「继续」按钮的编号。
const BUTTON_YES: i32 = 1001;
/// 任务对话框里「取消」按钮的编号。
const BUTTON_NO: i32 = 1002;

/// 点中的按钮是否是肯定项。
fn is_yes(button: i32) -> bool {
    button == BUTTON_YES
}

/// `TaskDialogIndirect` 的函数签名。
type TaskDialogIndirectFn =
    unsafe extern "system" fn(*const TASKDIALOGCONFIG, *mut i32, *mut i32, *mut BOOL) -> HRESULT;

/// 对话框创建后置顶并抢前台，行为与 `MessageBoxW` 的 `MB_TOPMOST` 一致。
unsafe extern "system" fn on_task_dialog_event(
    hwnd: HWND,
    msg: TASKDIALOG_NOTIFICATIONS,
    _wparam: WPARAM,
    _lparam: LPARAM,
    _data: isize,
) -> HRESULT {
    if msg == TDN_CREATED {
        // 置顶失败只是对话框可能被其它窗口盖住，不影响使用
        // SAFETY: hwnd 是系统传入的有效对话框窗口。
        discard(unsafe {
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE,
            )
        });
        // SAFETY: 同上。
        discard(unsafe { SetForegroundWindow(hwnd) });
    }
    S_OK
}

/// 取 `TaskDialogIndirect`；进程没有声明 Common Controls v6 时拿不到，返回 `None`。
fn task_dialog_fn() -> Option<TaskDialogIndirectFn> {
    // SAFETY: 参数为静态字符串；comctl32 随系统提供，加载后不卸载。
    let module = unsafe { LoadLibraryW(w!("comctl32.dll")) }.ok()?;
    // SAFETY: module 有效，函数名为静态字符串。
    let proc = unsafe { GetProcAddress(module, s!("TaskDialogIndirect")) }?;
    // SAFETY: TaskDialogIndirect 的真实签名与 TaskDialogIndirectFn 一致。
    Some(unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, TaskDialogIndirectFn>(proc)
    })
}

/// 弹出带「继续 / 取消」自定义按钮的确认框，点「继续」返回 `true`；关闭或取消返回 `false`。
///
/// 使用 `TaskDialogIndirect`，需要 exe 声明 Common Controls v6；缺失时（如单元测试进程）
/// 退回 [`confirm`] 的「是 / 否」框，文字合并为一段。
pub fn ask(
    title: &str,
    heading: &str,
    body: &str,
    yes_label: &str,
    no_label: &str,
) -> Result<bool, PlatformError> {
    let Some(task_dialog) = task_dialog_fn() else {
        return confirm(title, &format!("{heading}\n\n{body}"));
    };
    let title_wide = to_wide(title);
    let heading_wide = to_wide(heading);
    let body_wide = to_wide(body);
    let yes_wide = to_wide(yes_label);
    let no_wide = to_wide(no_label);
    let buttons = [
        TASKDIALOG_BUTTON {
            nButtonID: BUTTON_YES,
            pszButtonText: PCWSTR(yes_wide.as_ptr()),
        },
        TASKDIALOG_BUTTON {
            nButtonID: BUTTON_NO,
            pszButtonText: PCWSTR(no_wide.as_ptr()),
        },
    ];
    let config = TASKDIALOGCONFIG {
        cbSize: u32::try_from(std::mem::size_of::<TASKDIALOGCONFIG>()).unwrap_or(0),
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION,
        pszWindowTitle: PCWSTR(title_wide.as_ptr()),
        pszMainInstruction: PCWSTR(heading_wide.as_ptr()),
        pszContent: PCWSTR(body_wide.as_ptr()),
        cButtons: 2,
        pButtons: buttons.as_ptr(),
        nDefaultButton: BUTTON_NO,
        pfCallback: Some(on_task_dialog_event),
        Anonymous1: TASKDIALOGCONFIG_0 {
            pszMainIcon: TD_INFORMATION_ICON,
        },
        ..Default::default()
    };
    let mut pressed = 0i32;
    // SAFETY: config 及其引用的字符串、按钮数组在调用期间有效；输出参数指向有效局部变量，
    // 不需要的两个输出传空指针。
    unsafe {
        task_dialog(
            &config,
            &mut pressed,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    }
    .ok()
    .map_err(win32("TaskDialogIndirect"))?;
    Ok(is_yes(pressed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_yes_button_confirms() {
        assert!(is_yes(BUTTON_YES));
        assert!(!is_yes(BUTTON_NO));
        // IDCANCEL，窗口被关闭时系统返回
        assert!(!is_yes(2));
        assert!(!is_yes(0));
    }
}
