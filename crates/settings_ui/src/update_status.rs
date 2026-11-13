//! 关于页的检查更新状态：后台线程查询，结果经 channel 回到界面线程。

use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui;
use lenswin_core::update::{UpdateCheck, is_release_page_url};

/// 关于页显示的检查状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// 尚未检查。
    Idle,
    /// 检查中。
    Checking,
    /// 已是最新。
    UpToDate,
    /// 有新版本；地址未通过校验时为空，只显示版本不提供下载按钮。
    Available {
        /// 新版本号。
        version: String,
        /// Release 页地址。
        page_url: Option<String>,
    },
    /// 检查失败，附原因。
    Failed(String),
}

impl UpdateStatus {
    /// 卡片上的说明文字。
    pub fn text(&self) -> String {
        match self {
            UpdateStatus::Idle => "点击检查更新".to_string(),
            UpdateStatus::Checking => "检查中…".to_string(),
            UpdateStatus::UpToDate => "已是最新版本".to_string(),
            UpdateStatus::Available { version, .. } => format!("发现新版本 {version}"),
            UpdateStatus::Failed(message) => format!("检查失败：{message}"),
        }
    }
}

/// 检查结果转成界面状态。
fn status_of(result: Result<UpdateCheck, String>) -> UpdateStatus {
    match result {
        Ok(UpdateCheck::UpToDate) => UpdateStatus::UpToDate,
        Ok(UpdateCheck::Available(release)) => UpdateStatus::Available {
            version: release.version.to_string(),
            page_url: is_release_page_url(&release.page_url).then_some(release.page_url),
        },
        Err(message) => UpdateStatus::Failed(message),
    }
}

/// 检查更新的状态机；同一时间只跑一个检查。
pub struct UpdateChecker {
    status: UpdateStatus,
    pending: Option<Receiver<Result<UpdateCheck, String>>>,
    /// 后台打开下载页失败时回传原因。
    opening: Option<Receiver<String>>,
    /// 本次打开设置窗口后是否已做过进入关于页时的自动检查。
    auto_done: bool,
}

impl UpdateChecker {
    /// 尚未检查的初始状态。
    pub fn new() -> Self {
        Self {
            status: UpdateStatus::Idle,
            pending: None,
            opening: None,
            auto_done: false,
        }
    }

    /// 当前状态。
    pub fn status(&self) -> &UpdateStatus {
        &self.status
    }

    /// 进入关于页时调用：只在首次进入且开启了自动检查时发起检查。
    pub fn on_page_shown(&mut self, ctx: &egui::Context, auto_check: bool, prerelease: bool) {
        if self.auto_done {
            return;
        }
        self.auto_done = true;
        if auto_check {
            self.start(ctx, prerelease);
        }
    }

    /// 发起检查；正在检查时忽略。
    pub fn start(&mut self, ctx: &egui::Context, include_prerelease: bool) {
        if self.status == UpdateStatus::Checking {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("lenswin-settings-update".to_string())
            .spawn(move || {
                let result = lenswin_update::check(include_prerelease).map_err(|e| e.to_string());
                // 窗口已关闭时没有接收方，结果无需处理
                if tx.send(result).is_ok() {
                    ctx.request_repaint();
                }
            });
        match spawned {
            Ok(_) => {
                self.status = UpdateStatus::Checking;
                self.pending = Some(rx);
            }
            Err(e) => self.status = UpdateStatus::Failed(format!("无法创建检查线程：{e}")),
        }
    }

    /// 后台打开下载页，避免 ShellExecute 卡住界面；只接受 GitHub 页面。
    pub fn open_page(&mut self, ctx: &egui::Context, url: &str) -> Result<(), String> {
        if !is_release_page_url(url) {
            return Err("下载地址不受信任，已拒绝打开".to_string());
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let url = url.to_string();
        std::thread::Builder::new()
            .name("lenswin-settings-open".to_string())
            .spawn(move || {
                let Err(e) = lenswin_platform::shell::open_url(&url) else {
                    return;
                };
                // 窗口已关闭时没有接收方，无需提示
                if tx.send(e.to_string()).is_ok() {
                    ctx.request_repaint();
                }
            })
            .map_err(|e| format!("无法创建打开线程：{e}"))?;
        self.opening = Some(rx);
        Ok(())
    }

    /// 每帧调用，取回后台结果；返回打开下载页失败的原因。
    pub fn poll(&mut self) -> Option<String> {
        self.poll_check();
        self.poll_open()
    }

    fn poll_check(&mut self) {
        let Some(rx) = &self.pending else {
            return;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.status = status_of(result);
                self.pending = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.status = UpdateStatus::Failed("检查线程异常退出".to_string());
                self.pending = None;
            }
        }
    }

    fn poll_open(&mut self) -> Option<String> {
        let rx = self.opening.as_ref()?;
        match rx.try_recv() {
            Ok(message) => {
                self.opening = None;
                Some(message)
            }
            Err(TryRecvError::Empty) => None,
            // 成功打开时线程直接退出，不发送任何内容
            Err(TryRecvError::Disconnected) => {
                self.opening = None;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lenswin_core::update::{Release, Version};

    fn release(url: &str) -> UpdateCheck {
        UpdateCheck::Available(Release {
            version: Version::parse("0.2.0").expect("版本号"),
            page_url: url.to_string(),
            prerelease: false,
        })
    }

    #[test]
    fn results_map_to_status() {
        assert_eq!(status_of(Ok(UpdateCheck::UpToDate)), UpdateStatus::UpToDate);
        let url = "https://github.com/o/r/releases/tag/v0.2.0";
        assert_eq!(
            status_of(Ok(release(url))),
            UpdateStatus::Available {
                version: "0.2.0".to_string(),
                page_url: Some(url.to_string()),
            }
        );
        assert_eq!(
            status_of(Ok(release("https://evil.com/x"))),
            UpdateStatus::Available {
                version: "0.2.0".to_string(),
                page_url: None,
            }
        );
        assert_eq!(
            status_of(Err("超时".to_string())),
            UpdateStatus::Failed("超时".to_string())
        );
    }

    #[test]
    fn status_text_is_readable() {
        assert_eq!(UpdateStatus::Checking.text(), "检查中…");
        assert_eq!(
            UpdateStatus::Failed("超时".to_string()).text(),
            "检查失败：超时"
        );
        let available = UpdateStatus::Available {
            version: "0.2.0".to_string(),
            page_url: None,
        };
        assert_eq!(available.text(), "发现新版本 0.2.0");
    }

    #[test]
    fn untrusted_page_is_not_opened() {
        let ctx = egui::Context::default();
        let mut checker = UpdateChecker::new();
        assert!(checker.open_page(&ctx, "https://evil.com/x").is_err());
        assert!(checker.open_page(&ctx, "javascript:alert(1)").is_err());
        assert_eq!(checker.poll(), None);
    }

    #[test]
    fn auto_check_runs_only_once_and_respects_switch() {
        let ctx = egui::Context::default();
        let mut checker = UpdateChecker::new();
        checker.on_page_shown(&ctx, false, false);
        assert_eq!(checker.status(), &UpdateStatus::Idle);
        // 首次进入已处理过，再进入也不会检查
        checker.on_page_shown(&ctx, true, false);
        assert_eq!(checker.status(), &UpdateStatus::Idle);
    }
}
