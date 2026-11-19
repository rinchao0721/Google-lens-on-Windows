//! 长截图覆盖层会话：4 条严格在选区外侧的点穿边框窗口 + 控制面板窗口。
//!
//! 本模块只负责 UI 展示、预览渲染与用户操作回投，不调用任何截屏或滚动输入 API。

use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;

use lenswin_core::frame::CroppedImage;
use lenswin_core::geometry::{Point, Rect};
use lenswin_core::lens::{downscale, popup_placement};
use lenswin_core::snip::{ScrollEvent, ToolbarAction};
use lenswin_core::stitch::ScrollAxis;
use windows::Win32::Foundation::{
    COLORREF, D2DERR_RECREATE_TARGET, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, D2D1_DRAW_TEXT_OPTIONS_CLIP,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    ID2D1Bitmap, ID2D1HwndRenderTarget,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL, DWRITE_TEXT_ALIGNMENT_CENTER,
    IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetMonitorInfoW, HBRUSH,
    InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, PAINTSTRUCT,
    ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_NOREPEAT, RegisterHotKey, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, UnregisterHotKey,
    VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, HTTRANSPARENT, HWND_TOPMOST, IDC_ARROW,
    IsWindow, KillTimer, LoadCursorW, MA_NOACTIVATE, RegisterClassExW, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, SetCursor, SetTimer, SetWindowDisplayAffinity, SetWindowPos, UnregisterClassW,
    WDA_EXCLUDEFROMCAPTURE, WM_ERASEBKGND, WM_HOTKEY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_MOUSEACTIVATE,
    WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT, WM_RBUTTONDOWN, WM_SETCURSOR, WM_TIMER, WNDCLASSEXW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::error::{OverlayError, win32};
use crate::input::point_from_lparam;
use crate::layout::{self, ScrollButton, ScrollPanelLayout, TooltipCommand, TooltipState};
use crate::render::{
    Brushes, CORNER_RADIUS, Fonts, Renderer, color, draw_button_glyph_on, draw_tooltip_on,
    fill_rounded, measure_text_width, rect_to_d2d, rgba_to_bgra, to_f32,
};

const BORDER_CLASS: PCWSTR = w!("LenswinScrollBorder");
const PANEL_CLASS: PCWSTR = w!("LenswinScrollPanel");
const DEFAULT_DPI: u32 = 96;
const PANEL_GAP: u32 = 12;
const HOTKEY_ESC: i32 = 1;
const TOOLTIP_TIMER_ID: usize = 1;
const TOOLTIP_DELAY_MS: u32 = 400;
/// 亮蓝色边框颜色（COLORREF 顺序为 0x00BBGGRR，对应 RGB(77, 153, 255)）。
const BORDER_COLORREF: u32 = 0x00FF_994D;

type ScrollSink = Rc<dyn Fn(ScrollEvent)>;

fn discard<T>(_value: T) {}

struct PanelResources {
    target: ID2D1HwndRenderTarget,
    brushes: Brushes,
    fonts: Fonts,
    auto_label: IDWriteTextFormat,
    preview_bitmap: Option<(ID2D1Bitmap, u32, u32)>,
}

struct ScrollState {
    renderer: Renderer,
    panel_hwnd: HWND,
    panel_size: (u32, u32),
    dpi: u32,
    layout: ScrollPanelLayout,
    sink: ScrollSink,
    status: String,
    auto_scrolling: bool,
    actions_enabled: bool,
    hovered: Option<ScrollButton>,
    tooltip: TooltipState<ScrollButton>,
    tooltip_timer: bool,
    tracking_leave: bool,
    preview: Option<CroppedImage>,
    preview_dirty: bool,
    axis: Option<ScrollAxis>,
    resources: Option<PanelResources>,
    pending_events: Vec<ScrollEvent>,
}

thread_local! {
    static SCROLL_SESSION: RefCell<Option<ScrollState>> = const { RefCell::new(None) };
}

fn with_scroll<R>(f: impl FnOnce(&mut ScrollState) -> R) -> Option<R> {
    SCROLL_SESSION
        .try_with(|cell| {
            let mut guard = cell.try_borrow_mut().ok()?;
            guard.as_mut().map(f)
        })
        .ok()
        .flatten()
}

/// 长截图非阻塞会话：选区外围 4 条点穿边框与独立控制面板。
///
/// 只能在主线程创建与使用，窗口由调用方的消息循环驱动；销毁时释放全部窗口与全局热键。
pub struct ScrollSession {
    instance: HINSTANCE,
    panel_hwnd: HWND,
    border_hwnds: [HWND; 4],
    _not_send: PhantomData<Rc<()>>,
}

impl ScrollSession {
    /// 打开长截图边框与控制面板，立即返回；用户交互通过 `sink` 回投 [`ScrollEvent`]。
    pub fn begin(bounds: Rect, sink: impl Fn(ScrollEvent) + 'static) -> Result<Self, OverlayError> {
        if bounds.is_empty() {
            return Err(OverlayError::InvalidInput("长截图选区为空".to_string()));
        }
        if scroll_in_use() {
            return Err(OverlayError::InvalidInput(
                "已有长截图会话正在进行".to_string(),
            ));
        }

        let renderer = Renderer::new()?;
        // SAFETY: 传入 None 取当前模块句柄，无需释放。
        let module = unsafe { GetModuleHandleW(None) }.map_err(win32("GetModuleHandleW"))?;
        let instance = HINSTANCE(module.0);

        register_class(instance, BORDER_CLASS, Some(border_wnd_proc))?;
        if let Err(e) = register_class(instance, PANEL_CLASS, Some(panel_wnd_proc)) {
            // SAFETY: BORDER_CLASS 刚在本函数内注册且无窗口使用。
            discard(unsafe { UnregisterClassW(BORDER_CLASS, Some(instance)) });
            return Err(e);
        }

        let (work_area, dpi) = monitor_metrics(&bounds);
        let desired = layout::scroll_panel_size(dpi);
        let gap = layout::scaled(PANEL_GAP, dpi);
        let panel_rect = popup_placement(&bounds, &work_area, desired.0, desired.1, gap)
            .ok_or_else(|| OverlayError::InvalidInput("无法放置长截图控制面板".to_string()))?;
        let panel_size = (panel_rect.width, panel_rect.height);
        let panel_layout = layout::scroll_panel_layout(panel_size, dpi);
        let border_rects = layout::scroll_border_rects(&bounds, dpi);

        let mut border_hwnds = [HWND::default(); 4];
        for (slot, rect) in border_hwnds.iter_mut().zip(&border_rects) {
            match create_border_window(instance, rect) {
                Ok(hwnd) => *slot = hwnd,
                Err(e) => {
                    cleanup_windows(instance, None, &border_hwnds);
                    return Err(e);
                }
            }
        }

        let panel_hwnd = match create_panel_window(instance, &panel_rect) {
            Ok(hwnd) => hwnd,
            Err(e) => {
                cleanup_windows(instance, None, &border_hwnds);
                return Err(e);
            }
        };

        // 即使失败也只放行，边框已在 bounds 外，面板只可能被自己遮挡
        // SAFETY: panel_hwnd 是本线程刚创建的有效窗口。
        discard(unsafe { SetWindowDisplayAffinity(panel_hwnd, WDA_EXCLUDEFROMCAPTURE) });

        // 面板不激活收不到按键，Esc 只能走全局热键；Enter 不注册，避免会话期间劫持其它程序的回车
        // SAFETY: panel_hwnd 由本线程创建且存活，注销会在 Drop 中成对执行。
        discard(unsafe {
            RegisterHotKey(
                Some(panel_hwnd),
                HOTKEY_ESC,
                MOD_NOREPEAT,
                u32::from(VK_ESCAPE.0),
            )
        });

        let state = ScrollState {
            renderer,
            panel_hwnd,
            panel_size,
            dpi,
            layout: panel_layout,
            sink: Rc::new(sink),
            status: "滚动以开始".to_string(),
            auto_scrolling: false,
            actions_enabled: false,
            hovered: None,
            tooltip: TooltipState::default(),
            tooltip_timer: false,
            tracking_leave: false,
            preview: None,
            preview_dirty: false,
            axis: None,
            resources: None,
            pending_events: Vec::new(),
        };

        if SCROLL_SESSION
            .try_with(|cell| {
                *cell.borrow_mut() = Some(state);
            })
            .is_err()
        {
            cleanup_windows(instance, Some(panel_hwnd), &border_hwnds);
            return Err(OverlayError::InvalidInput(
                "无法初始化长截图会话".to_string(),
            ));
        }

        for hwnd in border_hwnds {
            // SAFETY: hwnd 是刚创建的边框窗口。
            discard(unsafe {
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE
                        | SWP_SHOWWINDOW
                        | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                        | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE,
                )
            });
        }
        // SAFETY: panel_hwnd 是刚创建的控制面板窗口，不抢占被滚动窗口的焦点。
        if let Err(e) = unsafe {
            SetWindowPos(
                panel_hwnd,
                Some(HWND_TOPMOST),
                panel_rect.x,
                panel_rect.y,
                i32::try_from(panel_rect.width).unwrap_or(i32::MAX),
                i32::try_from(panel_rect.height).unwrap_or(i32::MAX),
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        }
        .map_err(win32("SetWindowPos"))
        {
            drop(Self {
                instance,
                panel_hwnd,
                border_hwnds,
                _not_send: PhantomData,
            });
            return Err(e);
        }

        invalidate(panel_hwnd);
        Ok(Self {
            instance,
            panel_hwnd,
            border_hwnds,
            _not_send: PhantomData,
        })
    }

    /// 更新控制面板上的状态文字并触发重绘。
    pub fn set_status(&self, text: &str) {
        let hwnd = with_scroll(|s| {
            if s.status != text {
                s.status = text.to_string();
                Some(s.panel_hwnd)
            } else {
                None
            }
        })
        .flatten();
        if let Some(hwnd) = hwnd {
            invalidate(hwnd);
        }
    }

    /// 更新自动滚动按钮状态并触发重绘。
    pub fn set_auto_scrolling(&self, active: bool) {
        let hwnd = with_scroll(|s| {
            if s.auto_scrolling != active {
                s.auto_scrolling = active;
                Some(s.panel_hwnd)
            } else {
                None
            }
        })
        .flatten();
        if let Some(hwnd) = hwnd {
            invalidate(hwnd);
        }
    }

    /// 设置预览缩略图和当前滚动轴方向，标记下次重绘更新位图。
    pub fn set_preview(&self, image: Option<CroppedImage>, axis: Option<ScrollAxis>) {
        let hwnd = with_scroll(|s| {
            s.actions_enabled = image.is_some();
            s.preview = image;
            s.axis = axis;
            s.preview_dirty = true;
            s.panel_hwnd
        });
        if let Some(hwnd) = hwnd {
            invalidate(hwnd);
        }
    }
}

impl Drop for ScrollSession {
    fn drop(&mut self) {
        // 先取出状态释放 Direct2D 资源，再销毁窗口与注销热键
        let taken = SCROLL_SESSION.try_with(|cell| {
            cell.try_borrow_mut()
                .ok()
                .and_then(|mut guard| guard.take())
        });
        discard(taken);
        cleanup_windows(self.instance, Some(self.panel_hwnd), &self.border_hwnds);
    }
}

fn scroll_in_use() -> bool {
    SCROLL_SESSION
        .try_with(|cell| {
            let Ok(mut guard) = cell.try_borrow_mut() else {
                return true;
            };
            let stale = guard.as_ref().is_some_and(|s| {
                // SAFETY: IsWindow 对已销毁句柄也安全。
                !unsafe { IsWindow(Some(s.panel_hwnd)) }.as_bool()
            });
            if stale {
                discard(guard.take());
            }
            guard.is_some()
        })
        .unwrap_or(true)
}

fn cleanup_windows(instance: HINSTANCE, panel: Option<HWND>, borders: &[HWND; 4]) {
    if let Some(hwnd) = panel
        && !hwnd.0.is_null()
    {
        // SAFETY: 窗口销毁前停掉悬停提示计时器；未启动时失败无影响。
        discard(unsafe { KillTimer(Some(hwnd), TOOLTIP_TIMER_ID) });
        // SAFETY: 注销本窗口登记的全局热键；未登记时失败无影响。
        discard(unsafe { UnregisterHotKey(Some(hwnd), HOTKEY_ESC) });
        // SAFETY: 窗口由本模块创建，此处只销毁一次。
        discard(unsafe { DestroyWindow(hwnd) });
    }
    for &hwnd in borders {
        if !hwnd.0.is_null() {
            // SAFETY: 窗口由本模块创建，此处只销毁一次。
            discard(unsafe { DestroyWindow(hwnd) });
        }
    }
    // SAFETY: 类名与注册时一致，窗口已全部销毁。
    discard(unsafe { UnregisterClassW(PANEL_CLASS, Some(instance)) });
    // SAFETY: 同上。
    discard(unsafe { UnregisterClassW(BORDER_CLASS, Some(instance)) });
}

fn register_class(
    instance: HINSTANCE,
    name: PCWSTR,
    wnd_proc: windows::Win32::UI::WindowsAndMessaging::WNDPROC,
) -> Result<(), OverlayError> {
    // SAFETY: IDC_ARROW 为系统预置光标，无需释放。
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.ok();
    let class = WNDCLASSEXW {
        cbSize: u32::try_from(std::mem::size_of::<WNDCLASSEXW>())
            .map_err(|_| OverlayError::InvalidInput("窗口类结构过大".to_string()))?,
        lpfnWndProc: wnd_proc,
        hInstance: instance,
        hCursor: cursor.unwrap_or_default(),
        lpszClassName: name,
        ..Default::default()
    };
    // SAFETY: class 内指针指向静态数据，窗口过程为有效 extern "system" 函数。
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(OverlayError::Win32 {
            api: "RegisterClassExW",
            // SAFETY: 紧跟失败调用读取错误码。
            source: windows::core::Error::from(unsafe { GetLastError() }),
        });
    }
    Ok(())
}

fn create_border_window(instance: HINSTANCE, rect: &Rect) -> Result<HWND, OverlayError> {
    let w = i32::try_from(rect.width.max(1))
        .map_err(|_| OverlayError::InvalidInput("边框宽度超出范围".to_string()))?;
    let h = i32::try_from(rect.height.max(1))
        .map_err(|_| OverlayError::InvalidInput("边框高度超出范围".to_string()))?;
    // SAFETY: 窗口类已注册，参数有效；点穿、置顶且不激活。
    unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
            BORDER_CLASS,
            w!(""),
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
    .map_err(win32("CreateWindowExW"))
}

fn create_panel_window(instance: HINSTANCE, rect: &Rect) -> Result<HWND, OverlayError> {
    let w = i32::try_from(rect.width.max(1))
        .map_err(|_| OverlayError::InvalidInput("面板宽度超出范围".to_string()))?;
    let h = i32::try_from(rect.height.max(1))
        .map_err(|_| OverlayError::InvalidInput("面板高度超出范围".to_string()))?;
    // SAFETY: 窗口类已注册，参数有效；置顶工具窗且不激活。
    unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            PANEL_CLASS,
            w!("长截图"),
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
    .map_err(win32("CreateWindowExW"))
}

fn monitor_metrics(anchor: &Rect) -> (Rect, u32) {
    let win_rect = RECT {
        left: anchor.x,
        top: anchor.y,
        right: i32::try_from(anchor.right()).unwrap_or(i32::MAX),
        bottom: i32::try_from(anchor.bottom()).unwrap_or(i32::MAX),
    };
    // SAFETY: win_rect 为有效局部变量；NEAREST 保证返回某个显示器。
    let monitor = unsafe { MonitorFromRect(&win_rect, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(0),
        ..Default::default()
    };
    // SAFETY: info.cbSize 已设置，monitor 来自 MonitorFromRect。
    let work_area = if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        Rect::from_edges(
            i64::from(info.rcWork.left),
            i64::from(info.rcWork.top),
            i64::from(info.rcWork.right),
            i64::from(info.rcWork.bottom),
        )
        .filter(|r| !r.is_empty())
        .unwrap_or(*anchor)
    } else {
        *anchor
    };
    let (mut dpi_x, mut dpi_y) = (DEFAULT_DPI, DEFAULT_DPI);
    // SAFETY: 两个输出参数为有效局部变量。
    let dpi = match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
    {
        Ok(()) if dpi_x > 0 => dpi_x,
        _ => DEFAULT_DPI,
    };
    (work_area, dpi)
}

fn invalidate(hwnd: HWND) {
    // 只是标记重绘区域，失败时下一次事件仍会触发重绘
    // SAFETY: hwnd 是本会话创建的有效窗口。
    discard(unsafe { InvalidateRect(Some(hwnd), None, false) });
}

fn deliver_events() {
    let batch = with_scroll(|s| {
        if s.pending_events.is_empty() {
            None
        } else {
            Some((Rc::clone(&s.sink), std::mem::take(&mut s.pending_events)))
        }
    })
    .flatten();
    if let Some((sink, events)) = batch {
        for event in events {
            sink(event);
        }
    }
}

impl ScrollState {
    fn is_button_enabled(&self, button: ScrollButton) -> bool {
        match button {
            ScrollButton::AutoScroll | ScrollButton::Cancel => true,
            ScrollButton::Copy
            | ScrollButton::Save
            | ScrollButton::Pin
            | ScrollButton::Search
            | ScrollButton::Ocr => self.actions_enabled,
        }
    }

    fn click(&mut self, p: Point) {
        let Some(button) = self.layout.hit(p) else {
            return;
        };
        if !self.is_button_enabled(button) {
            return;
        }
        match button {
            ScrollButton::AutoScroll => {
                let next = !self.auto_scrolling;
                self.auto_scrolling = next;
                self.pending_events.push(ScrollEvent::AutoScroll(next));
                invalidate(self.panel_hwnd);
            }
            ScrollButton::Copy => self
                .pending_events
                .push(ScrollEvent::Finished(ToolbarAction::Copy)),
            ScrollButton::Save => self
                .pending_events
                .push(ScrollEvent::Finished(ToolbarAction::Save)),
            ScrollButton::Pin => self
                .pending_events
                .push(ScrollEvent::Finished(ToolbarAction::Pin)),
            ScrollButton::Search => self
                .pending_events
                .push(ScrollEvent::Finished(ToolbarAction::Search)),
            ScrollButton::Ocr => self
                .pending_events
                .push(ScrollEvent::Finished(ToolbarAction::Ocr)),
            ScrollButton::Cancel => self.pending_events.push(ScrollEvent::Closed),
        }
    }

    fn apply_tooltip_cmd(&mut self, cmd: TooltipCommand) {
        match cmd {
            TooltipCommand::Keep => {}
            TooltipCommand::Arm => {
                // SAFETY: panel_hwnd 是本线程创建且存活的窗口，不使用回调函数。
                discard(unsafe {
                    SetTimer(
                        Some(self.panel_hwnd),
                        TOOLTIP_TIMER_ID,
                        TOOLTIP_DELAY_MS,
                        None,
                    )
                });
                self.tooltip_timer = true;
            }
            TooltipCommand::Disarm => {
                if self.tooltip_timer {
                    // SAFETY: panel_hwnd 持有该计时器。
                    discard(unsafe { KillTimer(Some(self.panel_hwnd), TOOLTIP_TIMER_ID) });
                    self.tooltip_timer = false;
                }
            }
        }
    }

    fn press_tooltip(&mut self) -> bool {
        let (cmd, vis_changed) = self.tooltip.on_press();
        self.apply_tooltip_cmd(cmd);
        vis_changed
    }

    fn ensure_resources(&mut self) -> Result<(), OverlayError> {
        if self.resources.is_none() {
            let target_props = D2D1_RENDER_TARGET_PROPERTIES {
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: self.panel_hwnd,
                pixelSize: D2D_SIZE_U {
                    width: self.panel_size.0,
                    height: self.panel_size.1,
                },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            // SAFETY: panel_hwnd 是本线程创建的有效窗口，两个属性结构均已填写。
            let target = unsafe {
                self.renderer
                    .factory()
                    .CreateHwndRenderTarget(&target_props, &hwnd_props)
            }
            .map_err(win32("CreateHwndRenderTarget"))?;
            let brushes = self.renderer.create_brushes(&target)?;
            let fonts = self.renderer.create_fonts(self.dpi)?;
            let px = |v: u32| to_f32(i64::from(layout::scaled(v, self.dpi)));
            let auto_label = self.renderer.text_format(
                w!("Microsoft YaHei UI"),
                DWRITE_FONT_WEIGHT_NORMAL,
                px(12),
                DWRITE_TEXT_ALIGNMENT_CENTER,
            )?;
            self.resources = Some(PanelResources {
                target,
                brushes,
                fonts,
                auto_label,
                preview_bitmap: None,
            });
            self.preview_dirty = true;
        }
        if self.preview_dirty {
            self.refresh_preview_bitmap()?;
            self.preview_dirty = false;
        }
        Ok(())
    }

    fn refresh_preview_bitmap(&mut self) -> Result<(), OverlayError> {
        let Some(res) = self.resources.as_mut() else {
            return Ok(());
        };
        let Some(img) = self.preview.clone() else {
            res.preview_bitmap = None;
            return Ok(());
        };
        // SAFETY: target 有效，无参数。
        let max_side = unsafe { res.target.GetMaximumBitmapSize() }.max(1);
        let scaled = if img.width().max(img.height()) > max_side {
            downscale(img, max_side)
                .map_err(|e| OverlayError::InvalidInput(format!("缩小预览图失败：{e}")))?
        } else {
            img
        };
        let (w, h) = (scaled.width(), scaled.height());
        let bgra = rgba_to_bgra(scaled.rgba());
        let pitch = w
            .checked_mul(4)
            .ok_or_else(|| OverlayError::InvalidInput("预览图行字节数溢出".to_string()))?;
        let bitmap_props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        // SAFETY: bgra 长度为 w * h * 4 且 pitch 与之一致，调用期间数据有效。
        let bitmap = unsafe {
            res.target.CreateBitmap(
                D2D_SIZE_U {
                    width: w,
                    height: h,
                },
                Some(bgra.as_ptr().cast()),
                pitch,
                &bitmap_props,
            )
        }
        .map_err(win32("CreateBitmap"))?;
        res.preview_bitmap = Some((bitmap, w, h));
        Ok(())
    }

    fn paint(&mut self) {
        if let Err(e) = self.ensure_resources() {
            self.pending_events.push(ScrollEvent::Failed(e.to_string()));
            return;
        }
        let Some(res) = self.resources.as_ref() else {
            return;
        };
        let target = &res.target;
        let brushes = &res.brushes;
        let fonts = &res.fonts;
        let button_style = self.renderer.button_style();
        let radius = to_f32(i64::from(layout::scaled(CORNER_RADIUS, self.dpi)));

        // SAFETY: target 与画刷、字体均为本窗口持有且有效，BeginDraw 与 EndDraw 成对调用。
        unsafe {
            target.BeginDraw();
            target.Clear(Some(&color(0.12, 0.12, 0.13, 1.0)));
        }

        // 面板边框线
        let panel_rect = rect_to_d2d(&self.layout.panel);
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷有效。
        unsafe {
            target.DrawRectangle(&panel_rect, &brushes.separator, 1.0, None);
        }

        // 预览缩略图背景与图像
        let preview_rect = rect_to_d2d(&self.layout.preview);
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，颜色与画刷有效。
        unsafe {
            brushes.swatch.SetColor(&color(0.06, 0.06, 0.07, 1.0));
            target.FillRectangle(&preview_rect, &brushes.swatch);
        }
        if let Some((bitmap, bw, bh)) = res.preview_bitmap.as_ref() {
            let dst = fit_preview_rect(&preview_rect, *bw, *bh, self.axis);
            // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，位图由同一目标创建，dst 有效。
            unsafe {
                target.DrawBitmap(
                    bitmap,
                    Some(&dst),
                    1.0,
                    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                    None,
                );
            }
        }

        // 状态文字
        let status_rect = rect_to_d2d(&self.layout.status);
        let status_wide: Vec<u16> = self.status.encode_utf16().collect();
        // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，文本、格式与画刷有效。
        unsafe {
            target.DrawText(
                &status_wide,
                &fonts.hint,
                &status_rect,
                &brushes.white,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }

        // 按钮组
        for (button, cell) in &self.layout.buttons {
            let r = rect_to_d2d(cell);
            let enabled = self.is_button_enabled(*button);
            let hovered = enabled && self.hovered == Some(*button);
            if *button == ScrollButton::AutoScroll {
                if self.auto_scrolling {
                    fill_rounded(target, &r, &brushes.active, radius);
                } else if hovered {
                    fill_rounded(target, &r, &brushes.hover, radius);
                } else {
                    // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，画刷有效。
                    unsafe { brushes.swatch.SetColor(&color(1.0, 1.0, 1.0, 0.08)) };
                    fill_rounded(target, &r, &brushes.swatch, radius);
                }
                let text = if self.auto_scrolling {
                    "停止滚动"
                } else {
                    "自动滚动"
                };
                let wide: Vec<u16> = text.encode_utf16().collect();
                // SAFETY: 在 BeginDraw 与 EndDraw 之间调用，文本、格式与画刷有效。
                unsafe {
                    target.DrawText(
                        &wide,
                        &res.auto_label,
                        &r,
                        &brushes.white,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            } else if let Some(tb) = button.as_toolbar_button() {
                if hovered {
                    fill_rounded(target, &r, &brushes.hover, radius);
                }
                let brush = if !enabled {
                    &brushes.disabled
                } else if *button == ScrollButton::Copy {
                    &brushes.accent
                } else {
                    &brushes.white
                };
                draw_button_glyph_on(target, button_style, fonts, tb, &r, brush, None);
            }
        }

        if let Some(button) = self.tooltip.visible_target()
            && let Some(anchor) = self.layout.button_rect(button)
        {
            let wide: Vec<u16> = button.tooltip(self.auto_scrolling).encode_utf16().collect();
            let text_w =
                measure_text_width(self.renderer.dwrite(), &wide, &fonts.tooltip, self.dpi);
            let size = layout::tooltip_size(text_w, self.dpi);
            let tip_rect = layout::tooltip_rect(&anchor, &self.layout.panel, None, size, self.dpi);
            let local = rect_to_d2d(&tip_rect);
            draw_tooltip_on(target, brushes, &fonts.tooltip, &local, radius, &wide);
        }

        // SAFETY: 与上面的 BeginDraw 配对。
        match unsafe { target.EndDraw(None, None) } {
            Ok(()) => {}
            Err(e) if e.code() == D2DERR_RECREATE_TARGET => {
                self.resources = None;
                invalidate(self.panel_hwnd);
            }
            Err(source) => {
                self.pending_events
                    .push(ScrollEvent::Failed(format!("EndDraw 失败：{source}")));
            }
        }
    }
}

/// 将预览位图等比放入目标区域；滚动方向有剩余空间时按最新一端对齐。
fn fit_preview_rect(
    container: &D2D_RECT_F,
    img_w: u32,
    img_h: u32,
    axis: Option<ScrollAxis>,
) -> D2D_RECT_F {
    let cw = (container.right - container.left).max(1.0);
    let ch = (container.bottom - container.top).max(1.0);
    let iw = to_f32(i64::from(img_w.max(1)));
    let ih = to_f32(i64::from(img_h.max(1)));
    let scale = (cw / iw).min(ch / ih);
    let dw = (iw * scale).min(cw);
    let dh = (ih * scale).min(ch);
    let left = match axis {
        Some(ScrollAxis::Horizontal) => container.right - dw,
        Some(ScrollAxis::Vertical) | None => container.left + (cw - dw) / 2.0,
    };
    let top = match axis {
        Some(ScrollAxis::Vertical) => container.bottom - dh,
        Some(ScrollAxis::Horizontal) | None => container.top + (ch - dh) / 2.0,
    };
    D2D_RECT_F {
        left,
        top,
        right: left + dw,
        bottom: top + dh,
    }
}

unsafe extern "system" fn border_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => return LRESULT(isize::try_from(HTTRANSPARENT).unwrap_or(-1)),
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            // SAFETY: hwnd 是本窗口过程收到的有效窗口，BeginPaint/EndPaint 成对调用。
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            if !hdc.0.is_null() {
                // SAFETY: COLORREF 为静态常量，画刷在本分支末尾释放。
                let brush: HBRUSH = unsafe { CreateSolidBrush(COLORREF(BORDER_COLORREF)) };
                if !brush.0.is_null() {
                    // SAFETY: hdc、ps.rcPaint 与 brush 均有效。
                    discard(unsafe { FillRect(hdc, &ps.rcPaint, brush) });
                    // SAFETY: brush 由上面的 CreateSolidBrush 创建。
                    discard(unsafe { DeleteObject(brush.into()) });
                }
                // SAFETY: 与上面的 BeginPaint 配对。
                discard(unsafe { EndPaint(hwnd, &ps) });
            }
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: 参数原样来自系统传入的窗口消息。
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

unsafe extern "system" fn panel_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_MOUSEACTIVATE => return LRESULT(isize::try_from(MA_NOACTIVATE).unwrap_or(3)),
        WM_SETCURSOR => {
            // SAFETY: 加载系统预置箭头光标，无需释放。
            if let Ok(cursor) = unsafe { LoadCursorW(None, IDC_ARROW) } {
                // SAFETY: cursor 为有效光标句柄。
                discard(unsafe { SetCursor(Some(cursor)) });
            }
            return LRESULT(1);
        }
        WM_PAINT => {
            with_scroll(ScrollState::paint);
            // SAFETY: hwnd 是本线程的有效窗口，None 表示整个客户区。
            discard(unsafe { ValidateRect(Some(hwnd), None) });
            deliver_events();
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let (x, y) = point_from_lparam(lparam.0);
            let changed = with_scroll(|s| {
                if !s.tracking_leave {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: u32::try_from(std::mem::size_of::<TRACKMOUSEEVENT>()).unwrap_or(0),
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    // SAFETY: tme 结构已填写且 hwnd 为本线程有效窗口。
                    if unsafe { TrackMouseEvent(&mut tme) }.is_ok() {
                        s.tracking_leave = true;
                    }
                }
                let next = s.layout.hit(Point::new(x, y));
                let hovered_changed = if next != s.hovered {
                    s.hovered = next;
                    true
                } else {
                    false
                };
                let (cmd, tip_changed) = s.tooltip.on_hover(next);
                s.apply_tooltip_cmd(cmd);
                hovered_changed || tip_changed
            })
            .unwrap_or(false);
            if changed {
                invalidate(hwnd);
            }
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            let changed = with_scroll(|s| {
                s.tracking_leave = false;
                let hovered_changed = s.hovered.take().is_some();
                let (cmd, tip_changed) = s.tooltip.reset();
                s.apply_tooltip_cmd(cmd);
                hovered_changed || tip_changed
            })
            .unwrap_or(false);
            if changed {
                invalidate(hwnd);
            }
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point_from_lparam(lparam.0);
            let tip_hidden = with_scroll(|s| {
                let hidden = s.press_tooltip();
                s.click(Point::new(x, y));
                hidden
            })
            .unwrap_or(false);
            if tip_hidden {
                invalidate(hwnd);
            }
            deliver_events();
            return LRESULT(0);
        }
        WM_RBUTTONDOWN => {
            let tip_hidden = with_scroll(ScrollState::press_tooltip).unwrap_or(false);
            if tip_hidden {
                invalidate(hwnd);
            }
            return LRESULT(0);
        }
        WM_TIMER => {
            if wparam.0 == TOOLTIP_TIMER_ID {
                let shown = with_scroll(|s| {
                    // SAFETY: hwnd 是设置该单次计时器的面板窗口。
                    discard(unsafe { KillTimer(Some(hwnd), TOOLTIP_TIMER_ID) });
                    s.tooltip_timer = false;
                    s.tooltip.on_timer()
                })
                .unwrap_or(false);
                if shown {
                    invalidate(hwnd);
                }
                return LRESULT(0);
            }
        }
        WM_HOTKEY => {
            let id = i32::try_from(wparam.0).unwrap_or(-1);
            if id == HOTKEY_ESC {
                with_scroll(|s| s.pending_events.push(ScrollEvent::Closed));
            }
            deliver_events();
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            let vk = u16::try_from(wparam.0).unwrap_or(0);
            with_scroll(|s| {
                if vk == VK_ESCAPE.0 {
                    s.pending_events.push(ScrollEvent::Closed);
                } else if vk == VK_RETURN.0 && s.actions_enabled {
                    s.pending_events
                        .push(ScrollEvent::Finished(ToolbarAction::Copy));
                }
            });
            deliver_events();
            return LRESULT(0);
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
    fn fit_preview_aligns_newest_edge_for_scroll_axis() {
        let box_rect = D2D_RECT_F {
            left: 10.0,
            top: 10.0,
            right: 210.0,
            bottom: 110.0,
        };
        // 竖向高图居中左右、贴底显示最新一端
        let v = fit_preview_rect(&box_rect, 100, 200, Some(ScrollAxis::Vertical));
        assert!((v.bottom - 110.0).abs() < 0.01);
        assert!((v.right - v.left - 50.0).abs() < 0.01);
        assert!((v.left - 85.0).abs() < 0.01);

        // 横向宽图贴右显示最新一端
        let h = fit_preview_rect(&box_rect, 400, 100, Some(ScrollAxis::Horizontal));
        assert!((h.right - 210.0).abs() < 0.01);
        assert!((h.bottom - h.top - 50.0).abs() < 0.01);
        assert!((h.top - 35.0).abs() < 0.01);
    }
}
