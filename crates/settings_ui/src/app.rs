//! 设置窗口的界面与交互。

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use eframe::egui;
use lenswin_core::annotation::NumberStyle;
use lenswin_core::config::{
    ANNOTATE_WIDTH_RANGE, Config, GlowMode, HIGHLIGHTER_WIDTH_RANGE, HISTORY_SIZE_RANGE,
    LASSO_PADDING_RANGE, LASSO_STROKE_WIDTH_RANGE, LitterboxExpiry, MOSAIC_BLOCK_RANGE,
    NUMBER_SIZE_RANGE, ResponseKind, SEARCH_ENGINE_PRESETS, SearchBackend, TEXT_SIZE_RANGE,
    TIMEOUT_SECS_RANGE, TrayLeftClick, UploadProvider, ValidationIssue, validate,
};
use lenswin_core::hotkey::{HotkeyChoice, Key, selectable_keys};
use lenswin_core::selection::SelectionMode;
use lenswin_platform::autostart::{self, AutostartState};
use lenswin_platform::paths::AppPaths;

use crate::client;
use crate::fonts::FontStatus;
use crate::form::{
    Draft, Feedback, FeedbackKind, HotkeyField, HotkeyStatus, Page, ProbeState, Staleness,
    changed_hotkeys, config_from_file_text, hotkey_cap_labels, hotkey_clash, hotkey_status,
    page_of_field, staleness,
};
use crate::icons;
use crate::recorder::{self, Capture};
use crate::theme::{self, CARD_GAP, FONT_DESC, Look, NAV_WIDTH, PAGE_MARGIN_X, Palette, ThemeKind};
use crate::update_status::{UpdateChecker, UpdateStatus};
use crate::widgets::{
    BannerKind, Card, banner, choice_chip, color_swatches, group_label, key_caps, modifier_chip,
    nav_item, page_title, primary_button, record_button, secondary_button, text_input, toggle,
};

/// 上传大小上限的界面范围（MiB），与 `MAX_BYTES_RANGE` 的上下界一致。
const MAX_MIB_RANGE: std::ops::RangeInclusive<u64> = 1..=1024;

/// 窗口启动前收集到的信息。
pub struct Startup {
    config: Config,
    config_path: Option<PathBuf>,
    mode_label: String,
    load_error: Option<String>,
    autostart: AutostartState,
    exe: Option<PathBuf>,
}

impl Startup {
    /// 只读地收集配置、路径和自启状态；任何一步失败都降级并记入 `load_error`。
    pub fn collect() -> Self {
        let mut load_error = None;
        let (config_path, mode_label) = match AppPaths::detect() {
            Ok(paths) => (Some(paths.config_file()), paths.mode.label().to_string()),
            Err(e) => {
                load_error = Some(format!("无法确定配置位置：{e}"));
                (None, "未知".to_string())
            }
        };
        let config = match &config_path {
            Some(path) => match read_config(path) {
                Ok(config) => config,
                Err(message) => {
                    load_error = Some(message);
                    Config::default()
                }
            },
            None => Config::default(),
        };
        let exe = std::env::current_exe().ok();
        let autostart = exe
            .as_deref()
            .and_then(|exe| autostart::state(autostart::VALUE_NAME, exe).ok())
            .unwrap_or(AutostartState::Disabled);
        Self {
            config,
            config_path,
            mode_label,
            load_error,
            autostart,
            exe,
        }
    }
}

fn read_config(path: &std::path::Path) -> Result<Config, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => return Err(format!("无法读取配置文件：{e}")),
    };
    config_from_file_text(text.as_deref())
        .map_err(|e| format!("配置文件有误，下面显示的是默认值，提交会被主程序拒绝：{e}"))
}

/// 设置窗口。
pub struct SettingsApp {
    draft: Draft,
    page: Page,
    issues: Vec<ValidationIssue>,
    feedback: Option<Feedback>,
    updates: UpdateChecker,
    fonts: FontStatus,
    look: Look,
    theme_kind: ThemeKind,
    conflicts: Vec<(HotkeyField, HotkeyChoice)>,
    config_path: Option<PathBuf>,
    config_path_text: String,
    baseline: Config,
    stale_prompt: bool,
    mode_label: String,
    load_error: Option<String>,
    autostart_enabled: bool,
    autostart_stale: bool,
    exe: Option<PathBuf>,
    keys: Vec<Key>,
    recording: Option<HotkeyField>,
    probes: Vec<(String, ProbeState)>,
    pointer_clicked: bool,
    record_clicked: bool,
}

/// 点击底部按钮后要执行的动作。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonAction {
    Ok,
    Apply,
    Cancel,
    Reload,
    Overwrite,
}

/// 底部「确定 / 应用 / 取消」三个按钮占的宽度。
const BUTTONS_WIDTH: f32 = 76.0 * 3.0 + 8.0 * 2.0;
/// 冲突提示多出的两个按钮占的宽度。
const STALE_BUTTONS_WIDTH: f32 = 220.0;
/// 右侧控件区的常用预留宽度。
const COMBO_WIDTH: f32 = 190.0;
/// 文本框的常用宽度。
const FIELD_WIDTH: f32 = 190.0;

impl SettingsApp {
    /// 由启动信息建立窗口状态。
    pub fn new(startup: Startup, fonts: FontStatus, theme_kind: ThemeKind) -> Self {
        let autostart_enabled = startup.autostart == AutostartState::Enabled;
        let autostart_stale = matches!(startup.autostart, AutostartState::StalePath(_));
        Self {
            draft: Draft::from_config(&startup.config, autostart_enabled),
            page: Page::General,
            issues: Vec::new(),
            feedback: None,
            updates: UpdateChecker::new(),
            fonts,
            look: Look {
                palette: Palette::of(theme_kind),
                icons: fonts.icons,
            },
            theme_kind,
            conflicts: Vec::new(),
            baseline: startup.config.clone(),
            stale_prompt: false,
            config_path: startup.config_path.clone(),
            config_path_text: startup
                .config_path
                .as_ref()
                .map_or_else(|| "未知".to_string(), |p| p.display().to_string()),
            mode_label: startup.mode_label,
            load_error: startup.load_error,
            autostart_enabled,
            autostart_stale,
            exe: startup.exe,
            keys: selectable_keys(),
            recording: None,
            probes: Vec::new(),
            pointer_clicked: false,
            record_clicked: false,
        }
    }

    /// 某配置字段上的校验问题文字。
    fn issue(&self, field: &str) -> Option<String> {
        self.issues
            .iter()
            .find(|i| i.field == field)
            .map(|i| i.message.clone())
    }

    fn set_feedback(&mut self, kind: FeedbackKind, text: impl Into<String>) {
        self.feedback = Some(Feedback {
            kind,
            text: text.into(),
        });
    }

    /// 此刻文件里的配置；没有路径时返回错误。
    fn read_current(&self) -> Result<Config, String> {
        match &self.config_path {
            Some(path) => read_config(path),
            None => Err("无法确定配置位置".to_string()),
        }
    }

    /// 重新读取配置文件并丢弃当前草稿里的修改（自启勾选保留）。
    fn reload_from_file(&mut self) {
        match self.read_current() {
            Ok(config) => {
                self.draft = Draft::from_config(&config, self.draft.autostart);
                self.baseline = config;
                self.issues.clear();
                self.conflicts.clear();
                self.probes.clear();
                self.stale_prompt = false;
                self.set_feedback(FeedbackKind::Success, "已重新载入配置文件");
            }
            Err(message) => self.set_feedback(FeedbackKind::Error, message),
        }
    }

    /// 校验并提交；`force` 为真时跳过「配置已在别处修改」的检查。返回是否完全成功。
    fn apply(&mut self, force: bool) -> bool {
        self.issues.clear();
        self.conflicts.clear();
        self.stale_prompt = false;
        let config = match self.draft.to_config() {
            Ok(config) => config,
            Err(message) => {
                self.page = Page::Hotkeys;
                self.set_feedback(FeedbackKind::Error, message);
                return false;
            }
        };
        if let Some(message) =
            hotkey_clash(&self.draft.search_hotkey, &self.draft.screenshot_hotkey)
        {
            self.page = Page::Hotkeys;
            self.set_feedback(FeedbackKind::Error, message);
            return false;
        }
        if let Err(issues) = validate(&config) {
            if let Some(first) = issues.first() {
                self.page = page_of_field(first.field);
            }
            let count = issues.len();
            self.issues = issues;
            self.set_feedback(
                FeedbackKind::Error,
                format!("有 {count} 项设置不合法，请按红字修改"),
            );
            return false;
        }
        if !force {
            let now = self.read_current();
            if staleness(&self.baseline, &now) == Staleness::ChangedElsewhere {
                self.stale_prompt = true;
                self.set_feedback(
                    FeedbackKind::Warning,
                    "配置已在别处修改（例如托盘菜单），直接应用会覆盖那些修改",
                );
                return false;
            }
        }
        let mut feedback = client::submit(&config);
        if let Some(problem) = self.apply_autostart() {
            feedback = Feedback {
                kind: FeedbackKind::Error,
                text: format!("{}；{problem}", feedback.text),
            };
        }
        let ok = feedback.kind == FeedbackKind::Success;
        // 警告级别只来自热键冲突：与旧配置相比变了的热键就是被占用的那些
        if feedback.kind == FeedbackKind::Warning {
            self.conflicts = changed_hotkeys(&self.baseline, &config);
        }
        // 成功或「已保存但热键冲突」时文件内容就是刚提交的配置，作为新的比较基准
        if feedback.kind != FeedbackKind::Error {
            self.baseline = config;
            // 托盘的热键已变，旧的探测结论作废
            self.probes.clear();
        }
        self.feedback = Some(feedback);
        ok
    }

    /// 让注册表里的自启项与草稿一致；失败返回说明。
    fn apply_autostart(&mut self) -> Option<String> {
        let wanted = self.draft.autostart;
        let in_sync = wanted == self.autostart_enabled && !(wanted && self.autostart_stale);
        if in_sync {
            return None;
        }
        let result = match (&self.exe, wanted) {
            (None, _) => return Some("无法确定程序路径，开机自启未改动".to_string()),
            (Some(exe), true) => autostart::enable(autostart::VALUE_NAME, exe),
            (Some(_), false) => autostart::disable(autostart::VALUE_NAME),
        };
        match result {
            Ok(()) => {
                self.autostart_enabled = wanted;
                self.autostart_stale = false;
                None
            }
            Err(e) => Some(format!("开机自启设置失败：{e}")),
        }
    }

    /// 跟随系统深浅色；主题变化时重装样式。
    fn sync_theme(&mut self, ctx: &egui::Context) {
        let kind = theme::resolve_system(ctx.system_theme());
        if kind != self.theme_kind {
            theme::install(ctx, kind);
            self.theme_kind = kind;
            self.look.palette = Palette::of(kind);
        }
    }

    fn general_page(&mut self, ui: &mut egui::Ui) {
        let look = self.look;
        let autostart_desc = if self.autostart_stale {
            "注册表里的自启项指向别的位置，打开并点「应用」即可更新到当前程序"
        } else {
            "登录 Windows 后自动启动 lenswin"
        };
        let autostart = &mut self.draft.autostart;
        Card::new("开机自启")
            .icon(icons::POWER)
            .desc(autostart_desc)
            .right_width(48.0)
            .show(ui, &look, |ui| {
                toggle(ui, &look, autostart);
            });
        let error = self.issue("general.notifications");
        let notifications = &mut self.draft.config.general.notifications;
        Card::new("显示提示气泡")
            .icon(icons::RINGER)
            .desc("关闭后只保留警告和错误")
            .error(error.as_deref())
            .right_width(48.0)
            .show(ui, &look, |ui| {
                toggle(ui, &look, notifications);
            });
        let error = self.issue("general.tray_left_click");
        let current = self.draft.config.general.tray_left_click;
        Card::new("左键单击托盘图标")
            .icon(icons::MOUSE)
            .desc("单击托盘图标时直接开始的操作")
            .error(error.as_deref())
            .right_width(COMBO_WIDTH)
            .show(ui, &look, |ui| {
                egui::ComboBox::from_id_salt("tray-left-click")
                    .width(COMBO_WIDTH)
                    .selected_text(current.label())
                    .show_ui(ui, |ui| {
                        for action in TrayLeftClick::ALL {
                            ui.selectable_value(
                                &mut self.draft.config.general.tray_left_click,
                                action,
                                action.label(),
                            );
                        }
                    });
            });
        let error = self.issue("update.auto_check");
        let auto_check = &mut self.draft.config.update.auto_check;
        Card::new("自动检查更新")
            .icon(icons::UPDATE)
            .desc("启动后检查一次新版本，会访问 api.github.com")
            .error(error.as_deref())
            .right_width(48.0)
            .show(ui, &look, |ui| {
                toggle(ui, &look, auto_check);
            });
        let error = self.issue("update.include_prerelease");
        let include_prerelease = &mut self.draft.config.update.include_prerelease;
        Card::new("接收预发布版本")
            .icon(icons::BETA)
            .desc("把 beta / rc 版本也当作可升级的版本")
            .error(error.as_deref())
            .right_width(48.0)
            .show(ui, &look, |ui| {
                toggle(ui, &look, include_prerelease);
            });
    }

    fn hotkeys_page(&mut self, ui: &mut egui::Ui) {
        let look = self.look;
        if self.draft.hotkey_fallback {
            banner(
                ui,
                &look,
                BannerKind::Warn,
                "配置文件里有无法识别的快捷键，下面显示的是默认值，应用后会覆盖",
            );
        }
        let keys = self.keys.clone();
        for (field, other_field, id, title, issue_field) in [
            (
                HotkeyField::Search,
                HotkeyField::Screenshot,
                "hotkey-search",
                "圈选搜索",
                "hotkeys.search",
            ),
            (
                HotkeyField::Screenshot,
                HotkeyField::Search,
                "hotkey-shot",
                "截屏到剪贴板",
                "hotkeys.screenshot_copy",
            ),
        ] {
            let choice = *self.hotkey_mut(field);
            let other = *self.hotkey_mut(other_field);
            let applied_conflict = self
                .conflicts
                .iter()
                .any(|(f, c)| *f == field && *c == choice);
            let probe = self.probe_cached(&choice);
            let status = hotkey_status(
                &choice,
                &other,
                self.issue(issue_field),
                applied_conflict,
                probe.as_ref(),
            );
            let recording = self.recording == Some(field);
            let card = HotkeyCard {
                id,
                title,
                status: &status,
                recording,
            };
            if hotkey_card(ui, &look, &keys, &card, self.hotkey_mut(field)) {
                self.record_clicked = true;
                self.recording = if recording { None } else { Some(field) };
            }
        }
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "Win 键录不到，请用按钮切换；PrintScreen、Esc 可在「更多按键」里选。被其它程序占用时仍会保存，但不生效",
            )
            .size(FONT_DESC)
            .color(look.palette.text_dim),
        );
    }

    fn hotkey_mut(&mut self, field: HotkeyField) -> &mut HotkeyChoice {
        match field {
            HotkeyField::Search => &mut self.draft.search_hotkey,
            HotkeyField::Screenshot => &mut self.draft.screenshot_hotkey,
        }
    }

    /// 探测结论按快捷键文字缓存，组合不变就不重复询问托盘；没有修饰键时不探测。
    fn probe_cached(&mut self, choice: &HotkeyChoice) -> Option<ProbeState> {
        let text = choice.to_text().ok()?;
        if let Some((_, state)) = self.probes.iter().find(|(t, _)| *t == text) {
            return Some(state.clone());
        }
        let state = client::probe(&text);
        self.probes.push((text, state.clone()));
        Some(state)
    }

    /// 录制态下先于所有控件截获本帧按键；失焦即取消。
    fn capture_keys(&mut self, ctx: &egui::Context) {
        self.pointer_clicked = false;
        self.record_clicked = false;
        let Some(field) = self.recording else {
            return;
        };
        let (capture, focused, clicked) = ctx.input_mut(|i| {
            let current = i.modifiers;
            (
                recorder::take_capture(&mut i.events, current),
                i.focused,
                i.pointer.any_click(),
            )
        });
        self.pointer_clicked = clicked;
        if !focused {
            self.recording = None;
            return;
        }
        match capture {
            Some(Capture::Recorded { key, modifiers }) => {
                recorder::apply_capture(self.hotkey_mut(field), key, modifiers);
                self.recording = None;
            }
            Some(Capture::Cancelled) => self.recording = None,
            None => {}
        }
    }

    fn search_upload_page(&mut self, ui: &mut egui::Ui) {
        self.search_section(ui);
        let image_host = self.draft.config.search.backend == SearchBackend::ImageHost;
        ui.add_enabled_ui(image_host, |ui| {
            self.engine_cards(ui);
            self.upload_section(ui);
        });
        self.privacy_section(ui);
    }

    fn backend_card(&mut self, ui: &mut egui::Ui) {
        let look = self.look;
        let backend = self.draft.config.search.backend;
        Card::new("搜索方式")
            .icon(icons::SEARCH)
            .desc("内置弹窗直接把截图交给 Google Lens；搜索引擎与上传设置仅图床方式生效")
            .right_width(COMBO_WIDTH)
            .show(ui, &look, |ui| {
                egui::ComboBox::from_id_salt("search-backend")
                    .width(COMBO_WIDTH)
                    .selected_text(backend.label())
                    .show_ui(ui, |ui| {
                        for option in SearchBackend::ALL {
                            ui.selectable_value(
                                &mut self.draft.config.search.backend,
                                option,
                                option.label(),
                            );
                        }
                    });
            });
    }

    fn search_section(&mut self, ui: &mut egui::Ui) {
        let look = self.look;
        group_label(ui, &look, "搜索");
        self.backend_card(ui);
        let current = self.draft.config.search.selection_mode;
        Card::new("选区模式")
            .icon(icons::EDIT)
            .desc("圈选的方式")
            .right_width(COMBO_WIDTH)
            .show(ui, &look, |ui| {
                egui::ComboBox::from_id_salt("selection-mode")
                    .width(COMBO_WIDTH)
                    .selected_text(current.label())
                    .show_ui(ui, |ui| {
                        for mode in SelectionMode::ALL {
                            ui.selectable_value(
                                &mut self.draft.config.search.selection_mode,
                                mode,
                                mode.label(),
                            );
                        }
                    });
            });
        let glow = self.draft.config.search.glow;
        Card::new("边缘辉光")
            .icon(icons::BRIGHTNESS)
            .desc("圈选时屏幕边缘的谷歌四色光带")
            .right_width(COMBO_WIDTH)
            .show(ui, &look, |ui| {
                egui::ComboBox::from_id_salt("search-glow")
                    .width(COMBO_WIDTH)
                    .selected_text(glow.label())
                    .show_ui(ui, |ui| {
                        for mode in GlowMode::ALL {
                            ui.selectable_value(
                                &mut self.draft.config.search.glow,
                                mode,
                                mode.label(),
                            );
                        }
                    });
            });
        let colorful = &mut self.draft.config.search.colorful_stroke;
        Card::new("炫彩描边")
            .icon(icons::COLOR)
            .desc("圈选轨迹为谷歌蓝加笔尖光点，选框为四色渐变；关闭后为原来的单色")
            .right_width(48.0)
            .show(ui, &look, |ui| {
                toggle(ui, &look, colorful);
            });
    }

    fn engine_cards(&mut self, ui: &mut egui::Ui) {
        let look = self.look;
        let engine_error = self.issue("search.engine_url_template");
        let custom = self.draft.engine_custom;
        let selected = if custom {
            "自定义"
        } else {
            SEARCH_ENGINE_PRESETS
                .iter()
                .find(|p| p.url_template == self.draft.config.search.engine_url_template)
                .map_or("自定义", |p| p.name)
        };
        Card::new("搜索引擎")
            .icon(icons::SEARCH)
            .desc("以图搜图使用的网站")
            .error(if custom {
                None
            } else {
                engine_error.as_deref()
            })
            .right_width(COMBO_WIDTH)
            .show(ui, &look, |ui| {
                egui::ComboBox::from_id_salt("search-engine")
                    .width(COMBO_WIDTH)
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for preset in SEARCH_ENGINE_PRESETS {
                            let is_current = !self.draft.engine_custom
                                && self.draft.config.search.engine_url_template
                                    == preset.url_template;
                            if ui.selectable_label(is_current, preset.name).clicked() {
                                self.draft.engine_custom = false;
                                self.draft.config.search.engine_url_template =
                                    preset.url_template.to_string();
                            }
                        }
                        if ui
                            .selectable_label(self.draft.engine_custom, "自定义")
                            .clicked()
                        {
                            self.draft.engine_custom = true;
                        }
                    });
            });
        if self.draft.engine_custom {
            let template = &mut self.draft.config.search.engine_url_template;
            Card::new("地址模板")
                .icon(icons::LINK)
                .desc("必须恰好含一个 {url}")
                .error(engine_error.as_deref())
                .show_wide(
                    ui,
                    &look,
                    |_| {},
                    |ui| {
                        let width = ui.available_width();
                        text_input(ui, template, width);
                    },
                );
        }
    }

    fn upload_section(&mut self, ui: &mut egui::Ui) {
        let look = self.look;
        group_label(ui, &look, "上传");
        let provider = self.draft.config.upload.provider;
        Card::new("图床")
            .icon(icons::CLOUD)
            .desc("截图上传到哪里")
            .right_width(COMBO_WIDTH)
            .show(ui, &look, |ui| {
                egui::ComboBox::from_id_salt("upload-provider")
                    .width(COMBO_WIDTH)
                    .selected_text(provider_label(provider))
                    .show_ui(ui, |ui| {
                        for option in [UploadProvider::Litterbox, UploadProvider::Custom] {
                            ui.selectable_value(
                                &mut self.draft.config.upload.provider,
                                option,
                                provider_label(option),
                            );
                        }
                    });
            });
        match self.draft.config.upload.provider {
            UploadProvider::Litterbox => {
                let current = self.draft.config.upload.litterbox.expiry;
                Card::new("图片保留时间")
                    .icon(icons::CLOCK)
                    .desc("到期后图床会删除图片")
                    .right_width(COMBO_WIDTH)
                    .show(ui, &look, |ui| {
                        egui::ComboBox::from_id_salt("litterbox-expiry")
                            .width(COMBO_WIDTH)
                            .selected_text(current.label())
                            .show_ui(ui, |ui| {
                                for expiry in LitterboxExpiry::ALL {
                                    ui.selectable_value(
                                        &mut self.draft.config.upload.litterbox.expiry,
                                        expiry,
                                        expiry.label(),
                                    );
                                }
                            });
                    });
            }
            UploadProvider::Custom => self.custom_upload_fields(ui),
        }
        let timeout_error = self.issue("upload.timeout_secs");
        Card::new("超时")
            .icon(icons::CLOCK)
            .desc("单次上传的等待上限")
            .error(timeout_error.as_deref())
            .right_width(110.0)
            .show(ui, &look, |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.draft.config.upload.timeout_secs)
                        .range(TIMEOUT_SECS_RANGE)
                        .suffix(" 秒"),
                );
            });
        let size_error = self.issue("upload.max_bytes");
        Card::new("大小上限")
            .icon(icons::DOCUMENT)
            .desc("超过上限的截图不会上传")
            .error(size_error.as_deref())
            .right_width(130.0)
            .show(ui, &look, |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.draft.max_mib)
                        .range(MAX_MIB_RANGE)
                        .suffix(" MiB"),
                );
            });
    }

    fn custom_upload_fields(&mut self, ui: &mut egui::Ui) {
        let look = self.look;
        let endpoint_error = self.issue("upload.custom.endpoint");
        let endpoint = &mut self.draft.config.upload.custom.endpoint;
        Card::new("上传端点")
            .icon(icons::LINK)
            .desc("http 或 https 地址")
            .error(endpoint_error.as_deref())
            .show_wide(
                ui,
                &look,
                |_| {},
                |ui| {
                    let width = ui.available_width();
                    text_input(ui, endpoint, width);
                },
            );
        let field_error = self.issue("upload.custom.file_field");
        let file_field = &mut self.draft.config.upload.custom.file_field;
        Card::new("文件字段名")
            .icon(icons::EDIT)
            .desc("表单里图片文件使用的字段名")
            .error(field_error.as_deref())
            .right_width(FIELD_WIDTH)
            .show(ui, &look, |ui| {
                text_input(ui, file_field, FIELD_WIDTH);
            });
        let current = self.draft.config.upload.custom.response;
        Card::new("响应格式")
            .icon(icons::DOCUMENT)
            .desc("图床返回图片地址的方式")
            .right_width(COMBO_WIDTH)
            .show(ui, &look, |ui| {
                egui::ComboBox::from_id_salt("custom-response")
                    .width(COMBO_WIDTH)
                    .selected_text(response_label(current))
                    .show_ui(ui, |ui| {
                        for option in [ResponseKind::Json, ResponseKind::Text] {
                            ui.selectable_value(
                                &mut self.draft.config.upload.custom.response,
                                option,
                                response_label(option),
                            );
                        }
                    });
            });
        if self.draft.config.upload.custom.response == ResponseKind::Json {
            let pointer_error = self.issue("upload.custom.json_pointer");
            let pointer = &mut self.draft.config.upload.custom.json_pointer;
            Card::new("JSON 指针")
                .icon(icons::EDIT)
                .desc("图片地址在响应 JSON 里的位置")
                .error(pointer_error.as_deref())
                .right_width(FIELD_WIDTH)
                .show(ui, &look, |ui| {
                    text_input(ui, pointer, FIELD_WIDTH);
                });
        }
    }

    fn privacy_section(&mut self, ui: &mut egui::Ui) {
        let look = self.look;
        group_label(ui, &look, "隐私");
        let host = self
            .draft
            .config
            .privacy
            .acknowledged_host
            .trim()
            .to_string();
        let desc = if host.is_empty() {
            "尚未确认；下次上传前会询问".to_string()
        } else {
            format!("{host}；重置后下次上传前会重新询问")
        };
        let error = self.issue("privacy.acknowledged_host");
        Card::new("已确认的图床")
            .icon(icons::LOCK)
            .desc(&desc)
            .error(error.as_deref())
            .right_width(110.0)
            .show(ui, &look, |ui| {
                if secondary_button(ui, &look, "重置确认").clicked() {
                    self.draft.config.privacy.acknowledged_host.clear();
                }
            });
        let lens_desc = if self.draft.config.privacy.lens_acknowledged {
            "已确认截图会直接发送给 Google；重置后下次使用内置弹窗前会重新询问"
        } else {
            "尚未确认；下次使用内置弹窗前会询问"
        };
        Card::new("内置弹窗")
            .icon(icons::LOCK)
            .desc(lens_desc)
            .right_width(110.0)
            .show(ui, &look, |ui| {
                if secondary_button(ui, &look, "重置确认").clicked() {
                    self.draft.config.privacy.lens_acknowledged = false;
                }
            });
    }

    fn screenshot_page(&mut self, ui: &mut egui::Ui) {
        let look = self.look;
        group_label(ui, &look, "保存");
        let dir_error = self.issue("screenshot.save_dir");
        let save_dir = &mut self.draft.config.screenshot.save_dir;
        Card::new("保存目录")
            .icon(icons::FOLDER)
            .desc("绝对路径；留空则只复制到剪贴板")
            .error(dir_error.as_deref())
            .show_wide(
                ui,
                &look,
                |_| {},
                |ui| {
                    let width = ui.available_width();
                    text_input(ui, save_dir, width);
                },
            );
        group_label(ui, &look, "选区");
        let error = self.issue("screenshot.magnifier");
        let magnifier = &mut self.draft.config.screenshot.magnifier;
        Card::new("放大镜")
            .icon(icons::SEARCH)
            .desc("在光标旁放大像素并显示颜色，按 C 复制颜色值")
            .error(error.as_deref())
            .right_width(48.0)
            .show(ui, &look, |ui| {
                toggle(ui, &look, magnifier);
            });
        let error = self.issue("screenshot.window_snap");
        let window_snap = &mut self.draft.config.screenshot.window_snap;
        Card::new("吸附窗口")
            .icon(icons::MOUSE)
            .desc("矩形选区时悬停高亮窗口，单击即选中；选区中按 Tab 临时开关")
            .error(error.as_deref())
            .right_width(48.0)
            .show(ui, &look, |ui| {
                toggle(ui, &look, window_snap);
            });
        let error = self.issue("screenshot.show_hints");
        let show_hints = &mut self.draft.config.screenshot.show_hints;
        Card::new("快捷键提示")
            .icon(icons::KEYBOARD)
            .desc("选区时在屏幕角落列出可用按键；选区中按 ? 或 F1 临时隐藏")
            .error(error.as_deref())
            .right_width(48.0)
            .show(ui, &look, |ui| {
                toggle(ui, &look, show_hints);
            });
        let error = self.issue("screenshot.history_size");
        Card::new("历史选区")
            .icon(icons::CLOCK)
            .desc("截屏时按 , 和 . 切换最近的选区；0 表示关闭")
            .error(error.as_deref())
            .right_width(110.0)
            .show(ui, &look, |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.draft.config.screenshot.history_size)
                        .range(HISTORY_SIZE_RANGE)
                        .suffix(" 条"),
                );
            });
        group_label(ui, &look, "套索");
        let padding_error = self.issue("selection.lasso.padding");
        Card::new("外扩边距")
            .icon(icons::EDIT)
            .desc("套索外围多截取的范围")
            .error(padding_error.as_deref())
            .right_width(110.0)
            .show(ui, &look, |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.draft.config.selection.lasso.padding)
                        .range(LASSO_PADDING_RANGE)
                        .suffix(" 像素"),
                );
            });
        let stroke_error = self.issue("selection.lasso.stroke_width");
        Card::new("线宽")
            .icon(icons::EDIT)
            .desc("套索轨迹的粗细")
            .error(stroke_error.as_deref())
            .right_width(110.0)
            .show(ui, &look, |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.draft.config.selection.lasso.stroke_width)
                        .range(LASSO_STROKE_WIDTH_RANGE)
                        .suffix(" 像素"),
                );
            });
        let show_bounds = &mut self.draft.config.selection.lasso.show_bounds;
        Card::new("显示外接矩形")
            .icon(icons::EDIT)
            .desc("圈选时显示截取范围的方框；关闭后只提亮套索围出的区域")
            .right_width(48.0)
            .show(ui, &look, |ui| {
                toggle(ui, &look, show_bounds);
            });
        self.annotate_cards(ui);
    }

    /// 标注工具的默认样式：每个工具一行；截屏结束时会自动记住最近一次的改动。
    fn annotate_cards(&mut self, ui: &mut egui::Ui) {
        let look = self.look;
        group_label(ui, &look, "标注");
        let pair =
            |app: &Self, color: &str, size: &str| app.issue(color).or_else(|| app.issue(size));
        let stroke_issues = [
            pair(self, "annotate.rectangle.color", "annotate.rectangle.width"),
            pair(self, "annotate.ellipse.color", "annotate.ellipse.width"),
            pair(self, "annotate.line.color", "annotate.line.width"),
            pair(self, "annotate.arrow.color", "annotate.arrow.width"),
            pair(self, "annotate.pen.color", "annotate.pen.width"),
        ];
        let highlighter_issue = pair(
            self,
            "annotate.highlighter.color",
            "annotate.highlighter.width",
        );
        let mosaic_issue = self.issue("annotate.mosaic.block");
        let number_issue = pair(self, "annotate.number.color", "annotate.number.size");
        let text_issue = pair(self, "annotate.text.color", "annotate.text.size");
        let a = &mut self.draft.config.annotate;
        let strokes = [
            ("矩形", &mut a.rectangle),
            ("椭圆", &mut a.ellipse),
            ("直线", &mut a.line),
            ("箭头", &mut a.arrow),
            ("画笔", &mut a.pen),
        ];
        for ((title, stroke), issue) in strokes.into_iter().zip(&stroke_issues) {
            Card::new(title)
                .icon(icons::COLOR)
                .desc("颜色与线宽")
                .error(issue.as_deref())
                .right_width(340.0)
                .show(ui, &look, |ui| {
                    ui.add(
                        egui::DragValue::new(&mut stroke.width)
                            .range(ANNOTATE_WIDTH_RANGE)
                            .suffix(" 像素"),
                    );
                    color_swatches(ui, &look, &mut stroke.color);
                });
        }
        let highlighter = &mut a.highlighter;
        Card::new("荧光笔")
            .icon(icons::COLOR)
            .desc("颜色与线宽，绘制时固定为 40% 不透明")
            .error(highlighter_issue.as_deref())
            .right_width(340.0)
            .show(ui, &look, |ui| {
                ui.add(
                    egui::DragValue::new(&mut highlighter.width)
                        .range(HIGHLIGHTER_WIDTH_RANGE)
                        .suffix(" 像素"),
                );
                color_swatches(ui, &look, &mut highlighter.color);
            });
        let mosaic = &mut a.mosaic;
        Card::new("马赛克")
            .icon(icons::COLOR)
            .desc("像素化的块大小")
            .error(mosaic_issue.as_deref())
            .right_width(110.0)
            .show(ui, &look, |ui| {
                ui.add(
                    egui::DragValue::new(&mut mosaic.block)
                        .range(MOSAIC_BLOCK_RANGE)
                        .suffix(" 像素"),
                );
            });
        let text = &mut a.text;
        Card::new("文字")
            .icon(icons::COLOR)
            .desc("颜色、字号、外观与排版方向；底色模式下颜色用作底色")
            .error(text_issue.as_deref())
            .right_width(580.0)
            .show(ui, &look, |ui| {
                ui.add(
                    egui::DragValue::new(&mut text.size)
                        .range(TEXT_SIZE_RANGE)
                        .suffix(" 像素"),
                );
                color_swatches(ui, &look, &mut text.color);
                // right_to_left 布局下先添加的在右侧，逆序添加使视觉顺序为「纯文字、底色」
                for (background, label) in [(true, "底色"), (false, "纯文字")] {
                    if choice_chip(ui, &look, text.background == background, label).clicked() {
                        text.background = background;
                    }
                }
                toggle(ui, &look, &mut text.vertical);
                ui.label("竖排");
            });
        let number = &mut a.number;
        Card::new("序号")
            .icon(icons::COLOR)
            .desc("颜色、圆的直径与编号风格")
            .error(number_issue.as_deref())
            .right_width(450.0)
            .show(ui, &look, |ui| {
                ui.add(
                    egui::DragValue::new(&mut number.size)
                        .range(NUMBER_SIZE_RANGE)
                        .suffix(" 像素"),
                );
                color_swatches(ui, &look, &mut number.color);
                // right_to_left 布局下先添加的在右侧，逆序添加使视觉顺序为「数字、字母」
                for style in NumberStyle::ALL.iter().rev() {
                    if choice_chip(ui, &look, number.style == *style, style.label()).clicked() {
                        number.style = *style;
                    }
                }
            });
    }

    fn update_card(&mut self, ui: &mut egui::Ui) {
        let look = self.look;
        let prerelease = self.draft.config.update.include_prerelease;
        self.updates
            .on_page_shown(ui.ctx(), self.draft.config.update.auto_check, prerelease);
        let status = self.updates.status().clone();
        let text = status.text();
        let failed = matches!(status, UpdateStatus::Failed(_));
        let mut clicked = false;
        Card::new("检查更新")
            .icon(icons::UPDATE)
            .desc(&text)
            .error(failed.then_some(text.as_str()))
            .right_width(110.0)
            .show(ui, &look, |ui| {
                clicked = match &status {
                    UpdateStatus::Available {
                        page_url: Some(_), ..
                    } => primary_button(ui, &look, "前往下载").clicked(),
                    UpdateStatus::Failed(_) => secondary_button(ui, &look, "重试").clicked(),
                    UpdateStatus::Checking => ui
                        .add_enabled_ui(false, |ui| secondary_button(ui, &look, "检查中"))
                        .inner
                        .clicked(),
                    UpdateStatus::Idle
                    | UpdateStatus::UpToDate
                    | UpdateStatus::Available { page_url: None, .. } => {
                        secondary_button(ui, &look, "检查更新").clicked()
                    }
                };
            });
        if !clicked {
            return;
        }
        match status {
            UpdateStatus::Available {
                page_url: Some(url),
                ..
            } => {
                if let Err(message) = self.updates.open_page(ui.ctx(), &url) {
                    self.set_feedback(FeedbackKind::Error, message);
                }
            }
            UpdateStatus::Idle
            | UpdateStatus::Checking
            | UpdateStatus::UpToDate
            | UpdateStatus::Available { page_url: None, .. }
            | UpdateStatus::Failed(_) => self.updates.start(ui.ctx(), prerelease),
        }
    }

    fn about_page(&mut self, ui: &mut egui::Ui) {
        let look = self.look;
        let p = look.palette;
        Card::new("lenswin")
            .icon(icons::INFO)
            .desc(&format!("版本 {}", env!("CARGO_PKG_VERSION")))
            .show(ui, &look, |_| {});
        self.update_card(ui);
        Card::new("运行方式")
            .icon(icons::SETTINGS)
            .desc(&self.mode_label)
            .show(ui, &look, |_| {});
        Card::new("配置文件").icon(icons::DOCUMENT).show_wide(
            ui,
            &look,
            |_| {},
            |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&self.config_path_text)
                            .monospace()
                            .color(p.text_dim),
                    )
                    .wrap(),
                );
            },
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("设置由主程序保存；手写在配置文件里的注释会在保存时丢失")
                .size(FONT_DESC)
                .color(p.text_dim),
        );
    }

    fn side_panel(&mut self, ctx: &egui::Context) {
        let look = self.look;
        egui::SidePanel::left("pages")
            .resizable(false)
            .exact_width(NAV_WIDTH)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(look.palette.nav)
                    .inner_margin(egui::Margin::symmetric(0, 12)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for page in Page::ALL {
                    let selected = self.page == page;
                    if nav_item(ui, &look, selected, icons::page_icon(page), page.title()).clicked()
                    {
                        self.page = page;
                    }
                }
            });
    }

    fn bottom_panel(&mut self, ctx: &egui::Context) -> Option<ButtonAction> {
        let look = self.look;
        let p = look.palette;
        let mut action = None;
        egui::TopBottomPanel::bottom("buttons")
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(p.window)
                    .inner_margin(egui::Margin::symmetric(PAGE_MARGIN_X, 12)),
            )
            .show(ctx, |ui| {
                let buttons_width = if self.stale_prompt {
                    BUTTONS_WIDTH + STALE_BUTTONS_WIDTH
                } else {
                    BUTTONS_WIDTH
                };
                let total = ui.available_width();
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2((total - buttons_width - 12.0).max(80.0), 30.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            if let Some(feedback) = &self.feedback {
                                let color = match feedback.kind {
                                    FeedbackKind::Success => p.text_dim,
                                    FeedbackKind::Warning => p.warn,
                                    FeedbackKind::Error => p.error,
                                };
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&feedback.text)
                                            .size(FONT_DESC)
                                            .color(color),
                                    )
                                    .wrap(),
                                );
                            }
                        },
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 30.0),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if secondary_button(ui, &look, "取消").clicked() {
                                action = Some(ButtonAction::Cancel);
                            }
                            if secondary_button(ui, &look, "应用").clicked() {
                                action = Some(ButtonAction::Apply);
                            }
                            if primary_button(ui, &look, "确定").clicked() {
                                action = Some(ButtonAction::Ok);
                            }
                            if self.stale_prompt {
                                ui.add_space(12.0);
                                if secondary_button(ui, &look, "仍然覆盖").clicked() {
                                    action = Some(ButtonAction::Overwrite);
                                }
                                if secondary_button(ui, &look, "重新载入").clicked() {
                                    action = Some(ButtonAction::Reload);
                                }
                            }
                        },
                    );
                });
            });
        action
    }

    fn banners(&self, ui: &mut egui::Ui) {
        let look = self.look;
        if !self.fonts.chinese {
            banner(
                ui,
                &look,
                BannerKind::Warn,
                "未找到系统中文字体，中文可能显示为方块",
            );
        }
        if let Some(error) = &self.load_error {
            banner(ui, &look, BannerKind::Error, error);
        }
    }
}

fn provider_label(provider: UploadProvider) -> &'static str {
    match provider {
        UploadProvider::Litterbox => "litterbox 临时图床",
        UploadProvider::Custom => "自定义端点",
    }
}

fn response_label(kind: ResponseKind) -> &'static str {
    match kind {
        ResponseKind::Json => "JSON",
        ResponseKind::Text => "纯文本",
    }
}

/// 一张快捷键卡片的展示参数。
struct HotkeyCard<'a> {
    id: &'a str,
    title: &'a str,
    status: &'a HotkeyStatus,
    recording: bool,
}

/// 快捷键卡片：右侧是按键小方块预览，下方是修饰键开关、录制按钮、兜底下拉和状态行。
/// 返回本帧是否点了录制按钮。
fn hotkey_card(
    ui: &mut egui::Ui,
    look: &Look,
    keys: &[Key],
    card: &HotkeyCard<'_>,
    choice: &mut HotkeyChoice,
) -> bool {
    let p = look.palette;
    let labels = hotkey_cap_labels(choice);
    let status_color = match card.status {
        HotkeyStatus::Invalid(_) | HotkeyStatus::Occupied => p.error,
        HotkeyStatus::Reserved(_) => p.warn,
        HotkeyStatus::Available => p.ok,
        HotkeyStatus::Unknown(_) => p.text_dim,
    };
    let mut record_clicked = false;
    Card::new(card.title)
        .icon(icons::KEYBOARD)
        .desc("点「录制」后按下组合键，修饰键可单独点选调整")
        .right_width(260.0)
        .show_wide(
            ui,
            look,
            |ui| key_caps(ui, look, &labels),
            |ui| {
                ui.horizontal(|ui| {
                    modifier_chip(ui, look, &mut choice.ctrl, "Ctrl");
                    modifier_chip(ui, look, &mut choice.alt, "Alt");
                    modifier_chip(ui, look, &mut choice.shift, "Shift");
                    modifier_chip(ui, look, &mut choice.win, "Win");
                    ui.add_space(8.0);
                    record_clicked = record_button(ui, look, card.recording).clicked();
                    egui::ComboBox::from_id_salt(card.id)
                        .width(90.0)
                        .height(320.0)
                        .selected_text("更多按键")
                        .show_ui(ui, |ui| {
                            for key in keys {
                                ui.selectable_value(&mut choice.key, *key, key.to_string());
                            }
                        });
                });
                ui.label(
                    egui::RichText::new(card.status.text())
                        .size(FONT_DESC)
                        .color(status_color),
                );
            },
        );
    record_clicked
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(message) = self.updates.poll() {
            self.set_feedback(FeedbackKind::Error, format!("无法打开下载页：{message}"));
        }
        self.sync_theme(ctx);
        self.capture_keys(ctx);
        let p = self.look.palette;
        self.side_panel(ctx);
        let action = self.bottom_panel(ctx);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p.content)
                    .inner_margin(egui::Margin {
                        left: PAGE_MARGIN_X,
                        right: PAGE_MARGIN_X,
                        top: 16,
                        bottom: 0,
                    }),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = CARD_GAP;
                let look = self.look;
                page_title(ui, &look, self.page.title());
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.banners(ui);
                        match self.page {
                            Page::General => self.general_page(ui),
                            Page::Hotkeys => self.hotkeys_page(ui),
                            Page::SearchUpload => self.search_upload_page(ui),
                            Page::Screenshot => self.screenshot_page(ui),
                            Page::About => self.about_page(ui),
                        }
                        ui.add_space(12.0);
                    });
            });
        // 录制中点了录制按钮以外的地方，视为放弃
        if self.pointer_clicked && !self.record_clicked {
            self.recording = None;
        }
        if self.page != Page::Hotkeys {
            self.recording = None;
        }
        match action {
            Some(ButtonAction::Apply) => {
                self.apply(false);
            }
            Some(ButtonAction::Ok) => {
                if self.apply(false) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Some(ButtonAction::Overwrite) => {
                if self.apply(true) {
                    self.feedback = Some(Feedback {
                        kind: FeedbackKind::Success,
                        text: "已应用，别处的修改已被覆盖".to_string(),
                    });
                }
            }
            Some(ButtonAction::Reload) => self.reload_from_file(),
            Some(ButtonAction::Cancel) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            None => {}
        }
    }
}
