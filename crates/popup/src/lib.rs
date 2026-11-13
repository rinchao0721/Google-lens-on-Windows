//! 内置搜索弹窗：WebView2 宿主窗口，图片由 WebView 直接 POST 给 Google Lens，结果页在弹窗内显示。
//!
//! 弹窗运行在主线程（STA），与托盘消息循环共用；对外的请求（在浏览器打开、复制链接、加载失败）
//! 只通过事件回投给调用方，本 crate 不调用 shell、剪贴板或写盘。

#![deny(unsafe_op_in_unsafe_fn)]

mod error;
mod page;
mod web;
mod window;

use std::cell::RefCell;
use std::marker::PhantomData;
use std::path::Path;
use std::rc::Rc;

use lenswin_core::geometry::Rect;
use lenswin_core::lens::LensRequest;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Controller, ICoreWebView2Environment,
};
use windows::Win32::Foundation::{HINSTANCE, HWND};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::core::HSTRING;

pub use error::PopupError;

use crate::error::win32;

/// 弹窗回投给调用方的请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupEvent {
    /// 用默认浏览器打开该地址。
    OpenExternal(String),
    /// 把该地址复制到剪贴板。
    CopyLink(String),
    /// 弹窗加载失败，附带原因。
    LoadFailed(String),
    /// 弹窗被用户（Esc、关闭按钮）或异常收起；调用方主动 [`PopupHost::hide`] 时不发。
    Closed,
}

/// 已就绪的 WebView 组件，COM 指针可廉价克隆。
#[derive(Clone)]
pub(crate) struct WebParts {
    pub(crate) env: ICoreWebView2Environment,
    pub(crate) controller: ICoreWebView2Controller,
    pub(crate) webview: ICoreWebView2,
}

/// WebView 的生命周期；`Absent` 也表示已因空闲释放。
pub(crate) enum Web {
    Absent,
    Creating,
    Ready(WebParts),
}

/// WebView 就绪前积压的内容。
pub(crate) enum Pending {
    Request(LensRequest),
    Error(String),
}

/// 弹窗的线程内状态；窗口过程和 WebView 回调都从这里取。
pub(crate) struct Host {
    pub(crate) instance: HINSTANCE,
    pub(crate) data_dir: HSTRING,
    pub(crate) sink: Rc<dyn Fn(PopupEvent)>,
    pub(crate) hwnd: Option<HWND>,
    pub(crate) web: Web,
    /// 每次创建或释放 WebView 自增，用来丢弃过期的异步回调。
    pub(crate) generation: u64,
    pub(crate) pending: Option<Pending>,
    pub(crate) current_navigation: Option<CurrentNavigation>,
    pub(crate) placement: Placement,
}

/// 窗口是否正由 `place` 设置位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placement {
    Idle,
    Placing,
}

/// 最近一次放行的顶层导航；只处理它的完成结果，被打断的旧导航一律忽略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CurrentNavigation {
    pub(crate) id: u64,
    /// 本地加载页或错误页；它们失败时不再显示错误页，避免互相打断形成循环。
    pub(crate) local: bool,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// 在状态上执行操作；状态不存在或正被借用（重入）时返回 `None`。
pub(crate) fn with_host<R>(f: impl FnOnce(&mut Host) -> R) -> Option<R> {
    HOST.try_with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
    .ok()
    .flatten()
}

/// 把事件交给调用方；调用前已释放状态借用，回调里再进入弹窗也安全。
pub(crate) fn emit(event: PopupEvent) {
    match with_host(|host| Rc::clone(&host.sink)) {
        Some(sink) => sink(event),
        None => log::warn!("弹窗状态不可用，丢弃事件：{event:?}"),
    }
}

pub(crate) fn web_parts() -> Option<WebParts> {
    with_host(|host| match &host.web {
        Web::Ready(parts) => Some(parts.clone()),
        Web::Absent | Web::Creating => None,
    })
    .flatten()
}

/// 内置搜索弹窗的所有者；只能在主线程创建和使用，销毁时释放窗口、WebView 与 COM。
pub struct PopupHost {
    com_initialized: bool,
    _not_send: PhantomData<Rc<()>>,
}

impl PopupHost {
    /// 初始化 COM、注册窗口类；窗口与 WebView 在第一次 [`PopupHost::show`] 时才创建。
    pub fn new(data_dir: &Path, sink: impl Fn(PopupEvent) + 'static) -> Result<Self, PopupError> {
        // SAFETY: 无前置条件；成功（含 S_FALSE）时在 Drop 中配对 CoUninitialize。
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let host = Self {
            com_initialized: hr.is_ok(),
            _not_send: PhantomData,
        };
        if !host.com_initialized {
            log::warn!("主线程 COM 初始化返回 {hr:?}，沿用现有套间");
        }
        // SAFETY: None 表示取当前进程 exe 的模块句柄，无需释放。
        let module = unsafe { GetModuleHandleW(None) }.map_err(win32("GetModuleHandleW"))?;
        let instance = HINSTANCE(module.0);
        window::register_class(instance)?;
        HOST.with(|cell| {
            cell.replace(Some(Host {
                instance,
                data_dir: HSTRING::from(data_dir.as_os_str()),
                sink: Rc::new(sink),
                hwnd: None,
                web: Web::Absent,
                generation: 0,
                pending: None,
                current_navigation: None,
                placement: Placement::Idle,
            }))
        });
        Ok(host)
    }

    /// 本机可用的 WebView2 Runtime 版本；未安装时返回 `None`。
    pub fn runtime_version() -> Option<String> {
        web::runtime_version()
    }

    /// 在选区旁显示弹窗和加载页；WebView 尚未创建时开始异步创建。
    pub fn show(&self, anchor: &Rect) -> Result<(), PopupError> {
        if web::runtime_version().is_none() {
            return Err(PopupError::RuntimeMissing);
        }
        let hwnd = window::ensure_window()?;
        window::place(hwnd, anchor)?;
        with_host(|host| host.pending = None);
        let activated = web::activate(hwnd);
        if activated.is_err() {
            window::hide(hwnd);
        }
        activated
    }

    /// 提交直传请求；WebView 未就绪时先积压，弹窗已被关闭则丢弃。
    pub fn submit(&self, request: LensRequest) {
        if !window::is_visible() {
            log::info!("弹窗已关闭，丢弃本次搜索请求");
            return;
        }
        match web_parts() {
            Some(parts) => web::navigate_post(&parts, &request),
            None => {
                with_host(|host| host.pending = Some(Pending::Request(request)));
            }
        }
    }

    /// 隐藏弹窗（如开始新的圈选前）；未显示时什么也不做。
    pub fn hide(&self) {
        if window::is_visible()
            && let Some(hwnd) = with_host(|host| host.hwnd).flatten()
        {
            window::hide(hwnd);
        }
    }

    /// 在弹窗内显示错误；WebView 未就绪时先积压。
    pub fn show_error(&self, message: &str) {
        if !window::is_visible() {
            return;
        }
        match web_parts() {
            Some(parts) => web::navigate_error(&parts, message),
            None => {
                with_host(|host| host.pending = Some(Pending::Error(message.to_string())));
            }
        }
    }
}

impl Drop for PopupHost {
    fn drop(&mut self) {
        web::release();
        window::destroy();
        if let Some(instance) = with_host(|host| host.instance) {
            window::unregister_class(instance);
        }
        HOST.with(|cell| cell.replace(None));
        if self.com_initialized {
            // SAFETY: 与 new 中成功的 CoInitializeEx 配对，仅调用一次。
            unsafe { CoUninitialize() };
        }
    }
}
