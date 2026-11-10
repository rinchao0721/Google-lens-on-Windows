//! 设置窗口的界面与交互。

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use eframe::egui;
use lenswin_core::config::{
    Config, LASSO_PADDING_RANGE, LASSO_STROKE_WIDTH_RANGE, LitterboxExpiry, ResponseKind,
    SEARCH_ENGINE_PRESETS, TIMEOUT_SECS_RANGE, UploadProvider, ValidationIssue, validate,
};
use lenswin_core::hotkey::{HotkeyChoice, Key, selectable_keys};
use lenswin_core::selection::SelectionMode;
use lenswin_platform::autostart::{self, AutostartState};
use lenswin_platform::paths::AppPaths;

use crate::client;
use crate::fonts::FontStatus;
use crate::form::{
    Draft, Feedback, FeedbackKind, HotkeyField, Page, Staleness, changed_hotkeys,
    config_from_file_text, hotkey_cap_labels, hotkey_clash, hotkey_display, page_of_field,
    staleness,
};
use crate::icons;
use crate::theme::{self, CARD_GAP, FONT_DESC, Look, NAV_WIDTH, PAGE_MARGIN_X, Palette, ThemeKind};
use crate::widgets::{
    BannerKind, Card, banner, group_label, key_caps, modifier_chip, nav_item, page_title,
    primary_button, secondary_button, text_input, toggle,
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
        for (field, id, title, issue_field, choice) in [
            (
                HotkeyField::Search,
                "hotkey-search",
                "圈选搜索",
                "hotkeys.search",
                self.draft.search_hotkey,
            ),
            (
                HotkeyField::Screenshot,
                "hotkey-shot",
                "截屏到剪贴板",
                "hotkeys.screenshot_copy",
                self.draft.screenshot_hotkey,
            ),
        ] {
            let conflict = self
                .conflicts
                .iter()
                .any(|(f, c)| *f == field && *c == choice)
                .then(|| format!("{} 已被其它程序占用", hotkey_display(&choice)));
            let error = self.issue(issue_field).or(conflict);
            let target = match field {
                HotkeyField::Search => &mut self.draft.search_hotkey,
                HotkeyField::Screenshot => &mut self.draft.screenshot_hotkey,
            };
            hotkey_card(ui, &look, &keys, id, title, error.as_deref(), target);
        }
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("快捷键至少要选一个修饰键；被其它程序占用时会保存但不生效")
                .size(FONT_DESC)
                .color(look.palette.text_dim),
        );
    }

    fn search_upload_page(&mut self, ui: &mut egui::Ui) {
        self.search_section(ui);
        self.upload_section(ui);
        self.privacy_section(ui);
    }

    fn search_section(&mut self, ui: &mut egui::Ui) {
        let look = self.look;
        group_label(ui, &look, "搜索");
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
    }

    fn about_page(&self, ui: &mut egui::Ui) {
        let look = self.look;
        let p = look.palette;
        Card::new("lenswin")
            .icon(icons::INFO)
            .desc(&format!("版本 {}", env!("CARGO_PKG_VERSION")))
            .show(ui, &look, |_| {});
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

/// 快捷键卡片：右侧是按键小方块预览，下方是四个修饰键开关加主键下拉框。
fn hotkey_card(
    ui: &mut egui::Ui,
    look: &Look,
    keys: &[Key],
    id: &str,
    title: &str,
    error: Option<&str>,
    choice: &mut HotkeyChoice,
) {
    let labels = hotkey_cap_labels(choice);
    Card::new(title)
        .icon(icons::KEYBOARD)
        .desc("点按下方修饰键并选择主键")
        .error(error)
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
                    egui::ComboBox::from_id_salt(id)
                        .width(90.0)
                        .height(320.0)
                        .selected_text(choice.key.to_string())
                        .show_ui(ui, |ui| {
                            for key in keys {
                                ui.selectable_value(&mut choice.key, *key, key.to_string());
                            }
                        });
                });
            },
        );
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_theme(ctx);
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
