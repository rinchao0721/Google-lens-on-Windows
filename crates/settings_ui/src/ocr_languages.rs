//! OCR 已安装语言的后台查询与列表编辑纯逻辑。

use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui;
use lenswin_core::ocr::MAX_OCR_LANGUAGES;
use lenswin_platform::com::MtaScope;
use lenswin_platform::ocr::{OcrLanguage, available_languages};

/// 已安装语言列表的查询状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrLanguageState {
    /// 尚未查询。
    Idle,
    /// 正在后台线程查询。
    Loading,
    /// 已获取系统语言列表。
    Loaded(Vec<OcrLanguage>),
    /// 查询失败。
    Failed(String),
}

/// 在后台 MTA 线程查询系统已安装 OCR 语言，结果经 channel 回到界面线程。
pub struct OcrLanguageLoader {
    state: OcrLanguageState,
    pending: Option<Receiver<Result<Vec<OcrLanguage>, String>>>,
    started: bool,
}

impl OcrLanguageLoader {
    /// 尚未查询的初始状态。
    pub fn new() -> Self {
        Self {
            state: OcrLanguageState::Idle,
            pending: None,
            started: false,
        }
    }

    /// 当前状态。
    pub fn state(&self) -> &OcrLanguageState {
        &self.state
    }

    /// 首次进入截屏页时触发一次查询；已经开始过则不重复触发。
    pub fn ensure_loaded(&mut self, ctx: &egui::Context) {
        if self.started {
            return;
        }
        self.started = true;
        self.start(ctx);
    }

    /// 重新发起一次查询（用于查询失败后重试）；正在查询时忽略。
    pub fn reload(&mut self, ctx: &egui::Context) {
        if self.state == OcrLanguageState::Loading {
            return;
        }
        self.started = true;
        self.start(ctx);
    }

    fn start(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("lenswin-ocr-langs".to_string())
            .spawn(move || {
                let result = match MtaScope::enter() {
                    Ok(_mta) => available_languages().map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                };
                if tx.send(result).is_ok() {
                    ctx.request_repaint();
                }
            });
        match spawned {
            Ok(_) => {
                self.state = OcrLanguageState::Loading;
                self.pending = Some(rx);
            }
            Err(e) => {
                self.state = OcrLanguageState::Failed(format!("无法创建查询线程：{e}"));
            }
        }
    }

    /// 每帧调用，取回后台查询结果。
    pub fn poll(&mut self) {
        let Some(rx) = &self.pending else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(langs)) => {
                self.state = OcrLanguageState::Loaded(langs);
                self.pending = None;
            }
            Ok(Err(message)) => {
                self.state = OcrLanguageState::Failed(message);
                self.pending = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.state = OcrLanguageState::Failed("查询线程异常退出".to_string());
                self.pending = None;
            }
        }
    }
}

/// 生成列表项展示文字：在已安装列表找到则显示 `显示名称 (标签)`，否则仅显示标签。
pub fn display_label(tag: &str, installed: &[OcrLanguage]) -> String {
    match installed.iter().find(|l| l.tag.eq_ignore_ascii_case(tag)) {
        Some(lang) => format!("{} ({})", lang.display_name, lang.tag),
        None => tag.to_string(),
    }
}

/// 过滤出尚未加入 `selected` 的已安装语言，供添加下拉使用。
pub fn addable_languages<'a>(
    selected: &[String],
    installed: &'a [OcrLanguage],
) -> Vec<&'a OcrLanguage> {
    installed
        .iter()
        .filter(|lang| !selected.iter().any(|s| s.eq_ignore_ascii_case(&lang.tag)))
        .collect()
}

/// 当前是否还可以继续添加语言。
pub fn can_add_language(selected: &[String]) -> bool {
    selected.len() < MAX_OCR_LANGUAGES
}

/// 将 `tag` 追加到末尾；达到上限或已存在（忽略大小写）时不改动并返回 `false`。
pub fn add_language(selected: &mut Vec<String>, tag: &str) -> bool {
    if !can_add_language(selected) || selected.iter().any(|s| s.eq_ignore_ascii_case(tag)) {
        return false;
    }
    selected.push(tag.to_string());
    true
}

/// 把 `index` 处语言上移一位；越界或已在首位时返回 `false`。
pub fn move_up(selected: &mut [String], index: usize) -> bool {
    if index == 0 || index >= selected.len() {
        return false;
    }
    selected.swap(index - 1, index);
    true
}

/// 把 `index` 处语言下移一位；越界或已在末位时返回 `false`。
pub fn move_down(selected: &mut [String], index: usize) -> bool {
    if index + 1 >= selected.len() {
        return false;
    }
    selected.swap(index, index + 1);
    true
}

/// 移除 `index` 处语言；越界时返回 `false`。
pub fn remove_at(selected: &mut Vec<String>, index: usize) -> bool {
    if index >= selected.len() {
        return false;
    }
    selected.remove(index);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed() -> Vec<OcrLanguage> {
        vec![
            OcrLanguage {
                tag: "zh-Hans-CN".to_string(),
                display_name: "中文(简体)".to_string(),
            },
            OcrLanguage {
                tag: "en-US".to_string(),
                display_name: "英语(美国)".to_string(),
            },
            OcrLanguage {
                tag: "ja".to_string(),
                display_name: "日语".to_string(),
            },
        ]
    }

    #[test]
    fn display_label_uses_installed_name_case_insensitively() {
        let langs = installed();
        assert_eq!(
            display_label("zh-hans-cn", &langs),
            "中文(简体) (zh-Hans-CN)"
        );
        assert_eq!(display_label("fr-FR", &langs), "fr-FR");
    }

    #[test]
    fn addable_languages_filters_already_selected() {
        let langs = installed();
        let selected = vec!["ZH-HANS-CN".to_string()];
        let addable: Vec<&str> = addable_languages(&selected, &langs)
            .into_iter()
            .map(|l| l.tag.as_str())
            .collect();
        assert_eq!(addable, vec!["en-US", "ja"]);
    }

    #[test]
    fn list_edits_respect_bounds_uniqueness_and_limit() {
        let mut selected = vec!["zh-Hans-CN".to_string(), "en-US".to_string()];
        assert!(!add_language(&mut selected, "EN-us"));
        assert!(add_language(&mut selected, "ja"));
        assert_eq!(selected, vec!["zh-Hans-CN", "en-US", "ja"]);

        assert!(!move_up(&mut selected, 0));
        assert!(move_up(&mut selected, 2));
        assert_eq!(selected, vec!["zh-Hans-CN", "ja", "en-US"]);

        assert!(!move_down(&mut selected, 2));
        assert!(move_down(&mut selected, 0));
        assert_eq!(selected, vec!["ja", "zh-Hans-CN", "en-US"]);

        assert!(!remove_at(&mut selected, 3));
        assert!(remove_at(&mut selected, 1));
        assert_eq!(selected, vec!["ja", "en-US"]);

        let mut full: Vec<String> = (0..MAX_OCR_LANGUAGES).map(|i| format!("l-{i}")).collect();
        assert!(!can_add_language(&full));
        assert!(!add_language(&mut full, "extra"));
    }
}
