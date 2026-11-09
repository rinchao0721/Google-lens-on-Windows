//! 网址相关的纯函数：百分号编码、搜索地址拼接、主机名提取。

use crate::config::{URL_PLACEHOLDER, is_http_url};

/// 按 RFC 3986 对文本做百分号编码，只保留未保留字符。
pub fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for &byte in text.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.extend(hex_digit(byte >> 4));
            out.extend(hex_digit(byte & 0x0F));
        }
    }
    out
}

fn hex_digit(nibble: u8) -> Option<char> {
    char::from_digit(u32::from(nibble), 16).map(|c| c.to_ascii_uppercase())
}

/// 把编码后的图片地址填入搜索引擎模板；模板不含恰好一个占位符或结果不是 http(s) 时返回 `None`。
pub fn build_search_url(template: &str, image_url: &str) -> Option<String> {
    if template.matches(URL_PLACEHOLDER).count() != 1 {
        return None;
    }
    let url = template.replace(URL_PLACEHOLDER, &percent_encode(image_url));
    is_http_url(&url).then_some(url)
}

/// 取 http(s) 地址的小写主机名（不含用户信息和端口）；不是合法 http(s) 地址时返回 `None`。
pub fn url_host(url: &str) -> Option<String> {
    if !is_http_url(url) {
        return None;
    }
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next()?;
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        let end = stripped.find(']')?;
        host_port.get(..end.checked_add(2)?)?
    } else {
        host_port.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// 截屏保存的文件名，如 `lenswin_20261109_171000.png`。
pub fn screenshot_file_name(
    year: u16,
    month: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
) -> String {
    format!("lenswin_{year:04}{month:02}{day:02}_{hour:02}{minute:02}{second:02}.png")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_keeps_unreserved_and_escapes_rest() {
        assert_eq!(percent_encode("aZ09-._~"), "aZ09-._~");
        assert_eq!(
            percent_encode("https://a.b/c?d=e&f"),
            "https%3A%2F%2Fa.b%2Fc%3Fd%3De%26f"
        );
        assert_eq!(percent_encode("中 "), "%E4%B8%AD%20");
        assert_eq!(percent_encode(""), "");
    }

    #[test]
    fn search_url_encodes_image_url() {
        let url = build_search_url(
            "https://lens.google.com/uploadbyurl?url={url}",
            "https://litter.catbox.moe/a.png?x=1&y=2",
        );
        assert_eq!(
            url.as_deref(),
            Some(
                "https://lens.google.com/uploadbyurl?url=https%3A%2F%2Flitter.catbox.moe%2Fa.png%3Fx%3D1%26y%3D2"
            )
        );
    }

    #[test]
    fn search_url_rejects_bad_templates() {
        assert_eq!(build_search_url("https://a.b/?u=x", "https://c.d/e"), None);
        assert_eq!(
            build_search_url("https://a.b/?u={url}&v={url}", "https://c.d/e"),
            None
        );
        assert_eq!(build_search_url("file:///{url}", "https://c.d/e"), None);
        assert_eq!(build_search_url("{url}", "https://c.d/e"), None);
    }

    #[test]
    fn host_extraction() {
        assert_eq!(
            url_host("https://Litterbox.Catbox.moe/a/b").as_deref(),
            Some("litterbox.catbox.moe")
        );
        assert_eq!(
            url_host("http://user:pw@Example.com:8080/x?y#z").as_deref(),
            Some("example.com")
        );
        assert_eq!(url_host("http://[::1]:80/x").as_deref(), Some("[::1]"));
        assert_eq!(url_host("ftp://a.b"), None);
        assert_eq!(url_host("https://"), None);
        assert_eq!(url_host(""), None);
    }

    #[test]
    fn file_name_is_zero_padded() {
        assert_eq!(
            screenshot_file_name(2026, 1, 2, 3, 4, 5),
            "lenswin_20260102_030405.png"
        );
    }
}
