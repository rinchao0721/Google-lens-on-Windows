//! 具体功能：搜索、复制、保存，由注册表统一调度。

#![forbid(unsafe_code)]

mod copy;
mod encode;
mod lens;
mod save;
mod search;

use std::sync::Arc;

use lenswin_core::action::{Action, ActionKind};
use lenswin_core::error::ActionError;
use lenswin_platform::clipboard::{self, ClipboardOwner};

pub use copy::CopyAction;
pub use encode::encode_png;
pub use lens::prepare_lens_request;
pub use save::{SaveAction, default_file_name, save_to};
pub use search::SearchAction;

/// 动作注册表：新增功能只需在 [`ActionRegistry::new`] 登记一行。
pub struct ActionRegistry {
    actions: Vec<Arc<dyn Action>>,
    clipboard_owner: ClipboardOwner,
}

impl ActionRegistry {
    /// 登记全部内置动作；`clipboard_owner` 为复制动作使用的剪贴板所有者。
    pub fn new(clipboard_owner: ClipboardOwner) -> Self {
        Self {
            actions: vec![
                Arc::new(SearchAction),
                Arc::new(CopyAction::new(clipboard_owner)),
                Arc::new(SaveAction),
            ],
            clipboard_owner,
        }
    }

    /// 把文本（如取色结果）复制到剪贴板。
    pub fn copy_text(&self, text: &str) -> Result<(), ActionError> {
        clipboard::set_text(self.clipboard_owner, text)
            .map_err(|e| ActionError::Failed(format!("复制到剪贴板失败：{e}")))
    }

    /// 取出指定种类的动作。
    pub fn get(&self, kind: ActionKind) -> Option<Arc<dyn Action>> {
        self.actions.iter().find(|a| a.kind() == kind).cloned()
    }
}
