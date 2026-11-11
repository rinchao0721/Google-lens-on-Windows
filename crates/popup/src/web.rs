//! WebView2 的创建、配置、导航与释放。所有 COM 调用都在释放状态借用之后进行，避免回调重入。

use lenswin_core::lens::{LensRequest, NavigationDecision, classify_navigation};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_COLOR, COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND,
    COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR, COREWEBVIEW2_KEY_EVENT_KIND,
    COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN, COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
    COREWEBVIEW2_PROCESS_FAILED_KIND, COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE, COREWEBVIEW2_WEB_ERROR_STATUS,
    COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED, CreateCoreWebView2EnvironmentWithOptions,
    GetAvailableCoreWebView2BrowserVersionString, ICoreWebView2, ICoreWebView2_2, ICoreWebView2_11,
    ICoreWebView2AcceleratorKeyPressedEventArgs, ICoreWebView2ContextMenuRequestedEventArgs,
    ICoreWebView2Controller, ICoreWebView2Controller2, ICoreWebView2Environment,
    ICoreWebView2Environment2, ICoreWebView2Environment9, ICoreWebView2EnvironmentOptions,
    ICoreWebView2NavigationCompletedEventArgs, ICoreWebView2NavigationCompletedEventArgs2,
    ICoreWebView2NavigationStartingEventArgs, ICoreWebView2NewWindowRequestedEventArgs,
    ICoreWebView2ProcessFailedEventArgs,
};
use webview2_com::{
    AcceleratorKeyPressedEventHandler, ContextMenuRequestedEventHandler,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    CustomItemSelectedEventHandler, NavigationCompletedEventHandler,
    NavigationStartingEventHandler, NewWindowRequestedEventHandler, ProcessFailedEventHandler,
    take_pwstr,
};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, IsWindowVisible};
use windows::core::{BOOL, HSTRING, Interface, PCWSTR, PWSTR, w};

use crate::error::{PopupError, win32};
use crate::page::{error_page, loading_page};
use crate::{
    CurrentNavigation, Pending, PopupEvent, Web, WebParts, emit, web_parts, window, with_host,
};

pub(crate) fn runtime_version() -> Option<String> {
    let mut version = PWSTR::null();
    // SAFETY: 空路径表示查询已安装的 Runtime；成功时返回的字符串由 take_pwstr 释放。
    match unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version) } {
        Ok(()) if !version.is_null() => {
            let text = take_pwstr(version);
            (!text.trim().is_empty()).then_some(text)
        }
        Ok(()) => None,
        Err(e) => {
            log::warn!("查询 WebView2 Runtime 失败：{e}");
            None
        }
    }
}

/// 弹窗显示后调用：WebView 已就绪则显示加载页，否则开始创建。
pub(crate) fn activate(hwnd: HWND) -> Result<(), PopupError> {
    if let Some(parts) = web_parts() {
        set_visible(true);
        fit_to_window(hwnd);
        navigate_loading(&parts);
        focus();
        return Ok(());
    }
    let start = with_host(|host| match host.web {
        Web::Creating => None,
        Web::Absent | Web::Ready(_) => {
            host.web = Web::Creating;
            host.generation = host.generation.wrapping_add(1);
            Some((host.generation, host.data_dir.clone()))
        }
    })
    .flatten();
    let Some((generation, data_dir)) = start else {
        return Ok(());
    };
    let handler =
        CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |result, env| {
            on_environment(generation, hwnd, result, env);
            Ok(())
        }));
    // SAFETY: 数据目录为有效宽字符串；回调在本线程的消息循环中触发。
    let created = unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            &data_dir,
            None::<&ICoreWebView2EnvironmentOptions>,
            &handler,
        )
    };
    if let Err(e) = created {
        with_host(|host| host.web = Web::Absent);
        return Err(win32("CreateCoreWebView2EnvironmentWithOptions")(e));
    }
    Ok(())
}

fn is_current(generation: u64) -> bool {
    with_host(|host| host.generation == generation && matches!(host.web, Web::Creating))
        .unwrap_or(false)
}

fn creation_failed(generation: u64, message: String) {
    let current = with_host(|host| {
        let current = host.generation == generation;
        if current {
            host.web = Web::Absent;
            host.pending = None;
        }
        current
    })
    .unwrap_or(false);
    if current {
        log::error!("{message}");
        window::hide_current();
        emit(PopupEvent::LoadFailed(message));
    }
}

fn on_environment(
    generation: u64,
    hwnd: HWND,
    result: windows::core::Result<()>,
    env: Option<ICoreWebView2Environment>,
) {
    if !is_current(generation) {
        return;
    }
    let env = match (result, env) {
        (Ok(()), Some(env)) => env,
        (Err(e), _) => return creation_failed(generation, format!("无法启动 WebView2：{e}")),
        (Ok(()), None) => {
            return creation_failed(generation, "无法启动 WebView2：环境为空".to_string());
        }
    };
    let env_for_controller = env.clone();
    let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
        move |result, controller| {
            on_controller(generation, hwnd, env_for_controller, result, controller);
            Ok(())
        },
    ));
    // SAFETY: hwnd 为弹窗窗口；回调在本线程触发。
    if let Err(e) = unsafe { env.CreateCoreWebView2Controller(hwnd, &handler) } {
        creation_failed(generation, format!("无法创建 WebView2 控件：{e}"));
    }
}

fn on_controller(
    generation: u64,
    hwnd: HWND,
    env: ICoreWebView2Environment,
    result: windows::core::Result<()>,
    controller: Option<ICoreWebView2Controller>,
) {
    let controller = match (result, controller) {
        (Ok(()), Some(controller)) => controller,
        (Err(e), _) => return creation_failed(generation, format!("无法创建 WebView2 控件：{e}")),
        (Ok(()), None) => {
            return creation_failed(generation, "无法创建 WebView2 控件：控件为空".to_string());
        }
    };
    if !is_current(generation) {
        // SAFETY: 过期的控件不会再被使用，关闭后释放。
        if let Err(e) = unsafe { controller.Close() } {
            log::warn!("关闭过期的 WebView2 控件失败：{e}");
        }
        return;
    }
    let parts = match configure(env, controller) {
        Ok(parts) => parts,
        Err(e) => return creation_failed(generation, format!("配置 WebView2 失败：{e}")),
    };
    with_host(|host| host.web = Web::Ready(parts.clone()));
    // SAFETY: hwnd 为弹窗窗口。
    let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
    set_visible(visible);
    fit_to_window(hwnd);
    navigate_loading(&parts);
    if visible {
        focus();
    }
    flush_pending(&parts);
}

/// 关闭开发者工具等不需要的功能，并挂上导航、按键、右键菜单与进程异常处理。
fn configure(
    env: ICoreWebView2Environment,
    controller: ICoreWebView2Controller,
) -> windows::core::Result<WebParts> {
    // SAFETY: controller 刚创建成功，以下均为同线程 COM 调用。
    let webview = unsafe { controller.CoreWebView2() }?;
    // SAFETY: 同上。
    unsafe {
        let settings = webview.Settings()?;
        settings.SetAreDevToolsEnabled(false)?;
        settings.SetIsStatusBarEnabled(false)?;
    }
    if let Ok(controller2) = controller.cast::<ICoreWebView2Controller2>() {
        let white = COREWEBVIEW2_COLOR {
            A: 255,
            R: 255,
            G: 255,
            B: 255,
        };
        // SAFETY: 同线程 COM 调用。
        if let Err(e) = unsafe { controller2.SetDefaultBackgroundColor(white) } {
            log::warn!("设置 WebView2 背景色失败：{e}");
        }
    }
    attach_navigation(&webview)?;
    attach_keys(&controller)?;
    attach_process_failed(&webview)?;
    if let Err(e) = attach_context_menu(&webview, &env) {
        log::warn!("Runtime 不支持自定义右键菜单：{e}");
    }
    Ok(WebParts {
        env,
        controller,
        webview,
    })
}

fn read_uri(read: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>) -> Option<String> {
    let mut uri = PWSTR::null();
    match read(&mut uri) {
        Ok(()) if !uri.is_null() => Some(take_pwstr(uri)),
        Ok(()) => None,
        Err(e) => {
            log::warn!("读取导航地址失败：{e}");
            None
        }
    }
}

fn attach_navigation(webview: &ICoreWebView2) -> windows::core::Result<()> {
    let starting = NavigationStartingEventHandler::create(Box::new(|_, args| {
        if let Some(args) = args {
            on_navigation_starting(&args)?;
        }
        Ok(())
    }));
    let new_window = NewWindowRequestedEventHandler::create(Box::new(|sender, args| {
        if let (Some(sender), Some(args)) = (sender, args) {
            on_new_window(&sender, &args)?;
        }
        Ok(())
    }));
    let completed = NavigationCompletedEventHandler::create(Box::new(|_, args| {
        if let Some(args) = args {
            on_navigation_completed(&args)?;
        }
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: 同线程 COM 调用；处理器的生命周期由 WebView 引用计数管理。
    unsafe {
        webview.add_NavigationStarting(&starting, &mut token)?;
        webview.add_NewWindowRequested(&new_window, &mut token)?;
        webview.add_NavigationCompleted(&completed, &mut token)?;
    }
    Ok(())
}

fn on_navigation_starting(
    args: &ICoreWebView2NavigationStartingEventArgs,
) -> windows::core::Result<()> {
    // SAFETY: args 由 WebView 传入，仅在回调期间使用。
    let Some(uri) = read_uri(|p| unsafe { args.Uri(p) }) else {
        return Ok(());
    };
    log::debug!(
        "弹窗导航开始：{}",
        uri.chars().take(200).collect::<String>()
    );
    match classify_navigation(&uri) {
        NavigationDecision::Allow => {
            let mut id = 0u64;
            // SAFETY: 同上，输出参数为局部变量。
            unsafe { args.NavigationId(&mut id) }?;
            let local = !uri.trim_start().to_ascii_lowercase().starts_with("http");
            with_host(|host| host.current_navigation = Some(CurrentNavigation { id, local }));
        }
        NavigationDecision::External => {
            // SAFETY: 同上。
            unsafe { args.SetCancel(true) }?;
            emit(PopupEvent::OpenExternal(uri));
        }
        NavigationDecision::Block => {
            // SAFETY: 同上。
            unsafe { args.SetCancel(true) }?;
            log::warn!("已阻止弹窗内的非网页导航");
        }
    }
    Ok(())
}

fn on_new_window(
    webview: &ICoreWebView2,
    args: &ICoreWebView2NewWindowRequestedEventArgs,
) -> windows::core::Result<()> {
    // SAFETY: args 由 WebView 传入，仅在回调期间使用。
    unsafe { args.SetHandled(true) }?;
    // SAFETY: 同上。
    let Some(uri) = read_uri(|p| unsafe { args.Uri(p) }) else {
        return Ok(());
    };
    match classify_navigation(&uri) {
        // SAFETY: 同线程 COM 调用。
        NavigationDecision::Allow => unsafe { webview.Navigate(&HSTRING::from(uri.as_str())) }?,
        NavigationDecision::External => emit(PopupEvent::OpenExternal(uri)),
        NavigationDecision::Block => log::warn!("已阻止弹窗打开非网页新窗口"),
    }
    Ok(())
}

fn on_navigation_completed(
    args: &ICoreWebView2NavigationCompletedEventArgs,
) -> windows::core::Result<()> {
    let mut success = BOOL(0);
    // SAFETY: args 由 WebView 传入，输出参数为局部变量。
    unsafe { args.IsSuccess(&mut success) }?;
    if success.as_bool() {
        return Ok(());
    }
    let mut id = 0u64;
    // SAFETY: 同上。
    unsafe { args.NavigationId(&mut id) }?;
    let current = with_host(|host| host.current_navigation).flatten();
    // 被新导航打断的旧导航会报 CONNECTION_ABORTED 等状态，只看最新一次
    let Some(current) = current.filter(|c| c.id == id) else {
        return Ok(());
    };
    if current.local {
        log::debug!("弹窗本地页面被打断");
        return Ok(());
    }
    let mut status = COREWEBVIEW2_WEB_ERROR_STATUS(0);
    // SAFETY: 同上。
    unsafe { args.WebErrorStatus(&mut status) }?;
    // 新的搜索打断旧导航、或白名单外链接被取消时都会报这个状态，不算失败
    if status == COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED {
        return Ok(());
    }
    let http = args
        .cast::<ICoreWebView2NavigationCompletedEventArgs2>()
        .ok()
        .and_then(|args2| {
            let mut code = 0i32;
            // SAFETY: 同上。
            unsafe { args2.HttpStatusCode(&mut code) }
                .ok()
                .map(|()| code)
        })
        .filter(|code| *code > 0);
    // 服务器返回了页面（如 429 人机验证）时保留，让用户在弹窗里自己处理
    if let Some(code) = http {
        log::warn!("弹窗页面返回 HTTP {code}，保留服务器页面");
        return Ok(());
    }
    let message = format!("页面加载失败（WebView2 错误码 {}）", status.0);
    log::warn!("{message}");
    if let Some(parts) = web_parts() {
        navigate_error(&parts, &message);
    }
    emit(PopupEvent::LoadFailed(message));
    Ok(())
}

fn attach_keys(controller: &ICoreWebView2Controller) -> windows::core::Result<()> {
    let handler = AcceleratorKeyPressedEventHandler::create(Box::new(|_, args| {
        if let Some(args) = args {
            on_accelerator(&args)?;
        }
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: 同线程 COM 调用。
    unsafe { controller.add_AcceleratorKeyPressed(&handler, &mut token) }
}

fn on_accelerator(args: &ICoreWebView2AcceleratorKeyPressedEventArgs) -> windows::core::Result<()> {
    let mut kind = COREWEBVIEW2_KEY_EVENT_KIND(0);
    let mut key = 0u32;
    // SAFETY: args 由 WebView 传入，输出参数为局部变量。
    unsafe {
        args.KeyEventKind(&mut kind)?;
        args.VirtualKey(&mut key)?;
    }
    if kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN && key == u32::from(VK_ESCAPE.0) {
        // SAFETY: 同上。
        unsafe { args.SetHandled(true) }?;
        if let Some(hwnd) = with_host(|host| host.hwnd).flatten() {
            window::hide(hwnd);
        }
    }
    Ok(())
}

fn attach_process_failed(webview: &ICoreWebView2) -> windows::core::Result<()> {
    let handler = ProcessFailedEventHandler::create(Box::new(|sender, args| {
        if let (Some(sender), Some(args)) = (sender, args) {
            on_process_failed(&sender, &args)?;
        }
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: 同线程 COM 调用。
    unsafe { webview.add_ProcessFailed(&handler, &mut token) }
}

/// 只有浏览器主进程退出才销毁 WebView；渲染进程退出就重新加载，其它进程会自动恢复。
fn on_process_failed(
    webview: &ICoreWebView2,
    args: &ICoreWebView2ProcessFailedEventArgs,
) -> windows::core::Result<()> {
    let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND(0);
    // SAFETY: args 由 WebView 传入，输出参数为局部变量。
    unsafe { args.ProcessFailedKind(&mut kind) }?;
    if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED {
        log::error!("WebView2 浏览器进程异常退出");
        release();
        window::hide_current();
        emit(PopupEvent::LoadFailed(
            "WebView2 进程异常退出，请重新搜索".to_string(),
        ));
    } else if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
        || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE
    {
        log::warn!("WebView2 渲染进程异常（类型 {}），重新加载页面", kind.0);
        // SAFETY: 同线程 COM 调用。
        unsafe { webview.Reload() }?;
    } else {
        log::warn!("WebView2 辅助进程退出（类型 {}），等待自动恢复", kind.0);
    }
    Ok(())
}

fn attach_context_menu(
    webview: &ICoreWebView2,
    env: &ICoreWebView2Environment,
) -> windows::core::Result<()> {
    let webview11 = webview.cast::<ICoreWebView2_11>()?;
    // 处理器里现取，捕获 WebView 会与它互相持有、释放不掉
    env.cast::<ICoreWebView2Environment9>()?;
    let handler = ContextMenuRequestedEventHandler::create(Box::new(|_, args| {
        let env = web_parts().map(|parts| parts.env.cast::<ICoreWebView2Environment9>());
        if let (Some(env), Some(args)) = (env, args) {
            add_menu_items(&env?, &args)?;
        }
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: 同线程 COM 调用。
    unsafe { webview11.add_ContextMenuRequested(&handler, &mut token) }
}

fn current_uri() -> Option<String> {
    let parts = web_parts()?;
    // SAFETY: 同线程 COM 调用。
    read_uri(|p| unsafe { parts.webview.Source(p) })
        .filter(|uri| classify_navigation(uri) == NavigationDecision::Allow)
        .filter(|uri| uri.starts_with("http"))
}

/// 由当前页面地址生成事件。
type MakeEvent = fn(String) -> PopupEvent;

/// 在默认菜单最前面插入「在浏览器中打开」「复制当前链接」和分隔线。
fn add_menu_items(
    env: &ICoreWebView2Environment9,
    args: &ICoreWebView2ContextMenuRequestedEventArgs,
) -> windows::core::Result<()> {
    // SAFETY: args 由 WebView 传入，仅在回调期间使用；其余为同线程 COM 调用。
    let items = unsafe { args.MenuItems() }?;
    let entries: [(PCWSTR, MakeEvent); 2] = [
        (w!("在浏览器中打开"), PopupEvent::OpenExternal),
        (w!("复制当前链接"), PopupEvent::CopyLink),
    ];
    for (index, (label, make)) in (0u32..).zip(entries) {
        // SAFETY: 同上。
        let item = unsafe {
            env.CreateContextMenuItem(label, None, COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND)
        }?;
        let selected = CustomItemSelectedEventHandler::create(Box::new(move |_, _| {
            match current_uri() {
                Some(uri) => emit(make(uri)),
                None => log::info!("当前页面没有可用的链接"),
            }
            Ok(())
        }));
        let mut token = 0i64;
        // SAFETY: 同上。
        unsafe {
            item.add_CustomItemSelected(&selected, &mut token)?;
            items.InsertValueAtIndex(index, &item)?;
        }
    }
    // SAFETY: 同上。
    unsafe {
        let separator =
            env.CreateContextMenuItem(w!(""), None, COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR)?;
        items.InsertValueAtIndex(2, &separator)?;
    }
    Ok(())
}

fn flush_pending(parts: &WebParts) {
    match with_host(|host| host.pending.take()).flatten() {
        Some(Pending::Request(request)) => navigate_post(parts, &request),
        Some(Pending::Error(message)) => navigate_error(parts, &message),
        None => {}
    }
}

/// 主动导航前清掉记录，被自己打断的旧导航就不会被当成失败。
fn forget_current_navigation() {
    with_host(|host| host.current_navigation = None);
}

fn navigate_html(parts: &WebParts, html: &str) {
    forget_current_navigation();
    // SAFETY: 同线程 COM 调用，html 在调用期间有效。
    if let Err(e) = unsafe { parts.webview.NavigateToString(&HSTRING::from(html)) } {
        log::error!("显示本地页面失败：{e}");
    }
}

fn navigate_loading(parts: &WebParts) {
    navigate_html(parts, &loading_page());
}

/// 隐藏时换成加载页：停掉旧页面，下次打开不会先闪出上一次的结果。
pub(crate) fn reset_page() {
    with_host(|host| host.pending = None);
    if let Some(parts) = web_parts() {
        navigate_loading(&parts);
    }
}

pub(crate) fn navigate_error(parts: &WebParts, message: &str) {
    navigate_html(parts, &error_page(message));
}

/// 以 POST 把图片提交给 Lens，结果页由服务器跳转后在弹窗内加载。
pub(crate) fn navigate_post(parts: &WebParts, request: &LensRequest) {
    if let Err(e) = try_navigate_post(parts, request) {
        let message = format!("提交图片失败：{e}");
        log::error!("{message}");
        navigate_error(parts, &message);
        emit(PopupEvent::LoadFailed(message));
    }
}

fn try_navigate_post(parts: &WebParts, request: &LensRequest) -> windows::core::Result<()> {
    forget_current_navigation();
    let env2 = parts.env.cast::<ICoreWebView2Environment2>()?;
    let webview2 = parts.webview.cast::<ICoreWebView2_2>()?;
    // SAFETY: 请求体在调用期间有效，SHCreateMemStream 会复制一份。
    let stream = unsafe { SHCreateMemStream(Some(&request.body)) }
        .ok_or_else(windows::core::Error::from_thread)?;
    let headers = HSTRING::from(format!("Content-Type: {}", request.content_type));
    // SAFETY: 同线程 COM 调用，参数在调用期间有效。
    let web_request = unsafe {
        env2.CreateWebResourceRequest(
            &HSTRING::from(request.url.as_str()),
            w!("POST"),
            &stream,
            &headers,
        )
    }?;
    // SAFETY: 同上。
    unsafe { webview2.NavigateWithWebResourceRequest(&web_request) }
}

pub(crate) fn fit_to_window(hwnd: HWND) {
    let Some(parts) = web_parts() else {
        return;
    };
    let mut rect = RECT::default();
    // SAFETY: hwnd 为弹窗窗口，输出参数为局部变量。
    if let Err(e) = unsafe { GetClientRect(hwnd, &mut rect) } {
        log::warn!("读取弹窗客户区失败：{e}");
        return;
    }
    // SAFETY: 同线程 COM 调用。
    if let Err(e) = unsafe { parts.controller.SetBounds(rect) } {
        log::warn!("调整 WebView 尺寸失败：{e}");
    }
}

/// 窗口移动后通知 WebView 更新弹出层（如下拉框）位置。
pub(crate) fn notify_moved() {
    if let Some(parts) = web_parts() {
        // SAFETY: 同线程 COM 调用。
        if let Err(e) = unsafe { parts.controller.NotifyParentWindowPositionChanged() } {
            log::warn!("通知 WebView 窗口移动失败：{e}");
        }
    }
}

pub(crate) fn focus() {
    if let Some(parts) = web_parts() {
        let reason = COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC;
        // SAFETY: 同线程 COM 调用。
        if let Err(e) = unsafe { parts.controller.MoveFocus(reason) } {
            log::warn!("WebView 获取焦点失败：{e}");
        }
    }
}

/// 隐藏时让 WebView 降低资源占用。
pub(crate) fn set_visible(visible: bool) {
    if let Some(parts) = web_parts() {
        // SAFETY: 同线程 COM 调用。
        if let Err(e) = unsafe { parts.controller.SetIsVisible(visible) } {
            log::warn!("切换 WebView 可见性失败：{e}");
        }
    }
}

/// 关闭并释放 WebView；之后的异步回调因代数变化被丢弃。
pub(crate) fn release() {
    let taken = with_host(|host| {
        host.generation = host.generation.wrapping_add(1);
        host.pending = None;
        host.current_navigation = None;
        std::mem::replace(&mut host.web, Web::Absent)
    });
    if let Some(Web::Ready(parts)) = taken {
        // SAFETY: 控件不再被状态引用，关闭一次后随引用计数释放。
        if let Err(e) = unsafe { parts.controller.Close() } {
            log::warn!("关闭 WebView2 控件失败：{e}");
        }
    }
}
