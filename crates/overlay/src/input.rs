//! 鼠标键盘状态机：与 Win32 无关，窗口层只负责把消息翻译成这里的调用。

use lenswin_core::geometry::{Point, Rect};
use lenswin_core::monitor::{MonitorInfo, index_at};
use lenswin_core::selection::{Selection, SelectionShape};
use lenswin_core::snip::{ColorFormat, OverlayFlow, ToolbarAction, topmost_containing};

use crate::adjust::{self, Direction, Handle, NudgeMode};
use crate::hint::{self, Features, Hint, HintState, Stage};
use crate::layout::{self, ToolbarButton};
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

/// 窗口层绘制某个显示器时需要的内容。
#[derive(Debug, Clone, PartialEq)]
pub struct View<'a> {
    /// 高亮区域或套索轨迹。
    pub preview: Preview<'a>,
    /// 矩形选区的边框装饰。
    pub frame: FrameStyle,
    /// 需要标注尺寸的矩形。
    pub label: Option<Rect>,
    /// 工具栏范围与悬停的按钮。
    pub toolbar: Option<(Rect, Option<ToolbarButton>)>,
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

fn cursor_for(handle: Handle) -> CursorKind {
    match handle {
        Handle::N | Handle::S => CursorKind::SizeNs,
        Handle::E | Handle::W => CursorKind::SizeWe,
        Handle::NW | Handle::SE => CursorKind::SizeNwse,
        Handle::NE | Handle::SW => CursorKind::SizeNesw,
        Handle::Body => CursorKind::Move,
    }
}

fn action_of(button: ToolbarButton) -> Option<ToolbarAction> {
    match button {
        ToolbarButton::Search => Some(ToolbarAction::Search),
        ToolbarButton::Save => Some(ToolbarAction::Save),
        ToolbarButton::Copy => Some(ToolbarAction::Copy),
        ToolbarButton::Cancel => None,
    }
}

impl Interaction {
    /// 创建交互，`monitors` 的顺序即窗口序号。
    pub fn new(tool: Box<dyn SelectionTool>, monitors: Vec<MonitorInfo>, options: Options) -> Self {
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
        }
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

    fn toolbar(&self, adj: &Adjusting) -> Option<Rect> {
        if self.options.flow != OverlayFlow::Toolbar {
            return None;
        }
        let monitor = self.monitor_bounds(adj.monitor)?;
        Some(layout::toolbar_rect(
            &adj.rect,
            &monitor,
            self.dpi(adj.monitor),
        ))
    }

    fn toolbar_button_at(&self, adj: &Adjusting, p: Point) -> Option<ToolbarButton> {
        let bar = self.toolbar(adj)?;
        layout::button_at(&bar, p, self.dpi(adj.monitor))
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
        match self.phase {
            Phase::Drawing { .. } => return,
            Phase::Adjusting(mut adj) => {
                if let Some(button) = self.toolbar_button_at(&adj, p) {
                    match action_of(button) {
                        Some(action) => self.finish_with_action(adj, action),
                        None => self.finish(Outcome::Cancelled),
                    }
                    return;
                }
                if let Some(handle) =
                    adjust::hit_test(&adj.rect, p, layout::grip(self.dpi(adj.monitor)))
                {
                    adj.grab = Some(Grab {
                        handle,
                        origin: p,
                        start: adj.rect,
                    });
                    self.phase = Phase::Adjusting(adj);
                    return;
                }
                // 在选区外按下：放弃当前选区，重新框选
                self.phase = Phase::Idle;
                self.history_index = None;
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

    /// 移动鼠标；拖拽时 `window` 是起始窗口（鼠标已被捕获）。
    pub fn mouse_move(&mut self, window: usize, local: (i32, i32)) {
        if self.outcome.is_some() {
            return;
        }
        let Some(p) = self.point(window, local) else {
            return;
        };
        self.hover = Some(p);
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

    /// 双击：在调整中的选区内双击等同于复制，其余情况按普通按下处理。
    pub fn double_click(&mut self, window: usize, local: (i32, i32)) {
        if let (Phase::Adjusting(adj), Some(p)) = (self.phase, self.point(window, local))
            && self.options.flow == OverlayFlow::Toolbar
            && adj.rect.contains(p)
            && self.toolbar_button_at(&adj, p).is_none()
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
        match key {
            Key::Escape => self.cancel(),
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
        }
        None
    }

    fn arrow(&mut self, direction: Direction, mode: NudgeMode) -> Option<KeyCommand> {
        let Phase::Adjusting(mut adj) = self.phase else {
            let (dx, dy) = direction.delta();
            return Some(KeyCommand::MoveCursor(dx, dy));
        };
        // 常驻选框每次微调都会触发搜索，容易被限流，不支持键盘微调
        if adj.grab.is_some() || self.is_live() {
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
            Phase::Adjusting(adj) => adj.grab.is_some(),
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
            Phase::Adjusting(adj) => adj.grab.is_some() && !self.is_live(),
        }
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
                        view.toolbar = self.toolbar(&adj).map(|bar| {
                            let hovered = self
                                .hover
                                .and_then(|p| layout::button_at(&bar, p, self.dpi(window)));
                            (bar, hovered)
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
        let Some(p) = self.hover else {
            return CursorKind::Cross;
        };
        if self.toolbar_button_at(&adj, p).is_some() {
            return CursorKind::Arrow;
        }
        adjust::hit_test(&adj.rect, p, layout::grip(self.dpi(adj.monitor)))
            .map_or(CursorKind::Cross, cursor_for)
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
    use crate::modes::{LassoTool, RectTool};

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

    #[test]
    fn toolbar_buttons_finish_with_action_or_cancel() {
        let mut it = adjusting();
        let bar = it.view(1).toolbar.map(|(b, _)| b).expect("工具栏");
        let save = layout::button_rect(&bar, 1, 96).expect("按钮");
        it.mouse_move(1, (save.x + 2, save.y + 2));
        assert_eq!(
            it.view(1).toolbar.map(|(_, h)| h),
            Some(Some(ToolbarButton::Save))
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
        let bar = it.view(1).toolbar.map(|(b, _)| b).expect("工具栏");
        let cancel = layout::button_rect(&bar, 2, 96).expect("按钮");
        it.press(1, (cancel.x + 2, cancel.y + 2));
        assert_eq!(it.take_outcome(), Some(Outcome::Cancelled));
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
