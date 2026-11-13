//! 查询 GitHub Releases 并解析出候选版本；不下载、不安装任何文件。

#![forbid(unsafe_code)]

use std::time::Duration;

use lenswin_core::update::{Release, UpdateCheck, Version, is_release_page_url, newest_upgrade};
use lenswin_net::{Diagnosis, NetError, NetworkFault};
use reqwest::StatusCode;
use reqwest::header::ACCEPT;
use serde_json::Value;
use thiserror::Error;

/// Releases 列表接口；`/releases/latest` 不含预发布版，所以用列表。
pub const RELEASES_API_URL: &str =
    "https://api.github.com/repos/rinchao0721/Google-lens-on-Windows/releases?per_page=20";

const TIMEOUT: Duration = Duration::from_secs(10);

/// 列表里带着发布说明，给足余量。
const MAX_RESPONSE_BYTES: u64 = 512 * 1024;

/// 检查更新失败的原因。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum UpdateError {
    /// 网络层失败，附带面向用户的说明。
    #[error("网络请求失败：{0}")]
    Network(String),
    /// GitHub 限流。
    #[error("请求过于频繁，请稍后再试")]
    RateLimited,
    /// 非成功状态码。
    #[error("GitHub 返回 HTTP {0}")]
    Status(u16),
    /// 响应无法解析。
    #[error("响应异常：{0}")]
    BadResponse(String),
    /// 编译期写入的当前版本号不合法。
    #[error("当前版本号无法解析：{0}")]
    CurrentVersion(String),
}

/// 当前程序的版本号；工作区内各 crate 共用同一个版本。
pub fn current_version() -> Result<Version, UpdateError> {
    Version::parse(env!("CARGO_PKG_VERSION")).map_err(|e| UpdateError::CurrentVersion(e.0))
}

/// 检查是否有比当前版本新的发布；会发起网络请求，只能在工作线程调用。
pub fn check(include_prerelease: bool) -> Result<UpdateCheck, UpdateError> {
    let current = current_version()?;
    let releases = fetch_releases()?;
    Ok(conclude(&current, &releases, include_prerelease))
}

/// 由候选列表得出结论。
fn conclude(current: &Version, releases: &[Release], include_prerelease: bool) -> UpdateCheck {
    match newest_upgrade(current, releases, include_prerelease) {
        Some(release) => UpdateCheck::Available(release.clone()),
        None => UpdateCheck::UpToDate,
    }
}

fn fault_message(fault: NetworkFault) -> &'static str {
    match fault {
        NetworkFault::Timeout => "请求超时",
        NetworkFault::Connect => "无法连接到 GitHub，请检查网络",
        NetworkFault::Dns => "无法解析 GitHub 域名，请检查网络",
    }
}

fn net_error(error: NetError) -> UpdateError {
    match error {
        NetError::TooLarge => UpdateError::BadResponse("响应过大".to_string()),
        NetError::Client(_) | NetError::Read(_) => UpdateError::Network(error.to_string()),
    }
}

fn status_error(status: StatusCode) -> Option<UpdateError> {
    match status.as_u16() {
        200 => None,
        403 | 429 => Some(UpdateError::RateLimited),
        code => Some(UpdateError::Status(code)),
    }
}

/// 请求 Releases 列表并解析出合法的候选版本。
pub fn fetch_releases() -> Result<Vec<Release>, UpdateError> {
    let response = lenswin_net::client()
        .map_err(net_error)?
        .get(RELEASES_API_URL)
        .header(ACCEPT, "application/vnd.github+json")
        .timeout(TIMEOUT)
        .send()
        .map_err(|e| {
            let e = e.without_url();
            UpdateError::Network(match lenswin_net::diagnose(&e) {
                Diagnosis::Fault(fault) => fault_message(fault).to_string(),
                Diagnosis::Other(text) => text,
            })
        })?;
    if let Some(error) = status_error(response.status()) {
        return Err(error);
    }
    let body = lenswin_net::read_body(response, MAX_RESPONSE_BYTES).map_err(net_error)?;
    parse_releases(&body)
}

/// 解析 Releases 列表；格式不对的条目跳过，整体不是数组才算失败。
pub fn parse_releases(body: &[u8]) -> Result<Vec<Release>, UpdateError> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| UpdateError::BadResponse("不是合法的 JSON".to_string()))?;
    let items = value
        .as_array()
        .ok_or_else(|| UpdateError::BadResponse("顶层不是数组".to_string()))?;
    Ok(items.iter().filter_map(parse_release).collect())
}

fn parse_release(item: &Value) -> Option<Release> {
    // 草稿对外不可见，字段缺失也按草稿处理
    if item.get("draft")?.as_bool()? {
        return None;
    }
    let flagged = item.get("prerelease")?.as_bool()?;
    let version = Version::parse_tag(item.get("tag_name")?.as_str()?).ok()?;
    let page_url = item.get("html_url")?.as_str()?;
    if !is_release_page_url(page_url) {
        return None;
    }
    Some(Release {
        version,
        page_url: page_url.to_string(),
        prerelease: flagged || version.is_prerelease(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(tag: &str, url: &str, draft: bool, prerelease: bool) -> String {
        format!(
            r#"{{"tag_name":"{tag}","html_url":"{url}","draft":{draft},"prerelease":{prerelease},"body":"说明"}}"#
        )
    }

    fn page(tag: &str) -> String {
        format!("https://github.com/o/r/releases/tag/{tag}")
    }

    #[test]
    fn parses_valid_entries() {
        let body = format!(
            "[{},{}]",
            entry("v0.2.0", &page("v0.2.0"), false, false),
            entry("v0.3.0-beta.1", &page("v0.3.0-beta.1"), false, false)
        );
        let releases = parse_releases(body.as_bytes()).expect("解析");
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].version.to_string(), "0.2.0");
        assert!(!releases[0].prerelease);
        assert_eq!(releases[0].page_url, page("v0.2.0"));
        assert!(releases[1].prerelease);
    }

    #[test]
    fn skips_invalid_entries() {
        let missing_draft =
            r#"{"tag_name":"v0.3.0","html_url":"https://github.com/o/r","prerelease":false}"#;
        let bad_tag_type = r#"{"tag_name":7,"html_url":"https://github.com/o/r","draft":false,"prerelease":false}"#;
        let body = format!(
            "[{},{},{},{},{},{},{},42,\"x\",{{}}]",
            entry("v0.2.0", &page("v0.2.0"), true, false),
            entry("v0.2.1", "https://evil.com/x", false, false),
            entry("release-1", &page("release-1"), false, false),
            entry("v0.2.2-alpha.1", &page("v0.2.2"), false, false),
            missing_draft,
            bad_tag_type,
            entry("v0.4.0", &page("v0.4.0"), false, true),
        );
        let releases = parse_releases(body.as_bytes()).expect("解析");
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].version.to_string(), "0.4.0");
        assert!(releases[0].prerelease);
    }

    #[test]
    fn empty_array_is_ok() {
        assert_eq!(parse_releases(b"[]"), Ok(Vec::new()));
    }

    #[test]
    fn non_array_or_invalid_json_fails() {
        for body in [&b"{}"[..], b"null", b"\"x\"", b"", b"[", &[0xFF, 0xFE]] {
            assert!(
                matches!(parse_releases(body), Err(UpdateError::BadResponse(_))),
                "{body:?}"
            );
        }
    }

    #[test]
    fn status_mapping() {
        assert_eq!(status_error(StatusCode::OK), None);
        assert_eq!(
            status_error(StatusCode::FORBIDDEN),
            Some(UpdateError::RateLimited)
        );
        assert_eq!(
            status_error(StatusCode::TOO_MANY_REQUESTS),
            Some(UpdateError::RateLimited)
        );
        assert_eq!(
            status_error(StatusCode::NOT_FOUND),
            Some(UpdateError::Status(404))
        );
        assert_eq!(
            status_error(StatusCode::NO_CONTENT),
            Some(UpdateError::Status(204))
        );
    }

    #[test]
    fn network_errors_use_github_wording() {
        assert_eq!(
            net_error(NetError::TooLarge),
            UpdateError::BadResponse("响应过大".to_string())
        );
        assert_eq!(
            fault_message(NetworkFault::Connect),
            "无法连接到 GitHub，请检查网络"
        );
    }

    #[test]
    fn current_version_is_parsable() {
        assert!(current_version().is_ok());
    }

    #[test]
    fn conclusion_from_parsed_list() {
        let body = format!(
            "[{},{},{}]",
            entry("v0.3.0-rc.1", &page("v0.3.0-rc.1"), false, true),
            entry("v0.2.0", &page("v0.2.0"), false, false),
            entry("v0.1.0", &page("v0.1.0"), false, false)
        );
        let releases = parse_releases(body.as_bytes()).expect("解析");
        let current = Version::parse("0.1.0").expect("版本号");
        match conclude(&current, &releases, false) {
            UpdateCheck::Available(r) => {
                assert_eq!(r.version.to_string(), "0.2.0");
                assert_eq!(r.page_url, page("v0.2.0"));
            }
            UpdateCheck::UpToDate => panic!("应发现 0.2.0"),
        }
        match conclude(&current, &releases, true) {
            UpdateCheck::Available(r) => assert_eq!(r.version.to_string(), "0.3.0-rc.1"),
            UpdateCheck::UpToDate => panic!("应发现 0.3.0-rc.1"),
        }
        let newest = Version::parse("0.3.0").expect("版本号");
        assert_eq!(conclude(&newest, &releases, true), UpdateCheck::UpToDate);
        assert_eq!(conclude(&current, &[], true), UpdateCheck::UpToDate);
    }
}
