//! 向托盘进程提交配置、探测快捷键。

use lenswin_core::config::Config;
use lenswin_core::ipc::{IpcResult, MessageKind, TRAY_WINDOW_CLASS};
use lenswin_platform::ipc::send;

use crate::form::{Feedback, FeedbackKind, ProbeState, describe_result};

/// 同步等待托盘进程处理的上限，需大于托盘侧的写盘等待。
const SEND_TIMEOUT_MS: u32 = 5000;

/// 把结果码数值转成提示；数值未知时按失败处理。
pub fn feedback_for_code(code: u32) -> Feedback {
    match IpcResult::from_code(code) {
        Some(result) => describe_result(result),
        None => Feedback::new(
            FeedbackKind::Error,
            format!("主程序返回了未知结果码 {code}"),
        ),
    }
}

/// 序列化并提交配置，返回给用户看的提示。
pub fn submit(config: &Config) -> Feedback {
    let text = match config.to_toml_string() {
        Ok(text) => text,
        Err(e) => return Feedback::new(FeedbackKind::Error, format!("配置序列化失败：{e}")),
    };
    match send(
        TRAY_WINDOW_CLASS,
        MessageKind::ApplyConfig.code(),
        text.as_bytes(),
        SEND_TIMEOUT_MS,
    ) {
        Ok(code) => feedback_for_code(code),
        Err(e) => Feedback::new(FeedbackKind::Error, e.to_string()),
    }
}

/// 探测要比提交短得多，界面线程同步等待，不能卡太久。
const PROBE_TIMEOUT_MS: u32 = 800;

/// 把探测返回的结果码数值转成结论。
pub fn probe_state_for_code(code: u32) -> ProbeState {
    match IpcResult::from_code(code) {
        Some(IpcResult::Ok) => ProbeState::Available,
        Some(IpcResult::HotkeyConflict) => ProbeState::Occupied,
        Some(IpcResult::Busy) => ProbeState::Unavailable("主程序正忙".to_string()),
        Some(other) => ProbeState::Unavailable(describe_result(other).text),
        None => ProbeState::Unavailable(format!("主程序返回了未知结果码 {code}")),
    }
}

/// 请托盘进程探测快捷键文字 `text` 是否可用。
pub fn probe(text: &str) -> ProbeState {
    match send(
        TRAY_WINDOW_CLASS,
        MessageKind::ProbeHotkey.code(),
        text.as_bytes(),
        PROBE_TIMEOUT_MS,
    ) {
        Ok(code) => probe_state_for_code(code),
        Err(e) => ProbeState::Unavailable(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_codes_map_to_states() {
        assert_eq!(probe_state_for_code(0), ProbeState::Available);
        assert_eq!(probe_state_for_code(5), ProbeState::Occupied);
        assert!(matches!(
            probe_state_for_code(7),
            ProbeState::Unavailable(_)
        ));
        assert!(matches!(
            probe_state_for_code(1),
            ProbeState::Unavailable(_)
        ));
        match probe_state_for_code(999) {
            ProbeState::Unavailable(reason) => assert!(reason.contains("999")),
            other => panic!("应为无法检测：{other:?}"),
        }
    }

    #[test]
    fn known_and_unknown_codes_map_to_feedback() {
        assert_eq!(feedback_for_code(0).kind, FeedbackKind::Success);
        assert_eq!(feedback_for_code(5).kind, FeedbackKind::Warning);
        let unknown = feedback_for_code(999);
        assert_eq!(unknown.kind, FeedbackKind::Error);
        assert!(unknown.text.contains("999"));
    }
}
