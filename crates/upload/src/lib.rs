//! 图床抽象：字节到 URL。

#![forbid(unsafe_code)]

mod custom;
mod http;
mod litterbox;
mod response;

use std::time::Duration;

use lenswin_core::config::{UploadConfig, UploadProvider};
use lenswin_core::url::url_host;
use thiserror::Error;

pub use custom::CustomUploader;
pub use litterbox::{LITTERBOX_ENDPOINT, LitterboxUploader};

/// 上传失败的原因。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum UploadError {
    /// 图片超过配置的大小上限。
    #[error("图片大小 {size} 字节超过上限 {limit} 字节")]
    TooLarge {
        /// 实际大小。
        size: usize,
        /// 配置的上限。
        limit: u64,
    },
    /// 网络层失败，附带面向用户的说明。
    #[error("网络请求失败：{0}")]
    Network(String),
    /// 图床返回了非成功状态码。
    #[error("图床返回 HTTP {0}")]
    Status(u16),
    /// 图床响应无法解析出合法的 http(s) 地址。
    #[error("图床响应无法解析：{0}")]
    BadResponse(String),
    /// 上传配置不合法。
    #[error("上传配置不合法：{0}")]
    Config(String),
}

/// 图床：把 PNG 字节变成可公开访问的 http(s) 地址。
pub trait Uploader: Send + Sync {
    /// 上传并返回图片地址，返回值保证是 http 或 https。
    fn upload(&self, png: &[u8], file_name: &str) -> Result<String, UploadError>;
}

/// 上传前需要告知用户的信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacySummary {
    /// 图床主机名（小写）。
    pub host: String,
    /// 保留时间等说明。
    pub retention: String,
}

/// 按配置构造上传器。
pub fn from_config(config: &UploadConfig) -> Result<Box<dyn Uploader>, UploadError> {
    let timeout = Duration::from_secs(config.timeout_secs);
    match config.provider {
        UploadProvider::Litterbox => Ok(Box::new(LitterboxUploader::new(
            config.litterbox.expiry,
            timeout,
            config.max_bytes,
        ))),
        UploadProvider::Custom => Ok(Box::new(CustomUploader::new(
            &config.custom,
            timeout,
            config.max_bytes,
        )?)),
    }
}

/// 当前配置下图片会被送到哪里、保留多久。
pub fn privacy_summary(config: &UploadConfig) -> Result<PrivacySummary, UploadError> {
    match config.provider {
        UploadProvider::Litterbox => {
            let host = url_host(LITTERBOX_ENDPOINT)
                .ok_or_else(|| UploadError::Config("内置图床地址无效".to_string()))?;
            Ok(PrivacySummary {
                host,
                retention: format!(
                    "公开可访问，{} 后自动删除",
                    config.litterbox.expiry.as_str()
                ),
            })
        }
        UploadProvider::Custom => {
            let host = url_host(&config.custom.endpoint).ok_or_else(|| {
                UploadError::Config("自定义端点不是有效的 http(s) 地址".to_string())
            })?;
            Ok(PrivacySummary {
                host,
                retention: "公开状态与保留时间由该服务决定".to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lenswin_core::config::{Config, LitterboxExpiry};

    #[test]
    fn litterbox_summary_mentions_expiry() {
        let mut config = Config::default().upload;
        config.litterbox.expiry = LitterboxExpiry::Hours12;
        let summary = privacy_summary(&config).expect("摘要");
        assert_eq!(summary.host, "litterbox.catbox.moe");
        assert!(summary.retention.contains("12h"));
    }

    #[test]
    fn custom_summary_uses_endpoint_host() {
        let mut config = Config::default().upload;
        config.provider = UploadProvider::Custom;
        config.custom.endpoint = "https://Upload.Example.com:8443/api".to_string();
        assert_eq!(
            privacy_summary(&config).expect("摘要").host,
            "upload.example.com"
        );
    }

    #[test]
    fn custom_summary_rejects_bad_endpoint() {
        let mut config = Config::default().upload;
        config.provider = UploadProvider::Custom;
        config.custom.endpoint = "ftp://x".to_string();
        assert!(matches!(
            privacy_summary(&config),
            Err(UploadError::Config(_))
        ));
        assert!(from_config(&config).is_err());
    }

    #[test]
    fn from_config_builds_default_provider() {
        assert!(from_config(&Config::default().upload).is_ok());
    }
}
