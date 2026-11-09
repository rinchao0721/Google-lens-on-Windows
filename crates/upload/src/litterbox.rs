//! litterbox 临时图床：表单字段固定，响应为纯文本地址。

use std::time::Duration;

use lenswin_core::config::LitterboxExpiry;

use crate::http::{MultipartRequest, post};
use crate::response::parse_text;
use crate::{UploadError, Uploader};

/// litterbox 上传接口。
pub const LITTERBOX_ENDPOINT: &str = "https://litterbox.catbox.moe/resources/internals/api.php";

/// litterbox 上传器。
#[derive(Debug, Clone)]
pub struct LitterboxUploader {
    expiry: LitterboxExpiry,
    timeout: Duration,
    max_bytes: u64,
}

impl LitterboxUploader {
    /// 创建上传器。
    pub fn new(expiry: LitterboxExpiry, timeout: Duration, max_bytes: u64) -> Self {
        Self {
            expiry,
            timeout,
            max_bytes,
        }
    }

    fn fields(&self) -> Vec<(String, String)> {
        vec![
            ("reqtype".to_string(), "fileupload".to_string()),
            ("time".to_string(), self.expiry.as_str().to_string()),
        ]
    }
}

impl Uploader for LitterboxUploader {
    fn upload(&self, png: &[u8], file_name: &str) -> Result<String, UploadError> {
        let request = MultipartRequest {
            endpoint: LITTERBOX_ENDPOINT,
            fields: self.fields(),
            file_field: "fileToUpload",
            timeout: self.timeout,
            max_bytes: self.max_bytes,
        };
        parse_text(&post(&request, png, file_name)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_carry_expiry() {
        let uploader =
            LitterboxUploader::new(LitterboxExpiry::Hours24, Duration::from_secs(5), 1024);
        assert_eq!(
            uploader.fields(),
            vec![
                ("reqtype".to_string(), "fileupload".to_string()),
                ("time".to_string(), "24h".to_string()),
            ]
        );
    }

    #[test]
    fn oversize_fails_without_network() {
        let uploader = LitterboxUploader::new(LitterboxExpiry::Hours1, Duration::from_secs(5), 2);
        assert!(matches!(
            uploader.upload(&[0; 3], "a.png"),
            Err(UploadError::TooLarge { .. })
        ));
    }
}
