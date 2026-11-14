//! 文字标注的状态：编辑、选中与文本框的移动、缩放、换行、旋转。

use lenswin_core::annotation::{Color, Shape, TEXT_PADDING_DIVISOR, Tool};
use lenswin_core::geometry::{Point, Rect};
use lenswin_core::text_box::{
    BoxCorner, PointF, TextBox, TextHandle, anchor_corner, bounds_from_anchor,
    rotation_from_pointer, round_to_i32, scaled_size, scaled_wrap, snap_rotation,
    wrap_length_from_pointer,
};
use lenswin_core::text_edit::TextBuffer;

use super::{Annotator, TextStyle, clamp_point};

/// 文字排版参数：换行长度与排版方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextLayout {
    /// 换行长度（像素）：横排是宽、竖排是高；`None` 为不自动换行。
    pub wrap: Option<u32>,
    /// 竖排：列从右往左排。
    pub vertical: bool,
}

/// 文字排版度量，由窗口层的 DirectWrite 实现；状态机只依赖这个接口，保持与 Win32 无关。
pub trait TextMeasure {
    /// 排版后的（宽, 高）像素；空文字宽（竖排为高）为 0、另一边为一行。
    fn measure(&self, text: &str, size: u32, layout: TextLayout) -> (u32, u32);
    /// 第 `char_index` 个字符处光标的（x, y, 厚度），相对文字区域左上角；
    /// 厚度横排是行高、竖排是列宽。
    fn caret(
        &self,
        text: &str,
        size: u32,
        layout: TextLayout,
        char_index: usize,
    ) -> (i32, i32, u32);
    /// 沿换行方向移动一行（竖排为一列）后的字符序号；没有下一行（列）时返回 `None`。
    fn step_block(
        &self,
        text: &str,
        size: u32,
        layout: TextLayout,
        char_index: usize,
        forward: bool,
    ) -> Option<usize>;
}

/// 不依赖系统字体的近似度量：每个字符沿行宽 `字号 / 2`，行（列）厚度 `字号 × 5 / 4`。
#[derive(Debug, Clone, Copy, Default)]
pub struct ApproxMeasure;

impl ApproxMeasure {
    fn thickness(size: u32) -> u32 {
        size.saturating_mul(5) / 4
    }

    /// 每行（列）最多容纳的字符数。
    fn per_line(size: u32, layout: TextLayout) -> usize {
        let advance = usize::try_from((size / 2).max(1)).unwrap_or(1);
        layout.wrap.map_or(usize::MAX, |l| {
            (usize::try_from(l).unwrap_or(0) / advance).max(1)
        })
    }

    /// 每个字符所在的（行号, 列号）与总行数，逻辑换行和自动换行都算。
    fn places(text: &str, size: u32, layout: TextLayout) -> (Vec<(usize, usize)>, usize) {
        let per = Self::per_line(size, layout);
        let (mut line, mut col) = (0usize, 0usize);
        let mut out = Vec::new();
        for c in text.chars() {
            if c == '\n' {
                out.push((line, col));
                line += 1;
                col = 0;
                continue;
            }
            if col >= per {
                line += 1;
                col = 0;
            }
            out.push((line, col));
            col += 1;
        }
        (out, line + 1)
    }
}

impl TextMeasure for ApproxMeasure {
    fn measure(&self, text: &str, size: u32, layout: TextLayout) -> (u32, u32) {
        let (places, lines) = Self::places(text, size, layout);
        let longest = places
            .iter()
            .zip(text.chars())
            .filter(|(_, c)| *c != '\n')
            .map(|((_, col), _)| col + 1)
            .max()
            .unwrap_or(0);
        let along = u32::try_from(longest)
            .unwrap_or(u32::MAX)
            .saturating_mul(size / 2);
        let across = u32::try_from(lines)
            .unwrap_or(u32::MAX)
            .saturating_mul(Self::thickness(size));
        if layout.vertical {
            (across, along)
        } else {
            (along, across)
        }
    }

    fn caret(
        &self,
        text: &str,
        size: u32,
        layout: TextLayout,
        char_index: usize,
    ) -> (i32, i32, u32) {
        let (places, lines) = Self::places(text, size, layout);
        let (line, col) = places.get(char_index).copied().unwrap_or_else(|| {
            // 光标在末尾：接在最后一个字符之后
            match places.last().zip(text.chars().last()) {
                Some((&(l, _), '\n')) => (l + 1, 0),
                Some((&(l, c), _)) => (l, c + 1),
                None => (0, 0),
            }
        });
        let thickness = Self::thickness(size);
        let advance = i64::from(size / 2);
        let along = i64::try_from(col).unwrap_or(0) * advance;
        let block = i64::try_from(line).unwrap_or(0) * i64::from(thickness);
        let clamp = |v: i64| i32::try_from(v).unwrap_or(0);
        if layout.vertical {
            let total = i64::try_from(lines.max(line + 1)).unwrap_or(1);
            (
                clamp((total - 1) * i64::from(thickness) - block),
                clamp(along),
                thickness,
            )
        } else {
            (clamp(along), clamp(block), thickness)
        }
    }

    fn step_block(&self, _: &str, _: u32, _: TextLayout, _: usize, _: bool) -> Option<usize> {
        None
    }
}

/// 编辑中的按键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKey {
    /// 确认。
    Confirm,
    /// 换行（竖排为新起一列）。
    Newline,
    /// 丢弃本次编辑。
    Discard,
    /// 向前删除一个字符。
    Backspace,
    /// 向后删除一个字符。
    Delete,
    /// 光标左移（竖排为换到下一列）。
    Left,
    /// 光标右移（竖排为换到上一列）。
    Right,
    /// 光标上移一行（竖排为列内上移一个字符）。
    Up,
    /// 光标下移一行（竖排为列内下移一个字符）。
    Down,
    /// 行首。
    Home,
    /// 行尾。
    End,
    /// 插入 4 个空格。
    Tab,
}

/// 文字框内边距：底色模式下为字号的四分之一，纯文字为 0。
pub fn text_padding(size: u32, background: bool) -> u32 {
    if background {
        size / TEXT_PADDING_DIVISOR
    } else {
        0
    }
}

/// 未完成的输入法组字串。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Composition {
    text: String,
    /// 组字光标在组字串里的字符序号。
    cursor: usize,
}

/// 正在编辑的文本框。字号、颜色、外观和横竖排取自当前文字样式。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Editing {
    /// 锚点角（横排左上、竖排右上）的屏幕位置，文字增长时它不动。
    anchor: PointF,
    monitor: usize,
    buffer: TextBuffer,
    composition: Option<Composition>,
    /// 重新编辑的图元序号。
    replacing: Option<usize>,
    rotation: i32,
    wrap: Option<u32>,
}

/// 一个文字图元的全部数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TextData {
    text: String,
    color: Color,
    size: u32,
    background: bool,
    vertical: bool,
    rotation: i32,
    wrap: Option<u32>,
    bounds: Rect,
}

impl TextData {
    fn from_shape(shape: &Shape) -> Option<Self> {
        match shape {
            Shape::Text {
                text,
                color,
                size,
                background,
                vertical,
                rotation,
                wrap,
                bounds,
                ..
            } => Some(Self {
                text: text.clone(),
                color: *color,
                size: *size,
                background: *background,
                vertical: *vertical,
                rotation: *rotation,
                wrap: *wrap,
                bounds: *bounds,
            }),
            _ => None,
        }
    }

    fn to_shape(&self) -> Shape {
        Shape::Text {
            origin: Point::new(self.bounds.x, self.bounds.y),
            text: self.text.clone(),
            color: self.color,
            size: self.size,
            background: self.background,
            bounds: self.bounds,
            rotation: self.rotation,
            wrap: self.wrap,
            vertical: self.vertical,
        }
    }

    fn text_box(&self) -> TextBox {
        TextBox {
            bounds: self.bounds,
            rotation: self.rotation,
            vertical: self.vertical,
        }
    }

    fn style(&self) -> TextStyle {
        TextStyle {
            color: self.color,
            size: self.size,
            background: self.background,
            vertical: self.vertical,
        }
    }

    fn anchor(&self) -> PointF {
        self.text_box().corner(anchor_corner(self.vertical))
    }
}

/// 绘制编辑框所需的全部信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditView {
    /// 所在显示器序号。
    pub monitor: usize,
    /// 含组字串的显示文字。
    pub text: String,
    /// 组字串在 `text` 中的 UTF-16 范围（起点, 长度）。
    pub composition: Option<(usize, usize)>,
    /// 光标在 `text` 中的字符序号。
    pub caret: usize,
    /// 当前文字样式。
    pub style: TextStyle,
    /// 未旋转的本地外接框。
    pub bounds: Rect,
    /// 旋转角度。
    pub rotation: i32,
    /// 换行长度。
    pub wrap: Option<u32>,
    /// 重新编辑时需要隐藏的原图元序号。
    pub hidden: Option<usize>,
}

/// 文本框调整的对象。
#[derive(Debug, Clone, PartialEq, Eq)]
enum AdjustTarget {
    Editing,
    Selected(usize),
}

/// 正在进行的文本框调整（拖动中）。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TextAdjust {
    handle: TextHandle,
    target: AdjustTarget,
    press: PointF,
    start_box: TextBox,
    start_size: u32,
    start_wrap: Option<u32>,
    /// 选中状态下的原图元数据；编辑状态为 `None`。
    base: Option<TextData>,
    /// 选中状态下拖动中的预览数据。
    preview: Option<TextData>,
    /// 指针离开过按下的位置；没有移动过的点击不改变文本框。
    moved: bool,
}

/// 当前显示文本框的几何与状态，供绘制和命中使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextBoxView {
    /// 旋转后的文本框。
    pub geometry: TextBox,
    /// 编辑中（否则为选中）。
    pub editing: bool,
}

fn distance(a: PointF, b: PointF) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn to_f64(p: Point) -> PointF {
    (f64::from(p.x), f64::from(p.y))
}

impl Annotator {
    fn layout_of(wrap: Option<u32>, vertical: bool) -> TextLayout {
        TextLayout { wrap, vertical }
    }

    /// 文本框整体大小（含内边距）：设了换行长度时，换行方向的内部长度就是它；
    /// 空文本框在行方向上至少半个字号，保证点得到。
    fn box_size(&self, text: &str, size: u32, background: bool, layout: TextLayout) -> (u32, u32) {
        let pad = text_padding(size, background).saturating_mul(2);
        let (mut w, mut h) = self.measure.measure(text, size, layout);
        match (layout.vertical, layout.wrap) {
            (false, Some(l)) => w = l,
            (true, Some(l)) => h = l,
            _ => {}
        }
        if layout.vertical {
            h = h.max(size / 2);
        } else {
            w = w.max(size / 2);
        }
        (w.saturating_add(pad), h.saturating_add(pad))
    }

    /// 以 `corner` 角落在 `anchor` 重新排版，更新 `bounds`。
    fn relayout(&self, data: &mut TextData, anchor: PointF, corner: BoxCorner) {
        let layout = Self::layout_of(data.wrap, data.vertical);
        let size = self.box_size(&data.text, data.size, data.background, layout);
        data.bounds = bounds_from_anchor(anchor, corner, size, data.rotation);
    }

    /// 当前文字样式，字号按运行时上限夹紧。
    fn style(&self) -> TextStyle {
        TextStyle {
            size: self.effective_size(Tool::Text),
            ..self.styles.text
        }
    }

    /// 编辑中的文本框数据（文字为含组字串的显示文字）。
    fn editing_data(&self, edit: &Editing) -> TextData {
        let (text, _, _) = Self::display(edit);
        let style = self.style();
        let mut data = TextData {
            text,
            color: style.color,
            size: style.size,
            background: style.background,
            vertical: style.vertical,
            rotation: edit.rotation,
            wrap: edit.wrap,
            bounds: Rect::default(),
        };
        self.relayout(&mut data, edit.anchor, anchor_corner(style.vertical));
        data
    }

    /// 设置文字是否带底色。
    pub fn set_text_background(&mut self, background: bool) {
        self.styles.text.background = background;
        self.restyle_selected();
    }

    /// 设置文字横排或竖排。
    pub fn set_text_vertical(&mut self, vertical: bool) {
        self.styles.text.vertical = vertical;
        self.restyle_selected();
    }

    /// 选中的文字按当前样式重排并记为一步撤销。
    pub(super) fn restyle_selected(&mut self) {
        if self.tool != Some(Tool::Text) || self.editing.is_some() || self.text_adjust.is_some() {
            return;
        }
        let Some(index) = self.selected else {
            return;
        };
        let Some(old) = self
            .document
            .shapes()
            .get(index)
            .and_then(TextData::from_shape)
        else {
            return;
        };
        let style = self.style();
        let mut new = old.clone();
        new.color = style.color;
        new.size = style.size;
        new.background = style.background;
        new.vertical = style.vertical;
        let anchor = old.anchor();
        let corner = anchor_corner(new.vertical);
        self.relayout(&mut new, anchor, corner);
        if new != old {
            self.document.replace(index, new.to_shape());
        }
    }

    /// 正在编辑文字。
    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// 在 `p` 处开始新文本框；`bounds` 是所在显示器范围，`monitor` 是其序号。
    pub fn begin_text(&mut self, p: Point, bounds: &Rect, monitor: usize) {
        self.selected = None;
        self.text_adjust = None;
        self.editing = Some(Editing {
            anchor: to_f64(clamp_point(p, bounds)),
            monitor,
            buffer: TextBuffer::new(),
            composition: None,
            replacing: None,
            rotation: 0,
            wrap: None,
        });
    }

    /// 重新编辑第 `index` 个图元（必须是文字）：载入它的内容与样式，样式同时成为当前文字样式。
    pub fn begin_edit_existing(&mut self, index: usize, monitor: usize) -> bool {
        let Some(data) = self
            .document
            .shapes()
            .get(index)
            .and_then(TextData::from_shape)
        else {
            return false;
        };
        self.styles.text = data.style();
        self.selected = None;
        self.text_adjust = None;
        self.editing = Some(Editing {
            anchor: data.anchor(),
            monitor,
            buffer: TextBuffer::from_text(&data.text),
            composition: None,
            replacing: Some(index),
            rotation: data.rotation,
            wrap: data.wrap,
        });
        true
    }

    /// 编辑中的文本框内容为空白，且不是重新编辑。
    pub fn editing_blank_new(&self) -> bool {
        self.editing
            .as_ref()
            .is_some_and(|e| e.replacing.is_none() && e.buffer.is_blank())
    }

    /// 确认编辑：空白的新文本框不生成图元，空白的重新编辑删除原文字，其余新增或替换；
    /// 未完成的组字串丢弃。文档有变化返回 `true`。
    pub fn confirm_text(&mut self) -> bool {
        self.text_adjust = None;
        let Some(edit) = self.editing.take() else {
            return false;
        };
        let blank = edit.buffer.is_blank();
        match (edit.replacing, blank) {
            (None, true) => false,
            (Some(index), true) => self.document.remove(index),
            (replacing, false) => {
                let style = self.style();
                let mut data = TextData {
                    text: edit.buffer.text(),
                    color: style.color,
                    size: style.size,
                    background: style.background,
                    vertical: style.vertical,
                    rotation: edit.rotation,
                    wrap: edit.wrap,
                    bounds: Rect::default(),
                };
                self.relayout(&mut data, edit.anchor, anchor_corner(style.vertical));
                let shape = data.to_shape();
                match replacing {
                    None => {
                        self.document.push(shape);
                        true
                    }
                    Some(index) if self.document.shapes().get(index) == Some(&shape) => false,
                    Some(index) => self.document.replace(index, shape),
                }
            }
        }
    }

    /// 丢弃本次编辑，文档不变。
    pub fn discard_text(&mut self) {
        self.text_adjust = None;
        self.editing = None;
    }

    /// 编辑中的按键；返回内容是否有变化（用于重置光标闪烁）。
    pub fn edit_key(&mut self, key: EditKey) -> bool {
        match key {
            EditKey::Confirm => {
                self.confirm_text();
                return true;
            }
            EditKey::Discard => {
                self.discard_text();
                return true;
            }
            _ => {}
        }
        let style = self.style();
        let measure = self.measure.clone();
        let Some(edit) = self.editing.as_mut() else {
            return false;
        };
        // 组字期间的按键由输入法处理，不会走到这里；残留的组字串在编辑操作前清掉
        edit.composition = None;
        let layout = Self::layout_of(edit.wrap, style.vertical);
        let buffer = &mut edit.buffer;
        let step = |buffer: &mut TextBuffer, forward: bool| {
            let text = buffer.text();
            match measure.step_block(&text, style.size, layout, buffer.cursor(), forward) {
                Some(index) => buffer.set_cursor(index),
                None if forward => buffer.move_down(),
                None => buffer.move_up(),
            }
        };
        match (key, style.vertical) {
            (EditKey::Newline, _) => buffer.insert_newline(),
            (EditKey::Tab, _) => buffer.insert_tab(),
            (EditKey::Backspace, _) => {
                buffer.backspace();
            }
            (EditKey::Delete, _) => {
                buffer.delete();
            }
            (EditKey::Home, _) => buffer.move_home(),
            (EditKey::End, _) => buffer.move_end(),
            (EditKey::Left, false) | (EditKey::Up, true) => buffer.move_left(),
            (EditKey::Right, false) | (EditKey::Down, true) => buffer.move_right(),
            (EditKey::Down, false) | (EditKey::Left, true) => step(buffer, true),
            (EditKey::Up, false) | (EditKey::Right, true) => step(buffer, false),
            (EditKey::Confirm | EditKey::Discard, _) => {}
        }
        true
    }

    /// 插入一个字符（`WM_CHAR`）；控制字符被忽略。
    pub fn insert_char(&mut self, c: char) -> bool {
        self.editing
            .as_mut()
            .is_some_and(|e| e.buffer.insert_char(c))
    }

    /// 插入一段文字（粘贴或输入法结果），控制字符（换行除外）被忽略。
    pub fn insert_text(&mut self, text: &str) -> bool {
        self.editing
            .as_mut()
            .is_some_and(|e| e.buffer.insert_str(text) > 0)
    }

    /// 设置组字串；`cursor` 是组字光标在其中的字符序号，空串表示没有组字。
    pub fn set_composition(&mut self, text: &str, cursor: usize) {
        if let Some(edit) = self.editing.as_mut() {
            edit.composition = (!text.is_empty()).then(|| Composition {
                text: text.to_string(),
                cursor: cursor.min(text.chars().count()),
            });
        }
    }

    /// 清除组字串。
    pub fn clear_composition(&mut self) {
        if let Some(edit) = self.editing.as_mut() {
            edit.composition = None;
        }
    }

    /// 带组字串的显示文字、组字串的 UTF-16 范围和光标的字符序号。
    pub(super) fn display(edit: &Editing) -> (String, Option<(usize, usize)>, usize) {
        let before = edit.buffer.before_cursor();
        let after = edit.buffer.after_cursor();
        match &edit.composition {
            Some(c) => {
                let start = before.encode_utf16().count();
                let len = c.text.encode_utf16().count();
                let caret = edit.buffer.cursor() + c.cursor;
                (
                    format!("{before}{}{after}", c.text),
                    Some((start, len)),
                    caret,
                )
            }
            None => (format!("{before}{after}"), None, edit.buffer.cursor()),
        }
    }

    /// 编辑中文本框的外接范围（未旋转的本地框）。
    pub fn edit_bounds(&self) -> Option<Rect> {
        let edit = self.editing.as_ref()?;
        Some(self.editing_data(edit).bounds)
    }

    /// 绘制编辑框所需的信息。
    pub fn edit_view(&self) -> Option<EditView> {
        let edit = self.editing.as_ref()?;
        let data = self.editing_data(edit);
        let (text, composition, caret) = Self::display(edit);
        Some(EditView {
            monitor: edit.monitor,
            composition,
            caret,
            style: self.style(),
            bounds: data.bounds,
            rotation: edit.rotation,
            wrap: edit.wrap,
            hidden: edit.replacing,
            text,
        })
    }

    /// 光标所在的一行（虚拟桌面坐标的旋转后外接框）与是否竖排，输入法候选窗据此定位。
    pub fn caret_line(&self) -> Option<(usize, Rect, bool)> {
        let edit = self.editing.as_ref()?;
        let data = self.editing_data(edit);
        let (text, _, caret) = Self::display(edit);
        let layout = Self::layout_of(data.wrap, data.vertical);
        let (cx, cy, thickness) = self.measure.caret(&text, data.size, layout, caret);
        let pad = f64::from(text_padding(data.size, data.background));
        let (w, h) = (f64::from(data.bounds.width), f64::from(data.bounds.height));
        let (x0, y0) = (
            -w / 2.0 + pad + f64::from(cx),
            -h / 2.0 + pad + f64::from(cy),
        );
        let t = f64::from(thickness.max(1));
        let (x1, y1) = if data.vertical {
            (x0 + t, y0 + 1.0)
        } else {
            (x0 + 1.0, y0 + t)
        };
        let rect = data
            .text_box()
            .screen_bounds_of([(x0, y0), (x1, y0), (x1, y1), (x0, y1)])?;
        Some((edit.monitor, rect, data.vertical))
    }

    /// 选中的文字图元序号。
    pub fn selected_text(&self) -> Option<usize> {
        self.selected
    }

    /// 选中第 `index` 个文字图元，它的样式成为当前文字样式；不是文字时无效果。
    pub fn select_text(&mut self, index: usize) -> bool {
        let Some(data) = self
            .document
            .shapes()
            .get(index)
            .and_then(TextData::from_shape)
        else {
            return false;
        };
        self.styles.text = data.style();
        self.selected = Some(index);
        self.text_adjust = None;
        true
    }

    /// 取消选中；原本有选中返回 `true`。
    pub fn deselect_text(&mut self) -> bool {
        self.text_adjust = None;
        self.selected.take().is_some()
    }

    /// 删除选中的文字，算一步撤销。
    pub fn delete_selected(&mut self) -> bool {
        self.text_adjust = None;
        match self.selected.take() {
            Some(index) => self.document.remove(index),
            None => false,
        }
    }

    /// 把选中的文字移动 `(dx, dy)` 像素，算一步撤销。
    pub fn nudge_selected(&mut self, dx: i32, dy: i32) -> bool {
        let Some(index) = self.selected else {
            return false;
        };
        let Some(mut data) = self
            .document
            .shapes()
            .get(index)
            .and_then(TextData::from_shape)
        else {
            return false;
        };
        data.bounds = translated(&data.bounds, i64::from(dx), i64::from(dy));
        self.document.replace(index, data.to_shape())
    }

    /// 选中文字当前的数据：拖动中用预览，否则取文档里的。
    fn selected_data(&self) -> Option<(usize, TextData)> {
        let index = self.selected?;
        if let Some(preview) = self.text_adjust.as_ref().and_then(|a| a.preview.clone()) {
            return Some((index, preview));
        }
        let data = self
            .document
            .shapes()
            .get(index)
            .and_then(TextData::from_shape)?;
        Some((index, data))
    }

    /// 当前显示的文本框：编辑中的，否则选中的；只在文字工具下有。
    pub fn active_text_box(&self) -> Option<TextBoxView> {
        if self.tool != Some(Tool::Text) {
            return None;
        }
        if let Some(edit) = self.editing.as_ref() {
            return Some(TextBoxView {
                geometry: self.editing_data(edit).text_box(),
                editing: true,
            });
        }
        self.selected_data().map(|(_, data)| TextBoxView {
            geometry: data.text_box(),
            editing: false,
        })
    }

    /// 屏幕点命中当前文本框的哪个部位。
    pub fn text_handle_at(
        &self,
        p: Point,
        tolerance: f64,
        rotate_distance: f64,
    ) -> Option<TextHandle> {
        self.active_text_box()?
            .geometry
            .hit(to_f64(p), tolerance, rotate_distance)
    }

    /// 在 `handle` 上按下开始调整；没有可调整的文本框返回 `false`。
    pub fn begin_text_adjust(&mut self, handle: TextHandle, p: Point) -> bool {
        let (target, geometry, size, wrap, base) = if let Some(edit) = self.editing.as_ref() {
            let data = self.editing_data(edit);
            (
                AdjustTarget::Editing,
                data.text_box(),
                data.size,
                data.wrap,
                None,
            )
        } else if let Some((index, data)) = self.selected_data() {
            (
                AdjustTarget::Selected(index),
                data.text_box(),
                data.size,
                data.wrap,
                Some(data),
            )
        } else {
            return false;
        };
        self.text_adjust = Some(TextAdjust {
            handle,
            target,
            press: to_f64(p),
            start_box: geometry,
            start_size: size,
            start_wrap: wrap,
            base,
            preview: None,
            moved: false,
        });
        true
    }

    /// 正在拖动文本框。
    pub fn is_adjusting_text(&self) -> bool {
        self.text_adjust.is_some()
    }

    /// 正在拖动的手柄。
    pub fn adjusting_handle(&self) -> Option<TextHandle> {
        self.text_adjust.as_ref().map(|a| a.handle)
    }

    /// 拖动文本框的手柄：Shift 时旋转吸附到 15°。
    pub fn update_text_adjust(&mut self, p: Point) {
        let Some(adjust) = self.text_adjust.clone() else {
            return;
        };
        let current = to_f64(p);
        if !adjust.moved && current == adjust.press {
            return;
        }
        if let Some(live) = self.text_adjust.as_mut() {
            live.moved = true;
        }
        let (text, size, background, vertical, rotation) = match (&adjust.target, &adjust.base) {
            (AdjustTarget::Selected(_), Some(base)) => (
                base.text.clone(),
                base.size,
                base.background,
                base.vertical,
                base.rotation,
            ),
            _ => {
                let Some(edit) = self.editing.as_ref() else {
                    return;
                };
                let data = self.editing_data(edit);
                (
                    data.text,
                    data.size,
                    data.background,
                    data.vertical,
                    data.rotation,
                )
            }
        };
        let start = adjust.start_box;
        let mut new_size = size;
        let mut new_wrap = adjust.start_wrap;
        let mut new_rotation = rotation;
        // 固定不动的角：缩放和换行都以它为锚点重新排版
        let mut fixed: Option<(PointF, BoxCorner)> = None;
        match adjust.handle {
            TextHandle::Body | TextHandle::Border => {}
            TextHandle::Corner(corner) => {
                let anchor = start.corner(corner.opposite());
                let first = distance(anchor, start.corner(corner));
                new_size = scaled_size(
                    adjust.start_size,
                    first,
                    distance(anchor, current),
                    self.text_max,
                );
                new_wrap = scaled_wrap(adjust.start_wrap, adjust.start_size, new_size);
                fixed = Some((anchor, corner.opposite()));
            }
            TextHandle::Edge(edge) => {
                let pad = text_padding(size, background);
                new_wrap = Some(wrap_length_from_pointer(&start, edge, current, pad, size));
                let corner = edge.fixed_corner();
                fixed = Some((start.corner(corner), corner));
            }
            TextHandle::Rotate => {
                if let Some(raw) = rotation_from_pointer(start.center(), current, vertical) {
                    new_rotation = if self.shift { snap_rotation(raw) } else { raw };
                }
            }
        }
        let bounds = match fixed {
            Some((anchor, corner)) => {
                let layout = Self::layout_of(new_wrap, vertical);
                let box_size = self.box_size(&text, new_size, background, layout);
                bounds_from_anchor(anchor, corner, box_size, new_rotation)
            }
            None if adjust.handle == TextHandle::Rotate => start.bounds,
            None => translated(
                &start.bounds,
                i64::from(round_to_i32(current.0 - adjust.press.0)),
                i64::from(round_to_i32(current.1 - adjust.press.1)),
            ),
        };
        self.apply_adjust(&adjust, bounds, new_rotation, new_wrap, new_size);
    }

    /// 把调整结果写回：选中状态存预览，编辑状态直接改编辑框。
    fn apply_adjust(
        &mut self,
        adjust: &TextAdjust,
        bounds: Rect,
        rotation: i32,
        wrap: Option<u32>,
        size: u32,
    ) {
        match (&adjust.target, &adjust.base) {
            (AdjustTarget::Selected(_), Some(base)) => {
                let mut preview = base.clone();
                preview.bounds = bounds;
                preview.rotation = rotation;
                preview.wrap = wrap;
                preview.size = size;
                if let Some(current) = self.text_adjust.as_mut() {
                    current.preview = Some(preview);
                }
            }
            _ => {
                self.styles.text.size = size;
                let vertical = self.styles.text.vertical;
                if let Some(edit) = self.editing.as_mut() {
                    edit.rotation = rotation;
                    edit.wrap = wrap;
                    edit.anchor = TextBox {
                        bounds,
                        rotation,
                        vertical,
                    }
                    .corner(anchor_corner(vertical));
                }
            }
        }
    }

    /// 松开：选中状态下的调整记为一步撤销；返回文档是否有变化。
    pub fn finish_text_adjust(&mut self) -> bool {
        let Some(adjust) = self.text_adjust.take() else {
            return false;
        };
        if !adjust.moved {
            return false;
        }
        let (AdjustTarget::Selected(index), Some(base), Some(preview)) =
            (adjust.target, adjust.base, adjust.preview)
        else {
            return false;
        };
        if preview == base {
            return false;
        }
        if preview.size != base.size {
            self.styles.text.size = preview.size;
        }
        self.document.replace(index, preview.to_shape())
    }

    /// 拖动中的预览图元；没有拖动选中文字时为 `None`。
    pub fn text_preview(&self) -> Option<Shape> {
        self.text_adjust
            .as_ref()
            .and_then(|a| a.preview.as_ref())
            .map(TextData::to_shape)
    }

    /// 需要隐藏的图元序号：重新编辑的原文字，或正在拖动的选中文字。
    pub fn hidden_shape(&self) -> Option<usize> {
        if let Some(edit) = self.editing.as_ref() {
            return edit.replacing;
        }
        match self.text_adjust.as_ref() {
            Some(TextAdjust {
                target: AdjustTarget::Selected(index),
                preview: Some(_),
                ..
            }) => Some(*index),
            _ => None,
        }
    }

    /// 取消自动换行（双击边手柄）；有变化返回 `true`。选中状态下算一步撤销。
    pub fn clear_wrap(&mut self) -> bool {
        self.text_adjust = None;
        if let Some(edit) = self.editing.as_mut() {
            return edit.wrap.take().is_some();
        }
        let Some((index, old)) = self.selected_data() else {
            return false;
        };
        if old.wrap.is_none() {
            return false;
        }
        let mut new = old.clone();
        new.wrap = None;
        let anchor = old.anchor();
        let corner = anchor_corner(new.vertical);
        self.relayout(&mut new, anchor, corner);
        self.document.replace(index, new.to_shape())
    }
}

fn translated(rect: &Rect, dx: i64, dy: i64) -> Rect {
    Rect::from_edges(
        i64::from(rect.x) + dx,
        i64::from(rect.y) + dy,
        rect.right() + dx,
        rect.bottom() + dy,
    )
    .unwrap_or(*rect)
}

#[cfg(test)]
mod tests {
    use lenswin_core::annotation::PALETTE;
    use lenswin_core::config::AnnotateConfig;
    use lenswin_core::text_box::BoxEdge;

    use super::*;

    const MON: Rect = Rect::new(0, 0, 1000, 800);

    fn p(x: i32, y: i32) -> Point {
        Point::new(x, y)
    }

    /// 字号 24 的 "hello"：近似度量下每字宽 12、行高 30，框为 (100, 100, 60, 30)。
    fn with_hello() -> Annotator {
        let mut a = Annotator::new(&AnnotateConfig::default());
        a.toggle_tool(Tool::Text);
        a.begin_text(p(100, 100), &MON, 0);
        a.insert_text("hello");
        assert!(a.confirm_text());
        a
    }

    fn only(a: &Annotator) -> TextData {
        match a.document().shapes() {
            [shape] => TextData::from_shape(shape).expect("文字图元"),
            other => panic!("期望一个文字图元，实际为 {other:?}"),
        }
    }

    fn drag(a: &mut Annotator, handle: TextHandle, from: Point, to: Point) -> bool {
        assert!(a.begin_text_adjust(handle, from));
        a.update_text_adjust(to);
        a.finish_text_adjust()
    }

    #[test]
    fn select_loads_style_and_delete_is_one_undo_step() {
        let mut a = with_hello();
        a.set_color(PALETTE[4]);
        assert!(a.select_text(0));
        assert_eq!(a.selected_text(), Some(0));
        assert_eq!(a.style_view().and_then(|v| v.swatch), Some(0));
        assert!(a.delete_selected());
        assert!(a.document().is_empty() && a.selected_text().is_none());
        assert!(a.undo());
        assert_eq!(only(&a).text, "hello");
        assert!(!a.select_text(5));
        assert!(!a.delete_selected());
    }

    #[test]
    fn deselect_reports_whether_something_was_selected() {
        let mut a = with_hello();
        assert!(!a.deselect_text());
        a.select_text(0);
        assert!(a.deselect_text());
        assert!(!a.deselect_text());
    }

    #[test]
    fn nudging_moves_by_the_given_offset_one_step_each() {
        let mut a = with_hello();
        assert!(!a.nudge_selected(1, 0), "没有选中时无效果");
        a.select_text(0);
        assert!(a.nudge_selected(1, 0));
        assert!(a.nudge_selected(0, -10));
        assert_eq!(only(&a).bounds, Rect::new(101, 90, 60, 30));
        assert!(a.undo());
        assert_eq!(only(&a).bounds, Rect::new(101, 100, 60, 30));
    }

    #[test]
    fn dragging_the_body_previews_then_commits_one_step() {
        let mut a = with_hello();
        a.select_text(0);
        assert!(a.begin_text_adjust(TextHandle::Body, p(120, 110)));
        assert!(a.is_adjusting_text());
        a.update_text_adjust(p(150, 140));
        // 拖动中文档不变，只有预览，原文字被隐藏
        assert_eq!(only(&a).bounds, Rect::new(100, 100, 60, 30));
        assert_eq!(a.hidden_shape(), Some(0));
        let preview = a.text_preview().and_then(|s| TextData::from_shape(&s));
        assert_eq!(preview.map(|d| d.bounds), Some(Rect::new(130, 130, 60, 30)));
        assert!(a.finish_text_adjust());
        assert_eq!(only(&a).bounds, Rect::new(130, 130, 60, 30));
        assert_eq!(a.hidden_shape(), None);
        assert!(a.undo());
        assert_eq!(only(&a).bounds, Rect::new(100, 100, 60, 30));
        assert!(!a.can_undo() || a.redo());
    }

    #[test]
    fn clicking_a_handle_without_moving_changes_nothing() {
        for handle in [
            TextHandle::Edge(BoxEdge::Right),
            TextHandle::Corner(BoxCorner::BottomRight),
            TextHandle::Rotate,
        ] {
            let mut a = with_hello();
            a.select_text(0);
            let at = p(160, 115);
            assert!(!drag(&mut a, handle, at, at), "{handle:?}");
            assert_eq!(only(&a).wrap, None);
            assert_eq!(only(&a).size, 24);
            assert_eq!(only(&a).rotation, 0);
            let mut e = Annotator::new(&AnnotateConfig::default());
            e.toggle_tool(Tool::Text);
            e.begin_text(p(100, 100), &MON, 0);
            e.insert_text("hello");
            e.begin_text_adjust(handle, at);
            e.update_text_adjust(at);
            e.finish_text_adjust();
            assert_eq!(e.edit_view().map(|v| (v.wrap, v.rotation)), Some((None, 0)));
            assert_eq!(e.edit_view().map(|v| v.style.size), Some(24));
        }
    }

    #[test]
    fn click_without_movement_adds_no_undo_step() {
        let mut a = with_hello();
        a.select_text(0);
        assert!(!drag(&mut a, TextHandle::Body, p(120, 110), p(120, 110)));
        assert!(a.undo());
        assert!(a.document().is_empty());
    }

    #[test]
    fn corner_drag_scales_size_about_the_opposite_corner() {
        let mut a = with_hello();
        a.select_text(0);
        // 右下角 (160, 130) 到锚点 (100, 100) 的距离翻倍：字号 24 → 48
        assert!(drag(
            &mut a,
            TextHandle::Corner(BoxCorner::BottomRight),
            p(160, 130),
            p(220, 160)
        ));
        let data = only(&a);
        assert_eq!(data.size, 48);
        assert_eq!(data.bounds, Rect::new(100, 100, 120, 60));
        assert!(a.undo());
        assert_eq!(only(&a).size, 24);
    }

    #[test]
    fn corner_scale_clamps_and_scales_the_wrap_with_it() {
        let mut a = with_hello();
        a.select_text(0);
        assert!(drag(
            &mut a,
            TextHandle::Corner(BoxCorner::BottomRight),
            p(160, 130),
            p(101, 101)
        ));
        assert_eq!(only(&a).size, 12);
        let mut b = with_hello();
        b.select_text(0);
        assert!(drag(
            &mut b,
            TextHandle::Edge(BoxEdge::Right),
            p(160, 115),
            p(220, 115)
        ));
        assert_eq!(only(&b).wrap, Some(120));
        // 点左上角向外拖，以右下角为锚点放大；换行长度同比放大
        let before = only(&b);
        assert!(drag(
            &mut b,
            TextHandle::Corner(BoxCorner::TopLeft),
            p(100, 100),
            p(-20, 85)
        ));
        let after = only(&b);
        assert!(after.size > before.size);
        assert_eq!(
            after.wrap,
            scaled_wrap(before.wrap, before.size, after.size)
        );
        let anchor_before = before.text_box().corner(BoxCorner::BottomRight);
        let anchor_after = after.text_box().corner(BoxCorner::BottomRight);
        assert!(
            (anchor_before.0 - anchor_after.0).abs() <= 1.0
                && (anchor_before.1 - anchor_after.1).abs() <= 1.0
        );
    }

    #[test]
    fn edge_drag_sets_wrap_and_reflows_with_the_far_edge_fixed() {
        let mut a = with_hello();
        a.select_text(0);
        assert!(drag(
            &mut a,
            TextHandle::Edge(BoxEdge::Right),
            p(160, 115),
            p(220, 115)
        ));
        let data = only(&a);
        assert_eq!(data.wrap, Some(120));
        assert_eq!(data.bounds, Rect::new(100, 100, 120, 30));
        // 缩到最小：一个字号宽，每行两个字符，hello 排成三行
        assert!(drag(
            &mut a,
            TextHandle::Edge(BoxEdge::Right),
            p(220, 115),
            p(105, 115)
        ));
        let data = only(&a);
        assert_eq!(data.wrap, Some(24));
        assert_eq!(data.bounds, Rect::new(100, 100, 24, 90));
        // 拖左边时右缘固定
        assert!(drag(
            &mut a,
            TextHandle::Edge(BoxEdge::Left),
            p(100, 145),
            p(60, 145)
        ));
        let data = only(&a);
        assert_eq!(data.wrap, Some(64));
        assert_eq!(data.bounds.right(), 124);
    }

    #[test]
    fn clearing_wrap_is_one_step_and_restores_single_line() {
        let mut a = with_hello();
        a.select_text(0);
        assert!(!a.clear_wrap(), "没有换行时无变化");
        drag(
            &mut a,
            TextHandle::Edge(BoxEdge::Right),
            p(160, 115),
            p(110, 115),
        );
        assert!(only(&a).wrap.is_some());
        assert!(a.clear_wrap());
        let data = only(&a);
        assert_eq!(
            (data.wrap, data.bounds),
            (None, Rect::new(100, 100, 60, 30))
        );
        assert!(a.undo());
        assert!(only(&a).wrap.is_some());
    }

    #[test]
    fn rotation_follows_the_pointer_and_shift_snaps_to_15_degrees() {
        let mut a = with_hello();
        a.select_text(0);
        // 中心 (130, 115)，指针在正右方 → 90°
        assert!(drag(&mut a, TextHandle::Rotate, p(130, 70), p(230, 115)));
        let data = only(&a);
        assert_eq!(data.rotation, 90);
        assert_eq!(data.bounds, Rect::new(100, 100, 60, 30));
        let mut b = with_hello();
        b.select_text(0);
        b.set_shift(true);
        assert!(drag(&mut b, TextHandle::Rotate, p(130, 70), p(230, 85)));
        assert_eq!(only(&b).rotation, 75);
        let mut c = with_hello();
        c.select_text(0);
        assert!(drag(&mut c, TextHandle::Rotate, p(130, 70), p(230, 85)));
        assert_eq!(only(&c).rotation, 73);
    }

    #[test]
    fn rotation_handle_uses_the_vertical_orientation() {
        let mut a = with_hello();
        a.select_text(0);
        a.set_text_vertical(true);
        let data = only(&a);
        assert!(data.vertical);
        // 竖排旋转手柄在右侧：指针在正下方 → 90°
        let center = data.text_box().center();
        let below = p(round_to_i32(center.0), round_to_i32(center.1) + 100);
        let handle = p(round_to_i32(center.0) + 60, round_to_i32(center.1));
        assert!(drag(&mut a, TextHandle::Rotate, handle, below));
        assert_eq!(only(&a).rotation, 90);
    }

    #[test]
    fn vertical_switch_relayouts_selected_text_from_its_anchor() {
        let mut a = with_hello();
        a.select_text(0);
        a.set_text_vertical(true);
        let data = only(&a);
        // 竖排以右上角为锚点：原左上角 (100, 100) 变成右上角，列宽 30、列高 60
        assert_eq!(
            (data.vertical, data.bounds),
            (true, Rect::new(70, 100, 30, 60))
        );
        assert!(a.undo());
        assert!(!only(&a).vertical);
    }

    #[test]
    fn style_changes_on_selected_text_are_one_step_each() {
        let mut a = with_hello();
        a.select_text(0);
        a.set_color(PALETTE[2]);
        a.apply_preset(2);
        a.set_text_background(true);
        let data = only(&a);
        assert_eq!(
            (data.color, data.size, data.background),
            (PALETTE[2], 36, true)
        );
        assert!(a.undo() && a.undo() && a.undo());
        let data = only(&a);
        assert_eq!(
            (data.color, data.size, data.background),
            (PALETTE[0], 24, false)
        );
        assert!(a.redo());
        assert_eq!(only(&a).color, PALETTE[2]);
    }

    #[test]
    fn text_size_follows_the_monitor_limit_everywhere() {
        let mut a = with_hello();
        a.set_text_limit(100);
        a.select_text(0);
        // 滚轮：从 24 起每格 2，夹在 2 倍屏高 = 200
        a.wheel(500);
        assert_eq!(only(&a).size, 200);
        a.set_text_limit(3);
        a.apply_preset(2);
        assert_eq!(only(&a).size, 12, "预设也要夹紧，且不低于 12");
        a.set_text_limit(100);
        a.select_text(0);
        assert!(drag(
            &mut a,
            TextHandle::Corner(BoxCorner::BottomRight),
            p(112, 106),
            p(10_000, 5_000)
        ));
        assert_eq!(only(&a).size, 200);
    }

    #[test]
    fn stored_style_is_clamped_on_use_without_rewriting_the_config() {
        let mut config = AnnotateConfig::default();
        config.text.size = 5000;
        let mut a = Annotator::new(&config);
        a.toggle_tool(Tool::Text);
        a.set_text_limit(1080);
        assert_eq!(a.style_view().map(|v| v.size), Some(2160));
        assert_eq!(a.changed_config(), None);
        a.begin_text(p(10, 10), &MON, 0);
        a.insert_text("a");
        a.confirm_text();
        assert_eq!(only(&a).size, 2160);
        // 换到更高的显示器后上限放宽，配置里的值重新生效
        a.set_text_limit(4320);
        a.begin_text(p(10, 10), &MON, 0);
        assert_eq!(a.edit_view().map(|v| v.style.size), Some(5000));
        a.discard_text();
        a.wheel(-1);
        assert_eq!(a.style_view().map(|v| v.size), Some(4998));
    }

    #[test]
    fn large_sizes_are_allowed_up_to_the_config_limit() {
        let mut config = AnnotateConfig::default();
        config.text.size = 16_384;
        let mut a = Annotator::new(&config);
        a.toggle_tool(Tool::Text);
        a.set_text_limit(100_000);
        assert_eq!(a.style_view().map(|v| v.size), Some(16_384));
        a.wheel(10);
        assert_eq!(a.style_view().map(|v| v.size), Some(16_384));
    }

    #[test]
    fn wheel_resizes_the_selected_text() {
        let mut a = with_hello();
        a.select_text(0);
        a.wheel(2);
        assert_eq!(only(&a).size, 28);
    }

    #[test]
    fn selection_ends_with_undo_tool_change_or_new_box() {
        let mut a = with_hello();
        a.select_text(0);
        a.undo();
        assert_eq!(a.selected_text(), None);
        a.redo();
        a.select_text(0);
        a.toggle_tool(Tool::Pen);
        assert_eq!(a.selected_text(), None);
        a.toggle_tool(Tool::Text);
        a.select_text(0);
        a.begin_text(p(300, 300), &MON, 0);
        assert_eq!(a.selected_text(), None);
    }

    #[test]
    fn handles_exist_only_for_the_active_box_with_the_text_tool() {
        let mut a = with_hello();
        assert!(a.active_text_box().is_none());
        a.select_text(0);
        let view = a.active_text_box().expect("文本框");
        assert!(!view.editing);
        assert_eq!(
            a.text_handle_at(p(130, 115), 6.0, 24.0),
            Some(TextHandle::Body)
        );
        assert_eq!(
            a.text_handle_at(p(130, 100 - 24), 6.0, 24.0),
            Some(TextHandle::Rotate)
        );
        a.toggle_tool(Tool::Pen);
        assert!(a.active_text_box().is_none());
        assert_eq!(a.text_handle_at(p(130, 115), 6.0, 24.0), None);
    }

    #[test]
    fn editing_box_can_be_rotated_wrapped_and_moved_then_confirmed() {
        let mut a = Annotator::new(&AnnotateConfig::default());
        a.toggle_tool(Tool::Text);
        a.begin_text(p(100, 100), &MON, 0);
        a.insert_text("hello");
        assert!(a.active_text_box().is_some_and(|v| v.editing));
        // 编辑中的调整直接作用在编辑框上，不产生文档变化
        assert!(!drag(&mut a, TextHandle::Rotate, p(130, 70), p(230, 115)));
        assert_eq!(a.edit_view().map(|v| v.rotation), Some(90));
        let edge = a
            .active_text_box()
            .and_then(|v| v.geometry.edge_handles().get(1).map(|(_, point)| *point))
            .map(|point| p(round_to_i32(point.0), round_to_i32(point.1)))
            .expect("换行手柄");
        assert!(!drag(
            &mut a,
            TextHandle::Edge(BoxEdge::Right),
            edge,
            p(130, 300)
        ));
        assert!(a.edit_view().and_then(|v| v.wrap).is_some());
        let before = a.edit_bounds().expect("框");
        a.begin_text_adjust(TextHandle::Border, p(100, 100));
        a.update_text_adjust(p(150, 130));
        a.finish_text_adjust();
        let after = a.edit_bounds().expect("框");
        assert_eq!((after.x - before.x, after.y - before.y), (50, 30));
        assert!(a.confirm_text());
        let data = only(&a);
        assert_eq!(data.rotation, 90);
        assert!(data.wrap.is_some());
    }

    #[test]
    fn editing_scale_changes_the_current_text_size() {
        let mut a = Annotator::new(&AnnotateConfig::default());
        a.toggle_tool(Tool::Text);
        a.begin_text(p(100, 100), &MON, 0);
        a.insert_text("hello");
        a.begin_text_adjust(TextHandle::Corner(BoxCorner::BottomRight), p(160, 130));
        a.update_text_adjust(p(220, 160));
        a.finish_text_adjust();
        assert_eq!(a.edit_view().map(|v| v.style.size), Some(48));
        assert!(a.confirm_text());
        assert_eq!(only(&a).size, 48);
    }

    #[test]
    fn reediting_a_rotated_wrapped_text_keeps_it_unchanged() {
        let mut a = with_hello();
        a.select_text(0);
        drag(
            &mut a,
            TextHandle::Edge(BoxEdge::Right),
            p(160, 115),
            p(140, 115),
        );
        drag(&mut a, TextHandle::Rotate, p(130, 70), p(200, 90));
        let before = only(&a);
        let steps = a.document().shapes().len();
        assert!(a.begin_edit_existing(0, 0));
        let view = a.edit_view().expect("编辑中");
        assert_eq!((view.rotation, view.wrap), (before.rotation, before.wrap));
        assert_eq!(view.bounds, before.bounds);
        assert!(!a.confirm_text(), "没有改动时不应多出一步撤销");
        assert_eq!(only(&a), before);
        assert_eq!(a.document().shapes().len(), steps);
    }

    #[test]
    fn editing_rotated_text_keeps_the_anchor_corner_fixed_while_typing() {
        let mut a = with_hello();
        a.select_text(0);
        drag(&mut a, TextHandle::Rotate, p(130, 70), p(230, 115));
        let before = only(&a);
        a.begin_edit_existing(0, 0);
        let anchor = before.anchor();
        a.insert_text(" world");
        let grown = a.edit_view().expect("编辑中").bounds;
        let corner = TextBox {
            bounds: grown,
            rotation: 90,
            vertical: false,
        }
        .corner(BoxCorner::TopLeft);
        assert!((corner.0 - anchor.0).abs() <= 1.0 && (corner.1 - anchor.1).abs() <= 1.0);
        assert!(grown.width > before.bounds.width);
    }

    #[test]
    fn caret_line_is_the_rotated_axis_aligned_box() {
        let mut a = Annotator::new(&AnnotateConfig::default());
        a.toggle_tool(Tool::Text);
        a.begin_text(p(100, 100), &MON, 1);
        a.insert_text("ab");
        let (_, flat, _) = a.caret_line().expect("光标");
        assert_eq!(flat, Rect::new(124, 100, 1, 30));
        drag(&mut a, TextHandle::Rotate, p(112, 60), p(200, 115));
        let (monitor, rotated, vertical) = a.caret_line().expect("光标");
        assert_eq!((monitor, vertical), (1, false));
        // 旋转 90° 后竖线变成横线
        assert!(rotated.width > rotated.height);
    }

    #[test]
    fn vertical_editing_maps_arrow_keys_to_column_and_char_moves() {
        let mut a = Annotator::new(&AnnotateConfig::default());
        a.toggle_tool(Tool::Text);
        a.set_text_vertical(true);
        a.begin_text(p(300, 100), &MON, 0);
        a.insert_text("ab");
        a.edit_key(EditKey::Newline);
        a.insert_text("cd");
        assert_eq!(a.edit_view().map(|v| v.caret), Some(5));
        // 上下键在列内移动一个字符
        a.edit_key(EditKey::Up);
        assert_eq!(a.edit_view().map(|v| v.caret), Some(4));
        a.edit_key(EditKey::Down);
        assert_eq!(a.edit_view().map(|v| v.caret), Some(5));
        // 右键回到上一列，左键换到下一列
        a.edit_key(EditKey::Right);
        assert_eq!(a.edit_view().map(|v| v.caret), Some(2));
        a.edit_key(EditKey::Left);
        assert_eq!(a.edit_view().map(|v| v.caret), Some(5));
        assert!(a.confirm_text());
        assert!(only(&a).vertical);
    }

    #[test]
    fn vertical_new_box_anchors_at_its_top_right_corner() {
        let mut a = Annotator::new(&AnnotateConfig::default());
        a.toggle_tool(Tool::Text);
        a.set_text_vertical(true);
        a.begin_text(p(300, 100), &MON, 0);
        a.insert_text("ab");
        let bounds = a.edit_bounds().expect("框");
        assert_eq!(bounds.right(), 300);
        assert_eq!(bounds.y, 100);
        a.insert_text("cde");
        assert_eq!(a.edit_bounds().map(|b| b.right()), Some(300));
    }

    #[test]
    fn approx_measure_wraps_and_swaps_axes_for_vertical() {
        let m = ApproxMeasure;
        let wrapped = TextLayout {
            wrap: Some(24),
            vertical: false,
        };
        assert_eq!(m.measure("abcdef", 24, wrapped), (24, 90));
        assert_eq!(m.measure("abcdef", 24, TextLayout::default()), (72, 30));
        let vertical = TextLayout {
            wrap: None,
            vertical: true,
        };
        assert_eq!(m.measure("abc", 24, vertical), (30, 36));
        assert_eq!(m.measure("ab\ncd", 24, vertical), (60, 24));
        // 竖排第一列在最右
        assert_eq!(m.caret("ab\ncd", 24, vertical, 0).0, 30);
        assert_eq!(m.caret("ab\ncd", 24, vertical, 3).0, 0);
        assert_eq!(m.caret("abcdef", 24, wrapped, 3), (12, 30, 30));
    }

    #[test]
    fn box_size_applies_wrap_length_and_minimum_inline_size() {
        let a = Annotator::new(&AnnotateConfig::default());
        let flat = TextLayout::default();
        assert_eq!(a.box_size("", 24, false, flat), (12, 30));
        let wrapped = TextLayout {
            wrap: Some(100),
            vertical: false,
        };
        assert_eq!(a.box_size("ab", 24, false, wrapped), (100, 30));
        let tall = TextLayout {
            wrap: Some(100),
            vertical: true,
        };
        assert_eq!(a.box_size("ab", 24, true, tall), (30 + 12, 100 + 12));
        assert_eq!(
            a.box_size(
                "",
                24,
                false,
                TextLayout {
                    wrap: None,
                    vertical: true
                }
            ),
            (30, 12)
        );
    }
}
