//! 快捷键录制：录制态下截获按键事件，翻译成主键与修饰键。

use eframe::egui;
use lenswin_core::hotkey::{Hotkey, HotkeyChoice, Key, NamedKey};

/// 录制态下一帧按键的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    /// 录到一个组合。
    Recorded {
        /// 主键。
        key: Key,
        /// 修饰键状态（不含 Win）。
        modifiers: egui::Modifiers,
    },
    /// 按了 Esc，放弃录制。
    Cancelled,
}

/// egui 按键对应的主键；Esc 留作取消，egui 认识但本程序不支持的键返回 `None`。
pub fn key_of(key: egui::Key) -> Option<Key> {
    if key == egui::Key::Escape {
        return None;
    }
    // egui 的按键名与快捷键文字的写法一致，借解析器统一取值范围
    format!("Ctrl+{}", key.name())
        .parse::<Hotkey>()
        .ok()
        .map(|h| h.key())
}

/// egui-winit 把剪切、复制、粘贴组合吞成专用事件，这里按当时的修饰键还原主键。
fn clipboard_key(event: &egui::Event, modifiers: egui::Modifiers) -> Option<Key> {
    let shift_only = modifiers.shift && !modifiers.ctrl;
    match event {
        egui::Event::Copy => Some(Key::Letter('C')),
        egui::Event::Cut if shift_only => Some(Key::Named(NamedKey::Delete)),
        egui::Event::Cut => Some(Key::Letter('X')),
        egui::Event::Paste(_) if shift_only => Some(Key::Named(NamedKey::Insert)),
        egui::Event::Paste(_) => Some(Key::Letter('V')),
        _ => None,
    }
}

fn classify(event: &egui::Event, current: egui::Modifiers) -> Option<Capture> {
    match event {
        egui::Event::Key {
            key,
            physical_key,
            pressed: true,
            repeat: false,
            modifiers,
        } => {
            if *key == egui::Key::Escape {
                return Some(Capture::Cancelled);
            }
            // 优先物理键位，输入法或非英文布局下逻辑键可能不是字母本身
            let main = physical_key.and_then(key_of).or_else(|| key_of(*key))?;
            Some(Capture::Recorded {
                key: main,
                modifiers: *modifiers,
            })
        }
        other => clipboard_key(other, current).map(|key| Capture::Recorded {
            key,
            modifiers: current,
        }),
    }
}

/// 是否属于录制态下要吞掉的键盘输入。
fn is_keyboard_input(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::Key { .. }
            | egui::Event::Text(_)
            | egui::Event::Copy
            | egui::Event::Cut
            | egui::Event::Paste(_)
            | egui::Event::Ime(_)
    )
}

/// 从本帧事件里取第一个录制结论，并移除全部键盘输入，避免按键同时触发页面控件。
pub fn take_capture(events: &mut Vec<egui::Event>, current: egui::Modifiers) -> Option<Capture> {
    let capture = events.iter().find_map(|event| classify(event, current));
    events.retain(|event| !is_keyboard_input(event));
    capture
}

/// 把录到的组合写入选择：Ctrl/Alt/Shift 以这次按键为准，Win 保持原样（egui 拿不到 Win）。
pub fn apply_capture(choice: &mut HotkeyChoice, key: Key, modifiers: egui::Modifiers) {
    choice.key = key;
    choice.ctrl = modifiers.ctrl;
    choice.alt = modifiers.alt;
    choice.shift = modifiers.shift;
}

#[cfg(test)]
mod tests {
    use super::*;
    use lenswin_core::hotkey::selectable_keys;

    fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn maps_supported_keys_and_rejects_others() {
        assert_eq!(key_of(egui::Key::A), Some(Key::Letter('A')));
        assert_eq!(key_of(egui::Key::Num7), Some(Key::Digit(7)));
        assert_eq!(key_of(egui::Key::F13), Some(Key::Function(13)));
        assert_eq!(key_of(egui::Key::ArrowUp), Some(Key::Named(NamedKey::Up)));
        assert_eq!(
            key_of(egui::Key::PageDown),
            Some(Key::Named(NamedKey::PageDown))
        );
        assert_eq!(key_of(egui::Key::Escape), None);
        assert_eq!(key_of(egui::Key::F25), None);
        assert_eq!(key_of(egui::Key::Minus), None);
    }

    #[test]
    fn only_escape_and_printscreen_are_unrecordable() {
        let missing: Vec<Key> = selectable_keys()
            .into_iter()
            .filter(|k| !egui::Key::ALL.iter().any(|e| key_of(*e) == Some(*k)))
            .collect();
        assert_eq!(
            missing,
            vec![
                Key::Named(NamedKey::Escape),
                Key::Named(NamedKey::PrintScreen)
            ]
        );
    }

    #[test]
    fn records_first_press_and_swallows_keyboard_events() {
        let mods = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
        let mut events = vec![
            egui::Event::Text("k".to_string()),
            key_event(egui::Key::K, mods),
            key_event(egui::Key::J, mods),
            egui::Event::PointerGone,
        ];
        let capture = take_capture(&mut events, mods);
        assert_eq!(
            capture,
            Some(Capture::Recorded {
                key: Key::Letter('K'),
                modifiers: mods
            })
        );
        assert_eq!(events, vec![egui::Event::PointerGone]);
    }

    #[test]
    fn repeats_releases_and_unknown_keys_keep_waiting() {
        let mut events = vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::CTRL,
            },
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: true,
                modifiers: egui::Modifiers::CTRL,
            },
            key_event(egui::Key::Minus, egui::Modifiers::CTRL),
        ];
        assert_eq!(take_capture(&mut events, egui::Modifiers::CTRL), None);
        assert!(events.is_empty());
    }

    #[test]
    fn escape_cancels() {
        let mut events = vec![key_event(egui::Key::Escape, egui::Modifiers::NONE)];
        assert_eq!(
            take_capture(&mut events, egui::Modifiers::NONE),
            Some(Capture::Cancelled)
        );
    }

    #[test]
    fn physical_key_wins_over_logical() {
        let mut events = vec![egui::Event::Key {
            key: egui::Key::Q,
            physical_key: Some(egui::Key::A),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::ALT,
        }];
        assert!(matches!(
            take_capture(&mut events, egui::Modifiers::ALT),
            Some(Capture::Recorded {
                key: Key::Letter('A'),
                ..
            })
        ));
    }

    #[test]
    fn clipboard_shortcuts_are_recovered() {
        let ctrl = egui::Modifiers::CTRL;
        let shift = egui::Modifiers::SHIFT;
        let cases = [
            (egui::Event::Copy, ctrl, Key::Letter('C')),
            (egui::Event::Cut, ctrl, Key::Letter('X')),
            (egui::Event::Cut, shift, Key::Named(NamedKey::Delete)),
            (egui::Event::Paste("x".into()), ctrl, Key::Letter('V')),
            (
                egui::Event::Paste("x".into()),
                shift,
                Key::Named(NamedKey::Insert),
            ),
        ];
        for (event, mods, expected) in cases {
            let mut events = vec![event];
            assert_eq!(
                take_capture(&mut events, mods),
                Some(Capture::Recorded {
                    key: expected,
                    modifiers: mods
                })
            );
        }
    }

    #[test]
    fn capture_overwrites_ctrl_alt_shift_but_keeps_win() {
        let mut choice = HotkeyChoice::parse("Ctrl+Win+A").expect("解析");
        apply_capture(&mut choice, Key::Function(5), egui::Modifiers::SHIFT);
        assert_eq!(choice.to_text().expect("生成"), "Shift+Win+F5");
        let mut plain = HotkeyChoice::parse("Ctrl+A").expect("解析");
        apply_capture(&mut plain, Key::Letter('B'), egui::Modifiers::ALT);
        assert_eq!(plain.to_text().expect("生成"), "Alt+B");
    }
}
