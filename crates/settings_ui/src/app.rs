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
use crate::form::{
    Draft, Feedback, FeedbackKind, Page, Staleness, config_from_file_text, hotkey_clash,
    page_of_field, staleness,
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
    font_loaded: bool,
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

impl SettingsApp {
    /// 由启动信息建立窗口状态。
    pub fn new(startup: Startup, font_loaded: bool) -> Self {
        let autostart_enabled = startup.autostart == AutostartState::Enabled;
        let autostart_stale = matches!(startup.autostart, AutostartState::StalePath(_));
        Self {
            draft: Draft::from_config(&startup.config, autostart_enabled),
            page: Page::General,
            issues: Vec::new(),
            feedback: None,
            font_loaded,
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

    fn issue_text(&self, field: &str) -> Option<&str> {
        self.issues
            .iter()
            .find(|i| i.field == field)
            .map(|i| i.message.as_str())
    }

    fn show_issue(&self, ui: &mut egui::Ui, field: &str) {
        if let Some(message) = self.issue_text(field) {
            ui.colored_label(ui.visuals().error_fg_color, message);
        }
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
                self.stale_prompt = false;
                self.set_feedback(FeedbackKind::Success, "已重新载入配置文件");
            }
            Err(message) => self.set_feedback(FeedbackKind::Error, message),
        }
    }

    /// 校验并提交；`force` 为真时跳过「配置已在别处修改」的检查。返回是否完全成功。
    fn apply(&mut self, force: bool) -> bool {
        self.issues.clear();
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

    fn general_page(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.draft.autostart, "开机自启");
        if self.autostart_stale {
            ui.weak("注册表里的自启项指向别的位置，勾选并点「应用」即可更新到当前程序");
        }
        ui.checkbox(
            &mut self.draft.config.general.notifications,
            "显示提示气泡（关闭后只保留警告和错误）",
        );
        self.show_issue(ui, "general.notifications");
    }

    fn hotkeys_page(&mut self, ui: &mut egui::Ui) {
        if self.draft.hotkey_fallback {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "配置文件里有无法识别的快捷键，下面显示的是默认值，应用后会覆盖",
            );
        }
        ui.weak("快捷键至少要勾选一个修饰键；被其它程序占用时会保存但不生效");
        ui.add_space(8.0);
        let keys = self.keys.clone();
        ui.label("圈选搜索");
        hotkey_editor(ui, "hotkey-search", &mut self.draft.search_hotkey, &keys);
        self.show_issue(ui, "hotkeys.search");
        ui.add_space(8.0);
        ui.label("截屏到剪贴板");
        hotkey_editor(ui, "hotkey-shot", &mut self.draft.screenshot_hotkey, &keys);
        self.show_issue(ui, "hotkeys.screenshot_copy");
    }

    fn search_upload_page(&mut self, ui: &mut egui::Ui) {
        self.search_section(ui);
        ui.separator();
        self.upload_section(ui);
        ui.separator();
        self.privacy_section(ui);
    }

    fn search_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("搜索");
        ui.horizontal(|ui| {
            ui.label("选区模式");
            let current = self.draft.config.search.selection_mode;
            egui::ComboBox::from_id_salt("selection-mode")
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
        ui.horizontal(|ui| {
            ui.label("搜索引擎");
            let selected = if self.draft.engine_custom {
                "自定义"
            } else {
                SEARCH_ENGINE_PRESETS
                    .iter()
                    .find(|p| p.url_template == self.draft.config.search.engine_url_template)
                    .map_or("自定义", |p| p.name)
            };
            egui::ComboBox::from_id_salt("search-engine")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for preset in SEARCH_ENGINE_PRESETS {
                        let is_current = !self.draft.engine_custom
                            && self.draft.config.search.engine_url_template == preset.url_template;
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
            ui.label("地址模板（必须恰好含一个 {url}）");
            ui.add(
                egui::TextEdit::singleline(&mut self.draft.config.search.engine_url_template)
                    .desired_width(f32::INFINITY),
            );
        }
        self.show_issue(ui, "search.engine_url_template");
    }

    fn upload_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("上传");
        ui.horizontal(|ui| {
            ui.label("图床");
            let provider = self.draft.config.upload.provider;
            egui::ComboBox::from_id_salt("upload-provider")
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
                ui.horizontal(|ui| {
                    ui.label("图片保留时间");
                    let current = self.draft.config.upload.litterbox.expiry;
                    egui::ComboBox::from_id_salt("litterbox-expiry")
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
        ui.horizontal(|ui| {
            ui.label("超时（秒）");
            ui.add(
                egui::DragValue::new(&mut self.draft.config.upload.timeout_secs)
                    .range(TIMEOUT_SECS_RANGE),
            );
            ui.label("大小上限（MiB）");
            ui.add(egui::DragValue::new(&mut self.draft.max_mib).range(MAX_MIB_RANGE));
        });
        self.show_issue(ui, "upload.timeout_secs");
        self.show_issue(ui, "upload.max_bytes");
    }

    fn custom_upload_fields(&mut self, ui: &mut egui::Ui) {
        ui.label("上传端点（http 或 https）");
        ui.add(
            egui::TextEdit::singleline(&mut self.draft.config.upload.custom.endpoint)
                .desired_width(f32::INFINITY),
        );
        self.show_issue(ui, "upload.custom.endpoint");
        ui.horizontal(|ui| {
            ui.label("文件字段名");
            ui.text_edit_singleline(&mut self.draft.config.upload.custom.file_field);
        });
        self.show_issue(ui, "upload.custom.file_field");
        ui.horizontal(|ui| {
            ui.label("响应格式");
            let current = self.draft.config.upload.custom.response;
            egui::ComboBox::from_id_salt("custom-response")
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
            ui.horizontal(|ui| {
                ui.label("JSON 指针");
                ui.text_edit_singleline(&mut self.draft.config.upload.custom.json_pointer);
            });
            self.show_issue(ui, "upload.custom.json_pointer");
        }
    }

    fn privacy_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("隐私");
        let host = self
            .draft
            .config
            .privacy
            .acknowledged_host
            .trim()
            .to_string();
        let shown = if host.is_empty() {
            "尚未确认"
        } else {
            host.as_str()
        };
        ui.horizontal(|ui| {
            ui.label(format!("已确认的图床：{shown}"));
            if ui.button("重置确认").clicked() {
                self.draft.config.privacy.acknowledged_host.clear();
            }
        });
        ui.weak("重置后下次上传前会重新询问");
        self.show_issue(ui, "privacy.acknowledged_host");
    }

    fn screenshot_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("保存");
        ui.label("保存目录（绝对路径；留空则只复制到剪贴板）");
        ui.add(
            egui::TextEdit::singleline(&mut self.draft.config.screenshot.save_dir)
                .desired_width(f32::INFINITY),
        );
        self.show_issue(ui, "screenshot.save_dir");
        ui.separator();
        ui.heading("套索");
        ui.horizontal(|ui| {
            ui.label("外扩边距（像素）");
            ui.add(
                egui::DragValue::new(&mut self.draft.config.selection.lasso.padding)
                    .range(LASSO_PADDING_RANGE),
            );
            ui.label("线宽（像素）");
            ui.add(
                egui::DragValue::new(&mut self.draft.config.selection.lasso.stroke_width)
                    .range(LASSO_STROKE_WIDTH_RANGE),
            );
        });
        self.show_issue(ui, "selection.lasso.padding");
        self.show_issue(ui, "selection.lasso.stroke_width");
    }

    fn about_page(&self, ui: &mut egui::Ui) {
        ui.heading("lenswin");
        ui.label(format!("版本 {}", env!("CARGO_PKG_VERSION")));
        ui.label(format!("运行方式：{}", self.mode_label));
        ui.label("配置文件：");
        ui.monospace(&self.config_path_text);
        ui.weak("设置由主程序保存；手写在配置文件里的注释会在保存时丢失");
    }

    fn side_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("pages")
            .resizable(false)
            .exact_width(140.0)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                for page in Page::ALL {
                    ui.selectable_value(&mut self.page, page, page.title());
                }
            });
    }

    fn bottom_panel(&mut self, ctx: &egui::Context) -> Option<ButtonAction> {
        let mut action = None;
        egui::TopBottomPanel::bottom("buttons").show(ctx, |ui| {
            ui.add_space(6.0);
            if let Some(feedback) = &self.feedback {
                let color = match feedback.kind {
                    FeedbackKind::Success => ui.visuals().text_color(),
                    FeedbackKind::Warning => ui.visuals().warn_fg_color,
                    FeedbackKind::Error => ui.visuals().error_fg_color,
                };
                ui.colored_label(color, &feedback.text);
            }
            if self.stale_prompt {
                ui.horizontal(|ui| {
                    if ui.button("重新载入").clicked() {
                        action = Some(ButtonAction::Reload);
                    }
                    if ui.button("仍然覆盖").clicked() {
                        action = Some(ButtonAction::Overwrite);
                    }
                });
            }
            ui.horizontal(|ui| {
                if ui.button("确定").clicked() {
                    action = Some(ButtonAction::Ok);
                }
                if ui.button("应用").clicked() {
                    action = Some(ButtonAction::Apply);
                }
                if ui.button("取消").clicked() {
                    action = Some(ButtonAction::Cancel);
                }
            });
            ui.add_space(6.0);
        });
        action
    }

    fn banners(&self, ui: &mut egui::Ui) {
        if !self.font_loaded {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "未找到系统中文字体，中文可能显示为方块",
            );
        }
        if let Some(error) = &self.load_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
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

/// 四个修饰键复选框加一个主键下拉框。
fn hotkey_editor(ui: &mut egui::Ui, id: &str, choice: &mut HotkeyChoice, keys: &[Key]) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut choice.ctrl, "Ctrl");
        ui.checkbox(&mut choice.alt, "Alt");
        ui.checkbox(&mut choice.shift, "Shift");
        ui.checkbox(&mut choice.win, "Win");
        egui::ComboBox::from_id_salt(id)
            .selected_text(choice.key.to_string())
            .show_ui(ui, |ui| {
                for key in keys {
                    ui.selectable_value(&mut choice.key, *key, key.to_string());
                }
            });
    });
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.side_panel(ctx);
        let action = self.bottom_panel(ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            self.banners(ui);
            egui::ScrollArea::vertical().show(ui, |ui| match self.page {
                Page::General => self.general_page(ui),
                Page::Hotkeys => self.hotkeys_page(ui),
                Page::SearchUpload => self.search_upload_page(ui),
                Page::Screenshot => self.screenshot_page(ui),
                Page::About => self.about_page(ui),
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
