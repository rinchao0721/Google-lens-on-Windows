//! 截屏覆盖层与调用方之间传递的数据：流程、结果、取色格式、历史选区与窗口吸附。

use crate::config::AnnotateConfig;
use crate::frame::CroppedImage;
use crate::geometry::{Point, Rect};
use crate::selection::Selection;

/// 覆盖层流程。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayFlow {
    /// 选完立即返回，如以图搜图。
    Direct,
    /// 矩形选区可继续调整，由工具栏决定动作，如截屏。
    Toolbar,
    /// 选区落定后保留为可调整的选框，每次调整回投一次，如内置弹窗搜索。
    Live,
}

/// 常驻选框会话回投给调用方的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveEvent {
    /// 选框落定或调整后发生变化，需要重新搜索。
    Committed(Selection),
    /// 用户退出。
    Closed,
    /// 覆盖层出错，会话已不可用。
    Failed(String),
}

/// 工具栏上可选的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarAction {
    /// 复制到剪贴板。
    Copy,
    /// 保存为文件。
    Save,
    /// 以图搜图。
    Search,
}

/// 覆盖层的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayResult {
    /// 直接流程得到的选区。
    Selected(Selection),
    /// 工具栏流程得到的选区、用户选的动作和合成好的成图（含标注）。
    Action(Selection, ToolbarAction, CroppedImage),
    /// 取色结果，已按用户选择的格式格式化。
    Color(String),
}

/// 一次覆盖层会话的完整产出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayOutput {
    /// 用户的选择；取消为 `None`。
    pub result: Option<OverlayResult>,
    /// 本次会话改动了标注样式时的新样式，取消截屏也会带回；没有变化为 `None`。
    pub annotate: Option<AnnotateConfig>,
}

/// 取色文本格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorFormat {
    /// `#RRGGBB`。
    #[default]
    Hex,
    /// `R, G, B`。
    Rgb,
}

impl ColorFormat {
    /// 切换到下一种格式。
    pub const fn next(self) -> Self {
        match self {
            ColorFormat::Hex => ColorFormat::Rgb,
            ColorFormat::Rgb => ColorFormat::Hex,
        }
    }

    /// 按格式输出颜色文本。
    pub fn format(self, r: u8, g: u8, b: u8) -> String {
        match self {
            ColorFormat::Hex => format!("#{r:02X}{g:02X}{b:02X}"),
            ColorFormat::Rgb => format!("{r}, {g}, {b}"),
        }
    }
}

/// 把新选区放到历史最前；与已有记录重复时只移动位置，超出 `capacity` 时丢弃最旧的。
pub fn push_history(history: &mut Vec<Rect>, rect: Rect, capacity: usize) {
    if rect.is_empty() {
        return;
    }
    history.retain(|r| *r != rect);
    history.insert(0, rect);
    history.truncate(capacity);
}

/// 按 Z 序（最上层在前）找出包含该点的第一个窗口范围。
pub fn topmost_containing(windows: &[Rect], point: Point) -> Option<Rect> {
    windows.iter().copied().find(|r| r.contains(point))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_formats() {
        assert_eq!(ColorFormat::Hex.format(255, 0, 10), "#FF000A");
        assert_eq!(ColorFormat::Rgb.format(255, 0, 10), "255, 0, 10");
        assert_eq!(ColorFormat::Hex.format(0, 0, 0), "#000000");
    }

    #[test]
    fn color_format_cycles() {
        assert_eq!(ColorFormat::default(), ColorFormat::Hex);
        assert_eq!(ColorFormat::Hex.next(), ColorFormat::Rgb);
        assert_eq!(ColorFormat::Rgb.next(), ColorFormat::Hex);
    }

    #[test]
    fn history_puts_newest_first_and_caps() {
        let mut h = Vec::new();
        push_history(&mut h, Rect::new(0, 0, 1, 1), 2);
        push_history(&mut h, Rect::new(1, 1, 1, 1), 2);
        push_history(&mut h, Rect::new(2, 2, 1, 1), 2);
        assert_eq!(h, vec![Rect::new(2, 2, 1, 1), Rect::new(1, 1, 1, 1)]);
    }

    #[test]
    fn history_moves_duplicate_to_front() {
        let mut h = vec![Rect::new(1, 1, 1, 1), Rect::new(0, 0, 1, 1)];
        push_history(&mut h, Rect::new(0, 0, 1, 1), 10);
        assert_eq!(h, vec![Rect::new(0, 0, 1, 1), Rect::new(1, 1, 1, 1)]);
    }

    #[test]
    fn history_ignores_empty_and_zero_capacity() {
        let mut h = Vec::new();
        push_history(&mut h, Rect::new(0, 0, 0, 5), 10);
        assert!(h.is_empty());
        push_history(&mut h, Rect::new(0, 0, 5, 5), 0);
        assert!(h.is_empty());
    }

    #[test]
    fn topmost_window_wins() {
        let windows = [Rect::new(10, 10, 10, 10), Rect::new(0, 0, 100, 100)];
        assert_eq!(
            topmost_containing(&windows, Point::new(15, 15)),
            Some(Rect::new(10, 10, 10, 10))
        );
        assert_eq!(
            topmost_containing(&windows, Point::new(50, 50)),
            Some(Rect::new(0, 0, 100, 100))
        );
        assert_eq!(topmost_containing(&windows, Point::new(-1, 5)), None);
        assert_eq!(topmost_containing(&[], Point::new(0, 0)), None);
    }
}
