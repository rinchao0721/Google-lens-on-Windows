//! 以图搜图：编码、上传、拼接搜索地址并用浏览器打开。

use lenswin_core::action::{
    Action, ActionContext, ActionInput, ActionKind, ActionOutcome, InputKind,
};
use lenswin_core::error::ActionError;
use lenswin_core::selection::SelectionMode;
use lenswin_core::url::build_search_url;
use lenswin_platform::shell;
use lenswin_upload::{from_config, privacy_summary};

use crate::encode::encode_png;

/// 上传时使用的文件名。
const UPLOAD_FILE_NAME: &str = "lenswin.png";

/// 以图搜图动作。
#[derive(Debug, Default, Clone, Copy)]
pub struct SearchAction;

impl Action for SearchAction {
    fn kind(&self) -> ActionKind {
        ActionKind::Search
    }

    fn input_kind(&self) -> InputKind {
        InputKind::Image
    }

    fn preferred_mode(&self) -> SelectionMode {
        SelectionMode::Lasso
    }

    fn run(&self, ctx: &ActionContext, input: ActionInput) -> Result<ActionOutcome, ActionError> {
        let ActionInput::Image(image) = input else {
            return Err(ActionError::InvalidInput);
        };
        let config = &ctx.config;
        let summary =
            privacy_summary(&config.upload).map_err(|e| ActionError::Failed(e.to_string()))?;
        // 即使调用方漏了确认流程，这里也不允许未确认就上传
        if !config.privacy.is_acknowledged(&summary.host) {
            return Err(ActionError::Failed(
                "尚未确认图片上传的隐私提示，已取消上传".to_string(),
            ));
        }
        let png = encode_png(&image)?;
        let uploader =
            from_config(&config.upload).map_err(|e| ActionError::Failed(e.to_string()))?;
        let image_url = uploader
            .upload(&png, UPLOAD_FILE_NAME)
            .map_err(|e| ActionError::Failed(e.to_string()))?;
        log::debug!("图片已上传：{image_url}");
        let search_url = build_search_url(&config.search.engine_url_template, &image_url)
            .ok_or_else(|| ActionError::Failed("搜索引擎地址模板无效".to_string()))?;
        shell::open_url(&search_url).map_err(|e| ActionError::Failed(e.to_string()))?;
        Ok(ActionOutcome::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lenswin_core::config::Config;
    use lenswin_core::frame::CroppedImage;

    fn image() -> ActionInput {
        ActionInput::Image(CroppedImage::new(1, 1, vec![0, 0, 0, 255]).expect("图像"))
    }

    #[test]
    fn upload_is_refused_without_acknowledgement() {
        let ctx = ActionContext {
            config: Config::default(),
        };
        match SearchAction.run(&ctx, image()) {
            Err(ActionError::Failed(msg)) => assert!(msg.contains("隐私"), "{msg}"),
            Err(other) => panic!("期望隐私拒绝，实际为 {other:?}"),
            Ok(_) => panic!("未确认时不应成功"),
        }
    }

    #[test]
    fn acknowledgement_for_other_host_is_not_enough() {
        let mut config = Config::default();
        config.privacy.acknowledged_host = "uguu.se".to_string();
        let ctx = ActionContext { config };
        assert!(SearchAction.run(&ctx, image()).is_err());
    }
}
