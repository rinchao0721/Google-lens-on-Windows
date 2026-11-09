//! 托盘进程与设置进程之间的消息约定，两个进程共用。

use crate::config::Config;
use crate::error::ConfigError;

/// 托盘进程隐藏消息窗口的窗口类名。
pub const TRAY_WINDOW_CLASS: &str = "LenswinTrayWindow";
/// 托盘进程单实例互斥量名称。
pub const TRAY_MUTEX_NAME: &str = "Local\\lenswin";
/// 设置进程单实例互斥量名称。
pub const SETTINGS_MUTEX_NAME: &str = "Local\\lenswin-settings";
/// 第二个实例通知首个实例用的窗口消息，实际消息号为 `WM_APP + 该偏移`。
pub const SECOND_INSTANCE_MESSAGE_OFFSET: u32 = 2;
/// 单条消息负载上限（1 MiB）。
pub const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

/// `WM_COPYDATA` 的 `dwData` 消息类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// 提交新配置，负载为 UTF-8 的 TOML 文本。
    ApplyConfig,
}

impl MessageKind {
    /// 线上传输的数值。
    pub const fn code(self) -> u32 {
        match self {
            MessageKind::ApplyConfig => 1,
        }
    }

    /// 由数值还原，未知值返回 `None`。
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            1 => Some(MessageKind::ApplyConfig),
            _ => None,
        }
    }
}

/// 托盘进程返回给设置进程的结果码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcResult {
    /// 成功。
    Ok,
    /// 消息类型未知。
    InvalidMessage,
    /// 负载超过上限。
    PayloadTooLarge,
    /// 负载不是合法的 UTF-8 TOML，或含有未知字段。
    ParseFailed,
    /// 配置校验未通过。
    ValidationFailed,
    /// 快捷键被其它程序占用，已回滚。
    HotkeyConflict,
    /// 写盘失败。
    WriteFailed,
}

impl IpcResult {
    /// 线上传输的数值。
    pub const fn code(self) -> u32 {
        match self {
            IpcResult::Ok => 0,
            IpcResult::InvalidMessage => 1,
            IpcResult::PayloadTooLarge => 2,
            IpcResult::ParseFailed => 3,
            IpcResult::ValidationFailed => 4,
            IpcResult::HotkeyConflict => 5,
            IpcResult::WriteFailed => 6,
        }
    }

    /// 由数值还原，未知值返回 `None`。
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(IpcResult::Ok),
            1 => Some(IpcResult::InvalidMessage),
            2 => Some(IpcResult::PayloadTooLarge),
            3 => Some(IpcResult::ParseFailed),
            4 => Some(IpcResult::ValidationFailed),
            5 => Some(IpcResult::HotkeyConflict),
            6 => Some(IpcResult::WriteFailed),
            _ => None,
        }
    }
}

/// 解码并校验一条「提交配置」消息；任何失败都映射为结果码，不 panic。
pub fn decode_apply_config(kind_code: u32, payload: &[u8]) -> Result<Config, IpcResult> {
    match MessageKind::from_code(kind_code) {
        Some(MessageKind::ApplyConfig) => {}
        None => return Err(IpcResult::InvalidMessage),
    }
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(IpcResult::PayloadTooLarge);
    }
    let text = std::str::from_utf8(payload).map_err(|_| IpcResult::ParseFailed)?;
    Config::from_toml_str(text).map_err(|e| match e {
        ConfigError::Invalid(_) => IpcResult::ValidationFailed,
        ConfigError::Parse(_) | ConfigError::Serialize(_) => IpcResult::ParseFailed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPLY: u32 = 1;

    #[test]
    fn codes_roundtrip() {
        for r in [
            IpcResult::Ok,
            IpcResult::InvalidMessage,
            IpcResult::PayloadTooLarge,
            IpcResult::ParseFailed,
            IpcResult::ValidationFailed,
            IpcResult::HotkeyConflict,
            IpcResult::WriteFailed,
        ] {
            assert_eq!(IpcResult::from_code(r.code()), Some(r));
        }
        assert_eq!(IpcResult::from_code(99), None);
        assert_eq!(MessageKind::from_code(0), None);
        assert_eq!(
            MessageKind::from_code(MessageKind::ApplyConfig.code()),
            Some(MessageKind::ApplyConfig)
        );
    }

    #[test]
    fn decodes_valid_config() {
        let text = Config::default().to_toml_string().expect("序列化");
        assert_eq!(
            decode_apply_config(APPLY, text.as_bytes()),
            Ok(Config::default())
        );
    }

    #[test]
    fn rejects_unknown_message_kind() {
        assert_eq!(decode_apply_config(7, b""), Err(IpcResult::InvalidMessage));
    }

    #[test]
    fn rejects_oversized_payload() {
        let big = vec![b' '; MAX_PAYLOAD_BYTES + 1];
        assert_eq!(
            decode_apply_config(APPLY, &big),
            Err(IpcResult::PayloadTooLarge)
        );
    }

    #[test]
    fn accepts_payload_at_limit_boundary() {
        let at_limit = vec![b' '; MAX_PAYLOAD_BYTES];
        assert_eq!(decode_apply_config(APPLY, &at_limit), Ok(Config::default()));
    }

    #[test]
    fn rejects_invalid_utf8_and_bad_toml() {
        assert_eq!(
            decode_apply_config(APPLY, &[0xFF, 0xFE]),
            Err(IpcResult::ParseFailed)
        );
        assert_eq!(
            decode_apply_config(APPLY, b"this is = = not toml"),
            Err(IpcResult::ParseFailed)
        );
        assert_eq!(
            decode_apply_config(APPLY, b"[bogus]\nx = 1\n"),
            Err(IpcResult::ParseFailed)
        );
    }

    #[test]
    fn rejects_semantically_invalid_config() {
        assert_eq!(
            decode_apply_config(APPLY, b"[upload]\ntimeout_secs = 0\n"),
            Err(IpcResult::ValidationFailed)
        );
    }
}
