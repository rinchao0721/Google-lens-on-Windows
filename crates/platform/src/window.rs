//! 隐藏的消息窗口：接收热键、托盘与跨进程通知，转成 [`WindowEvent`] 排队。

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};

use lenswin_core::ipc::SECOND_INSTANCE_MESSAGE_OFFSET;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, PostMessageW, RegisterClassExW,
    RegisterWindowMessageW, UnregisterClassW, WM_APP, WM_CONTEXTMENU, WM_HOTKEY, WM_LBUTTONUP,
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

thread_local! {
    static EVENTS: RefCell<VecDeque<WindowEvent>> = const { RefCell::new(VecDeque::new()) };
}

fn enqueue(event: WindowEvent) {
    let result = EVENTS.try_with(|queue| {
        if let Ok(mut queue) = queue.try_borrow_mut() {
            queue.push_back(event);
        }
    });
    match result {
        Ok(()) | Err(_) => {} // 线程退出阶段产生的事件无需处理
    }
}

/// 丢弃尚未处理的热键事件，用于模态界面（如选区覆盖层）结束后清掉期间积压的热键。
///
/// 其它事件（托盘重建、第二实例通知、托盘点击）原样保留。
pub fn discard_pending_hotkeys() {
    let result = EVENTS.try_with(|queue| {
        if let Ok(mut queue) = queue.try_borrow_mut() {
            retain_non_hotkeys(&mut queue);
        }
    });
    match result {
        Ok(()) | Err(_) => {} // 线程退出阶段无事件可清
    }
}

fn retain_non_hotkeys(queue: &mut VecDeque<WindowEvent>) {
    queue.retain(|event| !matches!(event, WindowEvent::Hotkey(_)));
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

    /// 可跨线程使用的任务完成通知器。
    pub fn task_notifier(&self) -> TaskNotifier {
        TaskNotifier {
            hwnd: self.hwnd.0.expose_provenance(),
        }
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
                enqueue(WindowEvent::Hotkey(id));
            }
        }
        WM_TRAY => {
            // 传统托盘回调的鼠标消息在 lparam 低 16 位
            match u32::try_from(lparam.0 & 0xFFFF) {
                Ok(WM_LBUTTONUP) => enqueue(WindowEvent::Tray(TrayMouse::LeftClick)),
                Ok(WM_RBUTTONUP | WM_CONTEXTMENU) => {
                    enqueue(WindowEvent::Tray(TrayMouse::RightClick));
                }
                _ => {}
            }
        }
        WM_SECOND_INSTANCE => enqueue(WindowEvent::SecondInstance),
        WM_TASK_DONE => enqueue(WindowEvent::TaskDone),
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            enqueue(WindowEvent::TaskbarCreated);
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
    fn retain_non_hotkeys_drops_only_hotkeys() {
        let mut queue = VecDeque::from([
            WindowEvent::Hotkey(1),
            WindowEvent::TaskbarCreated,
            WindowEvent::Hotkey(2),
            WindowEvent::SecondInstance,
            WindowEvent::Tray(TrayMouse::RightClick),
        ]);
        retain_non_hotkeys(&mut queue);
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
    fn discard_pending_hotkeys_keeps_other_events_in_queue() {
        enqueue(WindowEvent::Hotkey(1));
        enqueue(WindowEvent::TaskbarCreated);
        enqueue(WindowEvent::Hotkey(2));
        enqueue(WindowEvent::SecondInstance);
        discard_pending_hotkeys();
        assert_eq!(pop_event(), Some(WindowEvent::TaskbarCreated));
        assert_eq!(pop_event(), Some(WindowEvent::SecondInstance));
        assert_eq!(pop_event(), None);
    }
}
