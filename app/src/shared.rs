//! 托盘分发器与跨进程提交回调共用的状态，以及设置进程提交配置的处理。

use std::cell::RefCell;

use lenswin_core::config::HotkeysConfig;
use lenswin_core::ipc::{IpcResult, decode_apply_config};
use lenswin_platform::window::WindowHandle;

use crate::config_store::{ConfigStore, SyncApplyError};
use crate::hotkeys::{HotkeySet, ReplaceReport};

/// 主线程内共享的状态；回调与分发器各持有一份 `Rc`。
///
/// 所有 `RefCell` 的借用都必须短暂，不得跨越会处理窗口消息的调用，否则回调重入会 panic。
pub struct AppShared {
    /// 配置中心。
    pub store: ConfigStore,
    /// 已注册的热键。
    pub hotkeys: RefCell<HotkeySet>,
    /// 消息窗口。
    pub window: WindowHandle,
}

/// 热键替换结果对应的返回码：只要有这次改动却被占用的键，就是冲突。
fn hotkey_result(report: &ReplaceReport) -> IpcResult {
    if report.reverted.is_empty() {
        IpcResult::Ok
    } else {
        IpcResult::HotkeyConflict
    }
}

/// 处理设置进程提交的配置：校验、写盘、替换热键。
///
/// `replace_hotkeys` 由调用方提供，便于脱离 Win32 测试。被占用的键仍把用户填的值写入文件，
/// 当前继续使用旧热键，通过 [`IpcResult::HotkeyConflict`] 告知设置窗口。
pub fn handle_submission(
    store: &ConfigStore,
    replace_hotkeys: impl FnOnce(&HotkeysConfig) -> ReplaceReport,
    kind: u32,
    payload: &[u8],
) -> IpcResult {
    let config = match decode_apply_config(kind, payload) {
        Ok(config) => config,
        Err(result) => return result,
    };
    let hotkeys = config.hotkeys.clone();
    match store.apply_sync(config) {
        Ok(()) => {}
        Err(SyncApplyError::Invalid(_)) => return IpcResult::ValidationFailed,
        Err(SyncApplyError::WriteFailed(_)) => return IpcResult::WriteFailed,
    }
    hotkey_result(&replace_hotkeys(&hotkeys))
}

impl AppShared {
    /// 供窗口过程回调使用：返回结果码数值。
    pub fn on_submission(&self, kind: u32, payload: &[u8]) -> u32 {
        handle_submission(
            &self.store,
            |hotkeys| self.hotkeys.borrow_mut().replace(self.window, hotkeys),
            kind,
            payload,
        )
        .code()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::Command;
    use crate::hotkeys::Failure;
    use lenswin_core::config::Config;
    use lenswin_core::ipc::{MAX_PAYLOAD_BYTES, MessageKind};
    use std::cell::Cell;
    use std::fs;
    use std::path::{Path, PathBuf};

    const APPLY: u32 = 1;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lenswin-shared-{tag}-{}", std::process::id()));
        match fs::remove_dir_all(&dir) {
            Ok(()) | Err(_) => {}
        }
        dir
    }

    fn store(dir: &Path) -> ConfigStore {
        ConfigStore::open(dir.join("config.toml"), || {})
            .expect("打开")
            .0
    }

    fn clean() -> ReplaceReport {
        ReplaceReport::default()
    }

    fn conflict() -> ReplaceReport {
        ReplaceReport {
            reverted: vec![Failure {
                command: Command::Search,
                text: "Ctrl+9".to_string(),
                reason: "占用".to_string(),
            }],
            still_failing: Vec::new(),
        }
    }

    #[test]
    fn apply_message_kind_matches_wire_value() {
        assert_eq!(MessageKind::ApplyConfig.code(), APPLY);
    }

    #[test]
    fn bad_submissions_never_touch_hotkeys_or_disk() {
        let dir = temp_dir("bad");
        let store = store(&dir);
        let called = Cell::new(false);
        let replace = |_: &HotkeysConfig| {
            called.set(true);
            clean()
        };
        let valid = Config::default().to_toml_string().expect("序列化");
        let mut invalid = Config::default();
        invalid.upload.timeout_secs = 0;
        let invalid_text = invalid.to_toml_string().expect("序列化");
        let oversized = vec![b'#'; MAX_PAYLOAD_BYTES + 1];

        assert_eq!(
            handle_submission(&store, replace, 99, valid.as_bytes()),
            IpcResult::InvalidMessage
        );
        assert_eq!(
            handle_submission(&store, replace, APPLY, &oversized),
            IpcResult::PayloadTooLarge
        );
        assert_eq!(
            handle_submission(&store, replace, APPLY, &[0xFF, 0xFE]),
            IpcResult::ParseFailed
        );
        assert_eq!(
            handle_submission(&store, replace, APPLY, b"not toml ==="),
            IpcResult::ParseFailed
        );
        assert_eq!(
            handle_submission(&store, replace, APPLY, invalid_text.as_bytes()),
            IpcResult::ValidationFailed
        );
        assert!(!called.get());
        assert_eq!(store.snapshot(), Config::default());
        drop(store);
        fs::remove_dir_all(&dir).expect("清理");
    }

    #[test]
    fn good_submission_is_persisted_and_reports_hotkey_state() {
        let dir = temp_dir("good");
        let store = store(&dir);
        let mut next = Config::default();
        next.hotkeys.search = "Ctrl+Alt+F13".to_string();
        let text = next.to_toml_string().expect("序列化");

        let seen = RefCell::new(None);
        let result = handle_submission(
            &store,
            |hotkeys| {
                seen.replace(Some(hotkeys.clone()));
                clean()
            },
            APPLY,
            text.as_bytes(),
        );
        assert_eq!(result, IpcResult::Ok);
        assert_eq!(seen.into_inner(), Some(next.hotkeys.clone()));
        assert_eq!(store.snapshot(), next);

        // 冲突时配置仍已写入，只是结果码提示热键未生效
        let result = handle_submission(&store, |_| conflict(), APPLY, text.as_bytes());
        assert_eq!(result, IpcResult::HotkeyConflict);
        assert_eq!(store.snapshot(), next);
        drop(store);
        let saved = fs::read_to_string(dir.join("config.toml")).expect("读取");
        assert_eq!(Config::from_toml_str(&saved).expect("解析"), next);
        fs::remove_dir_all(&dir).expect("清理");
    }

    #[test]
    fn still_failing_keys_alone_are_not_a_conflict() {
        let report = ReplaceReport {
            reverted: Vec::new(),
            still_failing: conflict().reverted,
        };
        assert_eq!(hotkey_result(&report), IpcResult::Ok);
        assert_eq!(hotkey_result(&conflict()), IpcResult::HotkeyConflict);
    }
}
