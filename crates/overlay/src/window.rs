//! 每个显示器一个置顶无边框窗口，共享同一份会话状态。
//! 阻塞模式内嵌消息循环直到选区结束；常驻模式挂在调用方的消息循环上，通过回调回投事件。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use lenswin_core::config::{AnnotateConfig, GlowMode};
use lenswin_core::frame::CroppedImage;
use lenswin_core::geometry::{Point, Rect};
use lenswin_core::monitor::MonitorInfo;
use lenswin_core::selection::Selection;
use lenswin_core::snip::LiveEvent;
use windows::Win32::Foundation::{GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{InvalidateRect, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VIRTUAL_KEY, VK_A, VK_C, VK_CONTROL, VK_D,
    VK_DOWN, VK_ESCAPE, VK_F1, VK_LEFT, VK_OEM_2, VK_OEM_COMMA, VK_OEM_PERIOD, VK_RETURN, VK_RIGHT,
    VK_S, VK_SHIFT, VK_TAB, VK_UP, VK_W, VK_Y, VK_Z,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
    GetMessageW, HWND_TOPMOST, IDC_ARROW, IDC_CROSS, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS,
    IDC_SIZENWSE, IDC_SIZEWE, KillTimer, LoadCursorW, MA_NOACTIVATE, MSG, PostQuitMessage,
    RegisterClassExW, SWP_NOZORDER, SWP_SHOWWINDOW, SetCursor, SetCursorPos, SetForegroundWindow,
    SetTimer, SetWindowPos, TranslateMessage, UnregisterClassW, WINDOWPOS, WM_ERASEBKGND,
    WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDOWN, WM_SETCURSOR, WM_TIMER, WM_WINDOWPOSCHANGING,
    WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::adjust::{Direction, NudgeMode};
use crate::error::{OverlayError, win32};
use crate::glow;
use crate::input::{CursorKind, Interaction, Key, KeyCommand, Outcome, point_from_lparam};
use crate::layout;
use crate::paint::FrameRef;
use crate::render::{DrawStatus, Effects, Renderer, Surface, TargetResources};

/// 覆盖层窗口类名。
const CLASS_NAME: PCWSTR = w!("LenswinOverlayWindow");
/// 辉光动画计时器的标识。
const GLOW_TIMER_ID: usize = 1;
/// 辉光动画的帧间隔（毫秒），约 60 帧每秒。
const GLOW_FRAME_MS: u32 = 16;

/// 显式丢弃无法处理的返回值。
fn discard<T>(_value: T) {}

struct WindowSlot {
    hwnd: HWND,
    origin: Point,
    size: (u32, u32),
    dpi: u32,
    bgra: Vec<u8>,
    target: Option<TargetResources>,
}

struct Session {
    renderer: Renderer,
    interaction: Interaction,
    windows: Vec<WindowSlot>,
    stroke_width: u32,
    show_lasso_bounds: bool,
    visual: Visual,
    started: Instant,
    error: Option<OverlayError>,
    mode: SessionMode,
}

/// 会话的运行方式。
#[derive(Clone)]
pub(crate) enum SessionMode {
    /// 内嵌消息循环，结束后返回结局。
    Blocking,
    /// 挂在调用方的消息循环上，事件交给回调。
    Live(LiveSink),
}

/// 常驻模式的事件回调。
pub(crate) type LiveSink = Rc<dyn Fn(LiveEvent)>;

thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

/// 在会话上执行操作；会话不存在或正被借用（重入）时返回 `None`。
fn with_session<R>(f: impl FnOnce(&mut Session) -> R) -> Option<R> {
    SESSION
        .try_with(|cell| {
            let mut guard = cell.try_borrow_mut().ok()?;
            guard.as_mut().map(f)
        })
        .ok()
        .flatten()
}

impl Session {
    fn index_of(&self, hwnd: HWND) -> Option<usize> {
        self.windows.iter().position(|s| s.hwnd == hwnd)
    }

    fn invalidate_window(&self, index: usize) {
        if let Some(slot) = self.windows.get(index) {
            // 只是标记重绘区域，失败时下一次事件还会再标记
            // SAFETY: hwnd 是本会话创建且仍存活的窗口。
            discard(unsafe { InvalidateRect(Some(slot.hwnd), None, false) });
        }
    }

    fn invalidate_all(&self) {
        for slot in &self.windows {
            // 只是标记重绘区域，失败时下一次事件还会再标记
            // SAFETY: hwnd 是本会话创建且仍存活的窗口。
            discard(unsafe { InvalidateRect(Some(slot.hwnd), None, false) });
        }
    }

    fn fail(&mut self, error: OverlayError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
        self.interaction.cancel();
    }

    fn invalidate_windows(&self, indices: &[usize]) {
        for index in indices {
            self.invalidate_window(*index);
        }
    }

    /// 读取虚拟桌面坐标处冻结帧的颜色。
    fn pixel(&self, p: Point) -> Option<(u8, u8, u8)> {
        self.windows.iter().find_map(|slot| {
            let bounds = Rect::new(slot.origin.x, slot.origin.y, slot.size.0, slot.size.1);
            layout::pixel_at(&slot.bgra, &bounds, p)
        })
    }

    /// 结果弹窗已显示后覆盖层不再抢占前台和 Z 序，点击覆盖层时弹窗仍在上面。
    fn yields_to_popup(&self) -> bool {
        matches!(self.mode, SessionMode::Live(_)) && self.interaction.has_committed()
    }

    /// 取出常驻模式待回投的事件和回调；阻塞模式返回 `None`。
    fn take_live_events(&mut self) -> Option<(LiveSink, Vec<LiveEvent>)> {
        let SessionMode::Live(sink) = &self.mode else {
            return None;
        };
        let sink = Rc::clone(sink);
        let mut events: Vec<LiveEvent> = self
            .interaction
            .take_commits()
            .into_iter()
            .map(LiveEvent::Committed)
            .collect();
        if let Some(error) = self.error.take() {
            // 出错时 fail 已顺带取消，统一按失败上报，不再报一次退出
            discard(self.interaction.take_outcome());
            events.push(LiveEvent::Failed(error.to_string()));
        } else if self.interaction.take_outcome().is_some() {
            events.push(LiveEvent::Closed);
        }
        Some((sink, events))
    }

    /// 阻塞模式结束时的产出：取走结局，工具栏动作要先合成成图。
    fn finish_run(&mut self) -> Result<RunOutput, OverlayError> {
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        let annotate = self.interaction.annotate_change();
        let outcome = self
            .interaction
            .take_outcome()
            .unwrap_or(Outcome::Cancelled);
        let image = match &outcome {
            Outcome::Action(selection, _) => Some(self.compose(selection)?),
            _ => None,
        };
        Ok(RunOutput {
            outcome,
            image,
            annotate,
        })
    }

    /// 把选区所在显示器的冻结帧与标注合成为成图。
    fn compose(&self, selection: &Selection) -> Result<CroppedImage, OverlayError> {
        let slot = self
            .windows
            .get(selection.monitor_index())
            .ok_or_else(|| OverlayError::InvalidInput("选区所在显示器不存在".to_string()))?;
        let frame = FrameRef {
            bounds: Rect::new(slot.origin.x, slot.origin.y, slot.size.0, slot.size.1),
            bgra: &slot.bgra,
        };
        self.renderer.compose(
            frame,
            &selection.bounds(),
            self.interaction.document().shapes(),
        )
    }

    fn elapsed_secs(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    fn effects(&self) -> Effects {
        Effects {
            glow: glow::glow_frame(self.visual.glow, self.elapsed_secs()),
            dim: glow::dim_progress(self.visual.glow, self.elapsed_secs()),
            colorful: self.visual.colorful,
        }
    }

    fn paint(&mut self, index: usize) {
        let effects = self.effects();
        let Some(slot) = self.windows.get_mut(index) else {
            return;
        };
        if slot.target.is_none() {
            match self
                .renderer
                .create_target(slot.hwnd, slot.size, &slot.bgra, slot.dpi)
            {
                Ok(target) => slot.target = Some(target),
                Err(e) => {
                    self.fail(e);
                    return;
                }
            }
        }
        let Some(target) = slot.target.as_ref() else {
            return;
        };
        let surface = Surface {
            origin: slot.origin,
            size: slot.size,
            bgra: &slot.bgra,
            stroke_width: self.stroke_width,
            show_lasso_bounds: self.show_lasso_bounds,
            effects,
        };
        match target.draw(&surface, &self.interaction.view(index)) {
            Ok(DrawStatus::Done) => {}
            Ok(DrawStatus::RecreateTarget) => {
                slot.target = None;
                self.invalidate_all();
            }
            Err(e) => self.fail(e),
        }
    }
}

#[derive(Clone, Copy)]
enum Mouse {
    Down,
    DoubleClick,
    Move,
    Up,
}

/// 执行一次状态变更：前后有内容的窗口都要重绘，并按需捕获或释放鼠标。
fn update(hwnd: HWND, change: impl FnOnce(&mut Session)) {
    with_session(|s| {
        let before = s.interaction.painted_windows();
        let was_capturing = s.interaction.is_capturing();
        change(s);
        let now_capturing = s.interaction.is_capturing();
        if now_capturing && !was_capturing {
            // 捕获鼠标，拖出窗口或跨显示器时仍能收到移动和松开消息
            // SAFETY: hwnd 是本线程的有效窗口。
            discard(unsafe { SetCapture(hwnd) });
        } else if was_capturing && !now_capturing {
            // 释放失败只会让捕获保留到下次点击，选区本身不受影响
            // SAFETY: 无前置条件。
            discard(unsafe { ReleaseCapture() });
        }
        s.invalidate_windows(&before);
        s.invalidate_windows(&s.interaction.painted_windows());
    });
    deliver_live_events();
}

/// 把常驻模式积攒的事件交给回调；回调前已释放会话借用，回调里再进入覆盖层也安全。
fn deliver_live_events() {
    let Some((sink, events)) = with_session(Session::take_live_events).flatten() else {
        return;
    };
    for event in events {
        sink(event);
    }
}

fn on_mouse(hwnd: HWND, lparam: LPARAM, event: Mouse) {
    let local = point_from_lparam(lparam.0);
    let shift = is_down(VK_SHIFT);
    update(hwnd, |s| {
        let Some(index) = s.index_of(hwnd) else {
            return;
        };
        s.interaction.set_shift(shift);
        match event {
            Mouse::Down => s.interaction.press(index, local),
            Mouse::DoubleClick => s.interaction.double_click(index, local),
            Mouse::Move => s.interaction.mouse_move(index, local),
            Mouse::Up => s.interaction.release(index, local),
        }
    });
}

/// 虚拟键码转为覆盖层按键。
fn key_of(vk: VIRTUAL_KEY) -> Option<Key> {
    let key = match vk {
        VK_ESCAPE => Key::Escape,
        VK_RETURN => Key::Enter,
        VK_TAB => Key::Tab,
        VK_LEFT => Key::Arrow(Direction::Left),
        VK_RIGHT => Key::Arrow(Direction::Right),
        VK_UP => Key::Arrow(Direction::Up),
        VK_DOWN => Key::Arrow(Direction::Down),
        VK_A => Key::CursorStep(Direction::Left),
        VK_D => Key::CursorStep(Direction::Right),
        VK_W => Key::CursorStep(Direction::Up),
        VK_S => Key::CursorStep(Direction::Down),
        VK_C => Key::CopyColor,
        VK_SHIFT => Key::Shift,
        VK_OEM_COMMA => Key::OlderHistory,
        VK_OEM_PERIOD => Key::NewerHistory,
        // 美式键盘上 ? 与 / 同键，另给 F1 作为不依赖布局的备选
        VK_OEM_2 | VK_F1 => Key::ToggleHints,
        _ => return None,
    };
    Some(key)
}

fn is_down(vk: VIRTUAL_KEY) -> bool {
    // SAFETY: 无前置条件，只读取本线程的键盘状态。
    let state = unsafe { GetKeyState(i32::from(vk.0)) };
    // 最高位为 1 表示按下
    state < 0
}

fn nudge_mode() -> NudgeMode {
    if is_down(VK_CONTROL) {
        NudgeMode::Grow
    } else if is_down(VK_SHIFT) {
        NudgeMode::Shrink
    } else {
        NudgeMode::Move
    }
}

/// Ctrl+Z 撤销，Ctrl+Y 与 Ctrl+Shift+Z 重做。
fn history_key(vk: VIRTUAL_KEY) -> Option<Key> {
    if !is_down(VK_CONTROL) {
        return None;
    }
    match vk {
        VK_Z if is_down(VK_SHIFT) => Some(Key::Redo),
        VK_Z => Some(Key::Undo),
        VK_Y => Some(Key::Redo),
        _ => None,
    }
}

fn on_key(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) {
    let Some(vk) = u16::try_from(wparam.0).ok().map(VIRTUAL_KEY) else {
        return;
    };
    let Some(key) = history_key(vk).or_else(|| key_of(vk)) else {
        return;
    };
    // lparam 第 30 位为 1 表示按下前该键已处于按下状态，即自动重复
    let repeat = (lparam.0 >> 30) & 1 == 1;
    let mode = nudge_mode();
    let shift = is_down(VK_SHIFT);
    let mut command = None;
    update(hwnd, |s| {
        s.interaction.set_shift(shift);
        command = s.interaction.key(key, mode, repeat);
    });
    match command {
        Some(KeyCommand::MoveCursor(dx, dy)) => move_cursor(dx, dy),
        Some(KeyCommand::PickColor(p)) => update(hwnd, |s| {
            // 光标不在任何冻结帧上时取不到颜色，保持覆盖层不变
            if let Some(rgb) = s.pixel(p) {
                s.interaction.finish_color(rgb);
            }
        }),
        None => {}
    }
}

fn move_cursor(dx: i32, dy: i32) {
    let mut cursor = POINT::default();
    // SAFETY: cursor 是有效的局部变量。
    if unsafe { GetCursorPos(&mut cursor) }.is_err() {
        return;
    }
    // 移动失败只是这一步没动，用户可以再按一次
    // SAFETY: 无前置条件，坐标超出屏幕时系统会自动限制。
    discard(unsafe { SetCursorPos(cursor.x.saturating_add(dx), cursor.y.saturating_add(dy)) });
}

fn set_cursor() {
    let kind = with_session(|s| s.interaction.cursor_kind()).unwrap_or(CursorKind::Cross);
    let id = match kind {
        CursorKind::Cross => IDC_CROSS,
        CursorKind::Arrow => IDC_ARROW,
        CursorKind::Move => IDC_SIZEALL,
        CursorKind::SizeNs => IDC_SIZENS,
        CursorKind::SizeWe => IDC_SIZEWE,
        CursorKind::SizeNwse => IDC_SIZENWSE,
        CursorKind::SizeNesw => IDC_SIZENESW,
    };
    // SAFETY: 加载系统预置光标，共享句柄无需释放。
    if let Ok(cursor) = unsafe { LoadCursorW(None, id) } {
        // SAFETY: cursor 是有效的光标句柄。
        discard(unsafe { SetCursor(Some(cursor)) });
    }
}

/// 辉光动画的一帧：全部窗口重绘，动画结束后停掉计时器，最后一次重绘停在静态状态。
fn on_glow_tick(hwnd: HWND) {
    with_session(|s| {
        if !glow::is_animating(s.visual.glow, s.elapsed_secs()) {
            // 停不掉只会多几次无变化的重绘，窗口销毁时计时器也会随之释放
            // SAFETY: hwnd 是本线程创建的窗口，计时器由它持有。
            discard(unsafe { KillTimer(Some(hwnd), GLOW_TIMER_ID) });
        }
        s.invalidate_all();
    });
}

/// 需要动画时在 `hwnd` 上启动辉光计时器。
fn start_glow_timer(hwnd: HWND, visual: Visual) {
    if !glow::is_animating(visual.glow, 0.0) {
        return;
    }
    // 启动失败时辉光停在首帧，鼠标移动引起的重绘仍会推进动画，选区不受影响
    // SAFETY: hwnd 是本线程创建且已显示的窗口，不使用回调函数。
    discard(unsafe { SetTimer(Some(hwnd), GLOW_TIMER_ID, GLOW_FRAME_MS, None) });
}

fn on_paint(hwnd: HWND) {
    with_session(|s| {
        if let Some(index) = s.index_of(hwnd) {
            s.paint(index);
        }
    });
    // SAFETY: hwnd 是本线程的有效窗口，None 表示整个客户区。
    discard(unsafe { ValidateRect(Some(hwnd), None) });
    // 绘制失败会结束会话，常驻模式要据此回投
    deliver_live_events();
}

/// 让出 Z 序：保留系统的其它改动，只去掉 Z 序变化。
fn keep_z_order(lparam: LPARAM) {
    if !with_session(|s| s.yields_to_popup()).unwrap_or(false) {
        return;
    }
    let pos = lparam.0 as *mut WINDOWPOS;
    // SAFETY: WM_WINDOWPOSCHANGING 的 lParam 指向系统提供的可写 WINDOWPOS，仅在本消息期间有效。
    if let Some(pos) = unsafe { pos.as_mut() } {
        pos.flags |= SWP_NOZORDER;
    }
}

/// `WM_MOUSEWHEEL` 的滚动方向：向上为正，每条消息按一格计，高精度滚轮的细碎增量也不会被吞掉。
fn wheel_notches(wparam: WPARAM) -> i32 {
    let high = u16::try_from((wparam.0 >> 16) & 0xFFFF).unwrap_or(0);
    i32::from(i16::from_ne_bytes(high.to_ne_bytes())).signum()
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            on_paint(hwnd);
            return LRESULT(0);
        }
        // 整个客户区都由 Direct2D 绘制，禁止系统擦除以免闪烁
        WM_ERASEBKGND => return LRESULT(1),
        WM_SETCURSOR => {
            set_cursor();
            return LRESULT(1);
        }
        WM_LBUTTONDOWN => on_mouse(hwnd, lparam, Mouse::Down),
        WM_LBUTTONDBLCLK => on_mouse(hwnd, lparam, Mouse::DoubleClick),
        WM_MOUSEMOVE => {
            on_mouse(hwnd, lparam, Mouse::Move);
            // 捕获鼠标期间系统不再发 WM_SETCURSOR，光标形状需在移动时自行更新
            set_cursor();
        }
        WM_LBUTTONUP => on_mouse(hwnd, lparam, Mouse::Up),
        WM_RBUTTONDOWN => update(hwnd, |s| s.interaction.right_click()),
        WM_MOUSEWHEEL => {
            let notches = wheel_notches(wparam);
            update(hwnd, |s| s.interaction.wheel(notches));
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            on_key(hwnd, wparam, lparam);
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == GLOW_TIMER_ID => {
            on_glow_tick(hwnd);
            return LRESULT(0);
        }
        // 弹窗显示后点击覆盖层不激活，焦点和 Esc 留在弹窗
        WM_MOUSEACTIVATE if with_session(|s| s.yields_to_popup()).unwrap_or(false) => {
            return LRESULT(isize::try_from(MA_NOACTIVATE).unwrap_or(0));
        }
        WM_WINDOWPOSCHANGING => keep_z_order(lparam),
        _ => {}
    }
    // SAFETY: 参数原样来自系统传入的窗口消息。
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// 窗口类与窗口的所有者，销毁时先释放 Direct2D 资源，再销毁窗口并注销窗口类。
struct Windows {
    instance: HINSTANCE,
    hwnds: Vec<HWND>,
}

impl Drop for Windows {
    fn drop(&mut self) {
        // 先取出会话，释放引用窗口的渲染目标
        let taken = SESSION.try_with(|cell| cell.try_borrow_mut().ok().and_then(|mut g| g.take()));
        discard(taken);
        for hwnd in self.hwnds.drain(..) {
            // Drop 中无法上报错误，销毁失败只会泄漏窗口
            // SAFETY: 窗口由本结构创建，仅在此处销毁一次。
            discard(unsafe { DestroyWindow(hwnd) });
        }
        // SAFETY: 类名与注册时一致，窗口已全部销毁。
        discard(unsafe { UnregisterClassW(CLASS_NAME, Some(self.instance)) });
    }
}

fn register_class() -> Result<HINSTANCE, OverlayError> {
    // SAFETY: 传入 None 取当前模块句柄。
    let module = unsafe { GetModuleHandleW(None) }.map_err(win32("GetModuleHandleW"))?;
    let instance = HINSTANCE(module.0);
    let class = WNDCLASSEXW {
        cbSize: u32::try_from(std::mem::size_of::<WNDCLASSEXW>())
            .map_err(|_| OverlayError::InvalidInput("窗口类结构过大".to_string()))?,
        style: CS_DBLCLKS,
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: class 中的指针指向静态数据，窗口过程是有效的 extern "system" 函数。
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(OverlayError::Win32 {
            api: "RegisterClassExW",
            // SAFETY: 紧跟失败的调用读取错误码。
            source: windows::core::Error::from(unsafe { GetLastError() }),
        });
    }
    Ok(instance)
}

/// 辉光与炫彩描边的设置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Visual {
    pub glow: GlowMode,
    pub colorful: bool,
}

/// 会话所需的输入。
pub(crate) struct SessionInput {
    pub interaction: Interaction,
    pub monitors: Vec<MonitorInfo>,
    pub bgra_frames: Vec<Vec<u8>>,
    pub stroke_width: u32,
    pub show_lasso_bounds: bool,
    pub visual: Visual,
}

/// 阻塞会话的产出。
pub(crate) struct RunOutput {
    /// 结局。
    pub outcome: Outcome,
    /// 工具栏动作对应的合成成图，其它结局为 `None`。
    pub image: Option<CroppedImage>,
    /// 标注样式有变化时的新样式。
    pub annotate: Option<AnnotateConfig>,
}

/// 显示覆盖层并运行消息循环，直到用户完成或取消选区。
pub(crate) fn run(input: SessionInput) -> Result<RunOutput, OverlayError> {
    let _owner = open(input, SessionMode::Blocking)?;
    pump_until_finished()?;
    with_session(Session::finish_run).unwrap_or(Ok(RunOutput {
        outcome: Outcome::Cancelled,
        image: None,
        annotate: None,
    }))
}

/// 常驻会话的窗口所有者；销毁时关闭全部覆盖层窗口。
pub(crate) struct LiveWindows {
    _owner: Windows,
}

/// 显示覆盖层后立即返回，事件通过 `sink` 回投；窗口由调用方的消息循环驱动。
pub(crate) fn open_live(input: SessionInput, sink: LiveSink) -> Result<LiveWindows, OverlayError> {
    let owner = open(input, SessionMode::Live(sink))?;
    Ok(LiveWindows { _owner: owner })
}

/// 创建并显示每个显示器的覆盖层窗口，建立会话。
fn open(input: SessionInput, mode: SessionMode) -> Result<Windows, OverlayError> {
    let in_use = SESSION
        .try_with(|cell| cell.try_borrow().map_or(true, |g| g.is_some()))
        .unwrap_or(true);
    if in_use {
        return Err(OverlayError::InvalidInput("已有覆盖层正在显示".to_string()));
    }
    let SessionInput {
        interaction,
        monitors,
        bgra_frames,
        stroke_width,
        show_lasso_bounds,
        visual,
    } = input;
    let renderer = Renderer::new()?;
    let instance = register_class()?;
    let mut owner = Windows {
        instance,
        hwnds: Vec::new(),
    };

    let mut slots = Vec::with_capacity(monitors.len());
    for (monitor, bgra) in monitors.iter().zip(bgra_frames) {
        let (x, y) = (monitor.bounds.x, monitor.bounds.y);
        let (w, h) = (monitor.bounds.width, monitor.bounds.height);
        let width = i32::try_from(w)
            .map_err(|_| OverlayError::InvalidInput("显示器宽度超出范围".to_string()))?;
        let height = i32::try_from(h)
            .map_err(|_| OverlayError::InvalidInput("显示器高度超出范围".to_string()))?;
        // SAFETY: 窗口类已注册，参数均有效；先以 WS_POPUP 创建，下面再用 SetWindowPos 置顶并显示。
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                CLASS_NAME,
                w!(""),
                WS_POPUP,
                x,
                y,
                width,
                height,
                None,
                None,
                Some(instance),
                None,
            )
        }
        .map_err(win32("CreateWindowExW"))?;
        owner.hwnds.push(hwnd);
        slots.push(WindowSlot {
            hwnd,
            origin: Point::new(x, y),
            size: (w, h),
            dpi: monitor.dpi,
            bgra,
            target: None,
        });
    }

    let cursor = cursor_position();
    let focus_index = cursor
        .and_then(|p| lenswin_core::monitor::index_at(&monitors, p))
        .unwrap_or(0);
    let mut interaction = interaction;
    if let Some(p) = cursor {
        interaction.set_hover(p);
    }
    let placements: Vec<(HWND, i32, i32, i32, i32)> = slots
        .iter()
        .zip(&monitors)
        .map(|(slot, m)| {
            let w = i32::try_from(m.bounds.width).unwrap_or(0);
            let h = i32::try_from(m.bounds.height).unwrap_or(0);
            (slot.hwnd, m.bounds.x, m.bounds.y, w, h)
        })
        .collect();
    let focus_hwnd = slots.get(focus_index).map(|s| s.hwnd);
    let timer_hwnd = slots.first().map(|s| s.hwnd);

    SESSION
        .try_with(|cell| {
            *cell.borrow_mut() = Some(Session {
                renderer,
                interaction,
                windows: slots,
                stroke_width,
                show_lasso_bounds,
                visual,
                started: Instant::now(),
                error: None,
                mode,
            });
        })
        .map_err(|_| OverlayError::InvalidInput("无法初始化覆盖层会话".to_string()))?;

    for (hwnd, x, y, w, h) in placements {
        // SAFETY: hwnd 是刚创建的有效窗口。
        unsafe { SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_SHOWWINDOW) }
            .map_err(win32("SetWindowPos"))?;
    }
    if let Some(hwnd) = focus_hwnd {
        // 键盘焦点必须在某个覆盖窗口上才能收到 Esc；失败时仍可用右键取消
        // SAFETY: hwnd 是本线程创建且已显示的窗口。
        discard(unsafe { SetForegroundWindow(hwnd) });
        // SAFETY: 同上。
        discard(unsafe { SetFocus(Some(hwnd)) });
    }
    if let Some(hwnd) = timer_hwnd {
        // 动画从窗口显示时算起，各显示器共用同一起点
        with_session(|s| s.started = Instant::now());
        start_glow_timer(hwnd, visual);
    }
    Ok(owner)
}

/// 当前光标的虚拟桌面坐标，取不到时返回 `None`。
fn cursor_position() -> Option<Point> {
    let mut cursor = POINT::default();
    // SAFETY: cursor 是有效的局部变量。
    unsafe { GetCursorPos(&mut cursor) }.ok()?;
    Some(Point::new(cursor.x, cursor.y))
}

fn pump_until_finished() -> Result<(), OverlayError> {
    loop {
        if with_session(|s| s.interaction.is_finished()).unwrap_or(true) {
            return Ok(());
        }
        let mut msg = MSG::default();
        // SAFETY: msg 是有效的局部变量，None 表示接收本线程所有窗口的消息。
        let status = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        match status.0 {
            0 => {
                // 收到退出请求：结束覆盖层，并把 WM_QUIT 交还给外层消息循环
                with_session(|s| s.interaction.cancel());
                // SAFETY: 无前置条件，重新投递退出消息。
                unsafe { PostQuitMessage(i32::try_from(msg.wParam.0).unwrap_or(0)) };
                return Ok(());
            }
            -1 => {
                return Err(OverlayError::Win32 {
                    api: "GetMessageW",
                    source: windows::core::Error::from_thread(),
                });
            }
            _ => {}
        }
        // TranslateMessage 的返回值只表示是否产生了字符消息
        // SAFETY: msg 由 GetMessageW 填充。
        discard(unsafe { TranslateMessage(&msg) });
        // SAFETY: msg 由 GetMessageW 填充。
        discard(unsafe { DispatchMessageW(&msg) });
    }
}
