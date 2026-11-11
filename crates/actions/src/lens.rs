//! 内置弹窗搜索：在工作线程里缩图、编码 PNG 并组装直传 Lens 的请求。

use std::time::{SystemTime, UNIX_EPOCH};

use lenswin_core::config::Config;
use lenswin_core::error::ActionError;
use lenswin_core::frame::CroppedImage;
use lenswin_core::lens::{
    LENS_FILE_FIELD, LENS_FILE_NAME, LensRequest, MAX_LONG_SIDE, choose_boundary, downscale,
    multipart_body, upload_url,
};

use crate::encode::encode_png;

/// 准备一次直传请求；未确认隐私提示时拒绝。
pub fn prepare_lens_request(
    config: &Config,
    image: CroppedImage,
) -> Result<LensRequest, ActionError> {
    // 即使调用方漏了确认流程，这里也不允许未确认就发送
    if !config.privacy.lens_acknowledged {
        return Err(ActionError::Failed(
            "尚未确认内置弹窗的隐私提示，已取消搜索".to_string(),
        ));
    }
    let image = downscale(image, MAX_LONG_SIDE).map_err(|e| ActionError::Failed(e.to_string()))?;
    let png = encode_png(&image)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ActionError::Failed("系统时间早于 1970 年".to_string()))?;
    let millis = u64::try_from(now.as_millis()).unwrap_or(u64::MAX);
    let seed = u64::from(now.subsec_nanos()) ^ millis.rotate_left(17);
    let boundary = choose_boundary(&png, seed).map_err(|e| ActionError::Failed(e.to_string()))?;
    let (content_type, body) = multipart_body(
        &boundary,
        LENS_FILE_FIELD,
        LENS_FILE_NAME,
        "image/png",
        &png,
    )
    .map_err(|e| ActionError::Failed(e.to_string()))?;
    Ok(LensRequest {
        url: upload_url(millis),
        content_type,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32) -> CroppedImage {
        let len = usize::try_from(w * h * 4).expect("长度");
        CroppedImage::new(w, h, vec![128; len]).expect("图像")
    }

    #[test]
    fn refused_without_acknowledgement() {
        match prepare_lens_request(&Config::default(), image(2, 2)) {
            Err(ActionError::Failed(msg)) => assert!(msg.contains("隐私"), "{msg}"),
            other => panic!("期望隐私拒绝，实际为 {other:?}"),
        }
    }

    #[test]
    fn builds_multipart_request() {
        let mut config = Config::default();
        config.privacy.lens_acknowledged = true;
        let request = prepare_lens_request(&config, image(3000, 10)).expect("请求");
        assert!(
            request
                .url
                .starts_with("https://lens.google.com/v3/upload?")
        );
        let boundary = request
            .content_type
            .strip_prefix("multipart/form-data; boundary=")
            .expect("分隔串");
        let tail = format!("\r\n--{boundary}--\r\n");
        assert!(request.body.ends_with(tail.as_bytes()));
        assert!(request.body.windows(4).any(|w| w == b"\x89PNG"));
    }
}
