//! 检查更新的触发方式、结果回投与提示选择；网络请求在 `actions::check_update`。

use std::sync::mpsc::Sender;
use std::time::Duration;

use lenswin_core::update::{UpdateCheck, is_release_page_url};
use lenswin_platform::window::TaskNotifier;

/// 启动后多久做自动检查，避开开机时网络尚未就绪。
pub const AUTO_CHECK_DELAY: Duration = Duration::from_secs(30);

/// 工作线程没送出结果就结束时的提示。
const THREAD_ABORTED: &str = "检查线程异常退出";

/// 检查由谁发起，决定结果是否需要打扰用户。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateTrigger {
    /// 启动后自动检查。
    Auto,
    /// 用户在托盘菜单里点了「检查更新」。
    Manual,
}

/// 工作线程回投给主线程的消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateMessage {
    /// 自动检查的等待时间到了，由主线程再确认一次开关。
    AutoDue,
    /// 检查结束。
    Finished(Result<UpdateCheck, String>),
    /// 打开 Release 页失败。
    OpenFailed(String),
}

/// 收到新请求后的处理：是否需要新开线程，以及进行中检查的提示方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Admission {
    /// 需要新开检查线程。
    pub spawn: bool,
    /// 更新后的进行中状态。
    pub running: Option<UpdateTrigger>,
}

/// 同一时间只跑一个检查；自动检查进行中用户手动点了，就把这次结果按手动提示，免得没反应。
pub fn admit(running: Option<UpdateTrigger>, requested: UpdateTrigger) -> Admission {
    match (running, requested) {
        (None, _) => Admission {
            spawn: true,
            running: Some(requested),
        },
        (Some(_), UpdateTrigger::Manual) => Admission {
            spawn: false,
            running: Some(UpdateTrigger::Manual),
        },
        (Some(current), UpdateTrigger::Auto) => Admission {
            spawn: false,
            running: Some(current),
        },
    }
}

/// 工作线程持有：正常结束时送出结果；提前结束（如 panic）时在析构里补发失败，保证主线程能复位。
pub struct FinishGuard {
    tx: Sender<UpdateMessage>,
    notifier: TaskNotifier,
    sent: bool,
}

impl FinishGuard {
    /// 绑定回投通道与唤醒器。
    pub fn new(tx: Sender<UpdateMessage>, notifier: TaskNotifier) -> Self {
        Self {
            tx,
            notifier,
            sent: false,
        }
    }

    /// 送出检查结果。
    pub fn finish(mut self, result: Result<UpdateCheck, String>) {
        self.deliver(result);
    }

    fn deliver(&mut self, result: Result<UpdateCheck, String>) {
        self.sent = true;
        if self.tx.send(UpdateMessage::Finished(result)).is_err() {
            // 主线程已退出，结果无人处理
            return;
        }
        if let Err(e) = self.notifier.notify() {
            log::error!("投递检查更新结果失败：{e}");
        }
    }
}

impl Drop for FinishGuard {
    fn drop(&mut self) {
        if !self.sent {
            self.deliver(Err(THREAD_ABORTED.to_string()));
        }
    }
}

/// 需要弹出的对话框。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// 有新版本；`page_url` 未通过校验时为空，只提示不提供下载按钮。
    Upgrade {
        /// 新版本号。
        version: String,
        /// Release 页地址。
        page_url: Option<String>,
    },
    /// 已是最新。
    UpToDate,
    /// 检查失败，附原因。
    Failed(String),
}

/// 检查结果对应的对话框；自动检查只在发现新版本时打扰用户。
pub fn prompt_for(trigger: UpdateTrigger, result: &Result<UpdateCheck, String>) -> Option<Prompt> {
    match (result, trigger) {
        (Ok(UpdateCheck::Available(release)), _) => Some(Prompt::Upgrade {
            version: release.version.to_string(),
            // 解析时已校验过，这里再挡一次，保证只会打开 GitHub 页面
            page_url: is_release_page_url(&release.page_url).then(|| release.page_url.clone()),
        }),
        (Ok(UpdateCheck::UpToDate), UpdateTrigger::Manual) => Some(Prompt::UpToDate),
        (Err(message), UpdateTrigger::Manual) => Some(Prompt::Failed(message.clone())),
        (Ok(UpdateCheck::UpToDate) | Err(_), UpdateTrigger::Auto) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lenswin_core::update::{Release, Version};

    fn available(url: &str) -> Result<UpdateCheck, String> {
        Ok(UpdateCheck::Available(Release {
            version: Version::parse("0.2.0").expect("版本号"),
            page_url: url.to_string(),
            prerelease: false,
        }))
    }

    #[test]
    fn idle_request_spawns_a_check() {
        for trigger in [UpdateTrigger::Auto, UpdateTrigger::Manual] {
            assert_eq!(
                admit(None, trigger),
                Admission {
                    spawn: true,
                    running: Some(trigger),
                }
            );
        }
    }

    #[test]
    fn manual_request_upgrades_running_auto_check() {
        assert_eq!(
            admit(Some(UpdateTrigger::Auto), UpdateTrigger::Manual),
            Admission {
                spawn: false,
                running: Some(UpdateTrigger::Manual),
            }
        );
    }

    #[test]
    fn repeated_requests_do_not_spawn_or_downgrade() {
        assert_eq!(
            admit(Some(UpdateTrigger::Manual), UpdateTrigger::Manual),
            Admission {
                spawn: false,
                running: Some(UpdateTrigger::Manual),
            }
        );
        assert_eq!(
            admit(Some(UpdateTrigger::Manual), UpdateTrigger::Auto),
            Admission {
                spawn: false,
                running: Some(UpdateTrigger::Manual),
            }
        );
        assert_eq!(
            admit(Some(UpdateTrigger::Auto), UpdateTrigger::Auto),
            Admission {
                spawn: false,
                running: Some(UpdateTrigger::Auto),
            }
        );
    }

    #[test]
    fn new_version_is_announced_for_both_triggers() {
        let url = "https://github.com/o/r/releases/tag/v0.2.0";
        for trigger in [UpdateTrigger::Auto, UpdateTrigger::Manual] {
            assert_eq!(
                prompt_for(trigger, &available(url)),
                Some(Prompt::Upgrade {
                    version: "0.2.0".to_string(),
                    page_url: Some(url.to_string()),
                })
            );
        }
    }

    #[test]
    fn unsafe_link_is_dropped() {
        assert_eq!(
            prompt_for(UpdateTrigger::Auto, &available("https://evil.com/x")),
            Some(Prompt::Upgrade {
                version: "0.2.0".to_string(),
                page_url: None,
            })
        );
    }

    #[test]
    fn auto_check_stays_silent_unless_newer() {
        assert_eq!(
            prompt_for(UpdateTrigger::Auto, &Ok(UpdateCheck::UpToDate)),
            None
        );
        assert_eq!(
            prompt_for(UpdateTrigger::Auto, &Err("超时".to_string())),
            None
        );
    }

    #[test]
    fn manual_check_reports_every_outcome() {
        assert_eq!(
            prompt_for(UpdateTrigger::Manual, &Ok(UpdateCheck::UpToDate)),
            Some(Prompt::UpToDate)
        );
        assert_eq!(
            prompt_for(UpdateTrigger::Manual, &Err("请求超时".to_string())),
            Some(Prompt::Failed("请求超时".to_string()))
        );
    }
}
