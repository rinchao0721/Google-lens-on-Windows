//! 配置结构、默认值、取值范围与读写。校验规则见 [`validate`]。

mod validate;

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;
use crate::selection::SelectionMode;

pub use validate::{ValidationIssue, is_http_url, validate};

/// 上传超时（秒）的合法范围。
pub const TIMEOUT_SECS_RANGE: RangeInclusive<u64> = 1..=300;
/// 单次上传大小上限（字节）的合法范围。
pub const MAX_BYTES_RANGE: RangeInclusive<u64> = 1024..=1_073_741_824;
/// 套索外扩边距（像素）的合法范围。
pub const LASSO_PADDING_RANGE: RangeInclusive<u32> = 0..=200;
/// 套索线宽（像素）的合法范围。
pub const LASSO_STROKE_WIDTH_RANGE: RangeInclusive<u32> = 1..=50;

/// 搜索引擎模板中图片地址的占位符。
pub const URL_PLACEHOLDER: &str = "{url}";

/// 预置搜索引擎。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchEnginePreset {
    /// 显示名称。
    pub name: &'static str,
    /// 含 `{url}` 占位符的地址模板。
    pub url_template: &'static str,
}

/// 预置搜索引擎列表，第一项为默认。
pub const SEARCH_ENGINE_PRESETS: [SearchEnginePreset; 4] = [
    SearchEnginePreset {
        name: "Google Lens",
        url_template: "https://lens.google.com/uploadbyurl?url={url}",
    },
    SearchEnginePreset {
        name: "Bing",
        url_template: "https://www.bing.com/images/search?view=detailv2&iss=sbi&q=imgurl:{url}",
    },
    SearchEnginePreset {
        name: "Yandex",
        url_template: "https://yandex.com/images/search?rpt=imageview&url={url}",
    },
    SearchEnginePreset {
        name: "TinEye",
        url_template: "https://tineye.com/search?url={url}",
    },
];

/// 地址模板恰好等于某个预置时返回其序号；自定义模板返回 `None`。
pub fn preset_index_of(template: &str) -> Option<usize> {
    SEARCH_ENGINE_PRESETS
        .iter()
        .position(|preset| preset.url_template == template)
}

/// 顶层配置。缺失的段和字段使用默认值，未知字段视为错误。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// 常规。
    pub general: GeneralConfig,
    /// 快捷键。
    pub hotkeys: HotkeysConfig,
    /// 搜索。
    pub search: SearchConfig,
    /// 上传。
    pub upload: UploadConfig,
    /// 选区。
    pub selection: SelectionConfig,
    /// 截屏。
    pub screenshot: ScreenshotConfig,
    /// 隐私。
    pub privacy: PrivacyConfig,
}

impl Config {
    /// 解析并校验 TOML 文本；解析失败或校验不通过都返回错误，不返回半成品。
    pub fn from_toml_str(text: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(text)?;
        validate(&config).map_err(ConfigError::Invalid)?;
        Ok(config)
    }

    /// 序列化为 TOML 文本。
    pub fn to_toml_string(&self) -> Result<String, ConfigError> {
        Ok(toml::to_string_pretty(self)?)
    }
}

/// `[general]`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneralConfig {
    /// 是否弹出成功类提示；关闭后错误和警告仍会提示。
    pub notifications: bool,
    /// 左键单击托盘图标的动作。
    pub tray_left_click: TrayLeftClick,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            notifications: true,
            tray_left_click: TrayLeftClick::default(),
        }
    }
}

/// 左键单击托盘图标时执行的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayLeftClick {
    /// 圈选搜索。
    #[default]
    Search,
    /// 截屏到剪贴板。
    ScreenshotCopy,
    /// 不做任何事。
    #[serde(rename = "none")]
    Nothing,
}

impl TrayLeftClick {
    /// 全部取值，供界面生成选项。
    pub const ALL: [TrayLeftClick; 3] = [
        TrayLeftClick::Search,
        TrayLeftClick::ScreenshotCopy,
        TrayLeftClick::Nothing,
    ];

    /// 面向用户的中文名称。
    pub const fn label(self) -> &'static str {
        match self {
            TrayLeftClick::Search => "即圈即搜",
            TrayLeftClick::ScreenshotCopy => "截屏到剪贴板",
            TrayLeftClick::Nothing => "无操作",
        }
    }
}

/// `[hotkeys]`：快捷键字符串，语法见 [`crate::hotkey`]。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HotkeysConfig {
    /// 圈选搜索。
    pub search: String,
    /// 截屏到剪贴板。
    pub screenshot_copy: String,
}

impl Default for HotkeysConfig {
    fn default() -> Self {
        Self {
            search: "Win+Shift+A".to_string(),
            screenshot_copy: "Win+Shift+C".to_string(),
        }
    }
}

/// `[search]`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchConfig {
    /// 搜索引擎地址模板，必须恰好含一个 `{url}`。
    pub engine_url_template: String,
    /// 默认选区模式。
    pub selection_mode: SelectionMode,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            engine_url_template: SEARCH_ENGINE_PRESETS[0].url_template.to_string(),
            selection_mode: SelectionMode::default(),
        }
    }
}

/// 图床类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UploadProvider {
    /// litterbox 临时图床。
    #[default]
    Litterbox,
    /// 自定义端点。
    Custom,
}

/// `[upload]`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UploadConfig {
    /// 使用的图床。
    pub provider: UploadProvider,
    /// 超时秒数。
    pub timeout_secs: u64,
    /// 上传大小上限（字节）。
    pub max_bytes: u64,
    /// litterbox 设置。
    pub litterbox: LitterboxConfig,
    /// 自定义端点设置，`provider = "custom"` 时生效。
    pub custom: CustomUploadConfig,
}

impl Default for UploadConfig {
    fn default() -> Self {
        Self {
            provider: UploadProvider::default(),
            timeout_secs: 30,
            max_bytes: 104_857_600,
            litterbox: LitterboxConfig::default(),
            custom: CustomUploadConfig::default(),
        }
    }
}

/// litterbox 图片保留时间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LitterboxExpiry {
    /// 1 小时。
    #[default]
    #[serde(rename = "1h")]
    Hours1,
    /// 12 小时。
    #[serde(rename = "12h")]
    Hours12,
    /// 24 小时。
    #[serde(rename = "24h")]
    Hours24,
    /// 72 小时。
    #[serde(rename = "72h")]
    Hours72,
}

impl LitterboxExpiry {
    /// 全部取值，供界面生成选项。
    pub const ALL: [LitterboxExpiry; 4] = [
        LitterboxExpiry::Hours1,
        LitterboxExpiry::Hours12,
        LitterboxExpiry::Hours24,
        LitterboxExpiry::Hours72,
    ];

    /// 面向用户的中文名称。
    pub const fn label(self) -> &'static str {
        match self {
            LitterboxExpiry::Hours1 => "1 小时",
            LitterboxExpiry::Hours12 => "12 小时",
            LitterboxExpiry::Hours24 => "24 小时",
            LitterboxExpiry::Hours72 => "72 小时",
        }
    }

    /// 图床接口使用的取值，如 `1h`。
    pub const fn as_str(self) -> &'static str {
        match self {
            LitterboxExpiry::Hours1 => "1h",
            LitterboxExpiry::Hours12 => "12h",
            LitterboxExpiry::Hours24 => "24h",
            LitterboxExpiry::Hours72 => "72h",
        }
    }
}

/// `[upload.litterbox]`。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LitterboxConfig {
    /// 保留时间。
    pub expiry: LitterboxExpiry,
}

/// 自定义端点的响应格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResponseKind {
    /// JSON，用 `json_pointer` 取 URL。
    #[default]
    Json,
    /// 响应正文即 URL。
    Text,
}

/// `[upload.custom]`，默认值等价于 uguu.se。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CustomUploadConfig {
    /// 上传端点。
    pub endpoint: String,
    /// multipart 文件字段名。
    pub file_field: String,
    /// 响应格式。
    pub response: ResponseKind,
    /// RFC 6901 JSON 指针，`response = "json"` 时必填。
    pub json_pointer: String,
}

impl Default for CustomUploadConfig {
    fn default() -> Self {
        Self {
            endpoint: "https://uguu.se/upload".to_string(),
            file_field: "files[]".to_string(),
            response: ResponseKind::default(),
            json_pointer: "/files/0/url".to_string(),
        }
    }
}

/// `[selection]`。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SelectionConfig {
    /// 套索参数。
    pub lasso: LassoConfig,
}

/// `[selection.lasso]`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LassoConfig {
    /// 外接矩形外扩像素。
    pub padding: u32,
    /// 描边宽度像素。
    pub stroke_width: u32,
}

impl Default for LassoConfig {
    fn default() -> Self {
        Self {
            padding: 10,
            stroke_width: 4,
        }
    }
}

/// `[screenshot]`。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScreenshotConfig {
    /// 保存目录，为空则只复制到剪贴板。
    pub save_dir: String,
}

/// `[privacy]`。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrivacyConfig {
    /// 用户已确认过的图床主机名，为空表示尚未确认。
    pub acknowledged_host: String,
}

impl PrivacyConfig {
    /// 用户是否已确认过向该主机上传图片；主机名不区分大小写。
    pub fn is_acknowledged(&self, host: &str) -> bool {
        let known = self.acknowledged_host.trim();
        !known.is_empty() && known.eq_ignore_ascii_case(host.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acknowledgement_matches_host_case_insensitively() {
        let privacy = PrivacyConfig {
            acknowledged_host: " Litterbox.Catbox.moe ".to_string(),
        };
        assert!(privacy.is_acknowledged("litterbox.catbox.moe"));
        assert!(!privacy.is_acknowledged("uguu.se"));
        assert!(!privacy.is_acknowledged(""));
        assert!(!PrivacyConfig::default().is_acknowledged(""));
        assert!(!PrivacyConfig::default().is_acknowledged("uguu.se"));
    }

    #[test]
    fn preset_index_matches_only_exact_templates() {
        for (i, preset) in SEARCH_ENGINE_PRESETS.iter().enumerate() {
            assert_eq!(preset_index_of(preset.url_template), Some(i));
        }
        assert_eq!(preset_index_of("https://example.com/?u={url}"), None);
        assert_eq!(preset_index_of(""), None);
    }

    #[test]
    fn expiry_labels_are_distinct_and_match_all() {
        let labels: Vec<_> = LitterboxExpiry::ALL.iter().map(|e| e.label()).collect();
        let mut sorted = labels.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), LitterboxExpiry::ALL.len());
        assert_eq!(labels.first(), Some(&"1 小时"));
    }

    #[test]
    fn notifications_default_on_and_roundtrip_off() {
        assert!(Config::default().general.notifications);
        let parsed = Config::from_toml_str("[general]\nnotifications = false\n").expect("解析");
        assert!(!parsed.general.notifications);
        let text = parsed.to_toml_string().expect("序列化");
        assert_eq!(Config::from_toml_str(&text).expect("回读"), parsed);
        assert!(Config::from_toml_str("[general]\nbogus = 1\n").is_err());
    }

    #[test]
    fn tray_left_click_defaults_to_search_and_parses_every_value() {
        assert_eq!(
            Config::default().general.tray_left_click,
            TrayLeftClick::Search
        );
        for (text, expected) in [
            ("search", TrayLeftClick::Search),
            ("screenshot_copy", TrayLeftClick::ScreenshotCopy),
            ("none", TrayLeftClick::Nothing),
        ] {
            let toml = format!("[general]\ntray_left_click = \"{text}\"\n");
            let parsed = Config::from_toml_str(&toml).expect("解析");
            assert_eq!(parsed.general.tray_left_click, expected, "{text}");
            let back = parsed.to_toml_string().expect("序列化");
            assert!(
                back.contains(&format!("tray_left_click = \"{text}\"")),
                "{back}"
            );
        }
    }

    #[test]
    fn tray_left_click_rejects_unknown_value() {
        assert!(Config::from_toml_str("[general]\ntray_left_click = \"nothing\"\n").is_err());
        assert!(Config::from_toml_str("[general]\ntray_left_click = 1\n").is_err());
    }

    #[test]
    fn tray_left_click_labels_are_distinct() {
        let mut labels: Vec<_> = TrayLeftClick::ALL.iter().map(|t| t.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), TrayLeftClick::ALL.len());
    }

    #[test]
    fn default_config_is_valid() {
        assert_eq!(validate(&Config::default()), Ok(()));
    }

    #[test]
    fn default_presets_are_valid_templates() {
        for preset in SEARCH_ENGINE_PRESETS {
            let mut config = Config::default();
            config.search.engine_url_template = preset.url_template.to_string();
            assert_eq!(validate(&config), Ok(()), "{}", preset.name);
        }
    }

    #[test]
    fn toml_roundtrip_default() {
        let config = Config::default();
        let text = config.to_toml_string().expect("序列化");
        assert_eq!(Config::from_toml_str(&text).expect("反序列化"), config);
    }

    #[test]
    fn toml_roundtrip_non_default() {
        let mut config = Config::default();
        config.hotkeys.search = "Ctrl+Alt+S".to_string();
        config.search.selection_mode = SelectionMode::Rect;
        config.upload.provider = UploadProvider::Custom;
        config.upload.litterbox.expiry = LitterboxExpiry::Hours72;
        config.upload.custom.response = ResponseKind::Text;
        config.upload.custom.json_pointer = String::new();
        config.selection.lasso.padding = 20;
        config.screenshot.save_dir = "D:\\shots".to_string();
        config.privacy.acknowledged_host = "litterbox.catbox.moe".to_string();
        let text = config.to_toml_string().expect("序列化");
        assert_eq!(Config::from_toml_str(&text).expect("反序列化"), config);
    }

    #[test]
    fn empty_text_yields_defaults() {
        assert_eq!(
            Config::from_toml_str("").expect("空配置"),
            Config::default()
        );
    }

    #[test]
    fn partial_text_fills_defaults() {
        let config = Config::from_toml_str("[selection.lasso]\npadding = 30\n").expect("部分配置");
        assert_eq!(config.selection.lasso.padding, 30);
        assert_eq!(config.selection.lasso.stroke_width, 4);
        assert_eq!(config.hotkeys, HotkeysConfig::default());
    }

    #[test]
    fn unknown_field_is_rejected() {
        let err = Config::from_toml_str("[hotkeys]\nsearhc = \"Ctrl+A\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
        let err = Config::from_toml_str("[nonsense]\nx = 1\n").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn wrong_type_is_rejected() {
        let err = Config::from_toml_str("[upload]\ntimeout_secs = \"30\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
        let err = Config::from_toml_str("[selection.lasso]\npadding = -1\n").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn invalid_enum_value_is_rejected() {
        let err = Config::from_toml_str("[upload.litterbox]\nexpiry = \"5h\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
        let err = Config::from_toml_str("[search]\nselection_mode = \"circle\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn invalid_values_report_validation_issues() {
        let err = Config::from_toml_str("[upload]\ntimeout_secs = 0\n").unwrap_err();
        match err {
            ConfigError::Invalid(issues) => {
                assert_eq!(issues.len(), 1);
                assert_eq!(issues.first().map(|i| i.field), Some("upload.timeout_secs"));
            }
            other => panic!("期望校验错误，实际为 {other:?}"),
        }
    }

    #[test]
    fn litterbox_expiry_strings_match_serde() {
        for expiry in LitterboxExpiry::ALL {
            let text = format!("[upload.litterbox]\nexpiry = \"{}\"\n", expiry.as_str());
            let config = Config::from_toml_str(&text).expect("解析");
            assert_eq!(config.upload.litterbox.expiry, expiry);
        }
    }
}
