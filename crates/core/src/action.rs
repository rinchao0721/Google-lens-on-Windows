//! 动作抽象：动作种类、输入、结果与 `Action` trait。

use crate::config::Config;
use crate::error::ActionError;
use crate::frame::CroppedImage;
use crate::selection::{Selection, SelectionMode};

/// 动作种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionKind {
    /// 以图搜图。
    Search,
    /// 复制到剪贴板。
    Copy,
    /// 保存为文件。
    Save,
    /// 文字识别。
    Ocr,
    /// 录屏。
    Record,
    /// 带声音录屏。
    RecordWithAudio,
}

/// 动作需要的输入类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// 裁剪后的图像，截屏类动作使用。
    Image,
    /// 选区本身，录屏类动作使用。
    Region,
}

/// 动作的实际输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionInput {
    /// 裁剪后的图像。
    Image(CroppedImage),
    /// 选区。
    Region(Selection),
}

impl ActionInput {
    /// 输入对应的类型。
    pub fn kind(&self) -> InputKind {
        match self {
            ActionInput::Image(_) => InputKind::Image,
            ActionInput::Region(_) => InputKind::Region,
        }
    }
}

/// 长时间运行的会话，如录屏。
pub trait RunningSession: Send {
    /// 停止并收尾。
    fn stop(self: Box<Self>) -> Result<(), ActionError>;
}

/// 动作执行结果。
pub enum ActionOutcome {
    /// 已完成。
    Done,
    /// 仍在运行，需要调用方保管并在适当时机停止。
    Session(Box<dyn RunningSession>),
}

/// 动作运行时的上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionContext {
    /// 本次执行使用的配置快照。
    pub config: Config,
}

/// 一个可由快捷键或托盘触发的功能。
pub trait Action: Send + Sync {
    /// 动作种类。
    fn kind(&self) -> ActionKind;
    /// 需要的输入类型。
    fn input_kind(&self) -> InputKind;
    /// 偏好的选区模式。
    fn preferred_mode(&self) -> SelectionMode;
    /// 执行动作。
    fn run(&self, ctx: &ActionContext, input: ActionInput) -> Result<ActionOutcome, ActionError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;
    use crate::selection::SelectionShape;

    struct Probe;

    impl Action for Probe {
        fn kind(&self) -> ActionKind {
            ActionKind::Copy
        }
        fn input_kind(&self) -> InputKind {
            InputKind::Image
        }
        fn preferred_mode(&self) -> SelectionMode {
            SelectionMode::Rect
        }
        fn run(
            &self,
            _ctx: &ActionContext,
            input: ActionInput,
        ) -> Result<ActionOutcome, ActionError> {
            if input.kind() != self.input_kind() {
                return Err(ActionError::InvalidInput);
            }
            Ok(ActionOutcome::Done)
        }
    }

    #[test]
    fn input_kind_matches_variant() {
        let img = CroppedImage::new(1, 1, vec![0; 4]).expect("图像");
        assert_eq!(ActionInput::Image(img).kind(), InputKind::Image);
        let sel = Selection::new(SelectionShape::Rect, Rect::new(0, 0, 1, 1), 0).expect("选区");
        assert_eq!(ActionInput::Region(sel).kind(), InputKind::Region);
    }

    #[test]
    fn action_rejects_wrong_input() {
        let ctx = ActionContext {
            config: Config::default(),
        };
        let sel = Selection::new(SelectionShape::Rect, Rect::new(0, 0, 1, 1), 0).expect("选区");
        let err = Probe.run(&ctx, ActionInput::Region(sel)).err();
        assert_eq!(err, Some(ActionError::InvalidInput));

        let img = CroppedImage::new(1, 1, vec![0; 4]).expect("图像");
        assert!(matches!(
            Probe.run(&ctx, ActionInput::Image(img)),
            Ok(ActionOutcome::Done)
        ));
    }
}
