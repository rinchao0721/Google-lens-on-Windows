//! 鼠标键盘状态机：与 Win32 无关，窗口层只负责把消息翻译成这里的调用。

use std::rc::Rc;

use lenswin_core::annotation::{Document, PALETTE, Shape, Tool};
use lenswin_core::config::AnnotateConfig;
use lenswin_core::geometry::{Point, Rect};
use lenswin_core::monitor::{MonitorInfo, index_at};
use lenswin_core::selection::{Selection, SelectionShape};
use lenswin_core::snip::{ColorFormat, OverlayFlow, ToolbarAction, topmost_containing};
use lenswin_core::text_box::{TextBox, TextHandle, round_to_i32};

use crate::adjust::{self, Direction, Handle, NudgeMode};
use crate::annotate::{Annotator, EditKey, EditView, StyleView, TextBoxView, TextMeasure};
use crate::hint::{self, Features, Hint, HintState, Stage};
use crate::layout::{self, Hit, StripSpec, ToolbarButton, ToolbarLayout};
use crate::modes::{Preview, SelectionTool};

/// 按下和松开相距不超过该像素数时视为单击，用于窗口吸附。
const CLICK_SLOP: i64 = 2;

/// 一次选区交互的结局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 直接流程得到的选区。
    Selected(Selection),
    /// 工具栏流程得到的选区和动作。
    Action(Selection, ToolbarAction),
    /// 取色结果。
    Color(String),
    /// 用户取消。
    Cancelled,
}

/// 覆盖层内响应的按键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// 取消。
    Escape,
    /// 确认复制。
    Enter,
    /// 开关窗口吸附。
    Tab,
    /// 方向键。
    Arrow(Direction),
    /// `WASD` 移动光标。
    CursorStep(Direction),
    /// 复制颜色。
    CopyColor,
    /// 切换颜色格式。
    Shift,
    /// 更早的历史选区。
    OlderHistory,
    /// 更新的历史选区。
    NewerHistory,
    /// 显示或隐藏快捷键提示。
    ToggleHints,
    /// 编辑文字时的编辑键。
    Edit(EditKey),
    /// 编辑文字时粘贴剪贴板快照。
    Paste,
    /// 删除选中的文字。
    Delete,
    /// 撤销上一个标注。
    Undo,
    /// 重做被撤销的标注。
    Redo,
}

/// 需要窗口层调用系统 API 完成的按键效果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCommand {
    /// 把系统光标移动若干像素。
    MoveCursor(i32, i32),
    /// 读取光标处像素颜色并结束。
    PickColor(Point),
}

/// 光标形状。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorKind {
    /// 十字。
    Cross,
    /// 文字输入的 I 形光标。
    Text,
    /// 手形，旋转手柄用。
    Hand,
    /// 箭头。
    Arrow,
    /// 四向移动。
    Move,
    /// 上下。
    SizeNs,
    /// 左右。
    SizeWe,
    /// 左上右下。
    SizeNwse,
    /// 右上左下。
    SizeNesw,
}

/// 窗口吸附的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapState {
    /// 不可用：套索模式、配置关闭或没有窗口快照。
    Unavailable,
    /// 已开启。
    On,
    /// 用户按 Tab 临时关闭。
    Off,
}

/// 构造交互所需的选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// 流程。
    pub flow: OverlayFlow,
    /// 是否显示放大镜。
    pub magnifier: bool,
    /// 窗口吸附状态。
    pub snap: SnapState,
    /// 可吸附的窗口范围，最上层在前。
    pub windows: Vec<Rect>,
    /// 历史选区，最新的在前。
    pub history: Vec<Rect>,
    /// 快捷键提示的初始状态。
    pub hints: HintState,
    /// 标注工具的初始样式，只在截屏流程用到。
    pub annotate: AnnotateConfig,
    /// 冻结时的剪贴板文本快照，已清洗，文字标注粘贴用。
    pub clipboard_text: Option<String>,
}

/// 矩形选区的边框装饰。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameStyle {
    /// 不画装饰。
    Plain,
    /// 八个方形调整手柄（截屏流程）。
    Handles,
    /// 四角圆角折线（常驻选框）。
    Corners,
}

/// 工具栏与样式条的绘制内容。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolbarView {
    /// 布局。
    pub layout: ToolbarLayout,
    /// 光标悬停的控件。
    pub hovered: Option<Hit>,
    /// 当前选中的标注工具。
    pub active: Option<Tool>,
    /// 撤销按钮可用。
    pub can_undo: bool,
    /// 重做按钮可用。
    pub can_redo: bool,
    /// 样式条内容，没有选中工具时为 `None`。
    pub style: Option<StyleView>,
}

/// 需要画在冻结帧上的标注。
#[derive(Debug, Clone, PartialEq)]
pub struct Annotations<'a> {
    /// 已完成的图元，按添加顺序。
    pub shapes: &'a [Shape],
    /// 已完成图元的版本号，变化时绘制缓存失效。
    pub revision: u64,
    /// 重新编辑时被隐藏的图元序号。
    pub hidden: Option<usize>,
    /// 正在画的图元。
    pub draft: Option<Shape>,
}

/// 编辑中的文本框及光标状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorView {
    /// 文本框内容与样式。
    pub edit: EditView,
    /// 光标闪烁到可见的相位。
    pub caret_visible: bool,
}

/// 窗口层绘制某个显示器时需要的内容。
#[derive(Debug, Clone, PartialEq)]
pub struct View<'a> {
    /// 高亮区域或套索轨迹。
    pub preview: Preview<'a>,
    /// 矩形选区的边框装饰。
    pub frame: FrameStyle,
    /// 需要标注尺寸的矩形。
    pub label: Option<Rect>,
    /// 工具栏与样式条。
    pub toolbar: Option<ToolbarView>,
    /// 标注图元。
    pub annotations: Annotations<'a>,
    /// 编辑中的文本框，只在它所在的显示器上有值。
    pub editor: Option<EditorView>,
    /// 选中或编辑中的文字文本框，画虚线框和手柄。
    pub text_box: Option<TextBoxView>,
    /// 放大镜：光标位置与颜色格式。
    pub magnifier: Option<(Point, ColorFormat)>,
    /// 快捷键提示，为空表示不显示。
    pub hints: Vec<Hint>,
    /// 光标位置，仅当光标在该显示器上时有值，供提示面板避让。
    pub cursor: Option<Point>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Grab {
    handle: Handle,
    origin: Point,
    start: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Adjusting {
    rect: Rect,
    monitor: usize,
    grab: Option<Grab>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Drawing { window: usize, press: Point },
    Adjusting(Adjusting),
}

/// 交互状态：持有选区工具并记录结局。
pub struct Interaction {
    tool: Box<dyn SelectionTool>,
    monitors: Vec<MonitorInfo>,
    options: Options,
    phase: Phase,
    hover: Option<Point>,
    history_index: Option<usize>,
    color_format: ColorFormat,
    outcome: Option<Outcome>,
    /// 常驻选框最近一次回投的选框；重画只是单击时据此恢复，也用来跳过没有变化的松手。
    committed: Option<Adjusting>,
    /// 尚未取走的常驻选框回投。
    commits: Vec<Selection>,
    annotator: Annotator,
    caret_visible: bool,
    blink_reset: bool,
    focus_request: Option<usize>,
}

/// 窗口客户区坐标转虚拟桌面坐标；窗口左上角即显示器左上角。
fn to_virtual(monitor: &MonitorInfo, local: (i32, i32)) -> Option<Point> {
    Some(Point::new(
        monitor.bounds.x.checked_add(local.0)?,
        monitor.bounds.y.checked_add(local.1)?,
    ))
}

/// 解码鼠标消息的 `lparam`：低 16 位为 x，高 16 位为 y，均为有符号数，
/// 鼠标被捕获时可能是负值或超出窗口。
pub fn point_from_lparam(lparam: isize) -> (i32, i32) {
    let signed = |bits: isize| {
        let word = u16::try_from(bits & 0xFFFF).unwrap_or(0);
        i32::from(i16::from_ne_bytes(word.to_ne_bytes()))
    };
    (signed(lparam), signed(lparam >> 16))
}

fn is_click(a: Point, b: Point) -> bool {
    (i64::from(a.x) - i64::from(b.x)).abs() <= CLICK_SLOP
        && (i64::from(a.y) - i64::from(b.y)).abs() <= CLICK_SLOP
}

/// 文字文本框上各部位的光标：缩放按旋转后的方向取最接近的系统缩放光标。
fn text_cursor(handle: TextHandle, geometry: &TextBox) -> CursorKind {
    let center = geometry.center();
    let target = match handle {
        TextHandle::Body | TextHandle::Border => return CursorKind::Move,
        TextHandle::Rotate => return CursorKind::Hand,
        TextHandle::Corner(corner) => geometry.corner(corner),
        TextHandle::Edge(edge) => geometry
            .edge_handles()
            .iter()
            .find(|(e, _)| *e == edge)
            .map_or(center, |(_, p)| *p),
    };
    resize_cursor(target.0 - center.0, target.1 - center.1)
}

/// 由中心指向手柄的方向取最接近的缩放光标（屏幕坐标，y 向下）。
fn resize_cursor(dx: f64, dy: f64) -> CursorKind {
    let degrees = round_to_i32(dy.atan2(dx).to_degrees()).rem_euclid(180);
    match ((degrees + 22) / 45) % 4 {
        0 => CursorKind::SizeWe,
        1 => CursorKind::SizeNwse,
        2 => CursorKind::SizeNs,
        _ => CursorKind::SizeNesw,
    }
}

fn cursor_for(handle: Handle) -> CursorKind {
    match handle {
        Handle::N | Handle::S => CursorKind::SizeNs,
        Handle::E | Handle::W => CursorKind::SizeWe,
        Handle::NW | Handle::SE => CursorKind::SizeNwse,
        Handle::NE | Handle::SW => CursorKind::SizeNesw,
        Handle::Body => CursorKind::Move,
    }
}

impl Interaction {
    /// 创建交互，`monitors` 的顺序即窗口序号。
    pub fn new(tool: Box<dyn SelectionTool>, monitors: Vec<MonitorInfo>, options: Options) -> Self {
        let annotator = Annotator::new(&options.annotate);
        Self {
            tool,
            monitors,
            options,
            phase: Phase::Idle,
            hover: None,
            history_index: None,
            color_format: ColorFormat::default(),
            outcome: None,
            committed: None,
            commits: Vec::new(),
            annotator,
            caret_visible: true,
            blink_reset: false,
            focus_request: None,
        }
    }

    /// 换上真实的文字度量，窗口层用 DirectWrite 提供。
    pub fn with_measure(mut self, measure: Rc<dyn TextMeasure>) -> Self {
        self.annotator.set_measure(measure);
        self
    }

    fn is_live(&self) -> bool {
        self.options.flow == OverlayFlow::Live
    }

    fn point(&self, window: usize, local: (i32, i32)) -> Option<Point> {
        self.monitors.get(window).and_then(|m| to_virtual(m, local))
    }

    fn monitor_bounds(&self, index: usize) -> Option<Rect> {
        self.monitors.get(index).map(|m| m.bounds)
    }

    fn dpi(&self, index: usize) -> u32 {
        self.monitors.get(index).map_or(96, |m| m.dpi)
    }

    fn toolbar(&self, adj: &Adjusting) -> Option<ToolbarLayout> {
        if self.options.flow != OverlayFlow::Toolbar {
            return None;
        }
        let monitor = self.monitor_bounds(adj.monitor)?;
        let strip = self.annotator.tool().map(|tool| StripSpec {
            has_color: tool.has_color(),
            number_style: tool == Tool::Number,
            appearance: tool == Tool::Text,
            orientation: tool == Tool::Text,
        });
        Some(layout::toolbar_layout(
            &adj.rect,
            &monitor,
            self.dpi(adj.monitor),
            strip,
        ))
    }

    /// 截屏流程里已选中标注工具。
    fn annotating(&self) -> bool {
        self.options.flow == OverlayFlow::Toolbar && self.annotator.tool().is_some()
    }

    /// 处理按在工具栏或样式条上的点击。
    fn press_toolbar(&mut self, adj: Adjusting, hit: Option<Hit>) {
        match hit {
            Some(Hit::Button(button)) => match button {
                ToolbarButton::Tool(tool) => self.annotator.toggle_tool(tool),
                ToolbarButton::Undo => {
                    self.annotator.undo();
                }
                ToolbarButton::Redo => {
                    self.annotator.redo();
                }
                ToolbarButton::Search => self.finish_with_action(adj, ToolbarAction::Search),
                ToolbarButton::Save => self.finish_with_action(adj, ToolbarAction::Save),
                ToolbarButton::Copy => self.finish_with_action(adj, ToolbarAction::Copy),
                ToolbarButton::Cancel => self.finish(Outcome::Cancelled),
            },
            Some(Hit::Swatch(index)) => {
                if let Some(color) = PALETTE.get(index) {
                    self.annotator.set_color(*color);
                }
            }
            Some(Hit::NumberStyle(style)) => self.annotator.set_number_style(style),
            Some(Hit::Background(on)) => self.annotator.set_text_background(on),
            Some(Hit::Vertical(on)) => self.annotator.set_text_vertical(on),
            Some(Hit::Preset(index)) => self.annotator.apply_preset(index),
            None => {}
        }
    }

    /// 光标下可吸附的窗口范围，已限制在光标所在显示器内。
    fn snap_candidate(&self) -> Option<(Rect, usize)> {
        if self.options.snap != SnapState::On || self.phase != Phase::Idle {
            return None;
        }
        let hover = self.hover?;
        let index = index_at(&self.monitors, hover)?;
        let monitor = self.monitor_bounds(index)?;
        let rect = topmost_containing(&self.options.windows, hover)?.intersect(&monitor)?;
        Some((rect, index))
    }

    fn finish(&mut self, outcome: Outcome) {
        if self.outcome.is_none() {
            self.outcome = Some(outcome);
        }
        self.tool.reset();
        self.phase = Phase::Idle;
    }

    /// 选区成形后：工具栏流程的矩形进入调整，其余直接结束。
    fn accept(&mut self, selection: Selection) {
        let is_rect = matches!(selection.shape(), SelectionShape::Rect);
        match (self.options.flow, is_rect) {
            (OverlayFlow::Toolbar, true) => {
                self.phase = Phase::Adjusting(Adjusting {
                    rect: selection.bounds(),
                    monitor: selection.monitor_index(),
                    grab: None,
                });
            }
            (OverlayFlow::Toolbar, false) => {
                self.finish(Outcome::Action(selection, ToolbarAction::Copy));
            }
            (OverlayFlow::Direct, _) => self.finish(Outcome::Selected(selection)),
            // 套索也收成外接矩形，即送去搜索的范围
            (OverlayFlow::Live, _) => self.settle(Adjusting {
                rect: selection.bounds(),
                monitor: selection.monitor_index(),
                grab: None,
            }),
        }
    }

    /// 常驻选框落定；与上次回投的不同才记一次回投。
    fn settle(&mut self, adj: Adjusting) {
        let adj = Adjusting { grab: None, ..adj };
        self.phase = Phase::Adjusting(adj);
        let unchanged = self
            .committed
            .is_some_and(|c| c.rect == adj.rect && c.monitor == adj.monitor);
        if unchanged {
            return;
        }
        if let Some(selection) = self.adjusted_selection(&adj) {
            self.committed = Some(adj);
            self.commits.push(selection);
        }
    }

    /// 常驻选框的重画没有成形时恢复旧选框；否则回到空闲。
    fn restore_or_idle(&mut self) {
        self.phase = match (self.is_live(), self.committed) {
            (true, Some(adj)) => Phase::Adjusting(adj),
            _ => Phase::Idle,
        };
    }

    fn adjusted_selection(&self, adj: &Adjusting) -> Option<Selection> {
        Selection::new(SelectionShape::Rect, adj.rect, adj.monitor).ok()
    }

    fn finish_with_action(&mut self, adj: Adjusting, action: ToolbarAction) {
        if let Some(selection) = self.adjusted_selection(&adj) {
            self.finish(Outcome::Action(selection, action));
        }
    }

    /// 在序号为 `window` 的显示器窗口上按下左键。
    pub fn press(&mut self, window: usize, local: (i32, i32)) {
        if self.outcome.is_some() {
            return;
        }
        let Some(p) = self.point(window, local) else {
            return;
        };
        self.hover = Some(p);
        self.sync_text_limit();
        match self.phase {
            Phase::Drawing { .. } => return,
            Phase::Adjusting(mut adj) => {
                if let Some(toolbar) = self.toolbar(&adj)
                    && toolbar.covers(p)
                {
                    let hit = toolbar.hit(p);
                    // 样式条改的是当前文本框，其它按钮先确认文字再执行
                    if matches!(hit, Some(Hit::Button(_))) {
                        self.annotator.confirm_text();
                    }
                    self.press_toolbar(adj, hit);
                    return;
                }
                let text_tool = self.annotator.tool() == Some(Tool::Text) && self.annotating();
                if text_tool {
                    // 文字手柄优先于截屏选区手柄
                    match (
                        self.text_handle_at(p, adj.monitor),
                        self.annotator.is_editing(),
                    ) {
                        (Some(TextHandle::Body), true) => return,
                        (Some(handle), _) => {
                            self.annotator.begin_text_adjust(handle, p);
                            return;
                        }
                        _ => {}
                    }
                    if self.annotator.is_editing() {
                        // 点框外只确认；点到别的文字则顺带选中它
                        self.annotator.confirm_text();
                        self.select_text_at(p);
                        return;
                    }
                } else if self.annotator.is_editing() {
                    self.annotator.confirm_text();
                    return;
                }
                let hit = adjust::hit_test(&adj.rect, p, layout::grip(self.dpi(adj.monitor)));
                match hit {
                    // 有工具时选区内部用来画图，手柄仍然优先缩放
                    Some(Handle::Body) if self.annotating() => {
                        if text_tool {
                            // 单击已有文字选中它；点空白处时有选中就只取消选中，否则新建文本框
                            if self.select_text_at(p) || self.annotator.deselect_text() {
                                return;
                            }
                            if let Some(bounds) = self.monitor_bounds(adj.monitor) {
                                self.annotator.begin_text(p, &bounds, adj.monitor);
                                self.focus_request = Some(adj.monitor);
                                self.touch_caret();
                            }
                        } else if let Some(bounds) = self.monitor_bounds(adj.monitor) {
                            self.annotator.press(p, &bounds);
                        }
                        return;
                    }
                    Some(handle) => {
                        adj.grab = Some(Grab {
                            handle,
                            origin: p,
                            start: adj.rect,
                        });
                        self.phase = Phase::Adjusting(adj);
                        return;
                    }
                    None if self.annotating() => {
                        if text_tool {
                            self.annotator.deselect_text();
                        }
                        return;
                    }
                    None => {
                        self.phase = Phase::Idle;
                        self.history_index = None;
                    }
                }
            }
            Phase::Idle => {}
        }
        let Some(monitor) = self.monitors.get(window) else {
            self.restore_or_idle();
            return;
        };
        self.tool.press(p, monitor, window);
        self.phase = Phase::Drawing { window, press: p };
    }

    /// 屏幕点命中当前文字文本框的哪个部位。
    fn text_handle_at(&self, p: Point, monitor: usize) -> Option<TextHandle> {
        let dpi = self.dpi(monitor);
        self.annotator.text_handle_at(
            p,
            f64::from(layout::grip(dpi)),
            f64::from(layout::text_rotate_distance(dpi)),
        )
    }

    /// 选中 `p` 处最上面的文字；没有文字返回 `false`。
    fn select_text_at(&mut self, p: Point) -> bool {
        match self.annotator.document().text_at(p) {
            Some(index) => self.annotator.select_text(index),
            None => false,
        }
    }

    /// 移动鼠标；拖拽时 `window` 是起始窗口（鼠标已被捕获）。
    pub fn mouse_move(&mut self, window: usize, local: (i32, i32)) {
        if self.outcome.is_some() {
            return;
        }
        let Some(p) = self.point(window, local) else {
            return;
        };
        self.hover = Some(p);
        self.sync_text_limit();
        if self.annotator.is_adjusting_text() {
            self.annotator.update_text_adjust(p);
            return;
        }
        if self.annotator.is_drawing() {
            if let Phase::Adjusting(adj) = self.phase
                && let Some(bounds) = self.monitor_bounds(adj.monitor)
            {
                self.annotator.drag(p, &bounds);
            }
            return;
        }
        match self.phase {
            Phase::Drawing { .. } => self.tool.drag(p),
            Phase::Adjusting(mut adj) => {
                let Some(grab) = adj.grab else {
                    return;
                };
                let Some(monitor) = self.monitor_bounds(adj.monitor) else {
                    return;
                };
                let dx = i64::from(p.x) - i64::from(grab.origin.x);
                let dy = i64::from(p.y) - i64::from(grab.origin.y);
                if let Some(rect) = adjust::drag(&grab.start, grab.handle, dx, dy, &monitor) {
                    adj.rect = rect;
                    self.phase = Phase::Adjusting(adj);
                }
            }
            Phase::Idle => {}
        }
    }

    /// 松开左键；单击且光标下有可吸附窗口时选中该窗口，否则无效选区回到空闲。
    pub fn release(&mut self, window: usize, local: (i32, i32)) {
        if self.outcome.is_some() {
            return;
        }
        if self.annotator.is_adjusting_text() {
            if let Some(p) = self.point(window, local) {
                self.hover = Some(p);
                self.annotator.update_text_adjust(p);
            }
            self.annotator.finish_text_adjust();
            return;
        }
        if self.annotator.is_drawing() {
            self.finish_annotation(window, local);
            return;
        }
        match self.phase {
            Phase::Drawing { press, .. } => {
                let Some(p) = self.point(window, local) else {
                    self.tool.reset();
                    self.restore_or_idle();
                    return;
                };
                self.phase = Phase::Idle;
                // 常驻选框已有选框时，单击只是放弃重画，不吸附窗口
                let can_snap = !(self.is_live() && self.committed.is_some());
                // 先按空闲状态取吸附候选，再结束拖拽
                let snapped = (can_snap && is_click(press, p))
                    .then(|| {
                        self.hover = Some(press);
                        self.snap_candidate()
                    })
                    .flatten();
                let drawn = self.tool.release(p);
                self.hover = Some(p);
                let selection = match snapped {
                    Some((rect, index)) => Selection::new(SelectionShape::Rect, rect, index).ok(),
                    None => drawn.ok(),
                };
                match selection {
                    Some(selection) => self.accept(selection),
                    None => self.restore_or_idle(),
                }
            }
            Phase::Adjusting(adj) => {
                if self.is_live() && adj.grab.is_some() {
                    self.settle(adj);
                } else {
                    self.phase = Phase::Adjusting(Adjusting { grab: None, ..adj });
                }
            }
            Phase::Idle => {}
        }
    }

    /// 标注拖拽结束：图元有效就加入文档。
    fn finish_annotation(&mut self, window: usize, local: (i32, i32)) {
        let bounds = match self.phase {
            Phase::Adjusting(adj) => self.monitor_bounds(adj.monitor),
            _ => None,
        };
        match (self.point(window, local), bounds) {
            (Some(p), Some(bounds)) => {
                self.hover = Some(p);
                self.annotator.release(p, &bounds);
            }
            _ => self.annotator.cancel_drawing(),
        }
    }

    /// 文字工具下双击已有文字：重新编辑。双击的第一下若已建出空文本框先丢弃；
    /// 没有命中文字时返回 `false`，按普通按下处理。
    fn reedit_text(&mut self, window: usize, local: (i32, i32)) -> bool {
        if self.options.flow != OverlayFlow::Toolbar || self.annotator.tool() != Some(Tool::Text) {
            return false;
        }
        let (Phase::Adjusting(adj), Some(p)) = (self.phase, self.point(window, local)) else {
            return false;
        };
        // 双击换行边手柄：取消自动换行
        if let Some(TextHandle::Edge(_)) = self.text_handle_at(p, adj.monitor) {
            self.annotator.finish_text_adjust();
            // 没有换行可取消时按普通双击处理，贴边的文字仍然能双击进入编辑
            if self.annotator.clear_wrap() {
                return true;
            }
        }
        if !adj.rect.contains(p) || self.toolbar(&adj).is_some_and(|t| t.covers(p)) {
            return false;
        }
        if self.annotator.editing_blank_new() {
            self.annotator.discard_text();
        }
        if self.annotator.is_editing() {
            return false;
        }
        let Some(index) = self.annotator.document().text_at(p) else {
            return false;
        };
        if !self.annotator.begin_edit_existing(index, adj.monitor) {
            return false;
        }
        self.hover = Some(p);
        self.focus_request = Some(adj.monitor);
        self.touch_caret();
        true
    }

    /// 双击：在调整中的选区内双击等同于复制，其余情况按普通按下处理；有工具时双击只是普通按下。
    pub fn double_click(&mut self, window: usize, local: (i32, i32)) {
        if self.reedit_text(window, local) {
            return;
        }
        if let (Phase::Adjusting(adj), Some(p)) = (self.phase, self.point(window, local))
            && self.options.flow == OverlayFlow::Toolbar
            && self.annotator.tool().is_none()
            && adj.rect.contains(p)
            && !self.toolbar(&adj).is_some_and(|t| t.covers(p))
        {
            self.finish_with_action(adj, ToolbarAction::Copy);
            return;
        }
        self.press(window, local);
    }

    /// 右键：放弃正在进行的选区；没有选区时取消整个覆盖层。常驻选框直接退出。
    pub fn right_click(&mut self) {
        if self.is_live() {
            self.cancel();
            return;
        }
        // 编辑中右键等同于点框外：确认文字；选中状态下先取消选中
        if self.annotator.is_editing() {
            self.annotator.confirm_text();
            return;
        }
        if self.annotator.deselect_text() {
            return;
        }
        if self.annotator.exit_tool() {
            return;
        }
        match self.phase {
            Phase::Idle => self.cancel(),
            Phase::Drawing { .. } | Phase::Adjusting(_) => {
                self.tool.reset();
                self.phase = Phase::Idle;
                self.history_index = None;
            }
        }
    }

    /// 按键；`repeat` 表示按住不放产生的重复消息。
    pub fn key(&mut self, key: Key, mode: NudgeMode, repeat: bool) -> Option<KeyCommand> {
        if self.outcome.is_some() {
            return None;
        }
        if self.annotator.is_editing() {
            self.editing_key(key);
            return None;
        }
        if self.annotator.selected_text().is_some() && self.annotating() {
            match key {
                Key::Delete => {
                    self.annotator.delete_selected();
                    return None;
                }
                Key::Escape => {
                    self.annotator.deselect_text();
                    return None;
                }
                Key::Arrow(direction) => {
                    let step = if mode == NudgeMode::Shrink { 10 } else { 1 };
                    let (dx, dy) = direction.delta();
                    self.annotator
                        .nudge_selected(dx.saturating_mul(step), dy.saturating_mul(step));
                    return None;
                }
                _ => {}
            }
        }
        match key {
            Key::Escape => {
                if self.is_live() || !self.annotator.exit_tool() {
                    self.cancel();
                }
            }
            Key::Undo | Key::Redo => {
                if matches!(self.phase, Phase::Adjusting(_))
                    && self.options.flow == OverlayFlow::Toolbar
                {
                    if key == Key::Undo {
                        self.annotator.undo();
                    } else {
                        self.annotator.redo();
                    }
                }
            }
            Key::Enter => {
                if let (Phase::Adjusting(adj), OverlayFlow::Toolbar) =
                    (self.phase, self.options.flow)
                {
                    self.finish_with_action(adj, ToolbarAction::Copy);
                }
            }
            Key::Tab => {
                self.options.snap = match self.options.snap {
                    SnapState::On => SnapState::Off,
                    SnapState::Off => SnapState::On,
                    SnapState::Unavailable => SnapState::Unavailable,
                };
            }
            Key::CursorStep(direction) => {
                let (dx, dy) = direction.delta();
                return Some(KeyCommand::MoveCursor(dx, dy));
            }
            Key::Arrow(direction) => return self.arrow(direction, mode),
            Key::CopyColor => {
                // 常驻选框只回投选区，取色会结束会话却没有调用方接收颜色
                if !self.is_live() && self.magnifier_visible() {
                    return self.hover.map(KeyCommand::PickColor);
                }
            }
            Key::Shift => {
                if !repeat && self.magnifier_visible() {
                    self.color_format = self.color_format.next();
                }
            }
            Key::OlderHistory => self.step_history(true),
            Key::NewerHistory => self.step_history(false),
            Key::ToggleHints => self.options.hints = self.options.hints.toggled(),
            Key::Edit(_) | Key::Paste | Key::Delete => {}
        }
        None
    }

    /// 编辑文字时的按键：其它覆盖层快捷键全部暂停。
    fn editing_key(&mut self, key: Key) {
        match key {
            Key::Escape | Key::Undo => {
                self.annotator.edit_key(EditKey::Discard);
            }
            Key::Enter => {
                self.annotator.edit_key(EditKey::Confirm);
            }
            Key::Edit(edit) => {
                self.annotator.edit_key(edit);
                self.touch_caret();
            }
            Key::Paste => {
                if let Some(text) = &self.options.clipboard_text {
                    self.annotator.insert_text(text);
                    self.touch_caret();
                }
            }
            _ => {}
        }
    }

    fn touch_caret(&mut self) {
        self.caret_visible = true;
        self.blink_reset = true;
    }

    /// 正在编辑文字。
    pub fn is_editing(&self) -> bool {
        self.annotator.is_editing()
    }

    /// 插入一个字符（`WM_CHAR`）。
    pub fn char_input(&mut self, c: char) {
        if self.annotator.insert_char(c) {
            self.touch_caret();
        }
    }

    /// 设置输入法组字串；`cursor` 是组字光标在其中的字符序号。
    pub fn ime_composition(&mut self, text: &str, cursor: usize) {
        self.annotator.set_composition(text, cursor);
        self.touch_caret();
    }

    /// 输入法提交的结果，按普通输入插入。
    pub fn ime_commit(&mut self, text: &str) {
        if self.annotator.insert_text(text) {
            self.touch_caret();
        }
    }

    /// 组字结束。
    pub fn ime_end(&mut self) {
        self.annotator.clear_composition();
    }

    /// 光标所在显示器序号和所在行的范围（虚拟桌面坐标），输入法候选窗据此定位；没有编辑时为 `None`。
    pub fn ime_target(&self) -> Option<(usize, Rect, bool)> {
        self.annotator.caret_line()
    }

    /// 切换光标闪烁相位。
    pub fn toggle_caret(&mut self) {
        self.caret_visible = !self.caret_visible;
    }

    /// 取走「光标计时器需要重新计时」的标记。
    pub fn take_blink_reset(&mut self) -> bool {
        std::mem::take(&mut self.blink_reset)
    }

    /// 取走键盘焦点应移到哪块显示器窗口的请求。
    pub fn take_focus_request(&mut self) -> Option<usize> {
        self.focus_request.take()
    }

    fn arrow(&mut self, direction: Direction, mode: NudgeMode) -> Option<KeyCommand> {
        let Phase::Adjusting(mut adj) = self.phase else {
            let (dx, dy) = direction.delta();
            return Some(KeyCommand::MoveCursor(dx, dy));
        };
        // 常驻选框每次微调都会触发搜索，容易被限流，不支持键盘微调
        if adj.grab.is_some() || self.is_live() || self.annotator.is_drawing() {
            return None;
        }
        let monitor = self.monitor_bounds(adj.monitor)?;
        if let Some(rect) = adjust::nudge(&adj.rect, direction, mode, &monitor) {
            adj.rect = rect;
            self.phase = Phase::Adjusting(adj);
        }
        None
    }

    /// 在历史选区间切换，只在工具栏流程且没有拖拽时生效。
    fn step_history(&mut self, older: bool) {
        if self.options.flow != OverlayFlow::Toolbar || self.options.history.is_empty() {
            return;
        }
        let busy = match self.phase {
            Phase::Drawing { .. } => true,
            Phase::Adjusting(adj) => adj.grab.is_some() || self.annotator.is_drawing(),
            Phase::Idle => false,
        };
        if busy {
            return;
        }
        let last = self.options.history.len().saturating_sub(1);
        let next = match (self.history_index, older) {
            (None, true) => 0,
            (None, false) => return,
            (Some(i), true) => (i + 1).min(last),
            (Some(0), false) => {
                self.history_index = None;
                self.phase = Phase::Idle;
                self.annotator.exit_tool();
                return;
            }
            (Some(i), false) => i - 1,
        };
        let Some(rect) = self.options.history.get(next).copied() else {
            return;
        };
        let center = Point::new(
            i32::try_from(i64::from(rect.x) + i64::from(rect.width / 2)).unwrap_or(rect.x),
            i32::try_from(i64::from(rect.y) + i64::from(rect.height / 2)).unwrap_or(rect.y),
        );
        let Some(index) = index_at(&self.monitors, center) else {
            return;
        };
        let Some(clipped) = self.monitor_bounds(index).and_then(|m| rect.intersect(&m)) else {
            return;
        };
        self.history_index = Some(next);
        self.phase = Phase::Adjusting(Adjusting {
            rect: clipped,
            monitor: index,
            grab: None,
        });
    }

    /// 窗口层读到颜色后调用，结束覆盖层。
    pub fn finish_color(&mut self, rgb: (u8, u8, u8)) {
        let (r, g, b) = rgb;
        let text = self.color_format.format(r, g, b);
        self.finish(Outcome::Color(text));
    }

    /// 取消并结束。
    pub fn cancel(&mut self) {
        self.finish(Outcome::Cancelled);
    }

    /// 是否需要捕获鼠标：拖拽框选或拖动手柄期间。
    pub fn is_capturing(&self) -> bool {
        if self.annotator.is_drawing() || self.annotator.is_adjusting_text() {
            return true;
        }
        match self.phase {
            Phase::Drawing { .. } => true,
            Phase::Adjusting(adj) => adj.grab.is_some(),
            Phase::Idle => false,
        }
    }

    fn magnifier_visible(&self) -> bool {
        if !self.options.magnifier || self.hover.is_none() {
            return false;
        }
        match self.phase {
            Phase::Idle => true,
            Phase::Drawing { .. } => matches!(self.tool.preview(), Preview::Rect(_)),
            Phase::Adjusting(adj) => {
                adj.grab.is_some() && !self.is_live() && self.annotator.tool().is_none()
            }
        }
    }

    /// 记录 Shift 是否按下，标注拖拽时据此约束形状。
    pub fn set_shift(&mut self, down: bool) {
        self.annotator.set_shift(down);
    }

    /// 文字字号上限跟随选区所在的显示器。
    fn sync_text_limit(&mut self) {
        if let Phase::Adjusting(adj) = self.phase
            && let Some(bounds) = self.monitor_bounds(adj.monitor)
        {
            self.annotator.set_text_limit(bounds.height);
        }
    }

    /// 滚轮：`notches` 为正表示向上滚；调整当前标注工具的粗细，没有工具时无作用。
    pub fn wheel(&mut self, notches: i32) {
        self.sync_text_limit();
        if matches!(self.phase, Phase::Adjusting(_)) && self.options.flow == OverlayFlow::Toolbar {
            self.annotator.wheel(notches);
        }
    }

    /// 已完成的标注，合成成图时使用。
    pub fn document(&self) -> &Document {
        self.annotator.document()
    }

    /// 本次会话改动了标注样式时的新样式。
    pub fn annotate_change(&self) -> Option<AnnotateConfig> {
        self.annotator.changed_config()
    }

    /// 覆盖层打开时的光标位置，让提示面板和放大镜在第一次移动鼠标前就能显示。
    pub fn set_hover(&mut self, p: Point) {
        if self.hover.is_none() {
            self.hover = Some(p);
        }
    }

    /// 提示面板所在的显示器：光标所在的那块，还不知道光标位置时用第一块。
    fn hint_window(&self) -> usize {
        self.hover
            .and_then(|p| index_at(&self.monitors, p))
            .unwrap_or(0)
    }

    fn current_hints(&self) -> Vec<Hint> {
        if self.options.hints == HintState::Hidden {
            return Vec::new();
        }
        let stage = match self.phase {
            Phase::Idle => Stage::Idle,
            Phase::Drawing { .. } => Stage::Drawing,
            Phase::Adjusting(_) if self.is_live() => Stage::LiveAdjusting,
            Phase::Adjusting(_) if self.annotator.is_editing() => Stage::Editing,
            Phase::Adjusting(_) if self.annotating() => Stage::Annotating,
            Phase::Adjusting(_) => Stage::Adjusting,
        };
        let features = Features {
            magnifier: self.options.magnifier,
            snap: self.options.snap != SnapState::Unavailable,
            history: self.options.flow == OverlayFlow::Toolbar && !self.options.history.is_empty(),
        };
        hint::hints(stage, features)
    }

    /// 当前有内容需要绘制的窗口序号，状态变化前后取并集即为需要重绘的窗口。
    pub fn painted_windows(&self) -> Vec<usize> {
        // 标注可能留在别的显示器上，撤销重做要让所有窗口跟着变
        if self.options.flow == OverlayFlow::Toolbar
            && (!self.annotator.document().is_empty() || self.annotator.can_redo())
        {
            return (0..self.monitors.len()).collect();
        }
        let mut out = Vec::with_capacity(2);
        out.push(self.hint_window());
        let active = match self.phase {
            Phase::Drawing { window, .. } => Some(window),
            Phase::Adjusting(adj) => Some(adj.monitor),
            Phase::Idle => None,
        };
        if let Some(i) = active.filter(|i| !out.contains(i)) {
            out.push(i);
        }
        out
    }

    /// 序号为 `window` 的显示器需要绘制的内容。
    pub fn view(&self, window: usize) -> View<'_> {
        let mut view = View {
            preview: Preview::None,
            frame: FrameStyle::Plain,
            label: None,
            toolbar: None,
            annotations: Annotations {
                shapes: self.annotator.document().shapes(),
                revision: self.annotator.document().revision(),
                hidden: self.annotator.hidden_shape(),
                draft: self
                    .annotator
                    .preview()
                    .or_else(|| self.annotator.text_preview()),
            },
            text_box: self.annotator.active_text_box(),
            editor: self
                .annotator
                .edit_view()
                .filter(|v| v.monitor == window)
                .map(|edit| EditorView {
                    edit,
                    caret_visible: self.caret_visible,
                }),
            magnifier: None,
            hints: Vec::new(),
            cursor: self
                .hover
                .filter(|p| index_at(&self.monitors, *p) == Some(window)),
        };
        match self.phase {
            Phase::Idle => {
                if let Some((rect, _)) = self.snap_candidate().filter(|(_, i)| *i == window) {
                    view.preview = Preview::Rect(rect);
                    view.label = Some(rect);
                }
            }
            Phase::Drawing { window: active, .. } => {
                if active == window {
                    view.preview = self.tool.preview();
                    if let Preview::Rect(r) = view.preview {
                        view.label = Some(r);
                    }
                }
            }
            Phase::Adjusting(adj) => {
                if adj.monitor == window && self.is_live() {
                    view.preview = Preview::Rect(adj.rect);
                    view.frame = FrameStyle::Corners;
                } else if adj.monitor == window {
                    view.preview = Preview::Rect(adj.rect);
                    view.frame = FrameStyle::Handles;
                    view.label = Some(adj.rect);
                    if adj.grab.is_none() {
                        view.toolbar = self.toolbar(&adj).map(|layout| ToolbarView {
                            hovered: self.hover.and_then(|p| layout.hit(p)),
                            layout,
                            active: self.annotator.tool(),
                            can_undo: self.annotator.can_undo(),
                            can_redo: self.annotator.can_redo(),
                            style: self.annotator.style_view(),
                        });
                    }
                }
            }
        }
        if self.magnifier_visible()
            && let Some(p) = self.hover
            && index_at(&self.monitors, p) == Some(window)
        {
            view.magnifier = Some((p, self.color_format));
        }
        if self.hint_window() == window {
            view.hints = self.current_hints();
        }
        view
    }

    /// 光标在当前位置应有的形状。
    pub fn cursor_kind(&self) -> CursorKind {
        let Phase::Adjusting(adj) = self.phase else {
            return CursorKind::Cross;
        };
        if let Some(grab) = adj.grab {
            return cursor_for(grab.handle);
        }
        if self.annotator.is_drawing() {
            return CursorKind::Cross;
        }
        if let (Some(handle), Some(view)) = (
            self.annotator.adjusting_handle(),
            self.annotator.active_text_box(),
        ) {
            return text_cursor(handle, &view.geometry);
        }
        let Some(p) = self.hover else {
            return CursorKind::Cross;
        };
        if self.toolbar(&adj).is_some_and(|t| t.covers(p)) {
            return CursorKind::Arrow;
        }
        if let (Some(handle), Some(view)) = (
            self.text_handle_at(p, adj.monitor),
            self.annotator.active_text_box(),
        ) {
            return text_cursor(handle, &view.geometry);
        }
        let hit = adjust::hit_test(&adj.rect, p, layout::grip(self.dpi(adj.monitor)));
        match hit {
            Some(Handle::Body) if self.annotator.tool() == Some(Tool::Text) => CursorKind::Text,
            Some(Handle::Body) if self.annotating() => CursorKind::Cross,
            Some(handle) => cursor_for(handle),
            None if self.annotating() => CursorKind::Arrow,
            None => CursorKind::Cross,
        }
    }

    /// 取走常驻选框尚未回投的选区，按发生顺序排列。
    pub fn take_commits(&mut self) -> Vec<Selection> {
        std::mem::take(&mut self.commits)
    }

    /// 常驻选框是否已经回投过选区，即结果弹窗已在显示。
    pub fn has_committed(&self) -> bool {
        self.committed.is_some()
    }

    /// 已有结局。
    pub fn is_finished(&self) -> bool {
        self.outcome.is_some()
    }

    /// 取走结局。
    pub fn take_outcome(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::EditKey;
    use crate::modes::{LassoTool, RectTool};
    use lenswin_core::annotation::NumberStyle;

    fn monitors() -> Vec<MonitorInfo> {
        vec![
            MonitorInfo {
                bounds: Rect::new(1920, 0, 1920, 1080),
                dpi: 120,
                primary: false,
            },
            MonitorInfo {
                bounds: Rect::new(0, 0, 1920, 1080),
                dpi: 96,
                primary: true,
            },
        ]
    }

    fn options(flow: OverlayFlow) -> Options {
        Options {
            flow,
            magnifier: true,
            snap: SnapState::Unavailable,
            windows: Vec::new(),
            history: Vec::new(),
            hints: HintState::Shown,
            annotate: AnnotateConfig::default(),
            clipboard_text: None,
        }
    }

    fn rect(flow: OverlayFlow) -> Interaction {
        Interaction::new(Box::new(RectTool::new()), monitors(), options(flow))
    }

    fn lasso() -> Interaction {
        Interaction::new(
            Box::new(LassoTool::new(10, 4)),
            monitors(),
            options(OverlayFlow::Direct),
        )
    }

    /// 在主显示器（窗口 1）上从 (100,100) 拖到 (500,400)，进入调整状态。
    fn adjusting() -> Interaction {
        let mut it = rect(OverlayFlow::Toolbar);
        it.press(1, (100, 100));
        it.mouse_move(1, (300, 300));
        it.release(1, (500, 400));
        assert!(!it.is_finished());
        it
    }

    fn adjusted_rect(it: &Interaction) -> Option<Rect> {
        match it.view(1).preview {
            Preview::Rect(r) => Some(r),
            _ => None,
        }
    }

    #[test]
    fn lparam_decodes_signed_coordinates() {
        assert_eq!(point_from_lparam(0x0064_00C8), (200, 100));
        // x = -5 (0xFFFB)，y = -1 (0xFFFF)
        let packed = isize::try_from(0xFFFF_FFFB_u32).expect("u32 可放入 isize");
        assert_eq!(point_from_lparam(packed), (-5, -1));
        assert_eq!(point_from_lparam(0), (0, 0));
    }

    #[test]
    fn direct_rect_selection_on_secondary_monitor_uses_virtual_coordinates() {
        let mut it = rect(OverlayFlow::Direct);
        it.press(0, (100, 200));
        assert!(it.is_capturing());
        it.mouse_move(0, (300, 400));
        it.release(0, (500, 600));
        match it.take_outcome() {
            Some(Outcome::Selected(sel)) => {
                assert_eq!(sel.bounds(), Rect::new(2020, 200, 400, 400));
                assert_eq!(sel.monitor_index(), 0);
            }
            other => panic!("期望得到选区，实际为 {other:?}"),
        }
    }

    #[test]
    fn mouse_capture_allows_negative_local_coordinates() {
        let mut it = rect(OverlayFlow::Direct);
        it.press(0, (50, 50));
        it.release(0, (-300, 200));
        match it.take_outcome() {
            Some(Outcome::Selected(sel)) => {
                // 超出起始显示器左边界的部分被截掉
                assert_eq!(sel.bounds(), Rect::new(1920, 50, 50, 150));
            }
            other => panic!("期望得到选区，实际为 {other:?}"),
        }
    }

    #[test]
    fn click_without_drag_keeps_overlay_open() {
        let mut it = rect(OverlayFlow::Direct);
        it.press(1, (10, 10));
        it.release(1, (10, 10));
        assert!(!it.is_finished());
        assert!(!it.is_capturing());
        it.press(1, (10, 10));
        it.release(1, (200, 200));
        assert!(it.is_finished());
    }

    #[test]
    fn lasso_click_without_drag_keeps_overlay_open() {
        let mut it = lasso();
        it.press(1, (10, 10));
        it.release(1, (10, 10));
        assert!(!it.is_finished());
        it.press(1, (10, 10));
        it.mouse_move(1, (12, 11));
        it.release(1, (11, 12));
        assert!(!it.is_finished());
    }

    #[test]
    fn lasso_flow_on_primary_monitor() {
        let mut it = lasso();
        it.press(1, (100, 100));
        it.mouse_move(1, (200, 100));
        it.mouse_move(1, (200, 200));
        it.release(1, (100, 200));
        match it.take_outcome() {
            Some(Outcome::Selected(sel)) => {
                assert_eq!(sel.bounds(), Rect::new(88, 88, 124, 124));
                assert_eq!(sel.monitor_index(), 1);
            }
            other => panic!("期望得到选区，实际为 {other:?}"),
        }
    }

    #[test]
    fn toolbar_flow_enters_adjusting_with_handles_and_toolbar() {
        let it = adjusting();
        let view = it.view(1);
        assert_eq!(view.preview, Preview::Rect(Rect::new(100, 100, 400, 300)));
        assert_eq!(view.frame, FrameStyle::Handles);
        assert_eq!(view.label, Some(Rect::new(100, 100, 400, 300)));
        assert!(view.toolbar.is_some());
        assert_eq!(it.view(0).preview, Preview::None);
    }

    #[test]
    fn dragging_corner_handle_resizes() {
        let mut it = adjusting();
        it.press(1, (500, 400));
        assert!(it.is_capturing());
        assert_eq!(it.cursor_kind(), CursorKind::SizeNwse);
        it.mouse_move(1, (600, 450));
        it.release(1, (600, 450));
        assert!(!it.is_capturing());
        assert_eq!(adjusted_rect(&it), Some(Rect::new(100, 100, 500, 350)));
    }

    #[test]
    fn dragging_body_moves_selection() {
        let mut it = adjusting();
        it.press(1, (300, 250));
        it.mouse_move(1, (310, 240));
        it.release(1, (310, 240));
        assert_eq!(adjusted_rect(&it), Some(Rect::new(110, 90, 400, 300)));
    }

    #[test]
    fn pressing_outside_starts_a_new_selection() {
        let mut it = adjusting();
        it.press(1, (1000, 800));
        it.release(1, (1100, 900));
        assert_eq!(adjusted_rect(&it), Some(Rect::new(1000, 800, 100, 100)));
    }

    fn bar_layout(it: &Interaction) -> ToolbarLayout {
        it.view(1).toolbar.map(|t| t.layout).expect("工具栏")
    }

    /// 在主显示器窗口上点按钮中心。
    fn click_button(it: &mut Interaction, button: ToolbarButton) {
        let r = bar_layout(it).button_rect(button).expect("按钮");
        it.press(1, (r.x + 2, r.y + 2));
        it.release(1, (r.x + 2, r.y + 2));
    }

    fn drag_on_window(it: &mut Interaction, from: (i32, i32), to: (i32, i32)) {
        it.press(1, from);
        it.mouse_move(1, to);
        it.release(1, to);
    }

    #[test]
    fn toolbar_buttons_finish_with_action_or_cancel() {
        let mut it = adjusting();
        let save = bar_layout(&it)
            .button_rect(ToolbarButton::Save)
            .expect("按钮");
        it.mouse_move(1, (save.x + 2, save.y + 2));
        assert_eq!(
            it.view(1).toolbar.map(|t| t.hovered),
            Some(Some(Hit::Button(ToolbarButton::Save)))
        );
        assert_eq!(it.cursor_kind(), CursorKind::Arrow);
        it.press(1, (save.x + 2, save.y + 2));
        match it.take_outcome() {
            Some(Outcome::Action(sel, ToolbarAction::Save)) => {
                assert_eq!(sel.bounds(), Rect::new(100, 100, 400, 300));
            }
            other => panic!("期望保存动作，实际为 {other:?}"),
        }

        let mut it = adjusting();
        click_button(&mut it, ToolbarButton::Cancel);
        assert_eq!(it.take_outcome(), Some(Outcome::Cancelled));

        for (button, expected) in [
            (ToolbarButton::Search, ToolbarAction::Search),
            (ToolbarButton::Copy, ToolbarAction::Copy),
        ] {
            let mut it = adjusting();
            click_button(&mut it, button);
            assert!(matches!(
                it.take_outcome(),
                Some(Outcome::Action(_, action)) if action == expected
            ));
        }
    }

    #[test]
    fn pressing_toolbar_padding_does_not_start_a_new_selection() {
        let mut it = adjusting();
        let bar = bar_layout(&it).bar;
        it.press(1, (bar.x + 1, bar.y + 1));
        it.release(1, (bar.x + 1, bar.y + 1));
        assert_eq!(adjusted_rect(&it), Some(Rect::new(100, 100, 400, 300)));
    }

    #[test]
    fn tool_button_selects_tool_and_shows_strip() {
        let mut it = adjusting();
        assert_eq!(it.view(1).toolbar.map(|t| t.active), Some(None));
        assert!(bar_layout(&it).strip.is_none());
        click_button(&mut it, ToolbarButton::Tool(Tool::Arrow));
        let toolbar = it.view(1).toolbar.expect("工具栏");
        assert_eq!(toolbar.active, Some(Tool::Arrow));
        assert!(toolbar.layout.strip.is_some());
        assert_eq!(toolbar.style.map(|s| s.size), Some(3));
        click_button(&mut it, ToolbarButton::Tool(Tool::Arrow));
        assert_eq!(it.view(1).toolbar.map(|t| t.active), Some(None));
    }

    #[test]
    fn drawing_inside_selection_adds_shape_and_captures_mouse() {
        let mut it = adjusting();
        click_button(&mut it, ToolbarButton::Tool(Tool::Rectangle));
        it.press(1, (200, 200));
        assert!(it.is_capturing());
        assert_eq!(it.cursor_kind(), CursorKind::Cross);
        it.mouse_move(1, (300, 260));
        assert!(it.view(1).annotations.draft.is_some());
        it.release(1, (300, 260));
        assert!(!it.is_capturing());
        let view = it.view(1);
        assert_eq!(view.annotations.shapes.len(), 1);
        assert!(view.annotations.draft.is_none());
        assert_eq!(adjusted_rect(&it), Some(Rect::new(100, 100, 400, 300)));
        assert!(view.toolbar.is_some_and(|t| t.can_undo && !t.can_redo));
    }

    #[test]
    fn handles_win_over_drawing_and_outside_is_ignored() {
        let mut it = adjusting();
        click_button(&mut it, ToolbarButton::Tool(Tool::Rectangle));
        // 右下角手柄：缩放选区而不是画图
        it.press(1, (500, 400));
        assert_eq!(it.cursor_kind(), CursorKind::SizeNwse);
        it.mouse_move(1, (600, 450));
        it.release(1, (600, 450));
        assert_eq!(adjusted_rect(&it), Some(Rect::new(100, 100, 500, 350)));
        assert!(it.view(1).annotations.shapes.is_empty());
        drag_on_window(&mut it, (1000, 800), (1100, 900));
        assert_eq!(adjusted_rect(&it), Some(Rect::new(100, 100, 500, 350)));
        assert!(it.view(1).annotations.shapes.is_empty());
        assert_eq!(it.cursor_kind(), CursorKind::Arrow);
    }

    #[test]
    fn shapes_stay_in_place_when_selection_moves() {
        let mut it = adjusting();
        click_button(&mut it, ToolbarButton::Tool(Tool::Line));
        drag_on_window(&mut it, (150, 150), (250, 250));
        it.right_click();
        drag_on_window(&mut it, (300, 300), (320, 310));
        assert_eq!(adjusted_rect(&it), Some(Rect::new(120, 110, 400, 300)));
        let shapes = it.view(1).annotations.shapes.to_vec();
        assert!(matches!(
            shapes.first(),
            Some(Shape::Line { from, to, .. }) if *from == Point::new(150, 150) && *to == Point::new(250, 250)
        ));
    }

    #[test]
    fn escape_and_right_click_leave_tool_before_cancelling() {
        let mut it = adjusting();
        click_button(&mut it, ToolbarButton::Tool(Tool::Pen));
        it.key(Key::Escape, NudgeMode::Move, false);
        assert!(!it.is_finished());
        assert_eq!(it.view(1).toolbar.map(|t| t.active), Some(None));
        click_button(&mut it, ToolbarButton::Tool(Tool::Pen));
        it.right_click();
        assert!(!it.is_finished());
        assert_eq!(adjusted_rect(&it), Some(Rect::new(100, 100, 400, 300)));
        it.right_click();
        assert_eq!(adjusted_rect(&it), None);
        it.right_click();
        assert_eq!(it.take_outcome(), Some(Outcome::Cancelled));

        let mut it = adjusting();
        click_button(&mut it, ToolbarButton::Tool(Tool::Pen));
        it.key(Key::Escape, NudgeMode::Move, false);
        it.key(Key::Escape, NudgeMode::Move, false);
        assert_eq!(it.take_outcome(), Some(Outcome::Cancelled));
    }

    #[test]
    fn escape_during_drawing_discards_the_stroke() {
        let mut it = adjusting();
        click_button(&mut it, ToolbarButton::Tool(Tool::Pen));
        it.press(1, (200, 200));
        it.mouse_move(1, (230, 230));
        it.key(Key::Escape, NudgeMode::Move, false);
        assert!(!it.is_capturing());
        assert!(!it.is_finished());
        it.release(1, (230, 230));
        assert!(it.view(1).annotations.shapes.is_empty());
    }

    #[test]
    fn undo_redo_via_buttons_and_keys() {
        let mut it = adjusting();
        click_button(&mut it, ToolbarButton::Tool(Tool::Line));
        drag_on_window(&mut it, (150, 150), (250, 250));
        drag_on_window(&mut it, (150, 200), (250, 300));
        assert_eq!(it.view(1).annotations.shapes.len(), 2);
        click_button(&mut it, ToolbarButton::Undo);
        assert_eq!(it.view(1).annotations.shapes.len(), 1);
        assert!(it.view(1).toolbar.is_some_and(|t| t.can_redo));
        it.key(Key::Redo, NudgeMode::Move, false);
        assert_eq!(it.view(1).annotations.shapes.len(), 2);
        it.key(Key::Undo, NudgeMode::Move, false);
        it.key(Key::Undo, NudgeMode::Move, false);
        it.key(Key::Undo, NudgeMode::Move, false);
        assert!(it.view(1).annotations.shapes.is_empty());
        assert!(
            it.view(1)
                .toolbar
                .is_some_and(|t| !t.can_undo && t.can_redo)
        );
        click_button(&mut it, ToolbarButton::Redo);
        assert_eq!(it.view(1).annotations.shapes.len(), 1);
        // 新图元清空重做栈
        drag_on_window(&mut it, (150, 220), (250, 320));
        assert!(it.view(1).toolbar.is_some_and(|t| !t.can_redo));
    }

    #[test]
    fn undo_keys_do_nothing_outside_adjusting_or_other_flows() {
        let mut idle = rect(OverlayFlow::Toolbar);
        idle.key(Key::Undo, NudgeMode::Move, false);
        assert!(idle.document().is_empty());
        let mut direct = rect(OverlayFlow::Direct);
        direct.key(Key::Redo, NudgeMode::Move, false);
        assert!(!direct.is_finished());
    }

    #[test]
    fn wheel_changes_size_only_with_a_tool() {
        let mut it = adjusting();
        it.wheel(3);
        assert_eq!(it.annotate_change(), None);
        click_button(&mut it, ToolbarButton::Tool(Tool::Rectangle));
        it.wheel(2);
        assert_eq!(
            it.view(1).toolbar.and_then(|t| t.style).map(|s| s.size),
            Some(5)
        );
        it.wheel(-10);
        assert_eq!(
            it.view(1).toolbar.and_then(|t| t.style).map(|s| s.size),
            Some(1)
        );
        assert_eq!(it.annotate_change().map(|c| c.rectangle.width), Some(1));
        let mut direct = rect(OverlayFlow::Direct);
        direct.wheel(2);
        assert_eq!(direct.annotate_change(), None);
    }

    #[test]
    fn strip_swatches_and_number_style_change_styles() {
        let mut it = adjusting();
        click_button(&mut it, ToolbarButton::Tool(Tool::Number));
        let strip = bar_layout(&it).strip.expect("样式条");
        let swatch = strip.swatches.get(4).copied().expect("色块");
        it.press(1, (swatch.x + 2, swatch.y + 2));
        it.release(1, (swatch.x + 2, swatch.y + 2));
        let style = it.view(1).toolbar.and_then(|t| t.style).expect("样式");
        assert_eq!(style.swatch, Some(4));
        let letter = bar_layout(&it)
            .strip
            .and_then(|s| s.number_styles.get(1).map(|(_, r)| *r))
            .expect("风格按钮");
        it.press(1, (letter.x + 2, letter.y + 2));
        it.release(1, (letter.x + 2, letter.y + 2));
        assert_eq!(
            it.view(1)
                .toolbar
                .and_then(|t| t.style)
                .and_then(|s| s.number_style),
            Some(NumberStyle::Letter)
        );
        // 点击样式条不会在其下方画出序号
        assert!(it.view(1).annotations.shapes.is_empty());
        let change = it.annotate_change().expect("样式变化");
        assert_eq!(change.number.color, "#007AFF");
        assert_eq!(change.number.style, NumberStyle::Letter);
        it.press(1, (300, 300));
        it.release(1, (300, 300));
        assert!(matches!(
            it.view(1).annotations.shapes.first(),
            Some(Shape::Number {
                value: 1,
                style: NumberStyle::Letter,
                ..
            })
        ));
    }

    #[test]
    fn preset_dot_sets_size_without_drawing_beneath() {
        let mut it = adjusting();
        click_button(&mut it, ToolbarButton::Tool(Tool::Mosaic));
        let dot = bar_layout(&it)
            .strip
            .and_then(|s| s.presets.get(2).copied())
            .expect("预设圆点");
        it.mouse_move(1, (dot.x + 2, dot.y + 2));
        assert_eq!(
            it.view(1).toolbar.map(|t| t.hovered),
            Some(Some(Hit::Preset(2)))
        );
        it.press(1, (dot.x + 2, dot.y + 2));
        assert!(!it.is_capturing());
        it.release(1, (dot.x + 2, dot.y + 2));
        let style = it.view(1).toolbar.and_then(|t| t.style).expect("样式");
        assert_eq!((style.size, style.preset), (32, Some(2)));
        assert!(it.view(1).annotations.shapes.is_empty());
        assert_eq!(it.annotate_change().map(|c| c.mosaic.block), Some(32));
    }

    fn text_session(clipboard: Option<&str>) -> Interaction {
        let mut it = Interaction::new(
            Box::new(RectTool::new()),
            monitors(),
            Options {
                clipboard_text: clipboard.map(str::to_string),
                ..options(OverlayFlow::Toolbar)
            },
        );
        it.press(1, (100, 100));
        it.mouse_move(1, (300, 300));
        it.release(1, (500, 400));
        click_button(&mut it, ToolbarButton::Tool(Tool::Text));
        it
    }

    fn type_in(it: &mut Interaction, text: &str) {
        for c in text.chars() {
            it.char_input(c);
        }
    }

    fn only_text(it: &Interaction) -> Option<(String, u32)> {
        match it.view(1).annotations.shapes {
            [Shape::Text { text, size, .. }] => Some((text.clone(), *size)),
            _ => None,
        }
    }

    fn click(it: &mut Interaction, at: (i32, i32)) {
        it.press(1, at);
        it.release(1, at);
    }

    #[test]
    fn clicking_with_text_tool_starts_editing_on_that_monitor() {
        let mut it = text_session(None);
        assert!(!it.is_editing());
        it.mouse_move(1, (200, 200));
        assert_eq!(it.cursor_kind(), CursorKind::Text);
        click(&mut it, (200, 200));
        assert!(it.is_editing());
        assert_eq!(it.take_focus_request(), Some(1));
        assert_eq!(it.take_focus_request(), None);
        assert!(it.view(1).editor.is_some());
        assert!(it.view(0).editor.is_none());
        let (monitor, line, _) = it.ime_target().expect("候选窗目标");
        assert_eq!(monitor, 1);
        assert_eq!((line.x, line.y), (200, 200));
        assert!(!it.is_capturing());
        let keys: Vec<_> = it.view(1).hints.iter().map(|h| h.keys).collect();
        assert!(keys.contains(&"Shift + Enter") && !keys.contains(&"?"));
    }

    #[test]
    fn typing_then_enter_confirms_and_enter_does_not_copy() {
        let mut it = text_session(None);
        click(&mut it, (200, 200));
        type_in(&mut it, "ab");
        it.key(Key::Enter, NudgeMode::Move, false);
        assert!(!it.is_editing() && !it.is_finished());
        assert_eq!(only_text(&it), Some(("ab".to_string(), 24)));
        assert_eq!(it.ime_target(), None);
        it.key(Key::Enter, NudgeMode::Move, false);
        assert!(matches!(
            it.take_outcome(),
            Some(Outcome::Action(_, ToolbarAction::Copy))
        ));
    }

    #[test]
    fn clicking_outside_the_box_only_confirms() {
        let mut it = text_session(None);
        click(&mut it, (200, 200));
        type_in(&mut it, "x");
        click(&mut it, (400, 350));
        assert!(!it.is_editing());
        assert_eq!(only_text(&it).map(|t| t.0), Some("x".to_string()));
        // 选区外和手柄上的点击同样只确认
        click(&mut it, (300, 250));
        type_in(&mut it, "y");
        click(&mut it, (1000, 800));
        assert!(!it.is_editing());
        click(&mut it, (350, 150));
        type_in(&mut it, "z");
        it.press(1, (500, 400));
        assert!(!it.is_editing() && !it.is_capturing());
        assert_eq!(adjusted_rect(&it), Some(Rect::new(100, 100, 400, 300)));
    }

    #[test]
    fn clicking_inside_the_box_keeps_editing() {
        let mut it = text_session(None);
        click(&mut it, (200, 200));
        type_in(&mut it, "abcd");
        click(&mut it, (210, 210));
        assert!(it.is_editing());
        assert!(only_text(&it).is_none());
    }

    #[test]
    fn blank_box_confirms_to_nothing() {
        let mut it = text_session(None);
        click(&mut it, (200, 200));
        type_in(&mut it, "  ");
        click(&mut it, (400, 350));
        assert!(!it.is_editing());
        assert!(it.view(1).annotations.shapes.is_empty());
        assert!(it.view(1).toolbar.is_some_and(|t| !t.can_undo));
    }

    #[test]
    fn toolbar_buttons_confirm_first_but_strip_does_not() {
        let mut it = text_session(None);
        click(&mut it, (200, 200));
        type_in(&mut it, "hi");
        let dot = bar_layout(&it)
            .strip
            .and_then(|s| s.presets.get(2).copied())
            .expect("预设圆点");
        click(&mut it, (dot.x + 2, dot.y + 2));
        assert!(it.is_editing(), "样式条不确认");
        let swatch = bar_layout(&it)
            .strip
            .and_then(|s| s.swatches.get(4).copied())
            .expect("色块");
        click(&mut it, (swatch.x + 2, swatch.y + 2));
        let chip = bar_layout(&it)
            .strip
            .and_then(|s| s.appearance.get(1).map(|(_, r)| *r))
            .expect("外观切换");
        click(&mut it, (chip.x + 2, chip.y + 2));
        assert!(it.is_editing());
        click_button(&mut it, ToolbarButton::Tool(Tool::Rectangle));
        assert!(!it.is_editing());
        match it.view(1).annotations.shapes {
            [
                Shape::Text {
                    size,
                    color,
                    background,
                    ..
                },
            ] => {
                assert_eq!((*size, *color, *background), (36, PALETTE[4], true));
            }
            other => panic!("期望一个文字图元，实际为 {other:?}"),
        }
        assert_eq!(
            it.view(1).toolbar.map(|t| t.active),
            Some(Some(Tool::Rectangle))
        );
    }

    #[test]
    fn search_and_copy_buttons_confirm_before_finishing() {
        let mut it = text_session(None);
        click(&mut it, (200, 200));
        type_in(&mut it, "hi");
        click_button(&mut it, ToolbarButton::Copy);
        assert!(!it.is_editing());
        assert!(matches!(
            it.take_outcome(),
            Some(Outcome::Action(_, ToolbarAction::Copy))
        ));
        assert_eq!(it.document().shapes().len(), 1);
    }

    #[test]
    fn escape_and_ctrl_z_discard_without_cancelling_overlay() {
        for key in [Key::Escape, Key::Undo] {
            let mut it = text_session(None);
            click(&mut it, (200, 200));
            type_in(&mut it, "gone");
            it.key(key, NudgeMode::Move, false);
            assert!(!it.is_editing() && !it.is_finished(), "{key:?}");
            assert!(it.view(1).annotations.shapes.is_empty());
            assert_eq!(it.view(1).toolbar.map(|t| t.active), Some(Some(Tool::Text)));
        }
    }

    #[test]
    fn other_shortcuts_are_paused_while_editing() {
        let mut it = text_session(None);
        click(&mut it, (200, 200));
        let shown = !it.view(1).hints.is_empty();
        assert!(shown);
        for key in [
            Key::CopyColor,
            Key::Shift,
            Key::Tab,
            Key::OlderHistory,
            Key::NewerHistory,
            Key::ToggleHints,
            Key::Redo,
            Key::Arrow(Direction::Left),
            Key::CursorStep(Direction::Up),
        ] {
            assert_eq!(it.key(key, NudgeMode::Move, false), None, "{key:?}");
        }
        assert!(it.is_editing() && !it.is_finished());
        assert_eq!(adjusted_rect(&it), Some(Rect::new(100, 100, 400, 300)));
        assert!(!it.view(1).hints.is_empty(), "提示不应被 ? 切换");
    }

    #[test]
    fn edit_keys_and_newline_reach_the_buffer() {
        let mut it = text_session(None);
        click(&mut it, (200, 200));
        type_in(&mut it, "ab");
        it.key(Key::Edit(EditKey::Newline), NudgeMode::Move, false);
        type_in(&mut it, "c");
        it.key(Key::Edit(EditKey::Left), NudgeMode::Move, false);
        it.key(Key::Edit(EditKey::Backspace), NudgeMode::Move, false);
        it.key(Key::Edit(EditKey::Tab), NudgeMode::Move, false);
        let text = it.view(1).editor.map(|e| e.edit.text);
        assert_eq!(text, Some("ab    c".to_string()));
    }

    #[test]
    fn paste_inserts_the_snapshot_or_nothing() {
        let mut it = text_session(Some("x\ny"));
        click(&mut it, (200, 200));
        it.key(Key::Paste, NudgeMode::Move, false);
        assert_eq!(
            it.view(1).editor.map(|e| e.edit.text),
            Some("x\ny".to_string())
        );
        let mut none = text_session(None);
        click(&mut none, (200, 200));
        none.key(Key::Paste, NudgeMode::Move, false);
        assert_eq!(
            none.view(1).editor.map(|e| e.edit.text),
            Some(String::new())
        );
    }

    #[test]
    fn double_click_reedits_existing_text_and_hides_it() {
        let mut it = text_session(None);
        click(&mut it, (150, 150));
        type_in(&mut it, "hello");
        click(&mut it, (400, 350));
        assert_eq!(only_text(&it).map(|t| t.0), Some("hello".to_string()));
        // 真实的双击顺序：第一下单击选中文字，第二下是 WM_LBUTTONDBLCLK
        it.press(1, (180, 165));
        it.release(1, (180, 165));
        assert!(!it.is_editing());
        assert_eq!(it.annotator.selected_text(), Some(0));
        it.double_click(1, (180, 165));
        it.release(1, (180, 165));
        assert!(it.is_editing());
        assert_eq!(it.view(1).annotations.hidden, Some(0));
        assert_eq!(
            it.view(1).editor.map(|e| e.edit.text),
            Some("hello".to_string())
        );
        assert_eq!(it.take_focus_request(), Some(1));
        type_in(&mut it, "!");
        it.key(Key::Enter, NudgeMode::Move, false);
        assert_eq!(only_text(&it).map(|t| t.0), Some("hello!".to_string()));
        assert_eq!(it.view(1).annotations.hidden, None);
        click_button(&mut it, ToolbarButton::Undo);
        assert_eq!(only_text(&it).map(|t| t.0), Some("hello".to_string()));
    }

    #[test]
    fn reedit_escape_restores_and_blank_deletes_with_undo() {
        let mut it = text_session(None);
        click(&mut it, (150, 150));
        type_in(&mut it, "ab");
        click(&mut it, (400, 350));
        it.press(1, (152, 152));
        it.double_click(1, (152, 152));
        it.release(1, (152, 152));
        it.key(Key::Edit(EditKey::Backspace), NudgeMode::Move, false);
        it.key(Key::Escape, NudgeMode::Move, false);
        assert_eq!(only_text(&it).map(|t| t.0), Some("ab".to_string()));

        it.press(1, (152, 152));
        it.double_click(1, (152, 152));
        it.release(1, (152, 152));
        it.key(Key::Edit(EditKey::Backspace), NudgeMode::Move, false);
        it.key(Key::Edit(EditKey::Backspace), NudgeMode::Move, false);
        it.key(Key::Enter, NudgeMode::Move, false);
        assert!(it.view(1).annotations.shapes.is_empty());
        click_button(&mut it, ToolbarButton::Undo);
        assert_eq!(only_text(&it).map(|t| t.0), Some("ab".to_string()));
    }

    #[test]
    fn double_click_on_empty_space_just_opens_one_new_box() {
        let mut it = text_session(None);
        it.press(1, (300, 300));
        it.release(1, (300, 300));
        it.double_click(1, (300, 300));
        it.release(1, (300, 300));
        assert!(it.is_editing());
        type_in(&mut it, "n");
        it.key(Key::Enter, NudgeMode::Move, false);
        assert_eq!(only_text(&it).map(|t| t.0), Some("n".to_string()));
    }

    #[test]
    fn double_click_never_copies_with_text_tool() {
        let mut it = text_session(None);
        it.double_click(1, (300, 300));
        assert!(!it.is_finished());
    }

    #[test]
    fn right_click_confirms_text_then_next_one_leaves_the_tool() {
        let mut it = text_session(None);
        click(&mut it, (200, 200));
        type_in(&mut it, "r");
        it.right_click();
        assert!(!it.is_editing());
        assert_eq!(only_text(&it).map(|t| t.0), Some("r".to_string()));
        it.right_click();
        assert_eq!(it.view(1).toolbar.map(|t| t.active), Some(None));
        assert!(!it.is_finished());
    }

    #[test]
    fn wheel_changes_the_font_size_of_the_box_being_edited() {
        let mut it = text_session(None);
        click(&mut it, (200, 200));
        type_in(&mut it, "w");
        it.wheel(2);
        assert_eq!(it.view(1).editor.map(|e| e.edit.style.size), Some(28));
        it.key(Key::Enter, NudgeMode::Move, false);
        assert_eq!(only_text(&it).map(|t| t.1), Some(28));
        assert_eq!(it.annotate_change().map(|c| c.text.size), Some(28));
    }

    #[test]
    fn handles_still_resize_and_outside_clicks_are_ignored_without_a_box() {
        let mut it = text_session(None);
        it.press(1, (500, 400));
        it.mouse_move(1, (600, 450));
        it.release(1, (600, 450));
        assert!(!it.is_editing());
        assert_eq!(adjusted_rect(&it), Some(Rect::new(100, 100, 500, 350)));
        click(&mut it, (1000, 800));
        assert!(!it.is_editing());
    }

    #[test]
    fn ime_composition_is_inline_and_result_is_inserted() {
        let mut it = text_session(None);
        click(&mut it, (200, 200));
        it.ime_composition("ni", 2);
        let edit = it.view(1).editor.map(|e| e.edit).expect("编辑中");
        assert_eq!(edit.text, "ni");
        assert_eq!(edit.composition, Some((0, 2)));
        it.ime_commit("你");
        it.ime_composition("", 0);
        it.ime_end();
        let edit = it.view(1).editor.map(|e| e.edit).expect("编辑中");
        assert_eq!((edit.text.as_str(), edit.composition), ("你", None));
        it.ime_composition("hao", 3);
        it.key(Key::Enter, NudgeMode::Move, false);
        assert_eq!(only_text(&it).map(|t| t.0), Some("你".to_string()));
        // 编辑结束后的输入法消息被忽略
        it.ime_commit("x");
        it.ime_composition("y", 1);
        it.char_input('z');
        assert_eq!(only_text(&it).map(|t| t.0), Some("你".to_string()));
    }

    #[test]
    fn candidate_window_follows_the_caret() {
        let mut it = text_session(None);
        click(&mut it, (200, 200));
        let first = it.ime_target().expect("目标").1;
        type_in(&mut it, "ab");
        let second = it.ime_target().expect("目标").1;
        assert!(second.x > first.x);
        assert_eq!(second.y, first.y);
        it.ime_composition("cd", 1);
        assert!(it.ime_target().expect("目标").1.x > second.x);
    }

    #[test]
    fn caret_blink_toggles_and_input_resets_it() {
        let mut it = text_session(None);
        click(&mut it, (200, 200));
        assert!(it.take_blink_reset());
        assert!(!it.take_blink_reset());
        assert_eq!(it.view(1).editor.map(|e| e.caret_visible), Some(true));
        it.toggle_caret();
        assert_eq!(it.view(1).editor.map(|e| e.caret_visible), Some(false));
        it.char_input('a');
        assert_eq!(it.view(1).editor.map(|e| e.caret_visible), Some(true));
        assert!(it.take_blink_reset());
        it.toggle_caret();
        it.key(Key::Edit(EditKey::Left), NudgeMode::Move, false);
        assert_eq!(it.view(1).editor.map(|e| e.caret_visible), Some(true));
    }

    #[test]
    fn text_tool_strip_shows_appearance_switch_and_text_presets() {
        let it = text_session(None);
        let toolbar = it.view(1).toolbar.expect("工具栏");
        let strip = toolbar.layout.strip.expect("样式条");
        assert_eq!(strip.appearance.len(), 2);
        let style = toolbar.style.expect("样式");
        assert_eq!(
            (style.size, style.preset, style.background),
            (24, Some(1), Some(false))
        );
    }

    #[test]
    fn typing_outside_editing_is_ignored() {
        let mut it = text_session(None);
        it.char_input('a');
        assert!(it.view(1).annotations.shapes.is_empty());
        assert!(!it.take_blink_reset());
    }

    /// 在 `at` 处建一个字号 24 的 "hello"（近似度量下框为 60×30）并确认。
    fn add_hello(it: &mut Interaction, at: (i32, i32)) {
        click(it, at);
        type_in(it, "hello");
        it.key(Key::Enter, NudgeMode::Move, false);
    }

    fn with_text() -> Interaction {
        let mut it = text_session(None);
        add_hello(&mut it, (150, 150));
        it
    }

    /// 第 `index` 个文字图元的（外接框, 旋转, 换行, 竖排, 字号）。
    fn text_at(it: &Interaction, index: usize) -> Option<(Rect, i32, Option<u32>, bool, u32)> {
        match it.view(1).annotations.shapes.get(index) {
            Some(Shape::Text {
                bounds,
                rotation,
                wrap,
                vertical,
                size,
                ..
            }) => Some((*bounds, *rotation, *wrap, *vertical, *size)),
            _ => None,
        }
    }

    fn drag_to(it: &mut Interaction, from: (i32, i32), to: (i32, i32)) {
        it.press(1, from);
        it.mouse_move(1, to);
        it.release(1, to);
    }

    #[test]
    fn single_click_selects_text_and_blank_click_only_deselects() {
        let mut it = with_text();
        click(&mut it, (180, 165));
        assert_eq!(it.annotator.selected_text(), Some(0));
        assert!(!it.is_editing());
        assert!(it.view(1).text_box.is_some_and(|t| !t.editing));
        click(&mut it, (350, 300));
        assert_eq!(it.annotator.selected_text(), None);
        assert!(!it.is_editing(), "有选中时点空白只取消选中");
        click(&mut it, (350, 300));
        assert!(it.is_editing(), "没有选中时点空白才新建文本框");
    }

    #[test]
    fn dragging_selected_text_moves_it_in_one_undo_step() {
        let mut it = with_text();
        click(&mut it, (180, 165));
        it.press(1, (180, 165));
        assert!(it.is_capturing());
        it.mouse_move(1, (200, 185));
        // 拖动中显示预览，原图元被隐藏
        assert!(it.view(1).annotations.draft.is_some());
        assert_eq!(it.view(1).annotations.hidden, Some(0));
        it.release(1, (200, 185));
        assert!(!it.is_capturing());
        assert_eq!(
            text_at(&it, 0).map(|t| t.0),
            Some(Rect::new(170, 170, 60, 30))
        );
        it.key(Key::Undo, NudgeMode::Move, false);
        assert_eq!(
            text_at(&it, 0).map(|t| t.0),
            Some(Rect::new(150, 150, 60, 30))
        );
        assert_eq!(adjusted_rect(&it), Some(Rect::new(100, 100, 400, 300)));
    }

    #[test]
    fn text_handles_win_over_the_screenshot_selection_handles() {
        let mut it = text_session(None);
        add_hello(&mut it, (440, 370));
        click(&mut it, (470, 385));
        assert_eq!(it.annotator.selected_text(), Some(0));
        // 文字右下角 (500, 400) 与截屏选区右下角重合：缩放文字而不是选区
        drag_to(&mut it, (500, 400), (560, 430));
        assert_eq!(adjusted_rect(&it), Some(Rect::new(100, 100, 400, 300)));
        assert!(text_at(&it, 0).is_some_and(|t| t.4 > 24));
    }

    #[test]
    fn keys_in_selected_state_delete_nudge_and_deselect() {
        let mut it = with_text();
        click(&mut it, (180, 165));
        it.key(Key::Arrow(Direction::Right), NudgeMode::Move, false);
        assert_eq!(text_at(&it, 0).map(|t| t.0.x), Some(151));
        it.key(Key::Arrow(Direction::Down), NudgeMode::Shrink, false);
        assert_eq!(text_at(&it, 0).map(|t| t.0.y), Some(160));
        assert_eq!(adjusted_rect(&it), Some(Rect::new(100, 100, 400, 300)));
        it.key(Key::Escape, NudgeMode::Move, false);
        assert_eq!(it.annotator.selected_text(), None);
        assert_eq!(it.view(1).toolbar.map(|t| t.active), Some(Some(Tool::Text)));
        it.key(Key::Escape, NudgeMode::Move, false);
        assert_eq!(it.view(1).toolbar.map(|t| t.active), Some(None));

        let mut again = with_text();
        click(&mut again, (180, 165));
        again.key(Key::Delete, NudgeMode::Move, false);
        assert!(again.view(1).annotations.shapes.is_empty());
        again.key(Key::Undo, NudgeMode::Move, false);
        assert_eq!(again.view(1).annotations.shapes.len(), 1);

        let mut third = with_text();
        click(&mut third, (180, 165));
        third.right_click();
        assert_eq!(third.annotator.selected_text(), None);
        assert_eq!(
            third.view(1).toolbar.map(|t| t.active),
            Some(Some(Tool::Text))
        );
        third.right_click();
        assert_eq!(third.view(1).toolbar.map(|t| t.active), Some(None));
    }

    #[test]
    fn cursors_follow_the_box_handles() {
        let mut it = with_text();
        click(&mut it, (180, 165));
        it.mouse_move(1, (180, 165));
        assert_eq!(it.cursor_kind(), CursorKind::Move);
        it.mouse_move(1, (210, 180));
        assert_eq!(it.cursor_kind(), CursorKind::SizeNwse);
        it.mouse_move(1, (210, 165));
        assert_eq!(it.cursor_kind(), CursorKind::SizeWe);
        it.mouse_move(1, (180, 126));
        assert_eq!(it.cursor_kind(), CursorKind::Hand);
        it.mouse_move(1, (150, 180));
        assert_eq!(it.cursor_kind(), CursorKind::SizeNesw);
        it.mouse_move(1, (300, 300));
        assert_eq!(it.cursor_kind(), CursorKind::Text);
    }

    #[test]
    fn resize_cursor_picks_the_nearest_axis() {
        assert_eq!(resize_cursor(10.0, 0.0), CursorKind::SizeWe);
        assert_eq!(resize_cursor(-10.0, 1.0), CursorKind::SizeWe);
        assert_eq!(resize_cursor(10.0, 10.0), CursorKind::SizeNwse);
        assert_eq!(resize_cursor(-10.0, -10.0), CursorKind::SizeNwse);
        assert_eq!(resize_cursor(0.0, 10.0), CursorKind::SizeNs);
        assert_eq!(resize_cursor(-10.0, 10.0), CursorKind::SizeNesw);
        assert_eq!(resize_cursor(10.0, -10.0), CursorKind::SizeNesw);
        assert_eq!(resize_cursor(10.0, 3.0), CursorKind::SizeWe);
    }

    #[test]
    fn double_clicking_an_edge_handle_cancels_the_wrap() {
        let mut it = with_text();
        click(&mut it, (180, 165));
        drag_to(&mut it, (210, 165), (170, 165));
        let wrapped = text_at(&it, 0).expect("文字");
        assert_eq!(wrapped.2, Some(24));
        assert_eq!(wrapped.0, Rect::new(150, 150, 24, 90));
        // 换行后右边手柄在 (174, 195)
        it.press(1, (174, 195));
        it.release(1, (174, 195));
        it.double_click(1, (174, 195));
        it.release(1, (174, 195));
        let restored = text_at(&it, 0).expect("文字");
        assert_eq!(
            (restored.2, restored.0),
            (None, Rect::new(150, 150, 60, 30))
        );
        assert!(!it.is_editing());
        it.key(Key::Undo, NudgeMode::Move, false);
        assert_eq!(text_at(&it, 0).and_then(|t| t.2), Some(24));
    }

    #[test]
    fn text_size_limit_follows_the_selection_monitor_height() {
        let mut it = text_session(None);
        // 选区所在的主显示器高 1080：上限 2160
        it.mouse_move(1, (300, 300));
        it.wheel(2000);
        assert_eq!(
            it.view(1).toolbar.and_then(|t| t.style).map(|s| s.size),
            Some(2160)
        );
        click(&mut it, (200, 200));
        type_in(&mut it, "a");
        it.key(Key::Enter, NudgeMode::Move, false);
        assert_eq!(text_at(&it, 0).map(|t| t.4), Some(2160));
        assert_eq!(it.annotate_change().map(|c| c.text.size), Some(2160));
    }

    #[test]
    fn double_clicking_a_wrapless_edge_still_reedits_the_text() {
        let mut it = with_text();
        click(&mut it, (180, 165));
        // 右边缘中点在 (210, 165)，没有换行可取消
        it.press(1, (209, 165));
        it.release(1, (209, 165));
        it.double_click(1, (209, 165));
        it.release(1, (209, 165));
        assert!(it.is_editing());
    }

    #[test]
    fn strip_changes_apply_to_the_selected_text_without_deselecting() {
        let mut it = with_text();
        click(&mut it, (180, 165));
        let chip = bar_layout(&it)
            .strip
            .and_then(|s| s.orientation.get(1).map(|(_, r)| *r))
            .expect("横竖切换");
        click(&mut it, (chip.x + 2, chip.y + 2));
        assert_eq!(it.annotator.selected_text(), Some(0));
        assert_eq!(text_at(&it, 0).map(|t| t.3), Some(true));
        let dot = bar_layout(&it)
            .strip
            .and_then(|s| s.presets.get(2).copied())
            .expect("预设圆点");
        click(&mut it, (dot.x + 2, dot.y + 2));
        assert_eq!(text_at(&it, 0).map(|t| t.4), Some(36));
        it.wheel(-1);
        assert_eq!(text_at(&it, 0).map(|t| t.4), Some(34));
        it.key(Key::Undo, NudgeMode::Move, false);
        assert_eq!(text_at(&it, 0).map(|t| t.4), Some(36));
    }

    #[test]
    fn rotating_while_editing_is_part_of_the_confirmed_text() {
        let mut it = with_text();
        it.press(1, (180, 165));
        it.release(1, (180, 165));
        it.double_click(1, (180, 165));
        it.release(1, (180, 165));
        assert!(it.is_editing());
        // 编辑中旋转手柄在框上方 24 像素
        drag_to(&mut it, (180, 126), (260, 165));
        assert_eq!(it.view(1).editor.map(|e| e.edit.rotation), Some(90));
        assert!(it.is_editing());
        it.key(Key::Enter, NudgeMode::Move, false);
        assert_eq!(text_at(&it, 0).map(|t| t.1), Some(90));
        it.key(Key::Undo, NudgeMode::Move, false);
        assert_eq!(text_at(&it, 0).map(|t| t.1), Some(0));
    }

    #[test]
    fn dragging_the_dashed_border_moves_the_text_being_edited() {
        let mut it = with_text();
        it.press(1, (180, 165));
        it.release(1, (180, 165));
        it.double_click(1, (180, 165));
        it.release(1, (180, 165));
        let before = it.view(1).editor.map(|e| e.edit.bounds).expect("编辑框");
        click(&mut it, (180, 165));
        assert!(it.is_editing());
        // 上边缘中点附近属于虚线框，可以拖动
        drag_to(&mut it, (180, 150), (200, 170));
        let after = it.view(1).editor.map(|e| e.edit.bounds).expect("编辑框");
        assert_eq!((after.x - before.x, after.y - before.y), (20, 20));
        assert!(it.is_editing());
    }

    #[test]
    fn clicking_another_text_while_editing_confirms_and_selects_it() {
        let mut it = text_session(None);
        add_hello(&mut it, (150, 150));
        add_hello(&mut it, (350, 300));
        it.press(1, (180, 165));
        it.release(1, (180, 165));
        it.double_click(1, (180, 165));
        it.release(1, (180, 165));
        assert!(it.is_editing());
        click(&mut it, (380, 315));
        assert!(!it.is_editing());
        assert_eq!(it.annotator.selected_text(), Some(1));
    }

    #[test]
    fn box_and_handles_are_not_part_of_the_composited_image() {
        let mut it = with_text();
        click(&mut it, (180, 165));
        // 选中状态下的虚线框和手柄只在 View 里，文档里没有额外图元
        assert!(it.view(1).text_box.is_some());
        assert_eq!(it.document().shapes().len(), 1);
    }

    #[test]
    fn tool_mode_hides_magnifier_and_ignores_double_click_copy() {
        let mut it = adjusting();
        click_button(&mut it, ToolbarButton::Tool(Tool::Rectangle));
        it.mouse_move(1, (300, 300));
        assert_eq!(it.view(1).magnifier, None);
        it.double_click(1, (300, 300));
        assert!(!it.is_finished());
        // 双击等同于普通按下，开始了一次拖拽
        assert!(it.is_capturing());
        it.release(1, (300, 300));
        it.key(Key::Enter, NudgeMode::Move, false);
        assert!(matches!(
            it.take_outcome(),
            Some(Outcome::Action(_, ToolbarAction::Copy))
        ));
    }

    #[test]
    fn arrows_still_nudge_with_tool_but_not_mid_stroke() {
        let mut it = adjusting();
        click_button(&mut it, ToolbarButton::Tool(Tool::Rectangle));
        it.key(Key::Arrow(Direction::Left), NudgeMode::Move, false);
        assert_eq!(adjusted_rect(&it), Some(Rect::new(99, 100, 400, 300)));
        it.press(1, (200, 200));
        it.key(Key::Arrow(Direction::Left), NudgeMode::Move, false);
        assert_eq!(adjusted_rect(&it), Some(Rect::new(99, 100, 400, 300)));
    }

    #[test]
    fn annotation_hints_replace_adjusting_hints() {
        let mut it = adjusting();
        click_button(&mut it, ToolbarButton::Tool(Tool::Pen));
        let keys: Vec<_> = it.view(1).hints.iter().map(|h| h.keys).collect();
        assert!(keys.contains(&"滚轮") && keys.contains(&"Ctrl + Z"));
        assert!(!keys.contains(&"方向键"));
    }

    #[test]
    fn shift_constrains_square_through_interaction() {
        let mut it = adjusting();
        click_button(&mut it, ToolbarButton::Tool(Tool::Rectangle));
        it.set_shift(true);
        drag_on_window(&mut it, (150, 150), (250, 200));
        assert!(matches!(
            it.view(1).annotations.shapes.first(),
            Some(Shape::Rectangle { to, .. }) if *to == Point::new(250, 250)
        ));
    }

    #[test]
    fn painted_windows_cover_all_when_shapes_exist() {
        let mut it = adjusting();
        assert_eq!(it.painted_windows().len(), 1);
        click_button(&mut it, ToolbarButton::Tool(Tool::Line));
        drag_on_window(&mut it, (150, 150), (250, 250));
        assert_eq!(it.painted_windows(), vec![0, 1]);
    }

    #[test]
    fn history_back_to_idle_leaves_tool() {
        let mut it = Interaction::new(
            Box::new(RectTool::new()),
            monitors(),
            Options {
                history: vec![Rect::new(10, 10, 50, 50)],
                ..options(OverlayFlow::Toolbar)
            },
        );
        it.key(Key::OlderHistory, NudgeMode::Move, false);
        click_button(&mut it, ToolbarButton::Tool(Tool::Pen));
        it.key(Key::NewerHistory, NudgeMode::Move, false);
        assert_eq!(it.view(1).preview, Preview::None);
        it.key(Key::OlderHistory, NudgeMode::Move, false);
        assert_eq!(it.view(1).toolbar.map(|t| t.active), Some(None));
    }

    #[test]
    fn enter_and_double_click_copy() {
        let mut it = adjusting();
        it.key(Key::Enter, NudgeMode::Move, false);
        assert!(matches!(
            it.take_outcome(),
            Some(Outcome::Action(_, ToolbarAction::Copy))
        ));
        let mut it = adjusting();
        it.double_click(1, (200, 200));
        assert!(matches!(
            it.take_outcome(),
            Some(Outcome::Action(_, ToolbarAction::Copy))
        ));
    }

    #[test]
    fn arrows_nudge_while_adjusting_and_move_cursor_otherwise() {
        let mut it = adjusting();
        assert_eq!(
            it.key(Key::Arrow(Direction::Left), NudgeMode::Move, false),
            None
        );
        assert_eq!(adjusted_rect(&it), Some(Rect::new(99, 100, 400, 300)));
        it.key(Key::Arrow(Direction::Down), NudgeMode::Grow, false);
        assert_eq!(adjusted_rect(&it), Some(Rect::new(99, 100, 400, 301)));
        let mut idle = rect(OverlayFlow::Toolbar);
        assert_eq!(
            idle.key(Key::Arrow(Direction::Up), NudgeMode::Move, false),
            Some(KeyCommand::MoveCursor(0, -1))
        );
        assert_eq!(
            idle.key(Key::CursorStep(Direction::Right), NudgeMode::Move, false),
            Some(KeyCommand::MoveCursor(1, 0))
        );
    }

    #[test]
    fn right_click_resets_selection_then_cancels() {
        let mut it = adjusting();
        it.right_click();
        assert!(!it.is_finished());
        assert_eq!(adjusted_rect(&it), None);
        it.right_click();
        assert_eq!(it.take_outcome(), Some(Outcome::Cancelled));
    }

    #[test]
    fn magnifier_and_color_pick() {
        let mut it = rect(OverlayFlow::Direct);
        it.mouse_move(1, (10, 20));
        assert_eq!(
            it.view(1).magnifier,
            Some((Point::new(10, 20), ColorFormat::Hex))
        );
        assert_eq!(it.view(0).magnifier, None);
        it.key(Key::Shift, NudgeMode::Shrink, false);
        it.key(Key::Shift, NudgeMode::Shrink, true);
        assert_eq!(
            it.view(1).magnifier,
            Some((Point::new(10, 20), ColorFormat::Rgb))
        );
        assert_eq!(
            it.key(Key::CopyColor, NudgeMode::Move, false),
            Some(KeyCommand::PickColor(Point::new(10, 20)))
        );
        it.finish_color((1, 2, 3));
        assert_eq!(
            it.take_outcome(),
            Some(Outcome::Color("1, 2, 3".to_string()))
        );
    }

    #[test]
    fn magnifier_hidden_while_adjusting_or_disabled() {
        let mut it = adjusting();
        it.mouse_move(1, (300, 300));
        assert_eq!(it.view(1).magnifier, None);
        assert_eq!(it.key(Key::CopyColor, NudgeMode::Move, false), None);
        let mut off = Interaction::new(
            Box::new(RectTool::new()),
            monitors(),
            Options {
                magnifier: false,
                ..options(OverlayFlow::Direct)
            },
        );
        off.mouse_move(1, (10, 10));
        assert_eq!(off.view(1).magnifier, None);
    }

    fn snapping(flow: OverlayFlow) -> Interaction {
        Interaction::new(
            Box::new(RectTool::new()),
            monitors(),
            Options {
                snap: SnapState::On,
                windows: vec![
                    Rect::new(100, 100, 300, 200),
                    Rect::new(-500, -500, 5000, 5000),
                ],
                ..options(flow)
            },
        )
    }

    #[test]
    fn hover_highlights_window_and_click_selects_it() {
        let mut it = snapping(OverlayFlow::Direct);
        it.mouse_move(1, (150, 150));
        assert_eq!(
            it.view(1).preview,
            Preview::Rect(Rect::new(100, 100, 300, 200))
        );
        it.press(1, (150, 150));
        it.release(1, (151, 151));
        match it.take_outcome() {
            Some(Outcome::Selected(sel)) => assert_eq!(sel.bounds(), Rect::new(100, 100, 300, 200)),
            other => panic!("期望吸附到窗口，实际为 {other:?}"),
        }
    }

    #[test]
    fn snapped_desktop_is_clipped_to_monitor() {
        let mut it = snapping(OverlayFlow::Toolbar);
        it.mouse_move(0, (10, 10));
        assert_eq!(
            it.view(0).preview,
            Preview::Rect(Rect::new(1920, 0, 1920, 1080))
        );
        it.press(0, (10, 10));
        it.release(0, (10, 10));
        assert!(!it.is_finished());
        assert_eq!(
            it.view(0).preview,
            Preview::Rect(Rect::new(1920, 0, 1920, 1080))
        );
    }

    #[test]
    fn tab_toggles_snapping_and_drag_ignores_it() {
        let mut it = snapping(OverlayFlow::Direct);
        it.key(Key::Tab, NudgeMode::Move, false);
        it.mouse_move(1, (150, 150));
        assert_eq!(it.view(1).preview, Preview::None);
        it.key(Key::Tab, NudgeMode::Move, false);
        it.press(1, (150, 150));
        it.release(1, (250, 250));
        match it.take_outcome() {
            Some(Outcome::Selected(sel)) => assert_eq!(sel.bounds(), Rect::new(150, 150, 100, 100)),
            other => panic!("期望拖出的选区，实际为 {other:?}"),
        }
    }

    #[test]
    fn history_steps_through_previous_selections() {
        let mut it = Interaction::new(
            Box::new(RectTool::new()),
            monitors(),
            Options {
                history: vec![Rect::new(10, 10, 50, 50), Rect::new(2000, 10, 50, 50)],
                ..options(OverlayFlow::Toolbar)
            },
        );
        it.key(Key::NewerHistory, NudgeMode::Move, false);
        assert_eq!(it.view(1).preview, Preview::None);
        it.key(Key::OlderHistory, NudgeMode::Move, false);
        assert_eq!(it.view(1).preview, Preview::Rect(Rect::new(10, 10, 50, 50)));
        it.key(Key::OlderHistory, NudgeMode::Move, false);
        assert_eq!(
            it.view(0).preview,
            Preview::Rect(Rect::new(2000, 10, 50, 50))
        );
        it.key(Key::OlderHistory, NudgeMode::Move, false);
        assert_eq!(
            it.view(0).preview,
            Preview::Rect(Rect::new(2000, 10, 50, 50))
        );
        it.key(Key::NewerHistory, NudgeMode::Move, false);
        it.key(Key::NewerHistory, NudgeMode::Move, false);
        assert_eq!(it.view(1).preview, Preview::None);
    }

    #[test]
    fn hints_follow_cursor_monitor_and_toggle() {
        let mut it = rect(OverlayFlow::Toolbar);
        assert!(!it.view(0).hints.is_empty());
        it.set_hover(Point::new(10, 10));
        assert!(it.view(0).hints.is_empty());
        assert!(!it.view(1).hints.is_empty());
        it.key(Key::ToggleHints, NudgeMode::Move, false);
        assert!(it.view(1).hints.is_empty());
        it.key(Key::ToggleHints, NudgeMode::Move, false);
        assert!(!it.view(1).hints.is_empty());
    }

    #[test]
    fn hints_change_with_stage() {
        let it = adjusting();
        let keys: Vec<_> = it.view(1).hints.iter().map(|h| h.keys).collect();
        assert!(keys.contains(&"方向键"));
        let hidden = Interaction::new(
            Box::new(RectTool::new()),
            monitors(),
            Options {
                hints: HintState::Hidden,
                ..options(OverlayFlow::Direct)
            },
        );
        assert!(hidden.view(0).hints.is_empty());
    }

    #[test]
    fn painted_windows_cover_hover_and_active_monitor() {
        let mut it = adjusting();
        it.mouse_move(0, (10, 10));
        assert_eq!(it.painted_windows(), vec![0, 1]);
    }

    #[test]
    fn cancel_ends_with_cancelled_and_stops_drag() {
        let mut it = rect(OverlayFlow::Direct);
        it.press(0, (10, 10));
        it.cancel();
        assert!(!it.is_capturing());
        assert_eq!(it.take_outcome(), Some(Outcome::Cancelled));
    }

    #[test]
    fn events_after_finish_are_ignored() {
        let mut it = rect(OverlayFlow::Direct);
        it.cancel();
        it.press(0, (10, 10));
        assert!(!it.is_capturing());
    }

    #[test]
    fn unknown_window_index_is_ignored() {
        let mut it = rect(OverlayFlow::Direct);
        it.press(9, (10, 10));
        assert!(!it.is_capturing());
    }

    #[test]
    fn move_without_press_does_nothing() {
        let mut it = rect(OverlayFlow::Direct);
        it.mouse_move(0, (10, 10));
        it.release(0, (20, 20));
        assert!(!it.is_finished());
    }

    /// 常驻选框：在主显示器上从 (100,100) 框到 (500,400)，取走首次回投。
    fn live() -> Interaction {
        let mut it = rect(OverlayFlow::Live);
        it.press(1, (100, 100));
        it.mouse_move(1, (300, 300));
        it.release(1, (500, 400));
        assert!(!it.is_finished());
        let commits = it.take_commits();
        assert_eq!(commits.len(), 1);
        assert_eq!(
            commits.first().map(Selection::bounds),
            Some(Rect::new(100, 100, 400, 300))
        );
        it
    }

    fn commit_bounds(it: &mut Interaction) -> Vec<Rect> {
        it.take_commits().iter().map(Selection::bounds).collect()
    }

    #[test]
    fn live_settles_into_corner_frame_without_toolbar() {
        let it = live();
        let view = it.view(1);
        assert_eq!(view.preview, Preview::Rect(Rect::new(100, 100, 400, 300)));
        assert_eq!(view.frame, FrameStyle::Corners);
        assert_eq!(view.label, None);
        assert_eq!(view.toolbar, None);
        assert!(it.has_committed());
        let keys: Vec<_> = view.hints.iter().map(|h| h.keys).collect();
        assert!(keys.contains(&"右键 / Esc") && !keys.contains(&"方向键"));
    }

    #[test]
    fn live_resize_and_move_commit_on_release() {
        let mut it = live();
        it.press(1, (500, 400));
        it.mouse_move(1, (600, 450));
        assert!(it.take_commits().is_empty());
        it.release(1, (600, 450));
        assert_eq!(commit_bounds(&mut it), vec![Rect::new(100, 100, 500, 350)]);
        it.press(1, (300, 250));
        it.mouse_move(1, (310, 240));
        it.release(1, (310, 240));
        assert_eq!(commit_bounds(&mut it), vec![Rect::new(110, 90, 500, 350)]);
    }

    #[test]
    fn live_release_without_change_does_not_commit() {
        let mut it = live();
        it.press(1, (300, 250));
        it.release(1, (300, 250));
        assert!(it.take_commits().is_empty());
        assert_eq!(adjusted_rect(&it), Some(Rect::new(100, 100, 400, 300)));
    }

    #[test]
    fn live_redraw_replaces_box_and_click_keeps_it() {
        let mut it = live();
        it.press(1, (1000, 800));
        it.mouse_move(1, (1050, 850));
        // 重画期间旧选框不再显示
        assert_eq!(it.view(1).frame, FrameStyle::Plain);
        it.release(1, (1100, 900));
        assert_eq!(commit_bounds(&mut it), vec![Rect::new(1000, 800, 100, 100)]);
        it.press(1, (10, 10));
        it.release(1, (10, 10));
        assert!(it.take_commits().is_empty());
        assert_eq!(adjusted_rect(&it), Some(Rect::new(1000, 800, 100, 100)));
        assert_eq!(it.view(1).frame, FrameStyle::Corners);
    }

    #[test]
    fn live_lasso_becomes_bounding_box() {
        let mut it = Interaction::new(
            Box::new(LassoTool::new(10, 4)),
            monitors(),
            options(OverlayFlow::Live),
        );
        it.press(1, (100, 100));
        it.mouse_move(1, (200, 100));
        it.mouse_move(1, (200, 200));
        it.release(1, (100, 200));
        assert_eq!(commit_bounds(&mut it), vec![Rect::new(88, 88, 124, 124)]);
        assert_eq!(it.view(1).frame, FrameStyle::Corners);
        assert!(!it.is_finished());
    }

    #[test]
    fn live_ignores_keyboard_confirm_nudge_and_color() {
        let mut it = live();
        assert_eq!(
            it.key(Key::Arrow(Direction::Left), NudgeMode::Move, false),
            None
        );
        it.key(Key::Enter, NudgeMode::Move, false);
        it.key(Key::CopyColor, NudgeMode::Move, false);
        it.double_click(1, (300, 250));
        it.release(1, (300, 250));
        assert!(!it.is_finished());
        assert!(it.take_commits().is_empty());
        assert_eq!(adjusted_rect(&it), Some(Rect::new(100, 100, 400, 300)));
    }

    #[test]
    fn live_right_click_and_escape_close() {
        let mut it = live();
        it.right_click();
        assert_eq!(it.take_outcome(), Some(Outcome::Cancelled));
        let mut it = live();
        it.key(Key::Escape, NudgeMode::Move, false);
        assert_eq!(it.take_outcome(), Some(Outcome::Cancelled));
    }
}
