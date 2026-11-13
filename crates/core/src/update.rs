//! 检查更新的纯逻辑：版本号解析与比较、候选版本挑选、Release 页地址校验。

use std::fmt;

use thiserror::Error;

/// Release 页地址必须以此开头，防止点击气泡打开任意地址。
pub const RELEASE_PAGE_PREFIX: &str = "https://github.com/";

/// Release 页地址长度上限。
const MAX_PAGE_URL_LEN: usize = 2048;

/// 版本号解析失败。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("版本号格式不是 MAJOR.MINOR.PATCH[-beta.N|-rc.N]：{0}")]
pub struct VersionError(pub String);

/// 发布通道；派生的顺序即 `beta < rc < 正式版`，同类按编号比较。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Channel {
    /// 测试版。
    Beta(u32),
    /// 候选版。
    Rc(u32),
    /// 正式版。
    Stable,
}

/// 语义化版本号，只支持本项目发版用到的子集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// 主版本号。
    pub major: u32,
    /// 次版本号。
    pub minor: u32,
    /// 修订号。
    pub patch: u32,
    /// 发布通道。
    pub channel: Channel,
}

/// 十进制非负整数，禁止前导零与符号，超出 `u32` 视为非法。
fn number(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if text.len() > 1 && text.starts_with('0') {
        return None;
    }
    text.parse().ok()
}

impl Version {
    /// 解析 `MAJOR.MINOR.PATCH[-beta.N|-rc.N]`，不接受前缀 `v`。
    pub fn parse(text: &str) -> Result<Self, VersionError> {
        let invalid = || VersionError(text.to_string());
        let (core, suffix) = match text.split_once('-') {
            Some((core, suffix)) => (core, Some(suffix)),
            None => (text, None),
        };
        let mut parts = core.split('.');
        let (Some(major), Some(minor), Some(patch), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid());
        };
        let channel = match suffix {
            None => Channel::Stable,
            Some(suffix) => {
                let (kind, n) = suffix.split_once('.').ok_or_else(invalid)?;
                let n = number(n).ok_or_else(invalid)?;
                match kind {
                    "beta" => Channel::Beta(n),
                    "rc" => Channel::Rc(n),
                    _ => return Err(invalid()),
                }
            }
        };
        Ok(Self {
            major: number(major).ok_or_else(invalid)?,
            minor: number(minor).ok_or_else(invalid)?,
            patch: number(patch).ok_or_else(invalid)?,
            channel,
        })
    }

    /// 解析 git tag：允许并去掉一个前缀 `v`。
    pub fn parse_tag(tag: &str) -> Result<Self, VersionError> {
        Self::parse(tag.strip_prefix('v').unwrap_or(tag))
    }

    /// 是否为预发布版本。
    pub const fn is_prerelease(&self) -> bool {
        !matches!(self.channel, Channel::Stable)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        match self.channel {
            Channel::Beta(n) => write!(f, "-beta.{n}"),
            Channel::Rc(n) => write!(f, "-rc.{n}"),
            Channel::Stable => Ok(()),
        }
    }
}

/// 一个可供升级的发布版本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// 版本号。
    pub version: Version,
    /// Release 页地址，已通过 [`is_release_page_url`] 校验。
    pub page_url: String,
    /// 是否算作预发布：版本号带后缀，或发布方标记为预发布。
    pub prerelease: bool,
}

/// 一次检查的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    /// 有可升级的版本。
    Available(Release),
    /// 当前已是最新。
    UpToDate,
}

/// 地址是否为可以安全打开的 GitHub 页面。
pub fn is_release_page_url(url: &str) -> bool {
    url.len() <= MAX_PAGE_URL_LEN
        && url.len() > RELEASE_PAGE_PREFIX.len()
        && url.starts_with(RELEASE_PAGE_PREFIX)
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// 挑出比 `current` 新的最高版本；关闭预发布时忽略预发布版本。
pub fn newest_upgrade<'a>(
    current: &Version,
    releases: &'a [Release],
    include_prerelease: bool,
) -> Option<&'a Release> {
    releases
        .iter()
        .filter(|r| include_prerelease || !r.prerelease)
        .filter(|r| r.version > *current)
        .max_by(|a, b| a.version.cmp(&b.version))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).expect("版本号")
    }

    fn release(text: &str, prerelease: bool) -> Release {
        Release {
            version: v(text),
            page_url: format!("https://github.com/o/r/releases/tag/v{text}"),
            prerelease,
        }
    }

    #[test]
    fn parses_stable_and_prerelease() {
        assert_eq!(
            v("0.1.0"),
            Version {
                major: 0,
                minor: 1,
                patch: 0,
                channel: Channel::Stable
            }
        );
        assert_eq!(v("1.2.3-beta.4").channel, Channel::Beta(4));
        assert_eq!(v("1.2.3-rc.0").channel, Channel::Rc(0));
        assert_eq!(Version::parse_tag("v2.0.0"), Ok(v("2.0.0")));
        assert_eq!(Version::parse_tag("2.0.0"), Ok(v("2.0.0")));
    }

    #[test]
    fn rejects_malformed_versions() {
        for text in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.02.3",
            "1.2.3-alpha.1",
            "1.2.3-beta",
            "1.2.3-beta.",
            "1.2.3-beta.01",
            "1.2.3-",
            "1.2.3+build",
            "+1.2.3",
            "1.-2.3",
            " 1.2.3",
            "4294967296.0.0",
            "vv1.2.3",
            "v1.2.3",
        ] {
            assert!(Version::parse(text).is_err(), "{text:?}");
        }
        assert!(Version::parse_tag("vv1.2.3").is_err());
        assert!(Version::parse("4294967295.0.0").is_ok());
    }

    #[test]
    fn ordering_follows_channel_then_number() {
        assert!(v("1.0.0-beta.1") < v("1.0.0-beta.2"));
        assert!(v("1.0.0-beta.9") < v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.9") < v("1.0.0"));
        assert!(v("1.0.0") < v("1.0.1-beta.1"));
        assert!(v("0.9.10") > v("0.9.9"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert_eq!(v("1.0.0"), v("1.0.0"));
    }

    #[test]
    fn display_roundtrips() {
        for text in ["0.1.0", "1.2.3-beta.4", "10.0.1-rc.12"] {
            assert_eq!(v(text).to_string(), text);
        }
        assert!(!v("1.0.0").is_prerelease());
        assert!(v("1.0.0-rc.1").is_prerelease());
    }

    #[test]
    fn page_url_must_be_github() {
        assert!(is_release_page_url(
            "https://github.com/o/r/releases/tag/v0.1.0"
        ));
        for url in [
            "",
            "https://github.com/",
            "http://github.com/o/r",
            "https://github.com.evil.com/o/r",
            "https://evil.com/https://github.com/",
            "https://github.com/o/r\n",
            "https://github.com/o r",
            "javascript:alert(1)",
        ] {
            assert!(!is_release_page_url(url), "{url:?}");
        }
        let long = format!("https://github.com/{}", "a".repeat(MAX_PAGE_URL_LEN));
        assert!(!is_release_page_url(&long));
    }

    #[test]
    fn picks_highest_newer_release() {
        let releases = vec![
            release("0.1.0", false),
            release("0.3.0", false),
            release("0.2.0", false),
            release("0.4.0-beta.1", true),
        ];
        let current = v("0.1.0");
        let found = newest_upgrade(&current, &releases, false).map(|r| r.version);
        assert_eq!(found, Some(v("0.3.0")));
        let found = newest_upgrade(&current, &releases, true).map(|r| r.version);
        assert_eq!(found, Some(v("0.4.0-beta.1")));
    }

    #[test]
    fn no_upgrade_when_current_is_newest_or_list_empty() {
        let releases = vec![release("0.1.0", false), release("0.1.0-rc.1", true)];
        assert_eq!(newest_upgrade(&v("0.1.0"), &releases, true), None);
        assert_eq!(newest_upgrade(&v("0.2.0"), &releases, true), None);
        assert_eq!(newest_upgrade(&v("0.1.0"), &[], true), None);
    }

    #[test]
    fn flagged_prerelease_is_skipped_even_without_suffix() {
        let releases = vec![release("0.2.0", true)];
        assert_eq!(newest_upgrade(&v("0.1.0"), &releases, false), None);
        assert!(newest_upgrade(&v("0.1.0"), &releases, true).is_some());
    }

    #[test]
    fn prerelease_user_is_offered_stable_of_same_version() {
        let releases = vec![release("0.2.0", false)];
        let found = newest_upgrade(&v("0.2.0-rc.1"), &releases, false).map(|r| r.version);
        assert_eq!(found, Some(v("0.2.0")));
    }
}
