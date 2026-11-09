//! 每个显示器一个置顶无边框窗口，共享同一份会话状态，内嵌消息循环直到选区结束。

use std::cell::RefCell;

use lenswin_core::geometry::Point;
use lenswin_core::monitor::MonitorInfo;
use windows::Win32::Foundation::{GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{InvalidateRect, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
    HWND_TOPMOST, IDC_CROSS, LoadCursorW, MSG, PostQuitMessage, RegisterClassExW, SWP_SHOWWINDOW,
    SetCursor, SetForegroundWindow, SetWindowPos, TranslateMessage, UnregisterClassW,
    WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT,
    WM_RBUTTONDOWN, WM_SETCURSOR, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::error::{OverlayError, win32};
use crate::input::{Interaction, Outcome, point_from_lparam};
use crate::render::{DrawStatus, Renderer, TargetResources};

/// 覆盖层窗口类名。
const CLASS_NAME: PCWSTR = w!("LenswinOverlayWindow");

/// 显式丢弃无法处理的返回值。
fn discard<T>(_value: T) {}

struct WindowSlot {
    hwnd: HWND,
    origin: Point,
    size: (u32, u32),
    bgra: Vec<u8>,
    target: Option<TargetResources>,
}

struct Session {
    renderer: Renderer,
    interaction: Interaction,
    windows: Vec<WindowSlot>,
    stroke_width: u32,
    error: Option<OverlayError>,
}

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

    fn paint(&mut self, index: usize) {
        let Some(slot) = self.windows.get_mut(index) else {
            return;
        };
        if slot.target.is_none() {
            match self
                .renderer
                .create_target(slot.hwnd, slot.size, &slot.bgra)
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
        match target.draw(
            slot.origin,
            slot.size,
            self.interaction.preview(),
            self.stroke_width,
        ) {
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
    Move,
    Up,
}

fn on_mouse(hwnd: HWND, lparam: LPARAM, event: Mouse) {
    with_session(|s| {
        let Some(index) = s.index_of(hwnd) else {
            return;
        };
        let local = point_from_lparam(lparam.0);
        let before = s.interaction.active_window();
        match event {
            Mouse::Down => {
                s.interaction.press(index, local);
                if s.interaction.is_dragging() {
                    // 捕获鼠标，拖出窗口或跨显示器时仍能收到移动和松开消息
                    // SAFETY: hwnd 是本线程的有效窗口。
                    discard(unsafe { SetCapture(hwnd) });
                }
            }
            Mouse::Move => s.interaction.drag(index, local),
            Mouse::Up => {
                let was_dragging = s.interaction.is_dragging();
                s.interaction.release(index, local);
                if was_dragging {
                    // 释放失败只会让捕获保留到下次点击，选区本身不受影响
                    // SAFETY: 无前置条件。
                    discard(unsafe { ReleaseCapture() });
                }
            }
        }
        // 预览只存在于按下时的显示器上，只重绘状态变化涉及的窗口
        let after = s.interaction.active_window();
        if let Some(i) = before {
            s.invalidate_window(i);
        }
        if let Some(i) = after.filter(|i| Some(*i) != before) {
            s.invalidate_window(i);
        }
    });
}

fn on_cancel() {
    with_session(|s| {
        s.interaction.cancel();
    });
}

fn on_paint(hwnd: HWND) {
    with_session(|s| {
        if let Some(index) = s.index_of(hwnd) {
            s.paint(index);
        }
    });
    // SAFETY: hwnd 是本线程的有效窗口，None 表示整个客户区。
    discard(unsafe { ValidateRect(Some(hwnd), None) });
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
            // SAFETY: 加载系统预置十字光标，共享句柄无需释放。
            if let Ok(cursor) = unsafe { LoadCursorW(None, IDC_CROSS) } {
                // SAFETY: cursor 是有效的光标句柄。
                discard(unsafe { SetCursor(Some(cursor)) });
            }
            return LRESULT(1);
        }
        WM_LBUTTONDOWN => on_mouse(hwnd, lparam, Mouse::Down),
        WM_MOUSEMOVE => on_mouse(hwnd, lparam, Mouse::Move),
        WM_LBUTTONUP => on_mouse(hwnd, lparam, Mouse::Up),
        WM_RBUTTONDOWN => on_cancel(),
        WM_KEYDOWN if wparam.0 == usize::from(VK_ESCAPE.0) => on_cancel(),
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

/// 会话所需的输入。
pub(crate) struct SessionInput {
    pub interaction: Interaction,
    pub monitors: Vec<MonitorInfo>,
    pub bgra_frames: Vec<Vec<u8>>,
    pub stroke_width: u32,
}

/// 显示覆盖层并运行消息循环，直到用户完成或取消选区。
pub(crate) fn run(input: SessionInput) -> Result<Outcome, OverlayError> {
    let SessionInput {
        interaction,
        monitors,
        bgra_frames,
        stroke_width,
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
            bgra,
            target: None,
        });
    }

    let focus_index = focus_window_index(&monitors);
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

    SESSION
        .try_with(|cell| {
            *cell.borrow_mut() = Some(Session {
                renderer,
                interaction,
                windows: slots,
                stroke_width,
                error: None,
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

    pump_until_finished()?;

    match with_session(|s| (s.error.take(), s.interaction.take_outcome())) {
        Some((Some(error), _)) => Err(error),
        Some((None, Some(outcome))) => Ok(outcome),
        _ => Ok(Outcome::Cancelled),
    }
}

/// 光标所在显示器对应的窗口序号，取不到时用第一个。
fn focus_window_index(monitors: &[MonitorInfo]) -> usize {
    let mut cursor = POINT::default();
    // SAFETY: cursor 是有效的局部变量。
    if unsafe { GetCursorPos(&mut cursor) }.is_err() {
        return 0;
    }
    lenswin_core::monitor::index_at(monitors, Point::new(cursor.x, cursor.y)).unwrap_or(0)
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
