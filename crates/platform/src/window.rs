//! 隐藏的消息窗口：接收热键、托盘与跨进程通知，转成 [`WindowEvent`] 排队。

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

use lenswin_core::ipc::{IpcResult, MAX_PAYLOAD_BYTES, SECOND_INSTANCE_MESSAGE_OFFSET};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, PostMessageW, RegisterClassExW,
    RegisterWindowMessageW, UnregisterClassW, WM_APP, WM_COPYDATA, WM_HOTKEY, WM_LBUTTONUP,
    WM_RBUTTONUP, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
};
use windows::core::{PCWSTR, w};

use crate::error::{PlatformError, last_error, win32};
use crate::handle::discard;
use crate::text::to_wide;

/// 托盘图标回调消息。
pub(crate) const WM_TRAY: u32 = WM_APP + 1;
/// 第二个实例的通知消息。
pub(crate) const WM_SECOND_INSTANCE: u32 = WM_APP + SECOND_INSTANCE_MESSAGE_OFFSET;

/// 后台任务完成的通知消息。
pub(crate) const WM_TASK_DONE: u32 = WM_APP + 3;

/// 唤醒消息循环去处理已入队事件。
///
/// 托盘回调是同步送入窗口过程的，`GetMessageW` 处理完不会返回，
/// 不另投一条消息的话，事件会一直留在队列里。
pub(crate) const WM_EVENT_PENDING: u32 = WM_APP + 4;

/// 资源管理器重启后系统广播的消息号，创建窗口时注册。
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// 托盘图标上的鼠标动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayMouse {
    /// 左键单击。
    LeftClick,
    /// 右键单击。
    RightClick,
}

/// 消息窗口产生的事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowEvent {
    /// 全局热键触发，值为注册时的 id。
    Hotkey(i32),
    /// 托盘图标被点击。
    Tray(TrayMouse),
    /// 资源管理器重启，托盘图标需要重建。
    TaskbarCreated,
    /// 又启动了一个实例。
    SecondInstance,
    /// 后台任务有结果待取。
    TaskDone,
}

/// 处理跨进程提交的回调：参数为消息类型与负载，返回 [`IpcResult`] 的数值。
type CopyDataHandler = Rc<dyn Fn(u32, &[u8]) -> u32>;

thread_local! {
    static EVENTS: RefCell<VecDeque<WindowEvent>> = const { RefCell::new(VecDeque::new()) };
    /// 已投递但窗口过程尚未收到的唤醒消息，避免重复投递。
    static WAKE_PENDING: Cell<bool> = const { Cell::new(false) };
    static COPYDATA_HANDLER: RefCell<Option<CopyDataHandler>> = const { RefCell::new(None) };
}

/// 登记处理 `WM_COPYDATA` 的回调，只对调用线程的消息窗口生效。
///
/// 回调在窗口过程里同步执行，发送方正阻塞等待返回值，所以只应做短小的工作，
/// 且不得在其中弹出模态界面或持有跨调用的借用。
pub fn set_copydata_handler(handler: impl Fn(u32, &[u8]) -> u32 + 'static) {
    COPYDATA_HANDLER.with(|slot| slot.replace(Some(Rc::new(handler))));
}

/// 撤销 [`set_copydata_handler`] 登记的回调，让回调持有的资源能在消息循环结束后释放。
pub fn clear_copydata_handler() {
    // 线程退出阶段 TLS 已销毁时无需处理
    match COPYDATA_HANDLER.try_with(|slot| slot.replace(None)) {
        Ok(_) | Err(_) => {}
    }
}

/// 解析 `WM_COPYDATA` 并交给回调；任何异常都返回对应结果码，不 panic。
fn handle_copydata(lparam: LPARAM) -> u32 {
    let reject = IpcResult::InvalidMessage.code();
    let address = usize::try_from(lparam.0).unwrap_or(0);
    if address == 0 {
        return reject;
    }
    let data_ptr = std::ptr::with_exposed_provenance::<COPYDATASTRUCT>(address);
    // SAFETY: WM_COPYDATA 的 lparam 在消息处理期间指向发送方构造的有效 COPYDATASTRUCT。
    let data = unsafe { &*data_ptr };
    let Ok(len) = usize::try_from(data.cbData) else {
        return IpcResult::PayloadTooLarge.code();
    };
    if len > MAX_PAYLOAD_BYTES {
        return IpcResult::PayloadTooLarge.code();
    }
    let payload: &[u8] = if len == 0 || data.lpData.is_null() {
        &[]
    } else {
        // SAFETY: 系统保证 lpData 指向 cbData 字节的可读内存，且在本次消息处理期间有效；len 已校验。
        unsafe { std::slice::from_raw_parts(data.lpData.cast::<u8>(), len) }
    };
    let Ok(kind) = u32::try_from(data.dwData) else {
        return reject;
    };
    // 先取出回调再调用，避免回调内部再登记时与借用冲突
    let handler = COPYDATA_HANDLER
        .try_with(|slot| slot.try_borrow().ok().and_then(|h| h.clone()))
        .ok()
        .flatten();
    match handler {
        Some(handler) => handler(kind, payload),
        None => reject,
    }
}

/// 入队；队列里已有未处理的右键时丢弃重复的右键，保证连点只弹一次菜单。
fn push_coalesced(queue: &mut VecDeque<WindowEvent>, event: WindowEvent) {
    let right_click = WindowEvent::Tray(TrayMouse::RightClick);
    if event == right_click && queue.contains(&right_click) {
        return;
    }
    queue.push_back(event);
}

/// 托盘回调 lparam 低 16 位对应的鼠标动作。
///
/// 右键只认 `WM_RBUTTONUP`：未设置 `NOTIFYICON_VERSION_4` 时系统只发鼠标消息，
/// 不发 `WM_CONTEXTMENU`；两者都收，一次点击就可能触发两次菜单。
fn classify_tray_mouse(message: u32) -> Option<TrayMouse> {
    match message {
        WM_LBUTTONUP => Some(TrayMouse::LeftClick),
        WM_RBUTTONUP => Some(TrayMouse::RightClick),
        _ => None,
    }
}

/// 尚无在途唤醒消息时返回 `true`，并把状态记为在途。
fn claim_wake(pending: &Cell<bool>) -> bool {
    !pending.replace(true)
}

fn enqueue(event: WindowEvent) {
    let result = EVENTS.try_with(|queue| {
        if let Ok(mut queue) = queue.try_borrow_mut() {
            push_coalesced(&mut queue, event);
        }
    });
    match result {
        Ok(()) | Err(_) => {} // 线程退出阶段产生的事件无需处理
    }
}

fn set_wake_pending(value: bool) {
    match WAKE_PENDING.try_with(|pending| pending.set(value)) {
        Ok(()) | Err(_) => {} // 线程退出阶段无状态可更新
    }
}

/// 入队并确保消息循环会被唤醒。
fn enqueue_and_wake(hwnd: HWND, event: WindowEvent) {
    enqueue(event);
    // 线程退出阶段读不到状态时直接投递
    if !WAKE_PENDING.try_with(claim_wake).unwrap_or(true) {
        return;
    }
    // SAFETY: hwnd 是正在处理消息的本窗口；PostMessageW 失败只返回错误。
    let posted = unsafe { PostMessageW(Some(hwnd), WM_EVENT_PENDING, WPARAM(0), LPARAM(0)) };
    if let Err(error) = posted {
        // 撤销在途标记，下个事件才能重试
        set_wake_pending(false);
        log::warn!("唤醒消息循环失败，事件会延迟到下一条消息：{error}");
    }
}

/// 丢弃尚未处理的热键与托盘左键事件，用于模态界面（如选区覆盖层）结束后清掉期间积压的触发。
///
/// 其它事件（托盘重建、第二实例通知、托盘右键）原样保留。
pub fn discard_pending_triggers() {
    let result = EVENTS.try_with(|queue| {
        if let Ok(mut queue) = queue.try_borrow_mut() {
            retain_non_triggers(&mut queue);
        }
    });
    match result {
        Ok(()) | Err(_) => {} // 线程退出阶段无事件可清
    }
}

fn retain_non_triggers(queue: &mut VecDeque<WindowEvent>) {
    queue.retain(|event| {
        !matches!(
            event,
            WindowEvent::Hotkey(_) | WindowEvent::Tray(TrayMouse::LeftClick)
        )
    });
}

/// 取出最早的待处理事件。
pub(crate) fn pop_event() -> Option<WindowEvent> {
    EVENTS
        .try_with(|queue| queue.try_borrow_mut().ok()?.pop_front())
        .ok()
        .flatten()
}

/// 隐藏的顶层消息窗口，销毁时同时注销窗口类。
///
/// 使用顶层窗口而非 message-only 窗口，才能收到 `TaskbarCreated` 广播并被 `FindWindowW` 找到。
pub struct MessageWindow {
    hwnd: HWND,
    class_name: Vec<u16>,
    instance: HINSTANCE,
}

impl MessageWindow {
    /// 创建窗口，`class_name` 为固定窗口类名。
    pub fn create(class_name: &str) -> Result<Self, PlatformError> {
        let class_wide = to_wide(class_name);
        // SAFETY: 传入 None 取当前模块句柄。
        let module = unsafe { GetModuleHandleW(None) }.map_err(win32("GetModuleHandleW"))?;
        let instance = HINSTANCE(module.0);
        let size = u32::try_from(std::mem::size_of::<WNDCLASSEXW>())
            .map_err(|_| PlatformError::Invalid("窗口类结构过大".to_string()))?;
        let class = WNDCLASSEXW {
            cbSize: size,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance,
            lpszClassName: PCWSTR(class_wide.as_ptr()),
            ..Default::default()
        };
        // SAFETY: class 中的类名指针在本次调用期间有效，窗口过程是有效的 extern "system" 函数。
        if unsafe { RegisterClassExW(&class) } == 0 {
            return Err(last_error("RegisterClassExW"));
        }
        // SAFETY: 参数均有效；窗口类刚刚注册，title 为静态字符串。
        let created = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                PCWSTR(class_wide.as_ptr()),
                w!("lenswin"),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            )
        };
        let hwnd = match created {
            Ok(hwnd) => hwnd,
            Err(source) => {
                // SAFETY: 类名与注册时一致，此时没有该类的窗口。
                discard(unsafe { UnregisterClassW(PCWSTR(class_wide.as_ptr()), Some(instance)) });
                return Err(PlatformError::Win32 {
                    api: "CreateWindowExW",
                    source,
                });
            }
        };
        // SAFETY: 参数为静态字符串。
        let taskbar = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        TASKBAR_CREATED.store(taskbar, Ordering::Relaxed);
        Ok(Self {
            hwnd,
            class_name: class_wide,
            instance,
        })
    }

    /// 窗口句柄。
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// 可复制的窗口句柄数值，供不持有窗口本身的代码（如回调）注册热键。
    pub fn handle(&self) -> WindowHandle {
        WindowHandle {
            hwnd: self.hwnd.0.expose_provenance(),
        }
    }

    /// 可跨线程使用的任务完成通知器。
    pub fn task_notifier(&self) -> TaskNotifier {
        TaskNotifier {
            hwnd: self.hwnd.0.expose_provenance(),
        }
    }
}

/// 消息窗口句柄的数值副本，窗口销毁后不再有效。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowHandle {
    hwnd: usize,
}

impl WindowHandle {
    pub(crate) fn hwnd(self) -> HWND {
        HWND(std::ptr::with_exposed_provenance_mut(self.hwnd))
    }
}

/// 工作线程用它向消息窗口投递 [`WindowEvent::TaskDone`]。
///
/// 只保存窗口句柄的数值，窗口销毁后投递会失败并返回错误。
#[derive(Debug, Clone, Copy)]
pub struct TaskNotifier {
    hwnd: usize,
}

impl TaskNotifier {
    /// 通知主线程有任务结果。
    pub fn notify(&self) -> Result<(), PlatformError> {
        let hwnd = HWND(std::ptr::with_exposed_provenance_mut(self.hwnd));
        // SAFETY: PostMessageW 可跨线程调用；句柄无效时只会返回错误。
        unsafe { PostMessageW(Some(hwnd), WM_TASK_DONE, WPARAM(0), LPARAM(0)) }
            .map_err(win32("PostMessageW"))
    }
}

impl Drop for MessageWindow {
    fn drop(&mut self) {
        // Drop 中无法上报错误，销毁失败只会泄漏窗口
        // SAFETY: 窗口由 create 创建且仅在此处销毁，销毁后才注销窗口类。
        discard(unsafe { DestroyWindow(self.hwnd) });
        // SAFETY: 类名与注册时一致，窗口已销毁。
        discard(unsafe { UnregisterClassW(PCWSTR(self.class_name.as_ptr()), Some(self.instance)) });
    }
}

/// 窗口过程只负责把消息转成事件入队，业务在消息循环外处理，避免重入。
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_HOTKEY => {
            if let Ok(id) = i32::try_from(wparam.0) {
                enqueue_and_wake(hwnd, WindowEvent::Hotkey(id));
            }
        }
        WM_TRAY => {
            // 传统托盘回调的鼠标消息在 lparam 低 16 位
            let mouse = u32::try_from(lparam.0 & 0xFFFF)
                .ok()
                .and_then(classify_tray_mouse);
            if let Some(mouse) = mouse {
                enqueue_and_wake(hwnd, WindowEvent::Tray(mouse));
            }
        }
        WM_COPYDATA => {
            let code = handle_copydata(lparam);
            return LRESULT(isize::try_from(code).unwrap_or(0));
        }
        WM_SECOND_INSTANCE => enqueue_and_wake(hwnd, WindowEvent::SecondInstance),
        WM_TASK_DONE => enqueue_and_wake(hwnd, WindowEvent::TaskDone),
        // 消息循环在本条消息分发后处理队列，这里只需清掉在途标记
        WM_EVENT_PENDING => set_wake_pending(false),
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            enqueue_and_wake(hwnd, WindowEvent::TaskbarCreated);
        }
        _ => {}
    }
    // SAFETY: 参数原样来自系统传入的窗口消息。
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_right_button_up_opens_menu() {
        use windows::Win32::UI::WindowsAndMessaging::WM_CONTEXTMENU;
        assert_eq!(
            classify_tray_mouse(WM_RBUTTONUP),
            Some(TrayMouse::RightClick)
        );
        assert_eq!(
            classify_tray_mouse(WM_LBUTTONUP),
            Some(TrayMouse::LeftClick)
        );
        assert_eq!(classify_tray_mouse(WM_CONTEXTMENU), None);
        assert_eq!(classify_tray_mouse(0), None);
    }

    #[test]
    fn claim_wake_posts_once_until_cleared() {
        let pending = Cell::new(false);
        assert!(claim_wake(&pending));
        assert!(!claim_wake(&pending));
        pending.set(false);
        assert!(claim_wake(&pending));
    }

    #[test]
    fn duplicate_right_clicks_are_coalesced() {
        let mut queue = VecDeque::new();
        push_coalesced(&mut queue, WindowEvent::Tray(TrayMouse::RightClick));
        push_coalesced(&mut queue, WindowEvent::Tray(TrayMouse::RightClick));
        push_coalesced(&mut queue, WindowEvent::Tray(TrayMouse::LeftClick));
        push_coalesced(&mut queue, WindowEvent::TaskDone);
        push_coalesced(&mut queue, WindowEvent::TaskDone);
        assert_eq!(
            Vec::from(queue),
            vec![
                WindowEvent::Tray(TrayMouse::RightClick),
                WindowEvent::Tray(TrayMouse::LeftClick),
                WindowEvent::TaskDone,
                WindowEvent::TaskDone,
            ]
        );
    }

    #[test]
    fn right_click_after_drain_is_accepted_again() {
        let mut queue = VecDeque::new();
        push_coalesced(&mut queue, WindowEvent::Tray(TrayMouse::RightClick));
        queue.pop_front();
        push_coalesced(&mut queue, WindowEvent::Tray(TrayMouse::RightClick));
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn retain_non_triggers_drops_hotkeys_and_left_clicks() {
        let mut queue = VecDeque::from([
            WindowEvent::Hotkey(1),
            WindowEvent::TaskbarCreated,
            WindowEvent::Hotkey(2),
            WindowEvent::Tray(TrayMouse::LeftClick),
            WindowEvent::SecondInstance,
            WindowEvent::Tray(TrayMouse::RightClick),
            WindowEvent::Tray(TrayMouse::LeftClick),
        ]);
        retain_non_triggers(&mut queue);
        assert_eq!(
            Vec::from(queue),
            vec![
                WindowEvent::TaskbarCreated,
                WindowEvent::SecondInstance,
                WindowEvent::Tray(TrayMouse::RightClick),
            ]
        );
    }

    #[test]
    fn discard_pending_triggers_keeps_other_events_in_queue() {
        enqueue(WindowEvent::Hotkey(1));
        enqueue(WindowEvent::TaskbarCreated);
        enqueue(WindowEvent::Tray(TrayMouse::LeftClick));
        enqueue(WindowEvent::Hotkey(2));
        enqueue(WindowEvent::SecondInstance);
        discard_pending_triggers();
        assert_eq!(pop_event(), Some(WindowEvent::TaskbarCreated));
        assert_eq!(pop_event(), Some(WindowEvent::SecondInstance));
        assert_eq!(pop_event(), None);
    }
}
