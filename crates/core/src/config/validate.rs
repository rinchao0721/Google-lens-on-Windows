//! 配置校验：规则只在这里定义，托盘与设置窗口共用。

use std::ops::RangeInclusive;

use super::{
    Config, LASSO_PADDING_RANGE, LASSO_STROKE_WIDTH_RANGE, MAX_BYTES_RANGE, ResponseKind,
    TIMEOUT_SECS_RANGE, URL_PLACEHOLDER, UploadProvider,
};
use crate::hotkey::Hotkey;

/// 一条校验问题，`field` 为点分隔的配置路径，便于界面就地标红。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    /// 出问题的配置项路径，如 `upload.timeout_secs`。
    pub field: &'static str,
    /// 面向用户的说明。
    pub message: String,
}

impl ValidationIssue {
    fn new(field: &'static str, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }
}

/// 是否为带非空主机的 http/https 地址；含空白或控制字符一律不合法。
pub fn is_http_url(text: &str) -> bool {
    if text.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    let Some(rest) = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    !host.is_empty() && !host.starts_with(':')
}

/// 校验整份配置，收集全部问题；通过返回 `Ok(())`。
pub fn validate(config: &Config) -> Result<(), Vec<ValidationIssue>> {
    let mut issues = Vec::new();
    check_hotkeys(config, &mut issues);
    check_search(config, &mut issues);
    check_upload(config, &mut issues);
    check_selection(config, &mut issues);
    check_misc(config, &mut issues);
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

fn check_range<T: PartialOrd + std::fmt::Display>(
    field: &'static str,
    value: T,
    range: &RangeInclusive<T>,
    issues: &mut Vec<ValidationIssue>,
) {
    if !range.contains(&value) {
        issues.push(ValidationIssue::new(
            field,
            format!("取值应在 {} 到 {} 之间", range.start(), range.end()),
        ));
    }
}

/// 配置中所有快捷键的「(字段路径, 字符串)」列表；新增快捷键只需在此登记。
fn hotkey_entries(config: &Config) -> [(&'static str, &str); 2] {
    [
        ("hotkeys.search", config.hotkeys.search.as_str()),
        (
            "hotkeys.screenshot_copy",
            config.hotkeys.screenshot_copy.as_str(),
        ),
    ]
}

fn check_hotkeys(config: &Config, issues: &mut Vec<ValidationIssue>) {
    check_hotkey_entries(&hotkey_entries(config), issues);
}

/// 逐个解析后两两比较；重复时报告后出现的字段，并指明与哪一项冲突。
fn check_hotkey_entries(entries: &[(&'static str, &str)], issues: &mut Vec<ValidationIssue>) {
    let mut seen: Vec<(&'static str, Hotkey)> = Vec::new();
    for &(field, text) in entries {
        match text.parse::<Hotkey>() {
            Ok(hotkey) => {
                if let Some((first, _)) = seen.iter().find(|(_, h)| *h == hotkey) {
                    issues.push(ValidationIssue::new(
                        field,
                        format!("与 {first} 的快捷键重复"),
                    ));
                } else {
                    seen.push((field, hotkey));
                }
            }
            Err(e) => issues.push(ValidationIssue::new(field, e.to_string())),
        }
    }
}

fn check_search(config: &Config, issues: &mut Vec<ValidationIssue>) {
    let template = &config.search.engine_url_template;
    let field = "search.engine_url_template";
    if template.matches(URL_PLACEHOLDER).count() != 1 {
        issues.push(ValidationIssue::new(
            field,
            format!("必须恰好包含一个 {URL_PLACEHOLDER}"),
        ));
    } else if !is_http_url(&template.replace(URL_PLACEHOLDER, "x")) {
        issues.push(ValidationIssue::new(field, "必须是 http 或 https 地址"));
    }
}

fn check_upload(config: &Config, issues: &mut Vec<ValidationIssue>) {
    let upload = &config.upload;
    check_range(
        "upload.timeout_secs",
        upload.timeout_secs,
        &TIMEOUT_SECS_RANGE,
        issues,
    );
    check_range(
        "upload.max_bytes",
        upload.max_bytes,
        &MAX_BYTES_RANGE,
        issues,
    );

    if upload.provider != UploadProvider::Custom {
        return;
    }
    let custom = &upload.custom;
    if !is_http_url(&custom.endpoint) {
        issues.push(ValidationIssue::new(
            "upload.custom.endpoint",
            "必须是 http 或 https 地址",
        ));
    }
    if custom.file_field.trim().is_empty() {
        issues.push(ValidationIssue::new("upload.custom.file_field", "不能为空"));
    }
    if custom.response == ResponseKind::Json && !is_json_pointer(&custom.json_pointer) {
        issues.push(ValidationIssue::new(
            "upload.custom.json_pointer",
            "response 为 json 时必须是以 / 开头的 JSON 指针",
        ));
    }
}

/// RFC 6901：以 `/` 开头，`~` 之后只能是 `0` 或 `1`。
fn is_json_pointer(text: &str) -> bool {
    if !text.starts_with('/') {
        return false;
    }
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return false;
        }
    }
    true
}

fn check_selection(config: &Config, issues: &mut Vec<ValidationIssue>) {
    let lasso = &config.selection.lasso;
    check_range(
        "selection.lasso.padding",
        lasso.padding,
        &LASSO_PADDING_RANGE,
        issues,
    );
    check_range(
        "selection.lasso.stroke_width",
        lasso.stroke_width,
        &LASSO_STROKE_WIDTH_RANGE,
        issues,
    );
}

fn check_misc(config: &Config, issues: &mut Vec<ValidationIssue>) {
    if config.screenshot.save_dir.chars().any(char::is_control) {
        issues.push(ValidationIssue::new(
            "screenshot.save_dir",
            "路径含有非法字符",
        ));
    }
    let host = &config.privacy.acknowledged_host;
    if host
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '/')
    {
        issues.push(ValidationIssue::new(
            "privacy.acknowledged_host",
            "应为纯主机名，不能含空白或 /",
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(config: &Config) -> Vec<&'static str> {
        match validate(config) {
            Ok(()) => Vec::new(),
            Err(issues) => issues.iter().map(|i| i.field).collect(),
        }
    }

    fn with_template(t: &str) -> Config {
        let mut c = Config::default();
        c.search.engine_url_template = t.to_string();
        c
    }

    #[test]
    fn http_url_accepts_and_rejects() {
        assert!(is_http_url("https://example.com/a?b=1"));
        assert!(is_http_url("HTTP://Example.com"));
        assert!(is_http_url("http://127.0.0.1:8080/x"));
        assert!(!is_http_url("ftp://example.com"));
        assert!(!is_http_url("javascript:alert(1)"));
        assert!(!is_http_url("https://"));
        assert!(!is_http_url("https:///path"));
        assert!(!is_http_url("https://:80/"));
        assert!(!is_http_url("https://exa mple.com"));
        assert!(!is_http_url("https://example.com/\n"));
        assert!(!is_http_url(""));
    }

    #[test]
    fn template_requires_exactly_one_placeholder() {
        assert!(fields(&with_template("https://e.com/?u={url}")).is_empty());
        assert_eq!(
            fields(&with_template("https://e.com/?u=1")),
            vec!["search.engine_url_template"]
        );
        assert_eq!(
            fields(&with_template("https://e.com/?u={url}&v={url}")),
            vec!["search.engine_url_template"]
        );
    }

    #[test]
    fn template_requires_http_scheme() {
        assert_eq!(
            fields(&with_template("ftp://e.com/?u={url}")),
            vec!["search.engine_url_template"]
        );
        assert_eq!(
            fields(&with_template("{url}")),
            vec!["search.engine_url_template"]
        );
    }

    #[test]
    fn numeric_bounds_are_enforced() {
        let mut c = Config::default();
        c.upload.timeout_secs = 0;
        assert_eq!(fields(&c), vec!["upload.timeout_secs"]);
        c.upload.timeout_secs = 301;
        assert_eq!(fields(&c), vec!["upload.timeout_secs"]);
        c.upload.timeout_secs = 300;
        assert!(fields(&c).is_empty());

        let mut c = Config::default();
        c.upload.max_bytes = 0;
        assert_eq!(fields(&c), vec!["upload.max_bytes"]);
        c.upload.max_bytes = u64::MAX;
        assert_eq!(fields(&c), vec!["upload.max_bytes"]);

        let mut c = Config::default();
        c.selection.lasso.padding = 201;
        c.selection.lasso.stroke_width = 0;
        assert_eq!(
            fields(&c),
            vec!["selection.lasso.padding", "selection.lasso.stroke_width"]
        );
    }

    #[test]
    fn custom_fields_only_checked_for_custom_provider() {
        let mut c = Config::default();
        c.upload.custom.endpoint = "not a url".to_string();
        c.upload.custom.file_field = String::new();
        assert!(fields(&c).is_empty());

        c.upload.provider = UploadProvider::Custom;
        assert_eq!(
            fields(&c),
            vec!["upload.custom.endpoint", "upload.custom.file_field"]
        );
    }

    #[test]
    fn custom_json_pointer_rules() {
        let mut c = Config::default();
        c.upload.provider = UploadProvider::Custom;

        c.upload.custom.json_pointer = "files/0/url".to_string();
        assert_eq!(fields(&c), vec!["upload.custom.json_pointer"]);
        c.upload.custom.json_pointer = "/a~2b".to_string();
        assert_eq!(fields(&c), vec!["upload.custom.json_pointer"]);
        c.upload.custom.json_pointer = "/a~".to_string();
        assert_eq!(fields(&c), vec!["upload.custom.json_pointer"]);
        c.upload.custom.json_pointer = "/a~0b/c~1d".to_string();
        assert!(fields(&c).is_empty());

        c.upload.custom.response = ResponseKind::Text;
        c.upload.custom.json_pointer = String::new();
        assert!(fields(&c).is_empty());
    }

    #[test]
    fn hotkeys_must_parse() {
        let mut c = Config::default();
        c.hotkeys.search = "A".to_string();
        c.hotkeys.screenshot_copy = "Ctrl+Nope".to_string();
        assert_eq!(
            fields(&c),
            vec!["hotkeys.search", "hotkeys.screenshot_copy"]
        );
    }

    #[test]
    fn hotkeys_must_be_distinct_even_if_spelled_differently() {
        let mut c = Config::default();
        c.hotkeys.search = "Win+Shift+A".to_string();
        c.hotkeys.screenshot_copy = "shift + win + a".to_string();
        assert_eq!(fields(&c), vec!["hotkeys.screenshot_copy"]);
    }

    #[test]
    fn duplicate_among_three_reports_later_field_only() {
        let entries = [
            ("a", "Ctrl+Alt+A"),
            ("b", "Ctrl+Alt+B"),
            ("c", "alt + ctrl + a"),
        ];
        let mut issues = Vec::new();
        check_hotkey_entries(&entries, &mut issues);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues.first().map(|i| i.field), Some("c"));
        assert!(issues.first().is_some_and(|i| i.message.contains('a')));
    }

    #[test]
    fn duplicate_pairs_each_report_their_later_field() {
        let entries = [
            ("a", "Ctrl+A"),
            ("b", "Ctrl+B"),
            ("c", "Ctrl+A"),
            ("d", "Ctrl+B"),
            ("e", "bad"),
        ];
        let mut issues = Vec::new();
        check_hotkey_entries(&entries, &mut issues);
        let got: Vec<_> = issues.iter().map(|i| i.field).collect();
        assert_eq!(got, vec!["c", "d", "e"]);
    }

    #[test]
    fn misc_fields_reject_bad_characters() {
        let mut c = Config::default();
        c.screenshot.save_dir = "D:\\a\0b".to_string();
        c.privacy.acknowledged_host = "https://example.com/".to_string();
        assert_eq!(
            fields(&c),
            vec!["screenshot.save_dir", "privacy.acknowledged_host"]
        );
    }

    #[test]
    fn all_issues_are_collected() {
        let mut c = Config::default();
        c.upload.timeout_secs = 0;
        c.search.engine_url_template = String::new();
        c.hotkeys.search = String::new();
        assert_eq!(fields(&c).len(), 3);
    }
}
