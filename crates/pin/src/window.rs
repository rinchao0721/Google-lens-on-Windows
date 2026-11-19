//! 贴图窗口宿主与 Win32 消息处理：管理最多 32 张置顶贴图窗口。

use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;

use lenswin_core::frame::CroppedImage;
use lenswin_core::geometry::{Point, Rect};
use lenswin_core::pin::{MAX_PIN_COUNT, PinEvent};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{InvalidateRect, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_1, VK_2, VK_C, VK_CONTROL, VK_ESCAPE,
    VK_MENU, VK_NUMPAD1, VK_NUMPAD2, VK_S,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    GW_HWNDNEXT, GWL_EXSTYLE, GetCursorPos, GetTopWindow, GetWindow, GetWindowLongPtrW, IDC_ARROW,
    IsWindowVisible, LWA_ALPHA, LoadCursorW, RegisterClassExW, SW_SHOW, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetLayeredWindowAttributes,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, UnregisterClassW, WM_CAPTURECHANGED, WM_DESTROY,
    WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONUP, WM_SETFOCUS, WNDCLASSEXW,
    WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::error::{PinError, last_error, win32};
use crate::menu::{self, MenuAction};
use crate::render::{Renderer, TargetResources, rgba_to_premultiplied_bgra};
use crate::state::PinState;

const CLASS_NAME: PCWSTR = w!("LenswinPinWindow");
const MAX_Z_WALK: usize = 10_000;

/// 贴图窗口标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PinId(u64);

struct PinSlot {
    hwnd: HWND,
    state: PinState,
    pbgra: Vec<u8>,
    target: Option<TargetResources>,
}

struct PinHost {
    instance: HINSTANCE,
    renderer: Renderer,
    sink: Rc<dyn Fn(PinEvent)>,
    next_id: u64,
    slots: Vec<PinSlot>,
}

thread_local! {
    static HOST: RefCell<Option<PinHost>> = const { RefCell::new(None) };
}

fn with_host<R>(f: impl FnOnce(&mut PinHost) -> R) -> Option<R> {
    HOST.try_with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
    .ok()
    .flatten()
}

fn with_slot<R>(hwnd: HWND, f: impl FnOnce(&mut PinSlot) -> R) -> Option<R> {
    with_host(|host| host.slots.iter_mut().find(|s| s.hwnd == hwnd).map(f)).flatten()
}

fn emit(event: PinEvent) {
    if let Some(sink) = with_host(|host| Rc::clone(&host.sink)) {
        sink(event);
    }
}

/// 贴图窗口管理器；只能在主线程创建与使用，销毁时关闭全部贴图并注销窗口类。
pub struct PinManager {
    _not_send: PhantomData<Rc<()>>,
}

impl PinManager {
    /// 注册贴图窗口类并初始化渲染器；`sink` 用于把右键菜单与快捷键动作回投给主循环。
    pub fn new(sink: impl Fn(PinEvent) + 'static) -> Result<Self, PinError> {
        let already_exists = with_host(|_| ()).is_some();
        if already_exists {
            return Err(PinError::InvalidInput("贴图管理器不能重复创建".to_string()));
        }
        // SAFETY: 传 None 取当前 exe 模块句柄，无需释放。
        let module = unsafe { GetModuleHandleW(None) }.map_err(win32("GetModuleHandleW"))?;
        let instance = HINSTANCE(module.0);
        let renderer = Renderer::new()?;
        register_class(instance)?;

        HOST.with(|cell| {
            cell.replace(Some(PinHost {
                instance,
                renderer,
                sink: Rc::new(sink),
                next_id: 1,
                slots: Vec::new(),
            }));
        });
        Ok(Self {
            _not_send: PhantomData,
        })
    }

    /// 新建一张贴图并显示在 `at` 指定的虚拟桌面位置。
    pub fn pin(&self, image: CroppedImage, at: Rect) -> Result<PinId, PinError> {
        let count = self.count();
        if count >= MAX_PIN_COUNT {
            return Err(PinError::TooMany);
        }
        let state = PinState::new(image, at).map_err(PinError::InvalidInput)?;
        let pbgra = rgba_to_premultiplied_bgra(state.image().rgba());
        let alpha = state.alpha();
        let rect = state.rect();
        let w = i32::try_from(rect.width.max(1))
            .map_err(|_| PinError::InvalidInput("贴图宽度超出范围".to_string()))?;
        let h = i32::try_from(rect.height.max(1))
            .map_err(|_| PinError::InvalidInput("贴图高度超出范围".to_string()))?;
        let instance = with_host(|host| host.instance)
            .ok_or_else(|| PinError::InvalidInput("贴图管理器未初始化".to_string()))?;

        // SAFETY: 窗口类已注册，instance 为本进程句柄，父窗口为空。
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
                CLASS_NAME,
                w!("lenswin-pin"),
                WS_POPUP,
                rect.x,
                rect.y,
                w,
                h,
                None,
                None,
                Some(instance),
                None,
            )
        }
        .map_err(win32("CreateWindowExW"))?;

        // SAFETY: hwnd 刚由 WS_EX_LAYERED 创建成功。
        if let Err(e) = unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA) } {
            // 失败时立即销毁未登记的窗口，避免句柄泄漏
            // SAFETY: hwnd 刚在本函数内创建。
            crate::discard(unsafe { DestroyWindow(hwnd) });
            return Err(win32("SetLayeredWindowAttributes")(e));
        }

        let id = with_host(|host| {
            let id = PinId(host.next_id);
            host.next_id = host.next_id.saturating_add(1);
            host.slots.push(PinSlot {
                hwnd,
                state,
                pbgra,
                target: None,
            });
            id
        })
        .ok_or_else(|| PinError::InvalidInput("贴图管理器已释放".to_string()))?;

        // SAFETY: hwnd 已登记到 HOST 中，可安全接收 WM_SETFOCUS 与 WM_PAINT。
        crate::discard(unsafe { ShowWindow(hwnd, SW_SHOW) });
        // SAFETY: 标记整张窗口重绘。
        crate::discard(unsafe { InvalidateRect(Some(hwnd), None, false) });
        Ok(id)
    }

    /// 关闭全部贴图窗口。
    pub fn close_all(&self) {
        let hwnds: Vec<HWND> =
            with_host(|host| host.slots.iter().map(|s| s.hwnd).collect()).unwrap_or_default();
        for hwnd in hwnds {
            destroy_pin_hwnd(hwnd);
        }
    }

    /// 取消全部贴图的鼠标穿透状态。
    pub fn clear_passthrough(&self) {
        let hwnds: Vec<HWND> = with_host(|host| {
            host.slots
                .iter()
                .filter(|s| s.state.is_passthrough())
                .map(|s| s.hwnd)
                .collect()
        })
        .unwrap_or_default();
        for hwnd in hwnds {
            apply_passthrough(hwnd, false);
        }
    }

    /// 切换指定屏幕坐标下最上层贴图的鼠标穿透状态；该位置没有贴图时不做任何事。
    pub fn toggle_passthrough_at(&self, point: Point) {
        let candidates: Vec<(HWND, Rect, bool)> = with_host(|host| {
            host.slots
                .iter()
                .map(|s| (s.hwnd, s.state.rect(), s.state.is_passthrough()))
                .collect()
        })
        .unwrap_or_default();
        if candidates.is_empty() {
            return;
        }
        let Some((hwnd, current)) = topmost_pin_at(&candidates, point) else {
            return;
        };
        apply_passthrough(hwnd, !current);
    }

    /// 当前存活的贴图数量。
    pub fn count(&self) -> usize {
        with_host(|host| host.slots.len()).unwrap_or(0)
    }

    /// 当前处于鼠标穿透状态的贴图数量。
    pub fn passthrough_count(&self) -> usize {
        with_host(|host| {
            host.slots
                .iter()
                .filter(|s| s.state.is_passthrough())
                .count()
        })
        .unwrap_or(0)
    }
}

impl Drop for PinManager {
    fn drop(&mut self) {
        self.close_all();
        if let Some(instance) = with_host(|host| host.instance) {
            unregister_class(instance);
        }
        HOST.with(|cell| {
            cell.replace(None);
        });
    }
}

fn topmost_pin_at(candidates: &[(HWND, Rect, bool)], point: Point) -> Option<(HWND, bool)> {
    // WindowFromPoint 会跳过 WS_EX_TRANSPARENT 窗口，恢复穿透必须按 Z 序从顶向下遍历
    // SAFETY: 传 None 从最顶层窗口开始只读遍历窗口链表。
    let mut cur = unsafe { GetTopWindow(None) }.ok();
    for _ in 0..MAX_Z_WALK {
        let Some(hwnd) = cur else {
            break;
        };
        if let Some(&(_, rect, passthrough)) = candidates.iter().find(|(h, _, _)| *h == hwnd)
            && rect.contains(point)
            // SAFETY: hwnd 属于当前贴图管理器且正在查询可见性。
            && unsafe { IsWindowVisible(hwnd) }.as_bool()
        {
            return Some((hwnd, passthrough));
        }
        // SAFETY: hwnd 为上一轮取得的顶层窗口句柄。
        cur = unsafe { GetWindow(hwnd, GW_HWNDNEXT) }.ok();
    }
    None
}

fn register_class(instance: HINSTANCE) -> Result<(), PinError> {
    // SAFETY: 加载系统标准箭头光标，共享句柄无需释放。
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.map_err(win32("LoadCursorW"))?;
    let wc = WNDCLASSEXW {
        cbSize: u32::try_from(std::mem::size_of::<WNDCLASSEXW>())
            .map_err(|_| PinError::InvalidInput("WNDCLASSEXW 尺寸溢出".to_string()))?,
        style: CS_DBLCLKS | CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance,
        hCursor: cursor,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: wc 完整初始化，lpfnWndProc 与 lpszClassName 均为静态有效项。
    let atom = unsafe { RegisterClassExW(&wc) };
    if atom == 0 {
        return Err(last_error("RegisterClassExW"));
    }
    Ok(())
}

fn unregister_class(instance: HINSTANCE) {
    // 析构时如果仍有残余消息失败也无法上报
    // SAFETY: CLASS_NAME 与 instance 与 register_class 配对。
    crate::discard(unsafe { UnregisterClassW(CLASS_NAME, Some(instance)) });
}

fn destroy_pin_hwnd(hwnd: HWND) {
    // 若当前窗口正在被鼠标捕获拖动，先释放捕获再销毁
    let dragging = with_slot(hwnd, |s| s.state.end_drag()).unwrap_or(false);
    if dragging {
        // SAFETY: 当前窗口正持有鼠标捕获。
        crate::discard(unsafe { ReleaseCapture() });
    }
    // DestroyWindow 会同步触发 WM_DESTROY，调用前不可持有 HOST 借用
    // SAFETY: hwnd 为本线程创建的贴图窗口。
    crate::discard(unsafe { DestroyWindow(hwnd) });
}

fn apply_passthrough(hwnd: HWND, enabled: bool) {
    let changed = with_slot(hwnd, |s| s.state.set_passthrough(enabled)).unwrap_or(false);
    if !changed {
        return;
    }
    // SAFETY: hwnd 为有效窗口，仅读写扩展样式。
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let transparent = isize::try_from(WS_EX_TRANSPARENT.0).unwrap_or(0);
    let next = if enabled {
        style | transparent
    } else {
        style & !transparent
    };
    // SAFETY: 同上。
    unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next) };
    // 扩展样式修改后需要 SWP_FRAMECHANGED 让命中测试立即刷新
    // SAFETY: hwnd 有效。
    crate::discard(unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )
    });
    // SAFETY: 重绘实线/虚线边框。
    crate::discard(unsafe { InvalidateRect(Some(hwnd), None, false) });
}

fn apply_rect(hwnd: HWND, rect: Rect) {
    let Ok(w) = i32::try_from(rect.width.max(1)) else {
        return;
    };
    let Ok(h) = i32::try_from(rect.height.max(1)) else {
        return;
    };
    // SAFETY: hwnd 为当前线程有效窗口，不改变 Z 序和焦点。
    crate::discard(unsafe {
        SetWindowPos(
            hwnd,
            None,
            rect.x,
            rect.y,
            w,
            h,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    });
    // SAFETY: 尺寸变化后标记重绘。
    crate::discard(unsafe { InvalidateRect(Some(hwnd), None, false) });
}

fn cursor_screen_pos() -> Option<Point> {
    let mut pt = POINT::default();
    // SAFETY: pt 是有效的输出局部变量。
    unsafe { GetCursorPos(&mut pt) }.ok()?;
    Some(Point::new(pt.x, pt.y))
}

fn key_down(vk: u16) -> bool {
    // SAFETY: 查询虚键当前按键状态，无副作用。
    let state = unsafe { GetKeyState(i32::from(vk)) };
    state < 0
}

fn wheel_steps(wparam: WPARAM) -> i32 {
    let hi = u16::try_from((wparam.0 >> 16) & 0xFFFF).unwrap_or(0);
    let delta = i32::from(i16::from_ne_bytes(hi.to_ne_bytes()));
    if delta == 0 {
        0
    } else if delta.abs() >= 120 {
        delta / 120
    } else {
        delta.signum()
    }
}

fn on_paint(hwnd: HWND) {
    let recreate = with_host(|host| {
        let Some(slot) = host.slots.iter_mut().find(|s| s.hwnd == hwnd) else {
            return false;
        };
        if slot.target.is_none() {
            let rect = slot.state.rect();
            let img_size = (slot.state.image().width(), slot.state.image().height());
            match host.renderer.create_target(
                hwnd,
                (rect.width, rect.height),
                img_size,
                &slot.pbgra,
            ) {
                Ok(target) => slot.target = Some(target),
                Err(_) => return false,
            }
        }
        let Some(target) = slot.target.as_mut() else {
            return false;
        };
        match host.renderer.draw(target, &slot.state) {
            Ok(need_recreate) => {
                if need_recreate {
                    slot.target = None;
                }
                need_recreate
            }
            Err(_) => false,
        }
    })
    .unwrap_or(false);

    if recreate {
        // 渲染目标因设备丢失被清空时下一次继续重绘
        // SAFETY: hwnd 为当前窗口。
        crate::discard(unsafe { InvalidateRect(Some(hwnd), None, false) });
    } else {
        // SAFETY: hwnd 为当前窗口，整个客户区已由 Direct2D 绘制。
        crate::discard(unsafe { ValidateRect(Some(hwnd), None) });
    }
}

fn on_mouse_wheel(hwnd: HWND, wparam: WPARAM) {
    let steps = wheel_steps(wparam);
    if steps == 0 {
        return;
    }
    if key_down(VK_CONTROL.0) {
        let alpha = with_slot(hwnd, |s| {
            s.state.adjust_opacity(steps).then(|| s.state.alpha())
        })
        .flatten();
        if let Some(a) = alpha {
            // SAFETY: hwnd 为 WS_EX_LAYERED 窗口，a 为 0..=255。
            crate::discard(unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), a, LWA_ALPHA) });
        }
        return;
    }
    let Some(cursor) = cursor_screen_pos() else {
        return;
    };
    let next_rect = with_slot(hwnd, |s| {
        s.state.zoom_at(cursor, steps).then(|| s.state.rect())
    })
    .flatten();
    if let Some(rect) = next_rect {
        apply_rect(hwnd, rect);
    }
}

fn on_key_down(hwnd: HWND, wparam: WPARAM) {
    let vk = u16::try_from(wparam.0 & 0xFFFF).unwrap_or(0);
    let ctrl = key_down(VK_CONTROL.0);
    let alt = key_down(VK_MENU.0);

    if vk == VK_ESCAPE.0 && !ctrl && !alt {
        destroy_pin_hwnd(hwnd);
        return;
    }
    if (vk == VK_1.0 || vk == VK_NUMPAD1.0) && !ctrl && !alt {
        if let Some(rect) = with_slot(hwnd, |s| s.state.rotate_ccw()) {
            apply_rect(hwnd, rect);
        }
        return;
    }
    if (vk == VK_2.0 || vk == VK_NUMPAD2.0) && !ctrl && !alt {
        if let Some(rect) = with_slot(hwnd, |s| s.state.rotate_cw()) {
            apply_rect(hwnd, rect);
        }
        return;
    }
    if vk == VK_C.0 && ctrl && !alt {
        if let Some(Ok(img)) = with_slot(hwnd, |s| s.state.export_image()) {
            emit(PinEvent::Copy(img));
        }
        return;
    }
    if vk == VK_S.0
        && ctrl
        && !alt
        && let Some(Ok(img)) = with_slot(hwnd, |s| s.state.export_image())
    {
        emit(PinEvent::Save(img));
    }
}

fn on_right_button_up(hwnd: HWND) {
    if with_slot(hwnd, |s| s.state.end_drag()).unwrap_or(false) {
        // SAFETY: 结束残留拖动状态。
        crate::discard(unsafe { ReleaseCapture() });
    }
    let Some(passthrough) = with_slot(hwnd, |s| s.state.is_passthrough()) else {
        return;
    };
    let Some(pt) = cursor_screen_pos() else {
        return;
    };
    let Ok(Some(action)) = menu::show_context_menu(hwnd, pt, passthrough) else {
        return;
    };
    match action {
        MenuAction::Copy => {
            if let Some(Ok(img)) = with_slot(hwnd, |s| s.state.export_image()) {
                emit(PinEvent::Copy(img));
            }
        }
        MenuAction::Save => {
            if let Some(Ok(img)) = with_slot(hwnd, |s| s.state.export_image()) {
                emit(PinEvent::Save(img));
            }
        }
        MenuAction::Search => {
            if let Some((Ok(img), rect)) =
                with_slot(hwnd, |s| (s.state.export_image(), s.state.rect()))
            {
                emit(PinEvent::Search(img, rect));
            }
        }
        MenuAction::Ocr => {
            if let Some(Ok(img)) = with_slot(hwnd, |s| s.state.export_image()) {
                emit(PinEvent::Ocr(img));
            }
        }
        MenuAction::TogglePassthrough => {
            apply_passthrough(hwnd, !passthrough);
        }
        MenuAction::Close => {
            destroy_pin_hwnd(hwnd);
        }
    }
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
        // 整个客户区由 Direct2D 绘制，跳过背景擦除防闪烁
        WM_ERASEBKGND => return LRESULT(1),
        // 拖动到不同 DPI 显示器时保持 1:1 物理像素尺寸，不采用系统建议矩形
        WM_DPICHANGED => return LRESULT(0),
        WM_SETFOCUS => {
            if with_slot(hwnd, |s| s.state.set_focused(true)).unwrap_or(false) {
                // SAFETY: hwnd 为当前窗口。
                crate::discard(unsafe { InvalidateRect(Some(hwnd), None, false) });
            }
            return LRESULT(0);
        }
        WM_KILLFOCUS => {
            if with_slot(hwnd, |s| s.state.set_focused(false)).unwrap_or(false) {
                // SAFETY: hwnd 为当前窗口。
                crate::discard(unsafe { InvalidateRect(Some(hwnd), None, false) });
            }
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            // SAFETY: 点击贴图时聚焦，确保快捷键即刻可用。
            crate::discard(unsafe { SetFocus(Some(hwnd)) });
            if let Some(pt) = cursor_screen_pos() {
                let started = with_slot(hwnd, |s| {
                    s.state.begin_drag(pt);
                    s.state.is_dragging()
                })
                .unwrap_or(false);
                if started {
                    // SAFETY: 当前线程刚收到左键按下消息。
                    unsafe { SetCapture(hwnd) };
                }
            }
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            if let Some(pt) = cursor_screen_pos()
                && let Some(Some(origin)) = with_slot(hwnd, |s| s.state.update_drag(pt))
            {
                // SAFETY: 只更新位置不改尺寸，避免拖动抖动。
                crate::discard(unsafe {
                    SetWindowPos(
                        hwnd,
                        None,
                        origin.x,
                        origin.y,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                });
            }
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            if with_slot(hwnd, |s| s.state.end_drag()).unwrap_or(false) {
                // SAFETY: 与 WM_LBUTTONDOWN 的 SetCapture 配对。
                crate::discard(unsafe { ReleaseCapture() });
            }
            return LRESULT(0);
        }
        WM_CAPTURECHANGED => {
            with_slot(hwnd, |s| s.state.end_drag());
            return LRESULT(0);
        }
        WM_LBUTTONDBLCLK => {
            destroy_pin_hwnd(hwnd);
            return LRESULT(0);
        }
        WM_RBUTTONUP => {
            on_right_button_up(hwnd);
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            on_mouse_wheel(hwnd, wparam);
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            on_key_down(hwnd, wparam);
            return LRESULT(0);
        }
        WM_DESTROY => {
            with_host(|host| {
                host.slots.retain(|s| s.hwnd != hwnd);
            });
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: 未处理的窗口消息转交系统默认处理。
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
