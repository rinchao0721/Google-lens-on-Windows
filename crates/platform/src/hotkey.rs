//! 全局快捷键注册。

use lenswin_core::hotkey::{Hotkey, Modifiers};
use windows::Win32::Foundation::{ERROR_HOTKEY_ALREADY_REGISTERED, HWND};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey,
};
use windows::core::HRESULT;

use crate::error::PlatformError;
use crate::handle::discard;
use crate::window::MessageWindow;

/// 已注册的热键，销毁时注销。
pub struct HotkeyGuard {
    hwnd: HWND,
    id: i32,
}

/// 核心层修饰键转 Win32 标志，并加上 `MOD_NOREPEAT` 避免按住时连发。
fn modifier_flags(modifiers: Modifiers) -> HOT_KEY_MODIFIERS {
    let table = [
        (Modifiers::CTRL, MOD_CONTROL),
        (Modifiers::ALT, MOD_ALT),
        (Modifiers::SHIFT, MOD_SHIFT),
        (Modifiers::WIN, MOD_WIN),
    ];
    table
        .into_iter()
        .filter(|(m, _)| modifiers.contains(*m))
        .fold(MOD_NOREPEAT, |acc, (_, flag)| acc | flag)
}

/// 在消息窗口上注册热键；被其它程序占用时返回 [`PlatformError::HotkeyConflict`]。
pub fn register(
    window: &MessageWindow,
    id: i32,
    hotkey: &Hotkey,
) -> Result<HotkeyGuard, PlatformError> {
    let vk = hotkey
        .key()
        .vk_code()
        .ok_or_else(|| PlatformError::Invalid(format!("快捷键 {hotkey} 的主键不合法")))?;
    let hwnd = window.hwnd();
    // SAFETY: hwnd 是本线程创建的有效窗口，参数均已校验。
    let result = unsafe {
        RegisterHotKey(
            Some(hwnd),
            id,
            modifier_flags(hotkey.modifiers()),
            u32::from(vk),
        )
    };
    match result {
        Ok(()) => Ok(HotkeyGuard { hwnd, id }),
        Err(source) if source.code() == HRESULT::from_win32(ERROR_HOTKEY_ALREADY_REGISTERED.0) => {
            Err(PlatformError::HotkeyConflict(hotkey.to_string()))
        }
        Err(source) => Err(PlatformError::Win32 {
            api: "RegisterHotKey",
            source,
        }),
    }
}

impl Drop for HotkeyGuard {
    fn drop(&mut self) {
        // Drop 中无法上报错误，注销失败时热键会随窗口销毁一并释放
        // SAFETY: 热键由 register 在同一窗口上注册，仅在此处注销一次。
        discard(unsafe { UnregisterHotKey(Some(self.hwnd), self.id) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_follow_modifiers_and_always_no_repeat() {
        let m = Modifiers::WIN.union(Modifiers::SHIFT);
        let flags = modifier_flags(m);
        assert_eq!(flags, MOD_WIN | MOD_SHIFT | MOD_NOREPEAT);
        assert_eq!(modifier_flags(Modifiers::CTRL), MOD_CONTROL | MOD_NOREPEAT);
        assert_eq!(modifier_flags(Modifiers::NONE), MOD_NOREPEAT);
    }
}
