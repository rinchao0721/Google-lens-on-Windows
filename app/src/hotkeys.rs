//! 全局热键的注册、替换与失败汇总。

use std::path::Path;

use lenswin_core::config::HotkeysConfig;
use lenswin_core::hotkey::Hotkey;
use lenswin_platform::hotkey::{self, HotkeyGuard};
use lenswin_platform::window::WindowHandle;

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

/// 各热键的新值是否注册成功。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NewRegistered {
    search: bool,
    screenshot_copy: bool,
}

/// 单个热键在一次替换中的结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyOutcome {
    /// 新旧文字相同：保持原样，注册失败也不回退。
    Unchanged,
    /// 改了且新值注册成功：采用新值。
    Adopted,
    /// 改了但新值注册失败：退回旧值。
    Reverted,
}

/// 两段快捷键文字是否是同一个组合：先按修饰键加主键比较，任一边解析不了再比较去掉首尾空白的原文。
fn same_hotkey(a: &str, b: &str) -> bool {
    match (a.parse::<Hotkey>(), b.parse::<Hotkey>()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a.trim() == b.trim(),
    }
}

fn outcome(old: &str, new: &str, new_registered: bool) -> KeyOutcome {
    if same_hotkey(old, new) {
        KeyOutcome::Unchanged
    } else if new_registered {
        KeyOutcome::Adopted
    } else {
        KeyOutcome::Reverted
    }
}

/// 每个键最终使用的文字，以及被退回的键。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Resolution {
    effective: HotkeysConfig,
    reverted: Vec<Command>,
}

/// 纯逻辑：逐键决定最终值，不涉及 Win32。
fn resolve(old: &HotkeysConfig, new: &HotkeysConfig, registered: NewRegistered) -> Resolution {
    let pick = |old: &str, new: &str, ok: bool| match outcome(old, new, ok) {
        KeyOutcome::Adopted => (new.to_string(), false),
        KeyOutcome::Unchanged => (old.to_string(), false),
        KeyOutcome::Reverted => (old.to_string(), true),
    };
    let (search, search_reverted) = pick(&old.search, &new.search, registered.search);
    let (screenshot_copy, shot_reverted) = pick(
        &old.screenshot_copy,
        &new.screenshot_copy,
        registered.screenshot_copy,
    );
    let mut reverted = Vec::new();
    if search_reverted {
        reverted.push(Command::Search);
    }
    if shot_reverted {
        reverted.push(Command::ScreenshotCopy);
    }
    Resolution {
        effective: HotkeysConfig {
            search,
            screenshot_copy,
        },
        reverted,
    }
}

fn text_of(config: &HotkeysConfig, command: Command) -> &str {
    match command {
        Command::ScreenshotCopy => &config.screenshot_copy,
        _ => &config.search,
    }
}

fn entries(config: &HotkeysConfig) -> [(Command, &str); 2] {
    [
        (Command::Search, config.search.as_str()),
        (Command::ScreenshotCopy, config.screenshot_copy.as_str()),
    ]
}

/// 纯逻辑：已注册的编号里，有没有哪个命令的生效文字就是 `hotkey`。
fn owned_by(effective: &HotkeysConfig, registered_ids: &[i32], hotkey: &Hotkey) -> bool {
    entries(effective).into_iter().any(|(command, text)| {
        command
            .hotkey_id()
            .is_some_and(|id| registered_ids.contains(&id))
            && text.parse::<Hotkey>().is_ok_and(|h| h == *hotkey)
    })
}

/// 试注册后立即注销，判断组合是否被其它程序占用。
pub fn probe(window: WindowHandle, hotkey: &Hotkey) -> Result<(), String> {
    hotkey::register(window, PROBE_HOTKEY_ID, hotkey)
        .map(drop)
        .map_err(|e| e.to_string())
}

/// 探测专用编号，与命令热键编号不重叠；应用程序可用范围为 0x0000–0xBFFF。
const PROBE_HOTKEY_ID: i32 = 0x7F00;

fn try_register(
    window: WindowHandle,
    command: Command,
    text: &str,
) -> Result<HotkeyGuard, Failure> {
    let fail = |reason: String| Failure {
        command,
        text: text.to_string(),
        reason,
    };
    let id = command
        .hotkey_id()
        .ok_or_else(|| fail("命令没有热键编号".to_string()))?;
    let parsed = text.parse::<Hotkey>().map_err(|e| fail(e.to_string()))?;
    hotkey::register(window, id, &parsed).map_err(|e| fail(e.to_string()))
}

/// 一次重新加载的热键结果。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReplaceReport {
    /// 这次改了但注册失败、已退回旧值的键（`text` 是文件里的新值）。
    pub reverted: Vec<Failure>,
    /// 没有改动、但仍然注册失败的键。
    pub still_failing: Vec<Failure>,
}

/// 已注册的热键，销毁时自动注销。
#[derive(Default)]
pub struct HotkeySet {
    guards: Vec<HotkeyGuard>,
    effective: HotkeysConfig,
}

impl HotkeySet {
    /// 按配置注册；返回失败项。
    pub fn register(window: WindowHandle, config: &HotkeysConfig) -> (Self, Vec<Failure>) {
        let mut guards = Vec::new();
        let mut failures = Vec::new();
        for (command, text) in entries(config) {
            match try_register(window, command, text) {
                Ok(guard) => {
                    log::info!("已注册热键 {text}（{}）", command.label());
                    guards.push(guard);
                }
                Err(failure) => {
                    log::warn!(
                        "热键 {text}（{}）注册失败：{}",
                        command.label(),
                        failure.reason
                    );
                    failures.push(failure);
                }
            }
        }
        (
            Self {
                guards,
                effective: config.clone(),
            },
            failures,
        )
    }

    /// 实际在使用的热键文字，菜单据此显示。
    pub fn effective(&self) -> &HotkeysConfig {
        &self.effective
    }

    /// 该组合是否正由本程序注册着；生效文字相同但注册失败的不算。
    pub fn owns(&self, hotkey: &Hotkey) -> bool {
        let registered: Vec<i32> = self.guards.iter().map(HotkeyGuard::id).collect();
        owned_by(&self.effective, &registered, hotkey)
    }

    /// 换成配置文件里的新热键，逐键处理：改了且成功就采用，改了但失败就退回旧值，
    /// 没改的键保持原样。
    pub fn replace(&mut self, window: WindowHandle, new: &HotkeysConfig) -> ReplaceReport {
        let old = self.effective.clone();
        // 同一组合键不能重复注册，先全部注销再逐个尝试
        self.guards.clear();
        let mut attempts: Vec<(Command, Result<HotkeyGuard, Failure>)> = entries(new)
            .into_iter()
            .map(|(command, text)| (command, try_register(window, command, text)))
            .collect();
        let registered_of = |attempts: &[(Command, Result<HotkeyGuard, Failure>)], c: Command| {
            attempts
                .iter()
                .any(|(command, result)| *command == c && result.is_ok())
        };
        let resolution = resolve(
            &old,
            new,
            NewRegistered {
                search: registered_of(&attempts, Command::Search),
                screenshot_copy: registered_of(&attempts, Command::ScreenshotCopy),
            },
        );

        let mut report = ReplaceReport::default();
        for (command, result) in attempts.drain(..) {
            let reverted = resolution.reverted.contains(&command);
            match result {
                Ok(guard) if !reverted => self.guards.push(guard),
                Ok(_) => {}
                Err(failure) if reverted => report.reverted.push(failure),
                Err(failure) => report.still_failing.push(failure),
            }
            if reverted {
                let kept = text_of(&old, command);
                match try_register(window, command, kept) {
                    Ok(guard) => self.guards.push(guard),
                    Err(e) => log::warn!("旧热键 {kept} 重新注册失败：{}", e.reason),
                }
            }
        }
        self.effective = resolution.effective;
        report
    }
}

/// 启动时失败汇总；气泡正文上限 255 个字符，配置路径放最前以免被截断。
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

/// 重新加载后给用户的说明；全部正常时返回 `None`。
///
/// 退回只发生在内存里，配置文件保持用户写的样子，所以要说明文件与实际不一致。
pub fn reload_summary(
    report: &ReplaceReport,
    effective: &HotkeysConfig,
    config_path: &Path,
) -> Option<String> {
    if report.reverted.is_empty() && report.still_failing.is_empty() {
        return None;
    }
    let mut lines = vec![format!("配置文件：{}", config_path.display())];
    for failure in &report.reverted {
        lines.push(format!(
            "文件里的{}快捷键 {} 被占用或无效，当前仍在使用旧的 {}",
            failure.command.label(),
            failure.text,
            text_of(effective, failure.command)
        ));
    }
    if !report.still_failing.is_empty() {
        let list = report
            .still_failing
            .iter()
            .map(|f| format!("{} {}", f.command.label(), f.text))
            .collect::<Vec<_>>()
            .join("、");
        lines.push(format!("仍未注册：{list}"));
    }
    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(search: &str, shot: &str) -> HotkeysConfig {
        HotkeysConfig {
            search: search.to_string(),
            screenshot_copy: shot.to_string(),
        }
    }

    fn registered(search: bool, screenshot_copy: bool) -> NewRegistered {
        NewRegistered {
            search,
            screenshot_copy,
        }
    }

    #[test]
    fn real_world_case_only_changed_key_is_adopted() {
        // 两个默认键都被占用；只把 search 改成 F13，Win+Shift+C 仍然失败
        let old = keys("Win+Shift+A", "Win+Shift+C");
        let new = keys("Ctrl+Alt+F13", "Win+Shift+C");
        let r = resolve(&old, &new, registered(true, false));
        assert_eq!(r.effective, new);
        assert!(r.reverted.is_empty());
    }

    #[test]
    fn changed_key_conflict_reverts_only_that_key() {
        let old = keys("Ctrl+1", "Ctrl+2");
        let new = keys("Ctrl+9", "Ctrl+2");
        let r = resolve(&old, &new, registered(false, true));
        assert_eq!(r.effective, old);
        assert_eq!(r.reverted, vec![Command::Search]);
    }

    #[test]
    fn unchanged_working_key_next_to_reverted_key_is_untouched() {
        let old = keys("Ctrl+1", "Ctrl+2");
        let new = keys("Ctrl+9", "Ctrl+8");
        let r = resolve(&old, &new, registered(false, true));
        assert_eq!(r.effective, keys("Ctrl+1", "Ctrl+8"));
        assert_eq!(r.reverted, vec![Command::Search]);
    }

    #[test]
    fn both_changed_and_both_registered_adopts_both() {
        let old = keys("Ctrl+1", "Ctrl+2");
        let new = keys("Ctrl+9", "Ctrl+8");
        let r = resolve(&old, &new, registered(true, true));
        assert_eq!(r.effective, new);
        assert!(r.reverted.is_empty());
    }

    #[test]
    fn both_changed_and_both_fail_reverts_both() {
        let old = keys("Ctrl+1", "Ctrl+2");
        let new = keys("Ctrl+9", "Ctrl+8");
        let r = resolve(&old, &new, registered(false, false));
        assert_eq!(r.effective, old);
        assert_eq!(r.reverted, vec![Command::Search, Command::ScreenshotCopy]);
    }

    #[test]
    fn respelling_alone_counts_as_unchanged() {
        assert!(same_hotkey("Win+Shift+A", "Shift+Win+A"));
        assert!(same_hotkey("Win+Shift+A", "shift+win+a"));
        assert!(same_hotkey(" Ctrl+F13 ", "ctrl + f13"));
        assert!(!same_hotkey("Win+Shift+A", "Win+Shift+B"));
        assert!(!same_hotkey("Win+Shift+A", "Ctrl+Shift+A"));
    }

    #[test]
    fn unparsable_text_falls_back_to_trimmed_comparison() {
        assert!(same_hotkey(" oops ", "oops"));
        assert!(!same_hotkey("oops", "Ctrl+A"));
        assert!(!same_hotkey("oops", "nope"));
        assert!(same_hotkey("", "  "));
    }

    #[test]
    fn respelled_conflicting_keys_are_not_reverted() {
        // 用户机器上两个默认键都被占用：设置窗口只是换了写法，不应触发回退
        let old = keys("Win+Shift+A", "Win+Shift+C");
        let new = keys("Shift+Win+A", "Shift+Win+C");
        let r = resolve(&old, &new, registered(false, false));
        assert!(r.reverted.is_empty());
        assert_eq!(r.effective, old);
    }

    #[test]
    fn real_change_is_still_detected_next_to_respelled_key() {
        let old = keys("Win+Shift+A", "Win+Shift+C");
        let new = keys("Shift+Win+A", "Ctrl+Alt+F13");
        let r = resolve(&old, &new, registered(false, true));
        assert_eq!(r.effective, keys("Win+Shift+A", "Ctrl+Alt+F13"));
        assert!(r.reverted.is_empty());
    }

    #[test]
    fn identical_config_never_reverts_even_if_registration_fails() {
        let same = keys("Win+Shift+A", "Win+Shift+C");
        for flags in [(true, true), (false, false), (true, false)] {
            let r = resolve(&same, &same, registered(flags.0, flags.1));
            assert_eq!(r.effective, same);
            assert!(r.reverted.is_empty());
        }
    }

    #[test]
    fn startup_summary_puts_path_first_and_lists_all_failures() {
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
    fn reload_summary_explains_file_differs_from_active_key() {
        let report = ReplaceReport {
            reverted: vec![Failure {
                command: Command::Search,
                text: "Ctrl+9".to_string(),
                reason: "占用".to_string(),
            }],
            still_failing: vec![Failure {
                command: Command::ScreenshotCopy,
                text: "Win+Shift+C".to_string(),
                reason: "占用".to_string(),
            }],
        };
        let effective = keys("Ctrl+1", "Win+Shift+C");
        let text = reload_summary(&report, &effective, Path::new(r"C:\c.toml")).expect("有提示");
        assert!(text.contains("文件里的圈选搜索快捷键 Ctrl+9"));
        assert!(text.contains("当前仍在使用旧的 Ctrl+1"));
        assert!(text.contains("仍未注册：截屏到剪贴板 Win+Shift+C"));
        assert!(text.starts_with("配置文件："));
        assert_eq!(
            reload_summary(&ReplaceReport::default(), &effective, Path::new("x")),
            None
        );
    }

    #[test]
    fn entries_cover_every_hotkey_command() {
        let config = HotkeysConfig::default();
        let commands: Vec<_> = entries(&config).iter().map(|(c, _)| *c).collect();
        assert_eq!(commands, vec![Command::Search, Command::ScreenshotCopy]);
    }

    fn hk(text: &str) -> Hotkey {
        text.parse().expect("解析")
    }

    #[test]
    fn owned_only_when_registered_and_text_matches() {
        let effective = keys("Win+Shift+A", "Win+Shift+C");
        assert!(owned_by(&effective, &[1, 2], &hk("shift+win+a")));
        assert!(owned_by(&effective, &[1, 2], &hk("Win+Shift+C")));
        assert!(!owned_by(&effective, &[1, 2], &hk("Win+Shift+B")));
        // 截屏热键启动时注册失败，组合被别的程序占着
        assert!(!owned_by(&effective, &[1], &hk("Win+Shift+C")));
        assert!(!owned_by(&effective, &[], &hk("Win+Shift+A")));
    }

    #[test]
    fn unparsable_effective_text_is_never_owned() {
        let effective = keys("garbage", "Win+Shift+C");
        assert!(!owned_by(&effective, &[1, 2], &hk("Win+Shift+A")));
    }

    #[test]
    fn probe_id_does_not_collide_with_commands() {
        assert_eq!(Command::from_hotkey_id(PROBE_HOTKEY_ID), None);
    }
}
