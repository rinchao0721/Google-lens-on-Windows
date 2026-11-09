//! 复制图像到剪贴板。

use lenswin_core::action::{
    Action, ActionContext, ActionInput, ActionKind, ActionOutcome, InputKind,
};
use lenswin_core::error::ActionError;
use lenswin_core::selection::SelectionMode;
use lenswin_platform::clipboard::{self, ClipboardOwner};

/// 复制动作。
#[derive(Debug, Clone, Copy)]
pub struct CopyAction {
    owner: ClipboardOwner,
}

impl CopyAction {
    /// 创建动作，`owner` 为剪贴板所有者窗口。
    pub fn new(owner: ClipboardOwner) -> Self {
        Self { owner }
    }
}

impl Action for CopyAction {
    fn kind(&self) -> ActionKind {
        ActionKind::Copy
    }

    fn input_kind(&self) -> InputKind {
        InputKind::Image
    }

    fn preferred_mode(&self) -> SelectionMode {
        SelectionMode::Rect
    }

    fn run(&self, _ctx: &ActionContext, input: ActionInput) -> Result<ActionOutcome, ActionError> {
        let ActionInput::Image(image) = input else {
            return Err(ActionError::InvalidInput);
        };
        clipboard::set_image(self.owner, &image)
            .map_err(|e| ActionError::Failed(format!("复制到剪贴板失败：{e}")))?;
        Ok(ActionOutcome::Done)
    }
}
