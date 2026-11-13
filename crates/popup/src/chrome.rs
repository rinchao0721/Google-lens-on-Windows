//! 弹窗顶部的自绘把手栏：布局、命中测试、主题与 Direct2D 绘制。

use std::cell::RefCell;

use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_COLOR;
use windows::Win32::Foundation::{
    D2DERR_RECREATE_TARGET, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D_SIZE_U, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_ROUNDED_RECT, D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget,
    ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection,
    IDWriteTextFormat,
};
use windows::Win32::Graphics::Dwm::{
    DWM_WINDOW_CORNER_PREFERENCE, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{InvalidateRect, ScreenToClient, ValidateRect};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetClientRect, HTCAPTION, HTCLIENT, HTTOP, HTTOPLEFT, HTTOPRIGHT, IsZoomed,
    NCCALCSIZE_PARAMS, SM_CXPADDEDBORDER, SM_CXSIZEFRAME, WM_NCCALCSIZE,
};
use windows::core::{BOOL, PCWSTR, w};

use crate::error::{PopupError, win32};
use crate::web;

const BASE_DPI: u32 = 96;
/// 把手栏高度（逻辑像素，下同）。
const BAR_HEIGHT: i32 = 32;
const HANDLE_WIDTH: i32 = 36;
const HANDLE_HEIGHT: i32 = 4;
/// 把手栏顶部可拖动缩放的高度。
const RESIZE_TOP: i32 = 4;
/// 顶部两角用于斜向缩放的宽度。
const RESIZE_CORNER: i32 = 8;
const TITLE_LEFT: i32 = 14;
const TITLE_GAP: i32 = 8;
const TITLE_SIZE: i32 = 12;
const BUTTON_WIDTH: i32 = 36;
const BUTTON_HEIGHT: i32 = 28;
const BUTTON_MARGIN: i32 = 4;
const BUTTON_RADIUS: i32 = 12;
const ICON_HALF: i32 = 5;
/// 关闭图标线宽，实际绘制取其一半。
const ICON_STROKE: i32 = 3;
const THEME_NOTIFICATION: &str = "ImmersiveColorSet";
const TITLE_TEXT: &str = "Google 智能镜头";
/// 首选字体；系统没有时退回 `FALLBACK_FAMILY`。
const PREFERRED_FAMILY: PCWSTR = w!("Segoe UI Variable Text");
const FALLBACK_FAMILY: PCWSTR = w!("Segoe UI");

/// 系统深浅色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Theme {
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rgb(u8, u8, u8);

impl Rgb {
    fn d2d(self) -> D2D1_COLOR_F {
        D2D1_COLOR_F {
            r: f32::from(self.0) / 255.0,
            g: f32::from(self.1) / 255.0,
            b: f32::from(self.2) / 255.0,
            a: 1.0,
        }
    }
}

struct Palette {
    bar: Rgb,
    handle: Rgb,
    text: Rgb,
    hover: Rgb,
    page: Rgb,
}

impl Theme {
    fn palette(self) -> Palette {
        match self {
            Self::Dark => Palette {
                bar: Rgb(0x28, 0x29, 0x2C),
                handle: Rgb(0x80, 0x86, 0x8B),
                text: Rgb(0xBD, 0xC1, 0xC6),
                hover: Rgb(0x3C, 0x40, 0x43),
                page: Rgb(0x20, 0x21, 0x24),
            },
            Self::Light => Palette {
                bar: Rgb(0xF1, 0xF3, 0xF4),
                handle: Rgb(0xDA, 0xDC, 0xE0),
                text: Rgb(0x5F, 0x63, 0x68),
                hover: Rgb(0xE8, 0xEA, 0xED),
                page: Rgb(0xFF, 0xFF, 0xFF),
            },
        }
    }

    /// WebView 页面尚未绘制时的默认底色。
    pub(crate) fn webview_background(self) -> COREWEBVIEW2_COLOR {
        let Rgb(r, g, b) = self.palette().page;
        COREWEBVIEW2_COLOR {
            A: 255,
            R: r,
            G: g,
            B: b,
        }
    }
}

/// 关闭按钮的交互状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseButton {
    Idle,
    Hover,
    Pressed,
}

/// 是否已登记 `WM_MOUSELEAVE` 跟踪。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tracking {
    Off,
    On,
}

/// 左键松开的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Click {
    None,
    Close,
}

/// 客户区内一点落在把手栏的哪个部位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Top,
    TopLeft,
    TopRight,
    Close,
    Caption,
    Other,
}

/// 像素矩形，右、下边界不含。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Span {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    fn center(&self) -> (i32, i32) {
        (
            self.left.saturating_add(self.right) / 2,
            self.top.saturating_add(self.bottom) / 2,
        )
    }

    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F {
            left: px(self.left),
            top: px(self.top),
            right: px(self.right),
            bottom: px(self.bottom),
        }
    }
}

/// 把手栏各元素的位置（物理像素）。
struct Geometry {
    bar: Span,
    handle: Span,
    close: Span,
    title: Span,
}

/// 窗口坐标转 `f32`。窗口尺寸远小于 `i16` 范围，超出的值饱和到边界即可。
fn px(value: i32) -> f32 {
    let clamped = value.clamp(i32::from(i16::MIN), i32::from(i16::MAX));
    f32::from(i16::try_from(clamped).unwrap_or(0))
}

fn scaled(logical: i32, dpi: u32) -> i32 {
    let dpi = if dpi == 0 { BASE_DPI } else { dpi };
    let value = i64::from(logical).saturating_mul(i64::from(dpi)) / i64::from(BASE_DPI);
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn bar_height(dpi: u32) -> i32 {
    scaled(BAR_HEIGHT, dpi)
}

fn close_rect(width: i32, dpi: u32) -> Span {
    let height = scaled(BUTTON_HEIGHT, dpi);
    let right = width.saturating_sub(scaled(BUTTON_MARGIN, dpi));
    let top = bar_height(dpi).saturating_sub(height) / 2;
    Span {
        left: right.saturating_sub(scaled(BUTTON_WIDTH, dpi)),
        top,
        right,
        bottom: top.saturating_add(height),
    }
}

fn geometry(width: i32, dpi: u32) -> Geometry {
    let bar_h = bar_height(dpi);
    let handle_w = scaled(HANDLE_WIDTH, dpi);
    let handle_h = scaled(HANDLE_HEIGHT, dpi);
    let handle_left = width.saturating_sub(handle_w) / 2;
    let handle_top = bar_h.saturating_sub(handle_h) / 2;
    let title_left = scaled(TITLE_LEFT, dpi);
    Geometry {
        bar: Span {
            left: 0,
            top: 0,
            right: width,
            bottom: bar_h,
        },
        handle: Span {
            left: handle_left,
            top: handle_top,
            right: handle_left.saturating_add(handle_w),
            bottom: handle_top.saturating_add(handle_h),
        },
        close: close_rect(width, dpi),
        title: Span {
            left: title_left,
            top: 0,
            right: handle_left
                .saturating_sub(scaled(TITLE_GAP, dpi))
                .max(title_left),
            bottom: bar_h,
        },
    }
}

fn hit_test(x: i32, y: i32, width: i32, dpi: u32) -> Hit {
    if x < 0 || x >= width || y < 0 || y >= bar_height(dpi) {
        return Hit::Other;
    }
    if y < scaled(RESIZE_TOP, dpi) {
        let corner = scaled(RESIZE_CORNER, dpi);
        return if x < corner {
            Hit::TopLeft
        } else if x >= width.saturating_sub(corner) {
            Hit::TopRight
        } else {
            Hit::Top
        };
    }
    if close_rect(width, dpi).contains(x, y) {
        Hit::Close
    } else {
        Hit::Caption
    }
}

/// 从 `lParam` 取低、高 16 位并按有符号数解释（多显示器下坐标可为负）。
fn signed_words(lparam: LPARAM) -> (i32, i32) {
    let word = |shift: u32| {
        let raw = u16::try_from((lparam.0 >> shift) & 0xFFFF).unwrap_or(0);
        i32::from(i16::from_ne_bytes(raw.to_ne_bytes()))
    };
    (word(0), word(16))
}

fn theme_from_dword(value: u32) -> Theme {
    if value == 0 {
        Theme::Dark
    } else {
        Theme::Light
    }
}

/// 读取系统应用主题；读失败按浅色。
fn read_theme() -> Theme {
    let mut data = 0u32;
    let mut size = u32::try_from(std::mem::size_of::<u32>()).unwrap_or(4);
    // SAFETY: 键、子键与值名为静态字符串；data 与 size 为有效局部变量，缓冲区大小与 DWORD 一致。
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut data).cast()),
            Some(&raw mut size),
        )
    };
    match status.ok() {
        Ok(()) => theme_from_dword(data),
        Err(e) => {
            log::warn!("读取系统深浅色失败，按浅色处理：{e}");
            Theme::Light
        }
    }
}

struct TitleFormat {
    dpi: u32,
    format: IDWriteTextFormat,
}

struct Target {
    render: ID2D1HwndRenderTarget,
    brush: ID2D1SolidColorBrush,
}

/// Direct2D 与 DirectWrite 资源；渲染目标失效时单独重建。
struct Gfx {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    family: PCWSTR,
    title: Option<TitleFormat>,
    target: Option<Target>,
}

struct State {
    theme: Theme,
    button: CloseButton,
    tracking: Tracking,
    gfx: Option<Gfx>,
}

thread_local! {
    static STATE: RefCell<State> = const {
        RefCell::new(State {
            theme: Theme::Light,
            button: CloseButton::Idle,
            tracking: Tracking::Off,
            gfx: None,
        })
    };
}

/// 在状态上执行操作；正被借用（重入）时返回 `None`。
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE
        .try_with(|cell| cell.try_borrow_mut().ok().map(|mut s| f(&mut s)))
        .ok()
        .flatten()
}

/// 当前主题。
pub(crate) fn theme() -> Theme {
    with_state(|s| s.theme).unwrap_or(Theme::Light)
}

fn dpi_of(hwnd: HWND) -> u32 {
    // SAFETY: hwnd 由弹窗模块创建；失败时返回 0，按 96 处理。
    match unsafe { GetDpiForWindow(hwnd) } {
        0 => BASE_DPI,
        dpi => dpi,
    }
}

/// 当前 DPI 下把手栏的物理像素高度。
pub(crate) fn bar_height_of(hwnd: HWND) -> i32 {
    bar_height(dpi_of(hwnd))
}

fn client_size(hwnd: HWND) -> Option<(i32, i32)> {
    let mut rect = RECT::default();
    // SAFETY: hwnd 为弹窗窗口，输出参数为局部变量。
    match unsafe { GetClientRect(hwnd, &mut rect) } {
        Ok(()) => Some((
            rect.right.saturating_sub(rect.left),
            rect.bottom.saturating_sub(rect.top),
        )),
        Err(e) => {
            log::warn!("读取弹窗客户区失败：{e}");
            None
        }
    }
}

fn invalidate(hwnd: HWND) {
    // SAFETY: hwnd 有效；空矩形表示整个客户区。
    if !unsafe { InvalidateRect(Some(hwnd), None, false) }.as_bool() {
        log::warn!("请求重绘把手栏失败");
    }
}

fn validate(hwnd: HWND) {
    // SAFETY: hwnd 有效；空矩形表示整个客户区。
    if !unsafe { ValidateRect(Some(hwnd), None) }.as_bool() {
        log::warn!("标记把手栏已绘制失败");
    }
}

/// 窗口创建后调用：设置系统圆角，并按当前系统主题初始化。
pub(crate) fn init(hwnd: HWND) {
    let corner = DWMWCP_ROUND;
    // SAFETY: corner 为有效局部变量，长度与该属性的枚举类型一致。
    let applied = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const corner).cast(),
            u32::try_from(std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>()).unwrap_or(0),
        )
    };
    if let Err(e) = applied {
        log::warn!("设置弹窗圆角失败，退回直角：{e}");
    }
    refresh_theme(hwnd);
}

fn apply_dark_mode(hwnd: HWND, theme: Theme) {
    let dark = BOOL::from(theme == Theme::Dark);
    // SAFETY: dark 为有效局部变量，长度与 BOOL 一致。
    let applied = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const dark).cast(),
            u32::try_from(std::mem::size_of::<BOOL>()).unwrap_or(0),
        )
    };
    if let Err(e) = applied {
        log::warn!("设置窗口深色模式失败：{e}");
    }
}

fn refresh_theme(hwnd: HWND) {
    let theme = read_theme();
    with_state(|s| s.theme = theme);
    apply_dark_mode(hwnd, theme);
    web::apply_theme();
    invalidate(hwnd);
}

/// `WM_SETTINGCHANGE`：系统深浅色变化时重读并刷新。
pub(crate) fn on_setting_change(hwnd: HWND, lparam: LPARAM) {
    if lparam.0 == 0 {
        return;
    }
    let text = lparam.0 as *const u16;
    // SAFETY: WM_SETTINGCHANGE 的 lParam 非空时指向系统提供的、以 NUL 结尾的宽字符串，仅在本消息期间有效。
    let name = unsafe { PCWSTR(text).to_string() };
    if name.is_ok_and(|n| n == THEME_NOTIFICATION) {
        refresh_theme(hwnd);
    }
}

/// `WM_NCCALCSIZE`：只去掉顶部标题栏，其余边框保留以便拖动缩放。
pub(crate) fn on_nccalcsize(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    if wparam.0 == 0 {
        return None;
    }
    let params = lparam.0 as *mut NCCALCSIZE_PARAMS;
    // SAFETY: wParam 为 TRUE 时 lParam 指向系统提供的有效 NCCALCSIZE_PARAMS，仅在本消息期间有效。
    let original_top = unsafe { params.as_ref() }?.rgrc.first()?.top;
    // SAFETY: 参数原样交给默认窗口过程。
    let result = unsafe { DefWindowProcW(hwnd, WM_NCCALCSIZE, wparam, lparam) };
    // 最大化时窗口比屏幕大出边框厚度，顶部要让出这部分
    // SAFETY: hwnd 有效。
    let inset = if unsafe { IsZoomed(hwnd) }.as_bool() {
        frame_thickness(dpi_of(hwnd))
    } else {
        0
    };
    // SAFETY: 同上，默认窗口过程返回后指针仍有效。
    let top = unsafe { params.as_mut() }?.rgrc.first_mut()?;
    top.top = original_top.saturating_add(inset);
    Some(result)
}

fn frame_thickness(dpi: u32) -> i32 {
    // SAFETY: 纯查询，无前置条件。
    let (frame, padded) = unsafe {
        (
            GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi),
            GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi),
        )
    };
    frame.saturating_add(padded)
}

/// `WM_NCHITTEST`：把手栏内的命中结果；不在把手栏内返回 `None`，交给默认处理。
pub(crate) fn on_nchittest(hwnd: HWND, lparam: LPARAM) -> Option<LRESULT> {
    let (x, y) = signed_words(lparam);
    let mut point = POINT { x, y };
    // SAFETY: hwnd 有效，point 为局部变量。
    if !unsafe { ScreenToClient(hwnd, &mut point) }.as_bool() {
        return None;
    }
    let width = client_size(hwnd)?.0;
    let code = match hit_test(point.x, point.y, width, dpi_of(hwnd)) {
        Hit::Top => HTTOP,
        Hit::TopLeft => HTTOPLEFT,
        Hit::TopRight => HTTOPRIGHT,
        Hit::Close => HTCLIENT,
        Hit::Caption => HTCAPTION,
        Hit::Other => return None,
    };
    Some(LRESULT(isize::try_from(code).unwrap_or(0)))
}

fn over_close(hwnd: HWND, lparam: LPARAM) -> bool {
    let (x, y) = signed_words(lparam);
    client_size(hwnd).is_some_and(|(width, _)| close_rect(width, dpi_of(hwnd)).contains(x, y))
}

fn set_button(hwnd: HWND, button: CloseButton) {
    let changed = with_state(|s| {
        let changed = s.button != button;
        s.button = button;
        changed
    })
    .unwrap_or(false);
    if changed {
        invalidate(hwnd);
    }
}

fn current_button() -> CloseButton {
    with_state(|s| s.button).unwrap_or(CloseButton::Idle)
}

fn track_leave(hwnd: HWND) {
    if with_state(|s| s.tracking) != Some(Tracking::Off) {
        return;
    }
    let mut event = TRACKMOUSEEVENT {
        cbSize: u32::try_from(std::mem::size_of::<TRACKMOUSEEVENT>()).unwrap_or(0),
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    // SAFETY: event 已完整填写，hwnd 有效。
    match unsafe { TrackMouseEvent(&mut event) } {
        Ok(()) => {
            with_state(|s| s.tracking = Tracking::On);
        }
        Err(e) => log::warn!("登记鼠标离开通知失败：{e}"),
    }
}

/// `WM_MOUSEMOVE`：更新关闭按钮的悬停状态。
pub(crate) fn mouse_move(hwnd: HWND, lparam: LPARAM) {
    track_leave(hwnd);
    let inside = over_close(hwnd, lparam);
    let next = match (current_button(), inside) {
        (CloseButton::Pressed, _) => CloseButton::Pressed,
        (_, true) => CloseButton::Hover,
        (_, false) => CloseButton::Idle,
    };
    set_button(hwnd, next);
}

/// `WM_MOUSELEAVE`：清除悬停；按下期间由鼠标捕获接管，不清除。
pub(crate) fn mouse_leave(hwnd: HWND) {
    with_state(|s| s.tracking = Tracking::Off);
    if current_button() != CloseButton::Pressed {
        set_button(hwnd, CloseButton::Idle);
    }
}

/// `WM_LBUTTONDOWN`：在关闭按钮上按下时开始捕获鼠标。
pub(crate) fn button_down(hwnd: HWND, lparam: LPARAM) {
    if !over_close(hwnd, lparam) {
        return;
    }
    // 返回值是之前持有捕获的窗口，无需处理
    // SAFETY: hwnd 为弹窗窗口，在本线程的消息处理中调用。
    let _previous = unsafe { SetCapture(hwnd) };
    set_button(hwnd, CloseButton::Pressed);
}

/// `WM_LBUTTONUP`：在关闭按钮内松开才算点击。
pub(crate) fn button_up(hwnd: HWND, lparam: LPARAM) -> Click {
    if current_button() != CloseButton::Pressed {
        return Click::None;
    }
    // SAFETY: 本窗口持有捕获，在本线程调用。
    if let Err(e) = unsafe { ReleaseCapture() } {
        log::warn!("释放鼠标捕获失败：{e}");
    }
    let clicked = over_close(hwnd, lparam);
    // 点击后窗口随即隐藏，不保留悬停底
    set_button(hwnd, CloseButton::Idle);
    if clicked { Click::Close } else { Click::None }
}

/// `WM_CAPTURECHANGED`：捕获被抢走时放弃按下状态。
pub(crate) fn capture_lost(hwnd: HWND) {
    if current_button() == CloseButton::Pressed {
        set_button(hwnd, CloseButton::Idle);
    }
}

/// 弹窗隐藏时清掉交互状态，下次显示不残留悬停底。
pub(crate) fn reset() {
    with_state(|s| {
        s.button = CloseButton::Idle;
        s.tracking = Tracking::Off;
    });
}

/// 窗口销毁时释放全部绘制资源。
pub(crate) fn release() {
    with_state(|s| s.gfx = None);
}

/// DPI 变化：丢弃文字格式（下次绘制按新 DPI 重建）并重绘。
pub(crate) fn dpi_changed(hwnd: HWND) {
    with_state(|s| {
        if let Some(gfx) = s.gfx.as_mut() {
            gfx.title = None;
        }
    });
    web::fit_to_window(hwnd);
    invalidate(hwnd);
}

/// 客户区尺寸变化：调整渲染目标并重绘。
pub(crate) fn resize(hwnd: HWND) {
    let size =
        client_size(hwnd).and_then(|(w, h)| Some((u32::try_from(w).ok()?, u32::try_from(h).ok()?)));
    if let Some((width, height)) = size.filter(|(w, h)| *w > 0 && *h > 0) {
        with_state(|s| {
            let Some(gfx) = s.gfx.as_mut() else {
                return;
            };
            let Some(target) = gfx.target.as_ref() else {
                return;
            };
            // SAFETY: 渲染目标有效，尺寸为局部变量。
            if let Err(e) = unsafe { target.render.Resize(&D2D_SIZE_U { width, height }) } {
                log::warn!("调整把手栏渲染目标失败，稍后重建：{e}");
                gfx.target = None;
            }
        });
    }
    invalidate(hwnd);
}

/// `WM_PAINT`：绘制把手栏；失败只写日志，不让窗口陷入重绘循环。
pub(crate) fn paint(hwnd: HWND) {
    match with_state(|s| s.draw(hwnd)) {
        Some(Ok(Drawn::Done)) | None => {}
        Some(Ok(Drawn::RecreateTarget)) => {
            with_state(|s| {
                if let Some(gfx) = s.gfx.as_mut() {
                    gfx.target = None;
                }
            });
            invalidate(hwnd);
        }
        Some(Err(e)) => log::warn!("绘制把手栏失败：{e}"),
    }
    validate(hwnd);
}

enum Drawn {
    Done,
    RecreateTarget,
}

impl State {
    fn draw(&mut self, hwnd: HWND) -> Result<Drawn, PopupError> {
        let Some((width, height)) = client_size(hwnd).filter(|(w, h)| *w > 0 && *h > 0) else {
            return Ok(Drawn::Done);
        };
        let size = (
            u32::try_from(width).unwrap_or(1),
            u32::try_from(height).unwrap_or(1),
        );
        if self.gfx.is_none() {
            self.gfx = Some(Gfx::new()?);
        }
        let Some(gfx) = self.gfx.as_mut() else {
            return Ok(Drawn::Done);
        };
        let dpi = dpi_of(hwnd);
        gfx.ensure_target(hwnd, size)?;
        gfx.ensure_title(dpi)?;
        gfx.draw(
            &geometry(width, dpi),
            dpi,
            self.theme.palette(),
            self.button,
        )
    }
}

impl Gfx {
    fn new() -> Result<Self, PopupError> {
        // SAFETY: 参数为合法的枚举值，无选项。
        let factory =
            unsafe { D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
                .map_err(win32("D2D1CreateFactory"))?;
        // SAFETY: 参数为合法的枚举值。
        let dwrite = unsafe { DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) }
            .map_err(win32("DWriteCreateFactory"))?;
        let family = pick_family(&dwrite);
        Ok(Self {
            factory,
            dwrite,
            family,
            title: None,
            target: None,
        })
    }

    fn ensure_target(&mut self, hwnd: HWND, size: (u32, u32)) -> Result<(), PopupError> {
        if self.target.is_some() {
            return Ok(());
        }
        // 渲染目标 DPI 固定为 96，使 1 个 DIP 等于 1 个物理像素
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: D2D_SIZE_U {
                width: size.0,
                height: size.1,
            },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        // SAFETY: hwnd 是本线程创建的有效窗口，两个属性结构均已完整填写。
        let render = unsafe { self.factory.CreateHwndRenderTarget(&props, &hwnd_props) }
            .map_err(win32("CreateHwndRenderTarget"))?;
        let initial = Theme::Light.palette().bar.d2d();
        // SAFETY: 颜色为有效局部变量，目标刚创建。
        let brush = unsafe { render.CreateSolidColorBrush(&initial, None) }
            .map_err(win32("CreateSolidColorBrush"))?;
        self.target = Some(Target { render, brush });
        Ok(())
    }

    fn ensure_title(&mut self, dpi: u32) -> Result<(), PopupError> {
        if self.title.as_ref().is_some_and(|t| t.dpi == dpi) {
            return Ok(());
        }
        let size = px(scaled(TITLE_SIZE, dpi));
        // SAFETY: 字体名与区域名为静态字符串；字体不存在时 DirectWrite 自动回退。
        let format = unsafe {
            self.dwrite.CreateTextFormat(
                self.family,
                None::<&IDWriteFontCollection>,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("zh-CN"),
            )
        }
        .map_err(win32("CreateTextFormat"))?;
        // SAFETY: format 刚创建，参数为合法枚举值。
        unsafe { format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING) }
            .map_err(win32("SetTextAlignment"))?;
        // SAFETY: 同上。
        unsafe { format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER) }
            .map_err(win32("SetParagraphAlignment"))?;
        self.title = Some(TitleFormat { dpi, format });
        Ok(())
    }

    fn draw(
        &self,
        geo: &Geometry,
        dpi: u32,
        palette: Palette,
        button: CloseButton,
    ) -> Result<Drawn, PopupError> {
        let (Some(target), Some(title)) = (self.target.as_ref(), self.title.as_ref()) else {
            return Ok(Drawn::Done);
        };
        // SAFETY: BeginDraw 与下方 EndDraw 成对调用，期间只使用本目标创建的资源。
        unsafe {
            target.render.BeginDraw();
            target.render.Clear(Some(&palette.page.d2d()));
        }
        target.fill_rect(&geo.bar, palette.bar);
        target.fill_round(&geo.handle, scaled(HANDLE_HEIGHT, dpi) / 2, palette.handle);
        self.draw_close(target, geo, dpi, &palette, button);
        let text: Vec<u16> = TITLE_TEXT.encode_utf16().collect();
        target.brush_color(palette.text);
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，文本、格式与画刷有效。
        unsafe {
            target.render.DrawText(
                &text,
                &title.format,
                &geo.title.d2d(),
                &target.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        // SAFETY: 与上面的 BeginDraw 配对。
        match unsafe { target.render.EndDraw(None, None) } {
            Ok(()) => Ok(Drawn::Done),
            Err(e) if e.code() == D2DERR_RECREATE_TARGET => Ok(Drawn::RecreateTarget),
            Err(source) => Err(PopupError::Win32 {
                api: "EndDraw",
                source,
            }),
        }
    }

    fn draw_close(
        &self,
        target: &Target,
        geo: &Geometry,
        dpi: u32,
        palette: &Palette,
        button: CloseButton,
    ) {
        let (cx, cy) = geo.close.center();
        if button != CloseButton::Idle {
            target.brush_color(palette.hover);
            let circle = ellipse(px(cx), px(cy), px(scaled(BUTTON_RADIUS, dpi)));
            // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，椭圆与画刷有效。
            unsafe { target.render.FillEllipse(&circle, &target.brush) };
        }
        target.brush_color(palette.text);
        let half = scaled(ICON_HALF, dpi);
        let width = (px(scaled(ICON_STROKE, dpi)) / 2.0).max(1.0);
        let corners = [
            (
                (cx.saturating_sub(half), cy.saturating_sub(half)),
                (cx.saturating_add(half), cy.saturating_add(half)),
            ),
            (
                (cx.saturating_add(half), cy.saturating_sub(half)),
                (cx.saturating_sub(half), cy.saturating_add(half)),
            ),
        ];
        for ((x0, y0), (x1, y1)) in corners {
            // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷有效。
            unsafe {
                target.render.DrawLine(
                    ellipse(px(x0), px(y0), 0.0).point,
                    ellipse(px(x1), px(y1), 0.0).point,
                    &target.brush,
                    width,
                    None,
                );
            }
        }
    }
}

impl Target {
    fn brush_color(&self, color: Rgb) {
        // SAFETY: 画刷由本目标创建，颜色为有效局部变量。
        unsafe { self.brush.SetColor(&color.d2d()) };
    }

    /// 只能在 BeginDraw 与 EndDraw 之间调用。
    fn fill_rect(&self, span: &Span, color: Rgb) {
        self.brush_color(color);
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，矩形与画刷有效。
        unsafe { self.render.FillRectangle(&span.d2d(), &self.brush) };
    }

    /// 只能在 BeginDraw 与 EndDraw 之间调用。
    fn fill_round(&self, span: &Span, radius: i32, color: Rgb) {
        self.brush_color(color);
        let rounded = D2D1_ROUNDED_RECT {
            rect: span.d2d(),
            radiusX: px(radius),
            radiusY: px(radius),
        };
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，圆角矩形与画刷有效。
        unsafe { self.render.FillRoundedRectangle(&rounded, &self.brush) };
    }
}

fn ellipse(x: f32, y: f32, radius: f32) -> D2D1_ELLIPSE {
    let mut holder = D2D1_ELLIPSE::default();
    holder.point.X = x;
    holder.point.Y = y;
    holder.radiusX = radius;
    holder.radiusY = radius;
    holder
}

/// 首选字体存在就用它，否则用 Segoe UI。
fn pick_family(dwrite: &IDWriteFactory) -> PCWSTR {
    let mut collection: Option<IDWriteFontCollection> = None;
    // SAFETY: collection 是有效的输出位置。
    if unsafe { dwrite.GetSystemFontCollection(&mut collection, false) }.is_err() {
        return FALLBACK_FAMILY;
    }
    let Some(collection) = collection else {
        return FALLBACK_FAMILY;
    };
    let mut index = 0u32;
    let mut exists = BOOL::from(false);
    // SAFETY: 字体名为静态字符串，两个输出参数指向有效局部变量。
    let found = unsafe { collection.FindFamilyName(PREFERRED_FAMILY, &mut index, &mut exists) };
    if found.is_ok() && exists.as_bool() {
        PREFERRED_FAMILY
    } else {
        FALLBACK_FAMILY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: i32 = 460;

    #[test]
    fn scaled_follows_dpi() {
        assert_eq!(scaled(32, 96), 32);
        assert_eq!(scaled(32, 144), 48);
        assert_eq!(scaled(32, 0), 32);
        assert_eq!(scaled(0, 192), 0);
    }

    #[test]
    fn hit_test_top_edge_and_corners() {
        assert_eq!(hit_test(WIDTH / 2, 0, WIDTH, 96), Hit::Top);
        assert_eq!(hit_test(0, 0, WIDTH, 96), Hit::TopLeft);
        assert_eq!(hit_test(WIDTH - 1, 3, WIDTH, 96), Hit::TopRight);
        assert_eq!(hit_test(WIDTH / 2, 4, WIDTH, 96), Hit::Caption);
    }

    #[test]
    fn hit_test_close_button_and_caption() {
        let close = close_rect(WIDTH, 96);
        let (cx, cy) = close.center();
        assert_eq!(hit_test(cx, cy, WIDTH, 96), Hit::Close);
        assert_eq!(hit_test(close.left - 1, cy, WIDTH, 96), Hit::Caption);
        assert_eq!(hit_test(20, 16, WIDTH, 96), Hit::Caption);
    }

    #[test]
    fn hit_test_outside_bar_is_other() {
        assert_eq!(hit_test(10, BAR_HEIGHT, WIDTH, 96), Hit::Other);
        assert_eq!(hit_test(-1, 10, WIDTH, 96), Hit::Other);
        assert_eq!(hit_test(WIDTH, 10, WIDTH, 96), Hit::Other);
        assert_eq!(hit_test(10, -1, WIDTH, 96), Hit::Other);
    }

    #[test]
    fn hit_test_scales_with_dpi() {
        let close = close_rect(WIDTH, 192);
        let (cx, cy) = close.center();
        assert_eq!(hit_test(cx, cy, WIDTH, 192), Hit::Close);
        assert_eq!(hit_test(WIDTH / 2, 7, WIDTH, 192), Hit::Top);
        assert_eq!(hit_test(WIDTH / 2, 8, WIDTH, 192), Hit::Caption);
    }

    #[test]
    fn geometry_handle_is_centered_and_title_clear_of_it() {
        let geo = geometry(WIDTH, 96);
        assert_eq!(geo.handle.right - geo.handle.left, HANDLE_WIDTH);
        assert_eq!(geo.handle.bottom - geo.handle.top, HANDLE_HEIGHT);
        assert_eq!(geo.handle.left + geo.handle.right, WIDTH);
        assert_eq!(geo.handle.top + geo.handle.bottom, BAR_HEIGHT);
        assert!(geo.title.right <= geo.handle.left);
        assert!(geo.close.right <= WIDTH);
        assert!(geo.close.bottom <= BAR_HEIGHT);
    }

    #[test]
    fn geometry_survives_tiny_width() {
        let geo = geometry(10, 96);
        assert!(geo.title.right >= geo.title.left);
    }

    #[test]
    fn signed_words_handles_negative_coordinates() {
        let packed = (-5i32 & 0xFFFF) | ((-7i32 & 0xFFFF) << 16);
        let lparam = LPARAM(isize::try_from(packed).unwrap_or(0));
        assert_eq!(signed_words(lparam), (-5, -7));
        assert_eq!(signed_words(LPARAM(0x0020_0010)), (16, 32));
    }

    #[test]
    fn theme_from_registry_value() {
        assert_eq!(theme_from_dword(0), Theme::Dark);
        assert_eq!(theme_from_dword(1), Theme::Light);
        assert_eq!(theme_from_dword(7), Theme::Light);
    }

    #[test]
    fn palettes_match_spec() {
        let dark = Theme::Dark.palette();
        assert_eq!(dark.bar, Rgb(0x28, 0x29, 0x2C));
        assert_eq!(dark.page, Rgb(0x20, 0x21, 0x24));
        let light = Theme::Light.palette();
        assert_eq!(light.bar, Rgb(0xF1, 0xF3, 0xF4));
        assert_eq!(light.page, Rgb(0xFF, 0xFF, 0xFF));
        let bg = Theme::Dark.webview_background();
        assert_eq!((bg.A, bg.R, bg.G, bg.B), (255, 0x20, 0x21, 0x24));
    }
}
