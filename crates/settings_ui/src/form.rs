//! 表单状态与纯逻辑：配置与界面草稿互转、校验问题归页、结果码说明。

use lenswin_core::config::Config;
use lenswin_core::hotkey::{HotkeyChoice, Modifiers};
use lenswin_core::ipc::IpcResult;

const MIB: u64 = 1024 * 1024;

/// 设置窗口的页面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// 常规。
    General,
    /// 快捷键。
    Hotkeys,
    /// 搜索与上传。
    SearchUpload,
    /// 截屏。
    Screenshot,
    /// 关于。
    About,
}

impl Page {
    /// 侧栏顺序。
    pub const ALL: [Page; 5] = [
        Page::General,
        Page::Hotkeys,
        Page::SearchUpload,
        Page::Screenshot,
        Page::About,
    ];

    /// 侧栏标题。
    pub const fn title(self) -> &'static str {
        match self {
            Page::General => "常规",
            Page::Hotkeys => "快捷键",
            Page::SearchUpload => "搜索与上传",
            Page::Screenshot => "截屏",
            Page::About => "关于",
        }
    }
}

/// 校验问题的配置路径所在的页面。
pub fn page_of_field(field: &str) -> Page {
    let section = field.split('.').next().unwrap_or_default();
    match section {
        "hotkeys" => Page::Hotkeys,
        "search" | "upload" | "privacy" => Page::SearchUpload,
        "screenshot" | "selection" => Page::Screenshot,
        _ => Page::General,
    }
}

/// 提交结果的提示级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackKind {
    /// 成功。
    Success,
    /// 已保存但有需要注意的地方。
    Warning,
    /// 失败。
    Error,
}

/// 展示在窗口底部的提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feedback {
    /// 级别。
    pub kind: FeedbackKind,
    /// 文字。
    pub text: String,
}

impl Feedback {
    pub(crate) fn new(kind: FeedbackKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }
}

/// 托盘进程返回的结果码对应的提示。
pub fn describe_result(result: IpcResult) -> Feedback {
    match result {
        IpcResult::Ok => Feedback::new(FeedbackKind::Success, "已应用"),
        IpcResult::HotkeyConflict => Feedback::new(
            FeedbackKind::Warning,
            "已保存，但有快捷键被其它程序占用，当前仍在使用旧的快捷键",
        ),
        IpcResult::ValidationFailed => {
            Feedback::new(FeedbackKind::Error, "主程序认为配置不合法，未应用")
        }
        IpcResult::WriteFailed => {
            Feedback::new(FeedbackKind::Error, "配置写入磁盘失败，重启后会丢失")
        }
        IpcResult::Busy => Feedback::new(FeedbackKind::Error, "主程序正忙，请稍后再试"),
        IpcResult::ParseFailed => Feedback::new(FeedbackKind::Error, "主程序无法解析提交的配置"),
        IpcResult::PayloadTooLarge => Feedback::new(FeedbackKind::Error, "配置数据过大"),
        IpcResult::InvalidMessage => Feedback::new(
            FeedbackKind::Error,
            "主程序无法识别这条消息，请确认版本一致",
        ),
    }
}

/// 把读到的配置文本变成配置；文件不存在时用默认值，文件有误返回原因。
pub fn config_from_file_text(text: Option<&str>) -> Result<Config, String> {
    match text {
        None => Ok(Config::default()),
        Some(text) => Config::from_toml_str(text).map_err(|e| e.to_string()),
    }
}

/// 提交前重新读取配置文件后，与打开窗口时读到的内容比较的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Staleness {
    /// 没有变化，或文件暂时读不出来（交给主程序判断）。
    Fresh,
    /// 文件内容已被别处修改（如托盘菜单切换了选项）。
    ChangedElsewhere,
}

/// 比较打开窗口时的配置 `baseline` 与此刻文件里的配置 `now`。
pub fn staleness(baseline: &Config, now: &Result<Config, String>) -> Staleness {
    match now {
        Ok(current) if current != baseline => Staleness::ChangedElsewhere,
        Ok(_) | Err(_) => Staleness::Fresh,
    }
}

/// 界面上的可编辑草稿：配置本体加几个无法直接绑定控件的字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// 绑定到大部分控件的配置。
    pub config: Config,
    /// 圈选搜索热键的编辑状态。
    pub search_hotkey: HotkeyChoice,
    /// 截屏热键的编辑状态。
    pub screenshot_hotkey: HotkeyChoice,
    /// 上传大小上限（MiB），仅在用户改动后才覆盖配置里的字节数。
    pub max_mib: u64,
    initial_max_mib: u64,
    /// 搜索引擎是否使用自定义模板。
    pub engine_custom: bool,
    /// 是否开机自启。
    pub autostart: bool,
    /// 配置里读不出的热键文字，界面用默认值顶替，提交时会被覆盖。
    pub hotkey_fallback: bool,
}

/// 无法解析的热键在界面里的占位。
fn fallback_choice(default_text: &str) -> HotkeyChoice {
    HotkeyChoice::parse(default_text).unwrap_or(HotkeyChoice {
        ctrl: true,
        alt: true,
        shift: false,
        win: false,
        key: lenswin_core::hotkey::Key::Letter('A'),
    })
}

impl Draft {
    /// 由当前配置和自启状态建立草稿。
    pub fn from_config(config: &Config, autostart: bool) -> Self {
        let defaults = Config::default().hotkeys;
        let search = HotkeyChoice::parse(&config.hotkeys.search);
        let shot = HotkeyChoice::parse(&config.hotkeys.screenshot_copy);
        let hotkey_fallback = search.is_none() || shot.is_none();
        let max_mib = config.upload.max_bytes.div_ceil(MIB).max(1);
        Self {
            config: config.clone(),
            search_hotkey: search.unwrap_or_else(|| fallback_choice(&defaults.search)),
            screenshot_hotkey: shot.unwrap_or_else(|| fallback_choice(&defaults.screenshot_copy)),
            max_mib,
            initial_max_mib: max_mib,
            engine_custom: lenswin_core::config::preset_index_of(
                &config.search.engine_url_template,
            )
            .is_none(),
            autostart,
            hotkey_fallback,
        }
    }

    /// 合成要提交的配置；热键没有修饰键时返回面向用户的说明。
    pub fn to_config(&self) -> Result<Config, String> {
        let mut config = self.config.clone();
        config.hotkeys.search = self
            .search_hotkey
            .to_text()
            .map_err(|_| "圈选搜索的快捷键至少要勾选一个修饰键".to_string())?;
        config.hotkeys.screenshot_copy = self
            .screenshot_hotkey
            .to_text()
            .map_err(|_| "截屏的快捷键至少要勾选一个修饰键".to_string())?;
        if self.max_mib != self.initial_max_mib {
            config.upload.max_bytes = self.max_mib.saturating_mul(MIB);
        }
        Ok(config)
    }
}

/// 两个热键相同（含修饰键集合）时返回说明，提交前先拦下。
pub fn hotkey_clash(a: &HotkeyChoice, b: &HotkeyChoice) -> Option<&'static str> {
    let same_mods = a.modifiers() == b.modifiers() && a.modifiers() != Modifiers::NONE;
    (same_mods && a.key == b.key).then_some("两个功能不能使用同一个快捷键")
}

/// 两个热键输入框之一。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyField {
    /// 圈选搜索。
    Search,
    /// 截屏到剪贴板。
    Screenshot,
}

/// 按键小方块上的文字：Ctrl、Alt、Shift、Win 依次排在主键前。
pub fn hotkey_cap_labels(choice: &HotkeyChoice) -> Vec<String> {
    [
        (choice.ctrl, "Ctrl"),
        (choice.alt, "Alt"),
        (choice.shift, "Shift"),
        (choice.win, "Win"),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, name)| name.to_string())
    .chain(std::iter::once(choice.key.to_string()))
    .collect()
}

/// 热键的展示文字，如 `Win+Shift+A`。
pub fn hotkey_display(choice: &HotkeyChoice) -> String {
    hotkey_cap_labels(choice).join("+")
}

/// 提交后被托盘报告冲突时，推断是哪些热键改动导致：与旧配置相比变了的那些。
pub fn changed_hotkeys(old: &Config, new: &Config) -> Vec<(HotkeyField, HotkeyChoice)> {
    let pairs = [
        (
            HotkeyField::Search,
            &old.hotkeys.search,
            &new.hotkeys.search,
        ),
        (
            HotkeyField::Screenshot,
            &old.hotkeys.screenshot_copy,
            &new.hotkeys.screenshot_copy,
        ),
    ];
    pairs
        .into_iter()
        .filter_map(|(field, before, after)| {
            let after = HotkeyChoice::parse(after)?;
            (HotkeyChoice::parse(before) != Some(after)).then_some((field, after))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lenswin_core::config::{UploadProvider, validate};

    #[test]
    fn cap_labels_list_modifiers_in_fixed_order_then_key() {
        let a = HotkeyChoice::parse("Ctrl+Alt+F13").expect("解析");
        assert_eq!(hotkey_cap_labels(&a), ["Ctrl", "Alt", "F13"]);
        let b = HotkeyChoice::parse("Shift+Win+A").expect("解析");
        assert_eq!(hotkey_cap_labels(&b), ["Shift", "Win", "A"]);
        assert_eq!(hotkey_display(&b), "Shift+Win+A");
        let c = HotkeyChoice::parse("Win+Ctrl+Alt+Shift+Space").expect("解析");
        assert_eq!(hotkey_cap_labels(&c).len(), 5);
        let none = HotkeyChoice {
            ctrl: false,
            alt: false,
            shift: false,
            win: false,
            key: lenswin_core::hotkey::Key::Letter('Z'),
        };
        assert_eq!(hotkey_cap_labels(&none), ["Z"]);
    }

    #[test]
    fn changed_hotkeys_reports_only_real_changes() {
        let old = Config::default();
        assert!(changed_hotkeys(&old, &old).is_empty());
        let mut new = old.clone();
        new.hotkeys.search = "Ctrl+Alt+F13".to_string();
        let changed = changed_hotkeys(&old, &new);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].0, HotkeyField::Search);
        assert_eq!(hotkey_display(&changed[0].1), "Ctrl+Alt+F13");
        new.hotkeys.screenshot_copy = "garbage".to_string();
        assert_eq!(changed_hotkeys(&old, &new).len(), 1);
    }

    #[test]
    fn draft_roundtrips_default_config_unchanged_apart_from_canonical_hotkeys() {
        let config = Config::default();
        let draft = Draft::from_config(&config, false);
        let back = draft.to_config().expect("合成");
        // 热键输出为固定修饰键顺序，其余字段保持不变
        let mut expected = config;
        expected.hotkeys.search = "Shift+Win+A".to_string();
        expected.hotkeys.screenshot_copy = "Shift+Win+C".to_string();
        assert_eq!(back, expected);
        assert_eq!(validate(&back), Ok(()));
    }

    #[test]
    fn untouched_odd_max_bytes_is_preserved() {
        let mut config = Config::default();
        config.upload.max_bytes = 1_500_000;
        let mut draft = Draft::from_config(&config, false);
        assert_eq!(draft.to_config().expect("合成").upload.max_bytes, 1_500_000);
        draft.max_mib = 3;
        assert_eq!(draft.to_config().expect("合成").upload.max_bytes, 3 * MIB);
    }

    #[test]
    fn hotkey_edits_are_written_back() {
        let mut draft = Draft::from_config(&Config::default(), false);
        draft.search_hotkey.win = false;
        draft.search_hotkey.shift = false;
        draft.search_hotkey.ctrl = true;
        draft.search_hotkey.alt = true;
        draft.search_hotkey.key = lenswin_core::hotkey::Key::Function(13);
        let config = draft.to_config().expect("合成");
        assert_eq!(config.hotkeys.search, "Ctrl+Alt+F13");
    }

    #[test]
    fn missing_modifier_is_a_user_facing_error() {
        let mut draft = Draft::from_config(&Config::default(), false);
        draft.screenshot_hotkey.win = false;
        draft.screenshot_hotkey.shift = false;
        let err = draft.to_config().expect_err("应失败");
        assert!(err.contains("截屏"), "{err}");
    }

    #[test]
    fn unparsable_hotkey_text_falls_back_and_is_flagged() {
        let mut config = Config::default();
        config.hotkeys.search = "oops".to_string();
        let draft = Draft::from_config(&config, true);
        assert!(draft.hotkey_fallback);
        assert!(draft.autostart);
        assert!(draft.to_config().is_ok());
    }

    #[test]
    fn engine_custom_follows_template() {
        let mut config = Config::default();
        assert!(!Draft::from_config(&config, false).engine_custom);
        config.search.engine_url_template = "https://example.com/?u={url}".to_string();
        assert!(Draft::from_config(&config, false).engine_custom);
        config.upload.provider = UploadProvider::Custom;
        assert!(
            Draft::from_config(&config, false).config.upload.provider == UploadProvider::Custom
        );
    }

    #[test]
    fn every_validation_field_maps_to_a_page() {
        let fields = [
            ("hotkeys.search", Page::Hotkeys),
            ("hotkeys.screenshot_copy", Page::Hotkeys),
            ("search.engine_url_template", Page::SearchUpload),
            ("upload.timeout_secs", Page::SearchUpload),
            ("upload.custom.endpoint", Page::SearchUpload),
            ("privacy.acknowledged_host", Page::SearchUpload),
            ("selection.lasso.padding", Page::Screenshot),
            ("screenshot.save_dir", Page::Screenshot),
            ("general.notifications", Page::General),
            ("unknown", Page::General),
        ];
        for (field, page) in fields {
            assert_eq!(page_of_field(field), page, "{field}");
        }
    }

    #[test]
    fn every_result_code_has_distinct_feedback_and_ok_is_success() {
        let all = [
            IpcResult::Ok,
            IpcResult::InvalidMessage,
            IpcResult::PayloadTooLarge,
            IpcResult::ParseFailed,
            IpcResult::ValidationFailed,
            IpcResult::HotkeyConflict,
            IpcResult::WriteFailed,
            IpcResult::Busy,
        ];
        let texts: Vec<_> = all.iter().map(|r| describe_result(*r).text).collect();
        let mut sorted = texts.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len());
        assert_eq!(describe_result(IpcResult::Ok).kind, FeedbackKind::Success);
        assert_eq!(
            describe_result(IpcResult::HotkeyConflict).kind,
            FeedbackKind::Warning
        );
        assert_eq!(
            describe_result(IpcResult::WriteFailed).kind,
            FeedbackKind::Error
        );
    }

    #[test]
    fn file_text_missing_means_defaults_and_broken_is_reported() {
        assert_eq!(config_from_file_text(None), Ok(Config::default()));
        assert!(config_from_file_text(Some("garbage ===")).is_err());
        assert!(config_from_file_text(Some("")).is_ok());
    }

    #[test]
    fn staleness_detects_only_real_changes() {
        let baseline = Config::default();
        assert_eq!(
            staleness(&baseline, &Ok(Config::default())),
            Staleness::Fresh
        );
        let mut changed = Config::default();
        changed.privacy.acknowledged_host = "litterbox.catbox.moe".to_string();
        assert_eq!(
            staleness(&baseline, &Ok(changed)),
            Staleness::ChangedElsewhere
        );
        assert_eq!(
            staleness(&baseline, &Err("文件有误".to_string())),
            Staleness::Fresh
        );
    }

    #[test]
    fn identical_hotkeys_clash_but_different_do_not() {
        let a = HotkeyChoice::parse("Ctrl+A").expect("解析");
        let b = HotkeyChoice::parse("Ctrl+A").expect("解析");
        let c = HotkeyChoice::parse("Ctrl+B").expect("解析");
        assert!(hotkey_clash(&a, &b).is_some());
        assert!(hotkey_clash(&a, &c).is_none());
    }
}
