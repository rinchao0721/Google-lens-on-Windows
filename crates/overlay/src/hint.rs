//! 快捷键提示：按交互阶段列出当前可用的按键，纯数据，不接触 Win32。

/// 一行提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hint {
    /// 按键文字。
    pub keys: &'static str,
    /// 作用说明。
    pub desc: &'static str,
}

const fn hint(keys: &'static str, desc: &'static str) -> Hint {
    Hint { keys, desc }
}

/// 提示面板的显示状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintState {
    /// 显示。
    Shown,
    /// 隐藏，可按 `?` 重新显示。
    Hidden,
}

impl HintState {
    /// 切换显示与隐藏。
    pub const fn toggled(self) -> Self {
        match self {
            HintState::Shown => HintState::Hidden,
            HintState::Hidden => HintState::Shown,
        }
    }
}

/// 决定提示内容的交互阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// 尚未开始框选。
    Idle,
    /// 正在拖拽框选。
    Drawing,
    /// 矩形选区确定后的调整阶段。
    Adjusting,
}

/// 生成提示所需的可选功能。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Features {
    /// 放大镜开启，可取色。
    pub magnifier: bool,
    /// 窗口吸附可用。
    pub snap: bool,
    /// 有历史选区可切换。
    pub history: bool,
}

/// 按阶段和可用功能列出提示行。
pub fn hints(stage: Stage, features: Features) -> Vec<Hint> {
    let mut out = Vec::with_capacity(9);
    match stage {
        Stage::Idle => {
            out.push(hint("W A S D", "移动光标 1 像素"));
            if features.magnifier {
                out.push(hint("C", "复制光标处颜色"));
                out.push(hint("Shift", "切换颜色格式"));
            }
            if features.snap {
                out.push(hint("Tab", "开关窗口吸附"));
            }
            if features.history {
                out.push(hint(", .", "切换历史选区"));
            }
            out.push(hint("右键 / Esc", "退出"));
        }
        Stage::Drawing => {
            out.push(hint("W A S D", "微调光标"));
            out.push(hint("右键", "放弃本次框选"));
            out.push(hint("Esc", "退出"));
        }
        Stage::Adjusting => {
            out.push(hint("方向键", "移动选区"));
            out.push(hint("Ctrl + 方向键", "扩大选区"));
            out.push(hint("Shift + 方向键", "缩小选区"));
            out.push(hint("Enter / 双击", "复制"));
            if features.history {
                out.push(hint(", .", "切换历史选区"));
            }
            out.push(hint("右键", "重新框选"));
            out.push(hint("Esc", "退出"));
        }
    }
    out.push(hint("?", "隐藏提示"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Features = Features {
        magnifier: true,
        snap: true,
        history: true,
    };
    const NONE: Features = Features {
        magnifier: false,
        snap: false,
        history: false,
    };

    fn keys(list: &[Hint]) -> Vec<&'static str> {
        list.iter().map(|h| h.keys).collect()
    }

    #[test]
    fn idle_lists_optional_features_only_when_available() {
        let full = keys(&hints(Stage::Idle, ALL));
        assert!(full.contains(&"C") && full.contains(&"Tab") && full.contains(&", ."));
        let bare = keys(&hints(Stage::Idle, NONE));
        assert!(!bare.contains(&"C") && !bare.contains(&"Tab") && !bare.contains(&", ."));
        assert_eq!(bare.first(), Some(&"W A S D"));
    }

    #[test]
    fn adjusting_lists_nudge_keys() {
        let list = keys(&hints(Stage::Adjusting, NONE));
        assert!(list.contains(&"方向键") && list.contains(&"Enter / 双击"));
        assert!(!list.contains(&"C"));
    }

    #[test]
    fn every_stage_ends_with_toggle_hint() {
        for stage in [Stage::Idle, Stage::Drawing, Stage::Adjusting] {
            assert_eq!(hints(stage, ALL).last().map(|h| h.keys), Some("?"));
        }
    }

    #[test]
    fn state_toggles() {
        assert_eq!(HintState::Shown.toggled(), HintState::Hidden);
        assert_eq!(HintState::Hidden.toggled(), HintState::Shown);
    }
}
