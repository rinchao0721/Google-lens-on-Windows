//! 图床响应解析：纯函数，不触网。

use lenswin_core::config::is_http_url;
use serde_json::Value;

use crate::UploadError;

fn checked_url(candidate: &str) -> Result<String, UploadError> {
    let trimmed = candidate.trim();
    if is_http_url(trimmed) {
        Ok(trimmed.to_string())
    } else {
        Err(UploadError::BadResponse(
            "返回内容不是 http(s) 地址".to_string(),
        ))
    }
}

/// 响应正文即地址。
pub(crate) fn parse_text(body: &str) -> Result<String, UploadError> {
    checked_url(body)
}

/// 响应为 JSON，按 RFC 6901 指针取字符串字段。
pub(crate) fn parse_json(body: &str, pointer: &str) -> Result<String, UploadError> {
    let value: Value = serde_json::from_str(body)
        .map_err(|_| UploadError::BadResponse("响应不是合法的 JSON".to_string()))?;
    let target = value
        .pointer(pointer)
        .ok_or_else(|| UploadError::BadResponse(format!("JSON 中找不到 {pointer}")))?;
    let text = target
        .as_str()
        .ok_or_else(|| UploadError::BadResponse(format!("{pointer} 不是字符串")))?;
    checked_url(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_accepts_url_with_whitespace() {
        assert_eq!(
            parse_text("https://litter.catbox.moe/a.png\n").as_deref(),
            Ok("https://litter.catbox.moe/a.png")
        );
    }

    #[test]
    fn text_rejects_non_urls() {
        for body in [
            "",
            "error: too large",
            "javascript:alert(1)",
            "ftp://a.b/c",
            "a b",
        ] {
            assert!(parse_text(body).is_err(), "{body}");
        }
    }

    #[test]
    fn json_extracts_by_pointer() {
        let body = r#"{"success":true,"files":[{"url":"https://a.uguu.se/x.png"}]}"#;
        assert_eq!(
            parse_json(body, "/files/0/url").as_deref(),
            Ok("https://a.uguu.se/x.png")
        );
    }

    #[test]
    fn json_rejects_bad_shapes() {
        let ok = r#"{"files":[{"url":"https://a.b/c"}]}"#;
        assert!(parse_json("not json", "/files/0/url").is_err());
        assert!(parse_json(ok, "/files/1/url").is_err());
        assert!(parse_json(r#"{"files":[{"url":42}]}"#, "/files/0/url").is_err());
        assert!(parse_json(r#"{"files":[{"url":"file:///c"}]}"#, "/files/0/url").is_err());
        assert!(parse_json(ok, "").is_err());
    }
}
