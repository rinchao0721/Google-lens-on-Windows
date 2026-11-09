//! 自定义端点：字段名与响应解析规则由配置决定。

use std::time::Duration;

use lenswin_core::config::{CustomUploadConfig, ResponseKind, is_http_url};

use crate::http::{MultipartRequest, post};
use crate::response::{parse_json, parse_text};
use crate::{UploadError, Uploader};

/// 自定义端点上传器。
#[derive(Debug, Clone)]
pub struct CustomUploader {
    endpoint: String,
    file_field: String,
    response: ResponseKind,
    json_pointer: String,
    timeout: Duration,
    max_bytes: u64,
}

impl CustomUploader {
    /// 创建上传器；端点不是 http(s)、字段名为空、JSON 指针不以 `/` 开头时拒绝。
    pub fn new(
        config: &CustomUploadConfig,
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<Self, UploadError> {
        if !is_http_url(&config.endpoint) {
            return Err(UploadError::Config("端点不是 http(s) 地址".to_string()));
        }
        if config.file_field.trim().is_empty() {
            return Err(UploadError::Config("文件字段名不能为空".to_string()));
        }
        if config.response == ResponseKind::Json && !config.json_pointer.starts_with('/') {
            return Err(UploadError::Config("JSON 指针必须以 / 开头".to_string()));
        }
        Ok(Self {
            endpoint: config.endpoint.clone(),
            file_field: config.file_field.clone(),
            response: config.response,
            json_pointer: config.json_pointer.clone(),
            timeout,
            max_bytes,
        })
    }

    fn parse(&self, body: &str) -> Result<String, UploadError> {
        match self.response {
            ResponseKind::Json => parse_json(body, &self.json_pointer),
            ResponseKind::Text => parse_text(body),
        }
    }
}

impl Uploader for CustomUploader {
    fn upload(&self, png: &[u8], file_name: &str) -> Result<String, UploadError> {
        let request = MultipartRequest {
            endpoint: &self.endpoint,
            fields: Vec::new(),
            file_field: &self.file_field,
            timeout: self.timeout,
            max_bytes: self.max_bytes,
        };
        self.parse(&post(&request, png, file_name)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(config: &CustomUploadConfig) -> Result<CustomUploader, UploadError> {
        CustomUploader::new(config, Duration::from_secs(5), 1024)
    }

    #[test]
    fn default_config_matches_uguu_response() {
        let uploader = build(&CustomUploadConfig::default()).expect("上传器");
        assert_eq!(
            uploader
                .parse(r#"{"files":[{"url":"https://a.uguu.se/x.png"}]}"#)
                .as_deref(),
            Ok("https://a.uguu.se/x.png")
        );
    }

    #[test]
    fn text_mode_ignores_pointer() {
        let config = CustomUploadConfig {
            response: ResponseKind::Text,
            json_pointer: String::new(),
            ..CustomUploadConfig::default()
        };
        let uploader = build(&config).expect("上传器");
        assert_eq!(
            uploader.parse("https://x.y/z.png").as_deref(),
            Ok("https://x.y/z.png")
        );
    }

    #[test]
    fn invalid_configs_are_rejected() {
        let base = CustomUploadConfig::default();
        let bad_endpoint = CustomUploadConfig {
            endpoint: "ftp://x".to_string(),
            ..base.clone()
        };
        let empty_field = CustomUploadConfig {
            file_field: " ".to_string(),
            ..base.clone()
        };
        let bad_pointer = CustomUploadConfig {
            json_pointer: "files/0".to_string(),
            ..base
        };
        for config in [bad_endpoint, empty_field, bad_pointer] {
            assert!(matches!(build(&config), Err(UploadError::Config(_))));
        }
    }
}
