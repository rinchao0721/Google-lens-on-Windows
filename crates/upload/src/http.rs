//! multipart 上传的共用实现；全局只建一个 `Client`。

use std::error::Error;
use std::io::{self, Read};
use std::sync::OnceLock;
use std::time::Duration;

use lenswin_core::config::is_http_url;
use reqwest::blocking::Client;
use reqwest::blocking::multipart::{Form, Part};

use crate::UploadError;

/// 响应正文读取上限，图床返回的只是一个地址或一小段 JSON。
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;

static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();

fn client() -> Result<&'static Client, UploadError> {
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .user_agent(concat!("lenswin/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|e| e.without_url().to_string())
        })
        .as_ref()
        .map_err(|e| UploadError::Network(e.clone()))
}

/// 一次 multipart 上传的参数。
pub(crate) struct MultipartRequest<'a> {
    pub endpoint: &'a str,
    pub fields: Vec<(String, String)>,
    pub file_field: &'a str,
    pub timeout: Duration,
    pub max_bytes: u64,
}

/// 大小检查：超过上限直接拒绝，不发请求。
pub(crate) fn check_size(size: usize, limit: u64) -> Result<(), UploadError> {
    match u64::try_from(size) {
        Ok(n) if n <= limit => Ok(()),
        _ => Err(UploadError::TooLarge { size, limit }),
    }
}

/// 从错误链中识别出的网络故障类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NetworkFault {
    Timeout,
    Connect,
    Dns,
}

impl NetworkFault {
    fn message(self) -> &'static str {
        match self {
            NetworkFault::Timeout => "请求超时",
            NetworkFault::Connect => "无法连接到图床，请检查网络",
            NetworkFault::Dns => "无法解析图床域名，请检查网络或域名",
        }
    }
}

fn io_fault(kind: io::ErrorKind) -> Option<NetworkFault> {
    match kind {
        io::ErrorKind::TimedOut => Some(NetworkFault::Timeout),
        io::ErrorKind::ConnectionRefused
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::NotConnected
        | io::ErrorKind::AddrNotAvailable
        | io::ErrorKind::HostUnreachable
        | io::ErrorKind::NetworkUnreachable
        | io::ErrorKind::NetworkDown => Some(NetworkFault::Connect),
        _ => None,
    }
}

/// 沿 `source()` 链判断故障类型；multipart 上传会把连接错误包进请求体错误里。
fn classify_chain(error: &(dyn Error + 'static)) -> Option<NetworkFault> {
    let mut current = Some(error);
    while let Some(e) = current {
        if let Some(r) = e.downcast_ref::<reqwest::Error>() {
            if r.is_timeout() {
                return Some(NetworkFault::Timeout);
            }
            if r.is_connect() {
                return Some(NetworkFault::Connect);
            }
        }
        if let Some(fault) = e
            .downcast_ref::<io::Error>()
            .and_then(|i| io_fault(i.kind()))
        {
            return Some(fault);
        }
        // hyper 的 DNS 失败只在文本里带 "dns error"
        if e.to_string().contains("dns error") {
            return Some(NetworkFault::Dns);
        }
        current = e.source();
    }
    None
}

/// 错误链最底层的文本。
fn root_cause(error: &(dyn Error + 'static)) -> String {
    let mut last = error;
    while let Some(next) = last.source() {
        last = next;
    }
    last.to_string()
}

fn describe_chain(error: &(dyn Error + 'static)) -> String {
    match classify_chain(error) {
        Some(fault) => fault.message().to_string(),
        None => root_cause(error),
    }
}

fn describe(error: &reqwest::Error) -> String {
    describe_chain(error)
}

/// 发送 multipart 请求并返回响应正文。
pub(crate) fn post(
    request: &MultipartRequest<'_>,
    png: &[u8],
    file_name: &str,
) -> Result<String, UploadError> {
    check_size(png.len(), request.max_bytes)?;
    if !is_http_url(request.endpoint) {
        return Err(UploadError::Config("端点不是 http(s) 地址".to_string()));
    }
    let part = Part::bytes(png.to_vec())
        .file_name(file_name.to_string())
        .mime_str("image/png")
        .map_err(|e| UploadError::Config(e.to_string()))?;
    let mut form = Form::new();
    for (name, value) in &request.fields {
        form = form.text(name.clone(), value.clone());
    }
    form = form.part(request.file_field.to_string(), part);

    let response = client()?
        .post(request.endpoint)
        .timeout(request.timeout)
        .multipart(form)
        .send()
        .map_err(|e| UploadError::Network(describe(&e.without_url())))?;
    let status = response.status();
    if !status.is_success() {
        return Err(UploadError::Status(status.as_u16()));
    }
    let mut body = Vec::new();
    response
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|e| UploadError::Network(format!("读取响应失败：{e}")))?;
    if u64::try_from(body.len()).map_or(true, |n| n > MAX_RESPONSE_BYTES) {
        return Err(UploadError::BadResponse("响应过大".to_string()));
    }
    String::from_utf8(body).map_err(|_| UploadError::BadResponse("响应不是 UTF-8 文本".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(endpoint: &str, max_bytes: u64) -> MultipartRequest<'_> {
        MultipartRequest {
            endpoint,
            fields: Vec::new(),
            file_field: "f",
            timeout: Duration::from_secs(1),
            max_bytes,
        }
    }

    #[test]
    fn size_limit_is_inclusive() {
        assert_eq!(check_size(10, 10), Ok(()));
        assert_eq!(check_size(0, 10), Ok(()));
        assert_eq!(
            check_size(11, 10),
            Err(UploadError::TooLarge {
                size: 11,
                limit: 10
            })
        );
    }

    #[test]
    fn oversize_is_rejected_before_any_request() {
        let req = request("http://127.0.0.1:9/never", 4);
        assert!(matches!(
            post(&req, &[0; 5], "a.png"),
            Err(UploadError::TooLarge { .. })
        ));
    }

    #[derive(Debug)]
    struct Wrapped {
        text: &'static str,
        source: Box<dyn Error + Send + Sync>,
    }

    impl std::fmt::Display for Wrapped {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.text)
        }
    }

    impl Error for Wrapped {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(self.source.as_ref())
        }
    }

    fn wrapped(text: &'static str, source: io::Error) -> Wrapped {
        Wrapped {
            text,
            source: Box::new(source),
        }
    }

    #[test]
    fn refused_connection_is_found_under_body_error() {
        let e = wrapped(
            "request or response body error",
            io::Error::from(io::ErrorKind::ConnectionRefused),
        );
        assert_eq!(classify_chain(&e), Some(NetworkFault::Connect));
        assert_eq!(describe_chain(&e), "无法连接到图床，请检查网络");
    }

    #[test]
    fn timeout_kind_is_recognised() {
        let e = wrapped("body", io::Error::from(io::ErrorKind::TimedOut));
        assert_eq!(classify_chain(&e), Some(NetworkFault::Timeout));
        assert_eq!(describe_chain(&e), "请求超时");
    }

    #[test]
    fn dns_failure_is_recognised_by_text() {
        let e = Wrapped {
            text: "outer",
            source: Box::new(wrapped("dns error", io::Error::other("No such host"))),
        };
        assert_eq!(classify_chain(&e), Some(NetworkFault::Dns));
    }

    #[test]
    fn unknown_error_shows_root_cause() {
        let e = wrapped("request or response body error", io::Error::other("boom"));
        assert_eq!(classify_chain(&e), None);
        assert_eq!(describe_chain(&e), "boom");
        let plain = io::Error::other("alone");
        assert_eq!(describe_chain(&plain), "alone");
    }

    #[test]
    fn non_http_endpoint_is_rejected() {
        let req = request("ftp://example.com", 100);
        assert!(matches!(
            post(&req, &[0; 1], "a.png"),
            Err(UploadError::Config(_))
        ));
    }
}
