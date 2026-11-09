//! 具体功能：搜索、复制、保存，由注册表统一调度。

#![forbid(unsafe_code)]

mod copy;
mod encode;
mod save;
mod search;

use std::sync::Arc;

use lenswin_core::action::{Action, ActionKind};
use lenswin_platform::clipboard::ClipboardOwner;

pub use copy::CopyAction;
pub use encode::encode_png;
pub use save::SaveAction;
pub use search::SearchAction;

/// 动作注册表：新增功能只需在 [`ActionRegistry::new`] 登记一行。
pub struct ActionRegistry {
    actions: Vec<Arc<dyn Action>>,
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
        }
    }

    /// 取出指定种类的动作。
    pub fn get(&self, kind: ActionKind) -> Option<Arc<dyn Action>> {
        self.actions.iter().find(|a| a.kind() == kind).cloned()
    }
}
