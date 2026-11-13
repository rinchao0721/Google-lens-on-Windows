//! 全局唯一的 HTTP `Client` 与网络错误识别，不含任何业务请求。

#![forbid(unsafe_code)]

use std::error::Error;
use std::io::{self, Read};
use std::sync::OnceLock;

use reqwest::blocking::{Client, Response};
use thiserror::Error;

/// 网络层自身的失败。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum NetError {
    /// 客户端初始化失败（如 TLS 后端不可用）。
    #[error("初始化 HTTP 客户端失败：{0}")]
    Client(String),
    /// 读取响应正文失败。
    #[error("读取响应失败：{0}")]
    Read(String),
    /// 响应正文超过上限。
    #[error("响应过大")]
    TooLarge,
}

static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();

/// 全局共用的客户端；超时由各请求自行设置。
pub fn client() -> Result<&'static Client, NetError> {
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .user_agent(concat!("lenswin/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|e| e.without_url().to_string())
        })
        .as_ref()
        .map_err(|e| NetError::Client(e.clone()))
}

/// 读取响应正文，超过 `limit` 字节视为失败。
pub fn read_body(response: Response, limit: u64) -> Result<Vec<u8>, NetError> {
    let mut body = Vec::new();
    response
        .take(limit.saturating_add(1))
        .read_to_end(&mut body)
        .map_err(|e| NetError::Read(e.to_string()))?;
    match u64::try_from(body.len()) {
        Ok(n) if n <= limit => Ok(body),
        _ => Err(NetError::TooLarge),
    }
}

/// 从错误链中识别出的网络故障类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkFault {
    /// 超时。
    Timeout,
    /// 连接失败。
    Connect,
    /// 域名解析失败。
    Dns,
}

/// 诊断结果：识别出的故障，或错误链最底层的文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnosis {
    /// 已识别的网络故障。
    Fault(NetworkFault),
    /// 未识别，附最底层错误文本。
    Other(String),
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

fn root_cause(error: &(dyn Error + 'static)) -> String {
    let mut last = error;
    while let Some(next) = last.source() {
        last = next;
    }
    last.to_string()
}

/// 诊断一个请求错误，供调用方换成面向用户的说明。
pub fn diagnose(error: &(dyn Error + 'static)) -> Diagnosis {
    match classify_chain(error) {
        Some(fault) => Diagnosis::Fault(fault),
        None => Diagnosis::Other(root_cause(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(diagnose(&e), Diagnosis::Fault(NetworkFault::Connect));
    }

    #[test]
    fn timeout_kind_is_recognised() {
        let e = wrapped("body", io::Error::from(io::ErrorKind::TimedOut));
        assert_eq!(diagnose(&e), Diagnosis::Fault(NetworkFault::Timeout));
    }

    #[test]
    fn dns_failure_is_recognised_by_text() {
        let e = Wrapped {
            text: "outer",
            source: Box::new(wrapped("dns error", io::Error::other("No such host"))),
        };
        assert_eq!(diagnose(&e), Diagnosis::Fault(NetworkFault::Dns));
    }

    #[test]
    fn unknown_error_shows_root_cause() {
        let e = wrapped("request or response body error", io::Error::other("boom"));
        assert_eq!(diagnose(&e), Diagnosis::Other("boom".to_string()));
        let plain = io::Error::other("alone");
        assert_eq!(diagnose(&plain), Diagnosis::Other("alone".to_string()));
    }
}
