//! 选区工具：只维护几何状态，不接触任何 Win32 对象，便于单元测试。

mod lasso;
mod rect;

use lenswin_core::config::LassoConfig;
use lenswin_core::error::SelectionError;
use lenswin_core::geometry::{Point, Rect};
use lenswin_core::monitor::MonitorInfo;
use lenswin_core::selection::{Selection, SelectionMode};

pub use lasso::LassoTool;
pub use rect::RectTool;

/// 渲染层据此绘制拖拽中的预览。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preview<'a> {
    /// 没有进行中的拖拽。
    None,
    /// 矩形框选的当前范围，已限制在起始显示器内。
    Rect(Rect),
    /// 套索轨迹，`bounds` 为按当前轨迹算出的最终裁剪范围。
    Lasso {
        /// 虚拟桌面坐标下的轨迹点。
        trail: &'a [Point],
        /// 外扩并与显示器求交后的范围，无效时为 `None`。
        bounds: Option<Rect>,
    },
}

/// 一种选区交互方式。
pub trait SelectionTool {
    /// 在 `monitor` 上按下鼠标，开始一次拖拽。
    fn press(&mut self, point: Point, monitor: &MonitorInfo, monitor_index: usize);
    /// 拖拽中移动鼠标。
    fn drag(&mut self, point: Point);
    /// 松开鼠标，得到选区；范围无效时返回错误且工具回到空闲状态。
    fn release(&mut self, point: Point) -> Result<Selection, SelectionError>;
    /// 当前预览。
    fn preview(&self) -> Preview<'_>;
    /// 放弃当前拖拽。
    fn reset(&mut self);
}

/// 按模式创建工具。
pub fn tool_for(mode: SelectionMode, lasso: &LassoConfig) -> Box<dyn SelectionTool> {
    match mode {
        SelectionMode::Rect => Box::new(RectTool::new()),
        SelectionMode::Lasso => Box::new(LassoTool::new(lasso.padding, lasso.stroke_width)),
    }
}
