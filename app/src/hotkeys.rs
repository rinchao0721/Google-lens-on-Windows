//! 全局热键的注册、替换与失败汇总。

use std::path::Path;

use lenswin_core::config::HotkeysConfig;
use lenswin_core::hotkey::Hotkey;
use lenswin_platform::hotkey::{self, HotkeyGuard};
use lenswin_platform::window::MessageWindow;

use crate::dispatcher::Command;

/// 一条注册失败记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// 对应的命令。
    pub command: Command,
    /// 配置里的快捷键文字。
    pub text: String,
    /// 失败原因。
    pub reason: String,
}

/// 已注册的热键，销毁时自动注销。
#[derive(Default)]
pub struct HotkeySet {
    guards: Vec<HotkeyGuard>,
}

fn entries(config: &HotkeysConfig) -> [(Command, &str); 2] {
    [
        (Command::Search, config.search.as_str()),
        (Command::ScreenshotCopy, config.screenshot_copy.as_str()),
    ]
}

/// 注册一组热键；单个失败不影响其它，失败项在返回值里。
fn register_all(
    window: &MessageWindow,
    config: &HotkeysConfig,
) -> (Vec<HotkeyGuard>, Vec<Failure>) {
    let mut guards = Vec::new();
    let mut failures = Vec::new();
    for (command, text) in entries(config) {
        let Some(id) = command.hotkey_id() else {
            continue;
        };
        let registered = text
            .parse::<Hotkey>()
            .map_err(|e| e.to_string())
            .and_then(|h| hotkey::register(window, id, &h).map_err(|e| e.to_string()));
        match registered {
            Ok(guard) => {
                log::info!("已注册热键 {text}（{}）", command.label());
                guards.push(guard);
            }
            Err(reason) => {
                log::warn!("热键 {text}（{}）注册失败：{reason}", command.label());
                failures.push(Failure {
                    command,
                    text: text.to_string(),
                    reason,
                });
            }
        }
    }
    (guards, failures)
}

impl HotkeySet {
    /// 按配置注册；返回失败项。
    pub fn register(window: &MessageWindow, config: &HotkeysConfig) -> (Self, Vec<Failure>) {
        let (guards, failures) = register_all(window, config);
        (Self { guards }, failures)
    }

    /// 把热键从 `old` 换成 `new`。
    ///
    /// 新热键有注册失败（如被占用）且与旧配置不同时，回滚为旧热键并返回失败项；
    /// 两份配置相同时失败项只是重试未成功，保留已成功的部分。
    pub fn replace(
        &mut self,
        window: &MessageWindow,
        old: &HotkeysConfig,
        new: &HotkeysConfig,
    ) -> Result<(), Vec<Failure>> {
        // 同一组键不能重复注册，先全部注销
        self.guards.clear();
        let (guards, failures) = register_all(window, new);
        if failures.is_empty() || old == new {
            self.guards = guards;
            return if failures.is_empty() {
                Ok(())
            } else {
                Err(failures)
            };
        }
        drop(guards);
        let (restored, restore_failures) = register_all(window, old);
        if !restore_failures.is_empty() {
            log::warn!("回滚旧热键时仍有 {} 项失败", restore_failures.len());
        }
        self.guards = restored;
        Err(failures)
    }
}

/// 失败汇总文字；气泡正文上限 255 个字符，配置路径放最前以免被截断。
pub fn failure_summary(failures: &[Failure], config_path: &Path) -> String {
    let list = failures
        .iter()
        .map(|f| format!("{} {}", f.command.label(), f.text))
        .collect::<Vec<_>>()
        .join("、");
    format!(
        "快捷键被占用或无效，可在配置文件中修改：{}\n注册失败：{list}",
        config_path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_puts_path_first_and_lists_all_failures() {
        let failures = vec![
            Failure {
                command: Command::Search,
                text: "Win+Shift+A".to_string(),
                reason: "占用".to_string(),
            },
            Failure {
                command: Command::ScreenshotCopy,
                text: "Win+Shift+C".to_string(),
                reason: "占用".to_string(),
            },
        ];
        let text = failure_summary(&failures, Path::new(r"C:\cfg\config.toml"));
        let path_at = text.find(r"C:\cfg\config.toml").expect("含路径");
        assert!(path_at < text.find("Win+Shift+A").expect("含热键"));
        assert!(text.contains("Win+Shift+C"));
    }

    #[test]
    fn entries_cover_every_hotkey_command() {
        let config = HotkeysConfig::default();
        let commands: Vec<_> = entries(&config).iter().map(|(c, _)| *c).collect();
        assert_eq!(commands, vec![Command::Search, Command::ScreenshotCopy]);
    }
}
