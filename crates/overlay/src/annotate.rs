//! 标注状态机：当前工具、各工具样式、正在画的图元和已完成的图元，与 Win32 无关。
//!
//! 图元坐标为虚拟桌面物理像素；所有输入点先限制在选区所在显示器内。

use std::rc::Rc;

use lenswin_core::annotation::{
    Color, Document, NumberStyle, PALETTE, Shape, ShapeHandle, Tool, constrain_square,
    drag_is_valid, hit_selected_shape, palette_index, resize_shape, shape_box, snap_angle,
    step_size, translate_shape,
};
use lenswin_core::config::TEXT_SIZE_RANGE;
use lenswin_core::config::{
    AnnotateConfig, HighlighterConfig, MosaicConfig, NumberConfig, StrokeConfig, TextConfig,
};
use lenswin_core::geometry::{Point, Rect};
use lenswin_core::text_box::{RotatedBox, max_text_size};

mod text;

#[cfg(test)]
pub use text::TextBoxView;
pub use text::{ApproxMeasure, EditKey, EditView, TextLayout, TextMeasure, text_padding};
use text::{EditAdjust, Editing, to_f64};

/// 文字排版方向：决定换行边手柄和旋转手柄的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxAxis {
    /// 横排：左右边调换行，旋转手柄在上方。
    Horizontal,
    /// 竖排：上下边调换行，旋转手柄在右侧。
    Vertical,
}

/// 矩形选中框的类型，决定绘制与可交互的手柄。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxKind {
    /// 矩形、椭圆：8 个缩放手柄 + 旋转手柄。
    Rotatable,
    /// 画笔、荧光笔、马赛克：8 个缩放手柄，无旋转手柄。
    Scalable,
    /// 序号：只有虚线框，无手柄。
    Plain,
    /// 文字：四角缩放 + 两方向换行边手柄 + 旋转手柄。
    Text(BoxAxis),
}

/// 当前选中或编辑中图元的选框与手柄几何。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionBox {
    /// 框形图元（含旋转）。
    Box {
        /// 外接旋转框。
        geometry: RotatedBox,
        /// 框与手柄类型。
        kind: BoxKind,
    },
    /// 直线与箭头：只画两端圆点，不画虚线框。
    Endpoints {
        /// 起点。
        from: Point,
        /// 终点。
        to: Point,
    },
}

/// 文字样式：颜色、字号、是否带底色、是否竖排。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextStyle {
    /// 文字颜色，底色模式下是底色。
    pub color: Color,
    /// 字号。
    pub size: u32,
    /// 是否带底色。
    pub background: bool,
    /// 是否竖排。
    pub vertical: bool,
}

/// 线条类工具的样式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stroke {
    /// 颜色。
    pub color: Color,
    /// 线宽。
    pub width: u32,
}

impl Stroke {
    fn from_hex(color: &str, width: u32, fallback: Color) -> Self {
        Self {
            color: Color::from_hex(color).unwrap_or(fallback),
            width,
        }
    }
}

/// 全部工具当前的样式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Styles {
    rectangle: Stroke,
    ellipse: Stroke,
    line: Stroke,
    arrow: Stroke,
    pen: Stroke,
    highlighter: Stroke,
    mosaic_block: u32,
    number_color: Color,
    number_size: u32,
    number_style: NumberStyle,
    text: TextStyle,
}

impl Styles {
    /// 由配置建立；颜色无法解析时退回默认色（配置校验过，只是兜底）。
    pub fn from_config(config: &AnnotateConfig) -> Self {
        let red = PALETTE[0];
        let stroke = |c: &StrokeConfig| Stroke::from_hex(&c.color, c.width, red);
        Self {
            rectangle: stroke(&config.rectangle),
            ellipse: stroke(&config.ellipse),
            line: stroke(&config.line),
            arrow: stroke(&config.arrow),
            pen: stroke(&config.pen),
            highlighter: Stroke::from_hex(
                &config.highlighter.color,
                config.highlighter.width,
                PALETTE[2],
            ),
            mosaic_block: config.mosaic.block,
            number_color: Color::from_hex(&config.number.color).unwrap_or(red),
            number_size: config.number.size,
            number_style: config.number.style,
            text: TextStyle {
                color: Color::from_hex(&config.text.color).unwrap_or(red),
                size: config.text.size,
                background: config.text.background,
                vertical: config.text.vertical,
            },
        }
    }

    /// 转回配置，颜色统一写成大写十六进制。
    pub fn to_config(&self) -> AnnotateConfig {
        let stroke = |s: &Stroke| StrokeConfig {
            color: s.color.to_hex(),
            width: s.width,
        };
        AnnotateConfig {
            rectangle: stroke(&self.rectangle),
            ellipse: stroke(&self.ellipse),
            line: stroke(&self.line),
            arrow: stroke(&self.arrow),
            pen: stroke(&self.pen),
            highlighter: HighlighterConfig {
                color: self.highlighter.color.to_hex(),
                width: self.highlighter.width,
            },
            mosaic: MosaicConfig {
                block: self.mosaic_block,
            },
            number: NumberConfig {
                color: self.number_color.to_hex(),
                size: self.number_size,
                style: self.number_style,
            },
            text: TextConfig {
                color: self.text.color.to_hex(),
                size: self.text.size,
                background: self.text.background,
                vertical: self.text.vertical,
            },
        }
    }

    fn stroke_mut(&mut self, tool: Tool) -> Option<&mut Stroke> {
        match tool {
            Tool::Rectangle => Some(&mut self.rectangle),
            Tool::Ellipse => Some(&mut self.ellipse),
            Tool::Line => Some(&mut self.line),
            Tool::Arrow => Some(&mut self.arrow),
            Tool::Pen => Some(&mut self.pen),
            Tool::Highlighter => Some(&mut self.highlighter),
            Tool::Mosaic | Tool::Number | Tool::Text => None,
        }
    }

    /// 工具的颜色；马赛克没有。
    pub fn color(&self, tool: Tool) -> Option<Color> {
        match tool {
            Tool::Rectangle => Some(self.rectangle.color),
            Tool::Ellipse => Some(self.ellipse.color),
            Tool::Line => Some(self.line.color),
            Tool::Arrow => Some(self.arrow.color),
            Tool::Pen => Some(self.pen.color),
            Tool::Highlighter => Some(self.highlighter.color),
            Tool::Number => Some(self.number_color),
            Tool::Text => Some(self.text.color),
            Tool::Mosaic => None,
        }
    }

    /// 工具的粗细：线宽、块大小或序号直径。
    pub fn size(&self, tool: Tool) -> u32 {
        match tool {
            Tool::Rectangle => self.rectangle.width,
            Tool::Ellipse => self.ellipse.width,
            Tool::Line => self.line.width,
            Tool::Arrow => self.arrow.width,
            Tool::Pen => self.pen.width,
            Tool::Highlighter => self.highlighter.width,
            Tool::Mosaic => self.mosaic_block,
            Tool::Number => self.number_size,
            Tool::Text => self.text.size,
        }
    }

    /// 文字样式。
    pub fn text(&self) -> TextStyle {
        self.text
    }

    /// 序号的编号风格。
    pub fn number_style(&self) -> NumberStyle {
        self.number_style
    }

    fn set_color(&mut self, tool: Tool, color: Color) {
        if tool == Tool::Number {
            self.number_color = color;
        } else if tool == Tool::Text {
            self.text.color = color;
        } else if let Some(stroke) = self.stroke_mut(tool) {
            stroke.color = color;
        }
    }

    fn set_size(&mut self, tool: Tool, size: u32) {
        match tool {
            Tool::Mosaic => self.mosaic_block = size,
            Tool::Number => self.number_size = size,
            Tool::Text => self.text.size = size,
            _ => {
                if let Some(stroke) = self.stroke_mut(tool) {
                    stroke.width = size;
                }
            }
        }
    }
}

/// 正在画的图元。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Drawing {
    /// 起止点两端的图元。
    Drag { from: Point, to: Point },
    /// 画笔、荧光笔的轨迹。
    Trail(Vec<Point>),
}

/// 正在拖动调整的已选中图元。
#[derive(Debug, Clone, PartialEq)]
struct ShapeAdjust {
    handle: ShapeHandle,
    press: Point,
    base: Shape,
    preview: Option<Shape>,
    moved: bool,
}

/// 标注状态机的互斥状态。
#[derive(Debug, Clone, PartialEq)]
enum State {
    /// 空闲。
    Idle,
    /// 正在绘制新图元。
    Drawing(Drawing),
    /// 已选中一个图元，可选正在拖动调整。
    Selected {
        index: usize,
        adjust: Option<ShapeAdjust>,
    },
    /// 正在编辑文字，可选正在拖动编辑框手柄。
    Editing {
        edit: Editing,
        adjust: Option<EditAdjust>,
    },
}

/// 样式条上当前应显示的内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyleView {
    /// 工具是否有颜色。
    pub has_color: bool,
    /// 当前颜色在色板中的序号；不在色板内为 `None`。
    pub swatch: Option<usize>,
    /// 粗细数值。
    pub size: u32,
    /// 当前粗细等于第几档预设；不等于任何一档为 `None`。
    pub preset: Option<usize>,
    /// 序号工具的编号风格；其它工具为 `None`。
    pub number_style: Option<NumberStyle>,
    /// 文字工具是否带底色；其它工具为 `None`。
    pub background: Option<bool>,
    /// 文字工具是否竖排；其它工具为 `None`。
    pub vertical: Option<bool>,
}

/// 标注状态机。
pub struct Annotator {
    document: Document,
    styles: Styles,
    initial: Styles,
    tool: Option<Tool>,
    state: State,
    /// 文字字号的运行时上限，随选区所在显示器高度变化。
    text_max: u32,
    shift: bool,
    measure: Rc<dyn TextMeasure>,
}

/// 把点限制在显示器范围内。
fn clamp_point(p: Point, bounds: &Rect) -> Point {
    let x = i64::from(p.x)
        .max(i64::from(bounds.x))
        .min(bounds.right() - 1);
    let y = i64::from(p.y)
        .max(i64::from(bounds.y))
        .min(bounds.bottom() - 1);
    Point::new(
        i32::try_from(x).unwrap_or(p.x),
        i32::try_from(y).unwrap_or(p.y),
    )
}

/// 约束后的终点若超出显示器，沿原方向等比缩短，保持正方形或 45° 角不变形。
fn fit_along(from: Point, to: Point, bounds: &Rect) -> Point {
    let dx = i128::from(to.x) - i128::from(from.x);
    let dy = i128::from(to.y) - i128::from(from.y);
    let room_x = if dx >= 0 {
        bounds.right() - 1 - i64::from(from.x)
    } else {
        i64::from(from.x) - i64::from(bounds.x)
    };
    let room_y = if dy >= 0 {
        bounds.bottom() - 1 - i64::from(from.y)
    } else {
        i64::from(from.y) - i64::from(bounds.y)
    };
    let (room_x, room_y) = (i128::from(room_x.max(0)), i128::from(room_y.max(0)));
    let (ax, ay) = (dx.abs(), dy.abs());
    // 取受限更严重的一轴，另一轴按同一比例缩短；超出的一轴位移必大于 0，分母不为 0
    let (num, den) = match (ax > room_x, ay > room_y) {
        (false, false) => return to,
        (true, false) => (room_x, ax),
        (false, true) => (room_y, ay),
        (true, true) if room_x * ay <= room_y * ax => (room_x, ax),
        (true, true) => (room_y, ay),
    };
    let scale = |d: i128| i64::try_from(d * num / den).unwrap_or(0);
    let x = i64::from(from.x) + scale(dx);
    let y = i64::from(from.y) + scale(dy);
    Point::new(
        i32::try_from(x).unwrap_or(from.x),
        i32::try_from(y).unwrap_or(from.y),
    )
}

/// 终点：Shift 时按工具约束，再保证落在显示器内。
fn end_point(tool: Tool, from: Point, raw: Point, shift: bool, bounds: &Rect) -> Point {
    let raw = clamp_point(raw, bounds);
    if !shift {
        return raw;
    }
    match tool {
        Tool::Rectangle | Tool::Ellipse => fit_along(from, constrain_square(from, raw), bounds),
        Tool::Line | Tool::Arrow => fit_along(from, snap_angle(from, raw), bounds),
        _ => raw,
    }
}

impl Annotator {
    /// 以配置里的样式建立，没有工具、没有图元。
    pub fn new(config: &AnnotateConfig) -> Self {
        let styles = Styles::from_config(config);
        Self {
            document: Document::new(),
            initial: styles.clone(),
            styles,
            tool: None,
            state: State::Idle,
            text_max: *TEXT_SIZE_RANGE.end(),
            shift: false,
            measure: Rc::new(ApproxMeasure),
        }
    }

    /// 换上真实的文字度量。
    pub fn set_measure(&mut self, measure: Rc<dyn TextMeasure>) {
        self.measure = measure;
    }

    /// 当前工具。
    pub fn tool(&self) -> Option<Tool> {
        self.tool
    }

    /// 点工具按钮：选中该工具，已选中同一个则退出工具。
    pub fn toggle_tool(&mut self, tool: Tool) {
        self.state = State::Idle;
        self.tool = if self.tool == Some(tool) {
            None
        } else {
            Some(tool)
        };
    }

    /// 退出工具；原本有工具返回 `true`。
    pub fn exit_tool(&mut self) -> bool {
        self.state = State::Idle;
        self.tool.take().is_some()
    }

    /// 正在拖拽绘制。
    pub fn is_drawing(&self) -> bool {
        matches!(self.state, State::Drawing(_))
    }

    /// 放弃正在画的图元。
    pub fn cancel_drawing(&mut self) {
        if self.is_drawing() {
            self.state = State::Idle;
        }
    }

    /// 记录 Shift 是否按下，下一次移动或松开时生效。
    pub fn set_shift(&mut self, down: bool) {
        self.shift = down;
    }

    /// 在显示器 `bounds` 内按下：序号立即放置并选中，画笔/荧光笔等进入拖拽，文字不走此接口。
    pub fn press(&mut self, p: Point, bounds: &Rect) {
        let Some(tool) = self.tool else {
            return;
        };
        let p = clamp_point(p, bounds);
        match tool {
            Tool::Number => {
                let style = self.styles.number_style;
                let value = self.document.next_number(style);
                self.document.push(Shape::Number {
                    center: p,
                    value,
                    style,
                    color: self.styles.number_color,
                    size: self.styles.number_size,
                });
                let index = self.document.shapes().len().saturating_sub(1);
                self.state = State::Selected {
                    index,
                    adjust: None,
                };
            }
            Tool::Pen | Tool::Highlighter => {
                self.state = State::Drawing(Drawing::Trail(vec![p]));
            }
            // 文字由 begin_text 开始编辑，不走拖拽
            Tool::Text => {}
            _ => {
                self.state = State::Drawing(Drawing::Drag { from: p, to: p });
            }
        }
    }

    /// 拖动。
    pub fn drag(&mut self, p: Point, bounds: &Rect) {
        let (Some(tool), State::Drawing(drawing)) = (self.tool, &mut self.state) else {
            return;
        };
        match drawing {
            Drawing::Drag { from, to } => {
                *to = end_point(tool, *from, p, self.shift, bounds);
            }
            Drawing::Trail(points) => {
                let p = clamp_point(p, bounds);
                if points.last() != Some(&p) {
                    points.push(p);
                }
            }
        }
    }

    /// 松开：图元有效就加入文档（画笔、荧光笔之外自动选中）；返回是否新增了图元。
    pub fn release(&mut self, p: Point, bounds: &Rect) -> bool {
        self.drag(p, bounds);
        let Some(tool) = self.tool else {
            self.cancel_drawing();
            return false;
        };
        let drawing = match std::mem::replace(&mut self.state, State::Idle) {
            State::Drawing(drawing) => drawing,
            other => {
                self.state = other;
                return false;
            }
        };
        let Some(shape) = build_shape(tool, &self.styles, &drawing) else {
            return false;
        };
        self.document.push(shape);
        // 画笔与荧光笔连续书写不自动选中，避免下一笔误拖动上一笔
        if !matches!(tool, Tool::Pen | Tool::Highlighter) {
            let index = self.document.shapes().len().saturating_sub(1);
            self.state = State::Selected {
                index,
                adjust: None,
            };
        }
        true
    }

    /// 正在画的图元，用于实时预览。
    pub fn preview(&self) -> Option<Shape> {
        let tool = self.tool?;
        let State::Drawing(drawing) = &self.state else {
            return None;
        };
        build_shape(tool, &self.styles, drawing)
    }

    /// 滚轮调当前工具的粗细，`notches` 为正表示增大；没有工具时无效果。
    pub fn wheel(&mut self, notches: i32) {
        let Some(tool) = self.tool else {
            return;
        };
        let mut spec = tool.size_spec();
        if tool == Tool::Text {
            spec.range = *spec.range.start()..=self.text_max.max(*spec.range.start());
        }
        let next = step_size(self.effective_size(tool), notches, &spec);
        self.styles.set_size(tool, next);
        self.restyle_selected();
    }

    /// 把当前工具的粗细设为第 `index` 档预设；没有工具或序号越界时无效果。
    pub fn apply_preset(&mut self, index: usize) {
        let Some(tool) = self.tool else {
            return;
        };
        if let Some(size) = tool.size_presets().get(index) {
            let size = if tool == Tool::Text {
                (*size).min(self.text_max.max(*TEXT_SIZE_RANGE.start()))
            } else {
                *size
            };
            self.styles.set_size(tool, size);
            self.restyle_selected();
        }
    }

    /// 设置当前工具的颜色；没有工具或工具没有颜色时无效果。
    pub fn set_color(&mut self, color: Color) {
        if let Some(tool) = self.tool.filter(|t| t.has_color()) {
            self.styles.set_color(tool, color);
            self.restyle_selected();
        }
    }

    /// 设置序号的编号风格。
    pub fn set_number_style(&mut self, style: NumberStyle) {
        self.styles.number_style = style;
        self.restyle_selected();
    }

    /// 撤销；有图元被撤销返回 `true`。
    pub fn undo(&mut self) -> bool {
        self.state = State::Idle;
        self.document.undo()
    }

    /// 重做；有图元被恢复返回 `true`。
    pub fn redo(&mut self) -> bool {
        self.state = State::Idle;
        self.document.redo()
    }

    /// 能否撤销。
    pub fn can_undo(&self) -> bool {
        self.document.can_undo()
    }

    /// 能否重做。
    pub fn can_redo(&self) -> bool {
        self.document.can_redo()
    }

    /// 已完成的图元。
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// 样式条的内容；没有工具时为 `None`。
    pub fn style_view(&self) -> Option<StyleView> {
        let tool = self.tool?;
        let color = self.styles.color(tool);
        Some(StyleView {
            has_color: color.is_some(),
            swatch: color.and_then(palette_index),
            size: self.effective_size(tool),
            preset: tool
                .size_presets()
                .iter()
                .position(|v| *v == self.effective_size(tool)),
            number_style: (tool == Tool::Number).then_some(self.styles.number_style),
            background: (tool == Tool::Text).then_some(self.styles.text.background),
            vertical: (tool == Tool::Text).then_some(self.styles.text.vertical),
        })
    }

    /// 按所在显示器高度更新文字字号上限（2 倍屏高，物理像素）。
    pub fn set_text_limit(&mut self, monitor_height: u32) {
        self.text_max = max_text_size(monitor_height);
    }

    /// 工具当前实际使用的粗细：文字字号按运行时上限夹紧，配置里存的值保持不变。
    fn effective_size(&self, tool: Tool) -> u32 {
        let size = self.styles.size(tool);
        if tool == Tool::Text {
            size.min(self.text_max.max(*TEXT_SIZE_RANGE.start()))
        } else {
            size
        }
    }

    /// 样式相对会话开始时有变化才返回新配置。
    pub fn changed_config(&self) -> Option<AnnotateConfig> {
        (self.styles != self.initial).then(|| self.styles.to_config())
    }

    /// 选中的图元序号。
    pub fn selected_shape(&self) -> Option<usize> {
        match self.state {
            State::Selected { index, .. } => Some(index),
            _ => None,
        }
    }

    /// 选中第 `index` 个图元：切换工具并载入样式（旋转与换行除外）；越界返回 `false`。
    pub fn select_shape(&mut self, index: usize) -> bool {
        let Some(shape) = self.document.shapes().get(index).cloned() else {
            return false;
        };
        self.load_shape_style(&shape);
        self.tool = Some(shape.tool());
        self.state = State::Selected {
            index,
            adjust: None,
        };
        true
    }

    /// 取消选中；原本有选中返回 `true`。
    pub fn deselect(&mut self) -> bool {
        if matches!(self.state, State::Selected { .. }) {
            self.state = State::Idle;
            true
        } else {
            false
        }
    }

    /// 删除选中的图元，算一步撤销。
    pub fn delete_selected(&mut self) -> bool {
        let State::Selected { index, .. } = self.state else {
            return false;
        };
        self.state = State::Idle;
        self.document.remove(index)
    }

    /// 把选中的图元移动 `(dx, dy)` 像素，算一步撤销。
    pub fn nudge_selected(&mut self, dx: i32, dy: i32) -> bool {
        let State::Selected { index, .. } = self.state else {
            return false;
        };
        let Some(shape) = self.document.shapes().get(index) else {
            return false;
        };
        self.document.replace(index, translate_shape(shape, dx, dy))
    }

    /// 正在拖动调整图元或编辑框。
    pub fn is_adjusting(&self) -> bool {
        matches!(
            self.state,
            State::Selected {
                adjust: Some(_),
                ..
            } | State::Editing {
                adjust: Some(_),
                ..
            }
        )
    }

    /// 正在拖动的手柄。
    pub fn adjusting_handle(&self) -> Option<ShapeHandle> {
        match &self.state {
            State::Selected {
                adjust: Some(a), ..
            } => Some(a.handle),
            State::Editing {
                adjust: Some(a), ..
            } => Some(ShapeHandle::from(a.handle)),
            _ => None,
        }
    }

    /// 当前应显示的选中框与手柄几何。
    pub fn selection_box(&self) -> Option<SelectionBox> {
        match &self.state {
            State::Editing { edit, .. } => {
                if self.tool != Some(Tool::Text) {
                    return None;
                }
                let data = self.editing_data(edit);
                Some(SelectionBox::Box {
                    geometry: data.text_box().rotated(),
                    kind: BoxKind::Text(axis_of(data.vertical)),
                })
            }
            State::Selected { index, adjust } => {
                let preview = adjust.as_ref().and_then(|a| a.preview.as_ref());
                let shape = preview.or_else(|| self.document.shapes().get(*index))?;
                selection_box_of(shape)
            }
            _ => None,
        }
    }

    /// 屏幕点命中当前选中框或编辑框的哪个部位。
    pub fn shape_handle_at(
        &self,
        p: Point,
        tolerance: f64,
        rotate_distance: f64,
    ) -> Option<ShapeHandle> {
        match &self.state {
            State::Editing { edit, .. } => {
                if self.tool != Some(Tool::Text) {
                    return None;
                }
                self.editing_data(edit)
                    .text_box()
                    .hit(to_f64(p), tolerance, rotate_distance)
                    .map(ShapeHandle::from)
            }
            State::Selected { index, adjust } => {
                let preview = adjust.as_ref().and_then(|a| a.preview.as_ref());
                let shape = preview.or_else(|| self.document.shapes().get(*index))?;
                hit_selected_shape(shape, p, tolerance, rotate_distance)
            }
            _ => None,
        }
    }

    /// 在 `handle` 上按下开始调整；没有可调整的图元或编辑框返回 `false`。
    pub fn begin_adjust(&mut self, handle: ShapeHandle, p: Point) -> bool {
        match &self.state {
            State::Editing { .. } => self.begin_edit_adjust(handle, p),
            State::Selected { index, .. } => {
                let index = *index;
                let Some(base) = self.document.shapes().get(index).cloned() else {
                    return false;
                };
                self.state = State::Selected {
                    index,
                    adjust: Some(ShapeAdjust {
                        handle,
                        press: p,
                        base,
                        preview: None,
                        moved: false,
                    }),
                };
                true
            }
            _ => false,
        }
    }

    /// 拖动选中图元或编辑框的手柄：Shift 时保持宽高比或角度吸附。
    pub fn update_adjust(&mut self, p: Point) {
        if matches!(self.state, State::Editing { .. }) {
            self.update_edit_adjust(p);
            return;
        }
        let State::Selected {
            adjust: Some(adjust),
            ..
        } = &self.state
        else {
            return;
        };
        if !adjust.moved && p == adjust.press {
            return;
        }
        let handle = adjust.handle;
        let press = adjust.press;
        let base = adjust.base.clone();
        let next = match handle {
            ShapeHandle::Body | ShapeHandle::Border => translate_shape(
                &base,
                p.x.saturating_sub(press.x),
                p.y.saturating_sub(press.y),
            ),
            _ if matches!(base, Shape::Text { .. }) => self.resize_selected_text(&base, handle, p),
            _ => resize_shape(&base, handle, p, self.shift),
        };
        if let State::Selected {
            adjust: Some(live), ..
        } = &mut self.state
        {
            live.moved = true;
            live.preview = Some(next);
        }
    }

    /// 松开调整：选中图元的变化记为一步撤销；返回文档是否有变化。
    pub fn finish_adjust(&mut self) -> bool {
        if let State::Editing { adjust, .. } = &mut self.state {
            *adjust = None;
            return false;
        }
        let State::Selected { index, adjust } = &mut self.state else {
            return false;
        };
        let index = *index;
        let Some(adjust) = adjust.take() else {
            return false;
        };
        if !adjust.moved {
            return false;
        }
        let Some(preview) = adjust.preview else {
            return false;
        };
        if preview == adjust.base {
            return false;
        }
        if let (Shape::Text { size, .. }, Shape::Text { size: old_size, .. }) =
            (&preview, &adjust.base)
            && size != old_size
        {
            self.styles.text.size = *size;
        }
        self.document.replace(index, preview)
    }

    /// 拖动中的预览图元；没有拖动选中图元时为 `None`。
    pub fn adjust_preview(&self) -> Option<Shape> {
        match &self.state {
            State::Selected {
                adjust: Some(a), ..
            } => a.preview.clone(),
            _ => None,
        }
    }

    /// 需要隐藏的图元序号：重新编辑的原文字，或正在拖动预览的选中图元。
    pub fn hidden_shape(&self) -> Option<usize> {
        match &self.state {
            State::Editing { edit, .. } => edit.replacing,
            State::Selected {
                index,
                adjust: Some(a),
            } if a.preview.is_some() => Some(*index),
            _ => None,
        }
    }

    fn load_shape_style(&mut self, shape: &Shape) {
        match shape {
            Shape::Rectangle { color, width, .. } => {
                self.styles.rectangle = Stroke {
                    color: *color,
                    width: *width,
                };
            }
            Shape::Ellipse { color, width, .. } => {
                self.styles.ellipse = Stroke {
                    color: *color,
                    width: *width,
                };
            }
            Shape::Line { color, width, .. } => {
                self.styles.line = Stroke {
                    color: *color,
                    width: *width,
                };
            }
            Shape::Arrow { color, width, .. } => {
                self.styles.arrow = Stroke {
                    color: *color,
                    width: *width,
                };
            }
            Shape::Pen { color, width, .. } => {
                self.styles.pen = Stroke {
                    color: *color,
                    width: *width,
                };
            }
            Shape::Highlighter { color, width, .. } => {
                self.styles.highlighter = Stroke {
                    color: *color,
                    width: *width,
                };
            }
            Shape::Mosaic { block, .. } => {
                self.styles.mosaic_block = *block;
            }
            Shape::Number {
                style, color, size, ..
            } => {
                self.styles.number_style = *style;
                self.styles.number_color = *color;
                self.styles.number_size = *size;
            }
            Shape::Text {
                color,
                size,
                background,
                vertical,
                ..
            } => {
                self.styles.text = TextStyle {
                    color: *color,
                    size: *size,
                    background: *background,
                    vertical: *vertical,
                };
            }
        }
    }

    fn apply_style_to_shape(&self, shape: &Shape) -> Shape {
        let mut updated = shape.clone();
        match &mut updated {
            Shape::Rectangle { color, width, .. } => {
                *color = self.styles.rectangle.color;
                *width = self.styles.rectangle.width;
            }
            Shape::Ellipse { color, width, .. } => {
                *color = self.styles.ellipse.color;
                *width = self.styles.ellipse.width;
            }
            Shape::Line { color, width, .. } => {
                *color = self.styles.line.color;
                *width = self.styles.line.width;
            }
            Shape::Arrow { color, width, .. } => {
                *color = self.styles.arrow.color;
                *width = self.styles.arrow.width;
            }
            Shape::Pen { color, width, .. } => {
                *color = self.styles.pen.color;
                *width = self.styles.pen.width;
            }
            Shape::Highlighter { color, width, .. } => {
                *color = self.styles.highlighter.color;
                *width = self.styles.highlighter.width;
            }
            Shape::Mosaic { block, .. } => {
                *block = self.styles.mosaic_block;
            }
            Shape::Number {
                style, color, size, ..
            } => {
                *style = self.styles.number_style;
                *color = self.styles.number_color;
                *size = self.styles.number_size;
            }
            Shape::Text { .. } => {}
        }
        updated
    }

    /// 选中图元应用当前样式并记为一步撤销。
    pub(super) fn restyle_selected(&mut self) {
        let State::Selected {
            index,
            adjust: None,
        } = self.state
        else {
            return;
        };
        let Some(old) = self.document.shapes().get(index).cloned() else {
            return;
        };
        if self.tool != Some(old.tool()) {
            return;
        }
        let next = match &old {
            Shape::Text { .. } => self.restyled_text(&old),
            _ => Some(self.apply_style_to_shape(&old)),
        };
        if let Some(next) = next
            && next != old
        {
            self.document.replace(index, next);
        }
    }
}

fn axis_of(vertical: bool) -> BoxAxis {
    if vertical {
        BoxAxis::Vertical
    } else {
        BoxAxis::Horizontal
    }
}

fn selection_box_of(shape: &Shape) -> Option<SelectionBox> {
    match shape {
        Shape::Line { from, to, .. } | Shape::Arrow { from, to, .. } => {
            Some(SelectionBox::Endpoints {
                from: *from,
                to: *to,
            })
        }
        Shape::Rectangle { .. } | Shape::Ellipse { .. } => Some(SelectionBox::Box {
            geometry: shape_box(shape)?,
            kind: BoxKind::Rotatable,
        }),
        Shape::Pen { .. } | Shape::Highlighter { .. } | Shape::Mosaic { .. } => {
            Some(SelectionBox::Box {
                geometry: shape_box(shape)?,
                kind: BoxKind::Scalable,
            })
        }
        Shape::Number { .. } => Some(SelectionBox::Box {
            geometry: shape_box(shape)?,
            kind: BoxKind::Plain,
        }),
        Shape::Text { vertical, .. } => Some(SelectionBox::Box {
            geometry: shape_box(shape)?,
            kind: BoxKind::Text(axis_of(*vertical)),
        }),
    }
}

/// 由正在画的图元和当前样式生成图元；零尺寸或点数不足时返回 `None`。
fn build_shape(tool: Tool, styles: &Styles, drawing: &Drawing) -> Option<Shape> {
    match (tool, drawing) {
        (Tool::Pen | Tool::Highlighter, Drawing::Trail(points)) => {
            let first = points.first()?;
            if points.len() < 2 || !points.iter().any(|q| drag_is_valid(*first, *q)) {
                return None;
            }
            let stroke = if tool == Tool::Pen {
                styles.pen
            } else {
                styles.highlighter
            };
            let points = points.clone();
            Some(if tool == Tool::Pen {
                Shape::Pen {
                    points,
                    color: stroke.color,
                    width: stroke.width,
                }
            } else {
                Shape::Highlighter {
                    points,
                    color: stroke.color,
                    width: stroke.width,
                }
            })
        }
        (_, Drawing::Drag { from, to }) => {
            let (from, to) = (*from, *to);
            if !drag_is_valid(from, to) {
                return None;
            }
            let has_area = from.x != to.x && from.y != to.y;
            match tool {
                Tool::Rectangle if has_area => Some(Shape::Rectangle {
                    from,
                    to,
                    color: styles.rectangle.color,
                    width: styles.rectangle.width,
                    rotation: 0,
                }),
                Tool::Ellipse if has_area => Some(Shape::Ellipse {
                    from,
                    to,
                    color: styles.ellipse.color,
                    width: styles.ellipse.width,
                    rotation: 0,
                }),
                Tool::Mosaic if has_area => Some(Shape::Mosaic {
                    from,
                    to,
                    block: styles.mosaic_block,
                }),
                Tool::Line => Some(Shape::Line {
                    from,
                    to,
                    color: styles.line.color,
                    width: styles.line.width,
                }),
                Tool::Arrow => Some(Shape::Arrow {
                    from,
                    to,
                    color: styles.arrow.color,
                    width: styles.arrow.width,
                }),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: Rect = Rect::new(0, 0, 1000, 800);

    fn p(x: i32, y: i32) -> Point {
        Point::new(x, y)
    }

    fn annotator() -> Annotator {
        Annotator::new(&AnnotateConfig::default())
    }

    fn with_tool(tool: Tool) -> Annotator {
        let mut a = annotator();
        a.toggle_tool(tool);
        a
    }

    fn drag(a: &mut Annotator, from: Point, to: Point) -> bool {
        a.press(from, &MON);
        a.drag(to, &MON);
        a.release(to, &MON)
    }

    #[test]
    fn tool_toggles_and_exits() {
        let mut a = annotator();
        assert_eq!(a.tool(), None);
        a.toggle_tool(Tool::Arrow);
        assert_eq!(a.tool(), Some(Tool::Arrow));
        a.toggle_tool(Tool::Pen);
        assert_eq!(a.tool(), Some(Tool::Pen));
        a.toggle_tool(Tool::Pen);
        assert_eq!(a.tool(), None);
        assert!(!a.exit_tool());
        a.toggle_tool(Tool::Line);
        assert!(a.exit_tool());
        assert_eq!(a.tool(), None);
    }

    #[test]
    fn no_tool_means_no_drawing_wheel_or_color() {
        let mut a = annotator();
        a.press(p(10, 10), &MON);
        assert!(!a.is_drawing());
        a.wheel(5);
        a.set_color(PALETTE[4]);
        assert_eq!(a.changed_config(), None);
        assert_eq!(a.style_view(), None);
        assert_eq!(a.preview(), None);
    }

    #[test]
    fn every_drag_tool_produces_its_shape() {
        for (tool, expect) in [
            (Tool::Rectangle, Tool::Rectangle),
            (Tool::Ellipse, Tool::Ellipse),
            (Tool::Line, Tool::Line),
            (Tool::Arrow, Tool::Arrow),
            (Tool::Mosaic, Tool::Mosaic),
        ] {
            let mut a = with_tool(tool);
            assert!(drag(&mut a, p(100, 100), p(200, 180)), "{tool:?}");
            assert_eq!(a.document().shapes().len(), 1);
            assert_eq!(a.document().shapes().first().map(Shape::tool), Some(expect));
            assert!(!a.is_drawing());
        }
    }

    #[test]
    fn trail_tools_record_points_and_drop_duplicates() {
        for tool in [Tool::Pen, Tool::Highlighter] {
            let mut a = with_tool(tool);
            a.press(p(10, 10), &MON);
            for q in [p(10, 10), p(12, 10), p(12, 10), p(15, 14)] {
                a.drag(q, &MON);
            }
            assert!(a.release(p(15, 14), &MON));
            match a.document().shapes().first() {
                Some(Shape::Pen { points, .. } | Shape::Highlighter { points, .. }) => {
                    assert_eq!(points, &vec![p(10, 10), p(12, 10), p(15, 14)]);
                }
                other => panic!("期望轨迹图元，实际为 {other:?}"),
            }
        }
    }

    #[test]
    fn click_without_movement_creates_nothing_for_drawn_tools() {
        for tool in [
            Tool::Rectangle,
            Tool::Ellipse,
            Tool::Line,
            Tool::Arrow,
            Tool::Pen,
            Tool::Highlighter,
            Tool::Mosaic,
        ] {
            let mut a = with_tool(tool);
            assert!(!drag(&mut a, p(50, 50), p(50, 50)), "{tool:?}");
            assert!(a.document().is_empty(), "{tool:?}");
        }
        for tool in [
            Tool::Rectangle,
            Tool::Ellipse,
            Tool::Line,
            Tool::Arrow,
            Tool::Mosaic,
        ] {
            let mut a = with_tool(tool);
            assert!(!drag(&mut a, p(50, 50), p(51, 51)), "{tool:?}");
            assert!(a.document().is_empty(), "{tool:?}");
        }
    }

    #[test]
    fn flat_box_is_dropped_but_line_is_kept() {
        let mut a = with_tool(Tool::Rectangle);
        assert!(!drag(&mut a, p(10, 10), p(80, 10)));
        a.toggle_tool(Tool::Mosaic);
        assert!(!drag(&mut a, p(10, 10), p(10, 80)));
        a.toggle_tool(Tool::Line);
        assert!(drag(&mut a, p(10, 10), p(80, 10)));
    }

    #[test]
    fn number_places_on_press_and_counts_per_style() {
        let mut a = with_tool(Tool::Number);
        a.press(p(100, 100), &MON);
        a.release(p(100, 100), &MON);
        a.press(p(200, 100), &MON);
        a.release(p(200, 100), &MON);
        a.deselect();
        a.set_number_style(NumberStyle::Letter);
        a.press(p(300, 100), &MON);
        a.release(p(300, 100), &MON);
        a.deselect();
        a.set_number_style(NumberStyle::Number);
        a.press(p(400, 100), &MON);
        let values: Vec<_> = a
            .document()
            .shapes()
            .iter()
            .filter_map(|s| match s {
                Shape::Number { value, style, .. } => Some((*value, *style)),
                _ => None,
            })
            .collect();
        assert_eq!(
            values,
            vec![
                (1, NumberStyle::Number),
                (2, NumberStyle::Number),
                (1, NumberStyle::Letter),
                (3, NumberStyle::Number)
            ]
        );
        assert!(a.undo());
        a.press(p(500, 100), &MON);
        assert_eq!(a.document().next_number(NumberStyle::Number), 4);
    }

    #[test]
    fn points_are_clamped_to_monitor() {
        let mut a = with_tool(Tool::Line);
        assert!(drag(&mut a, p(-50, -50), p(5000, 5000)));
        assert_eq!(
            a.document().shapes().first(),
            Some(&Shape::Line {
                from: p(0, 0),
                to: p(999, 799),
                color: PALETTE[0],
                width: 3
            })
        );
        let mut n = with_tool(Tool::Number);
        n.press(p(-10, 9999), &MON);
        match n.document().shapes().first() {
            Some(Shape::Number { center, .. }) => assert_eq!(*center, p(0, 799)),
            other => panic!("期望序号，实际为 {other:?}"),
        }
    }

    #[test]
    fn shift_makes_squares_and_snaps_lines_but_not_pens() {
        let mut a = with_tool(Tool::Rectangle);
        a.set_shift(true);
        a.press(p(100, 100), &MON);
        a.drag(p(180, 130), &MON);
        assert_eq!(
            a.preview(),
            Some(Shape::Rectangle {
                from: p(100, 100),
                to: p(180, 180),
                color: PALETTE[0],
                width: 3,
                rotation: 0,
            })
        );
        a.set_shift(false);
        a.drag(p(180, 130), &MON);
        assert!(matches!(a.preview(), Some(Shape::Rectangle { to, .. }) if to == p(180, 130)));
        a.cancel_drawing();

        let mut e = with_tool(Tool::Ellipse);
        e.set_shift(true);
        assert!(drag(&mut e, p(300, 300), p(250, 100)));
        assert!(matches!(
            e.document().shapes().first(),
            Some(Shape::Ellipse { to, .. }) if *to == p(100, 100)
        ));

        let mut l = with_tool(Tool::Arrow);
        l.set_shift(true);
        assert!(drag(&mut l, p(100, 100), p(300, 120)));
        assert!(matches!(
            l.document().shapes().first(),
            Some(Shape::Arrow { to, .. }) if *to == p(300, 100)
        ));

        let mut pen = with_tool(Tool::Pen);
        pen.set_shift(true);
        pen.press(p(100, 100), &MON);
        pen.drag(p(130, 107), &MON);
        assert!(matches!(
            pen.preview(),
            Some(Shape::Pen { points, .. }) if points == vec![p(100, 100), p(130, 107)]
        ));
    }

    #[test]
    fn shift_square_shrinks_to_fit_monitor() {
        let mut a = with_tool(Tool::Rectangle);
        a.set_shift(true);
        // 向右下拖，纵向只剩 700 的空间：边长缩到 699 仍是正方形
        assert!(drag(&mut a, p(100, 100), p(900, 150)));
        assert!(matches!(
            a.document().shapes().first(),
            Some(Shape::Rectangle { from, to, .. })
                if to.x - from.x == to.y - from.y && to.y <= 799 && to.x <= 999
        ));
        let mut l = with_tool(Tool::Line);
        l.set_shift(true);
        assert!(drag(&mut l, p(900, 700), p(990, 790)));
        assert!(matches!(
            l.document().shapes().first(),
            Some(Shape::Line { to, .. }) if to.x <= 999 && to.y <= 799 && to.x - 900 == to.y - 700
        ));
    }

    #[test]
    fn wheel_adjusts_size_per_tool_within_range() {
        let mut a = with_tool(Tool::Rectangle);
        a.wheel(2);
        assert_eq!(a.style_view().map(|v| v.size), Some(5));
        a.wheel(-100);
        assert_eq!(a.style_view().map(|v| v.size), Some(1));
        a.toggle_tool(Tool::Highlighter);
        a.wheel(1);
        assert_eq!(a.style_view().map(|v| v.size), Some(18));
        a.toggle_tool(Tool::Mosaic);
        a.wheel(1);
        assert_eq!(a.style_view().map(|v| v.size), Some(14));
        a.toggle_tool(Tool::Number);
        a.wheel(-1);
        assert_eq!(a.style_view().map(|v| v.size), Some(26));
        a.toggle_tool(Tool::Ellipse);
        assert_eq!(a.style_view().map(|v| v.size), Some(3));
    }

    #[test]
    fn presets_set_size_and_mark_the_matching_one() {
        let mut a = with_tool(Tool::Rectangle);
        assert_eq!(a.style_view().and_then(|v| v.preset), None);
        a.apply_preset(2);
        let view = a.style_view().expect("样式条");
        assert_eq!((view.size, view.preset), (8, Some(2)));
        a.wheel(1);
        assert_eq!(a.style_view().and_then(|v| v.preset), None);
        a.apply_preset(0);
        assert_eq!(a.style_view().map(|v| v.size), Some(2));
        assert_eq!(a.changed_config().map(|c| c.rectangle.width), Some(2));
        a.apply_preset(9);
        assert_eq!(a.style_view().map(|v| v.size), Some(2));
        for (tool, expect) in [
            (Tool::Highlighter, 20),
            (Tool::Mosaic, 16),
            (Tool::Number, 28),
        ] {
            a.toggle_tool(tool);
            a.apply_preset(1);
            let view = a.style_view().expect("样式条");
            assert_eq!((view.size, view.preset), (expect, Some(1)), "{tool:?}");
        }
        let config = a.changed_config().expect("样式变化");
        assert_eq!(config.highlighter.width, 20);
        assert_eq!(config.mosaic.block, 16);
        assert_eq!(config.number.size, 28);
        let mut none = annotator();
        none.apply_preset(1);
        assert_eq!(none.changed_config(), None);
    }

    fn text_annotator() -> Annotator {
        with_tool(Tool::Text)
    }

    fn type_text(a: &mut Annotator, text: &str) {
        a.insert_text(text);
    }

    fn only_text(a: &Annotator) -> (String, Rect) {
        match a.document().shapes() {
            [Shape::Text { text, bounds, .. }] => (text.clone(), *bounds),
            other => panic!("期望一个文字图元，实际为 {other:?}"),
        }
    }

    #[test]
    fn new_text_is_created_on_confirm_with_measured_bounds() {
        let mut a = text_annotator();
        a.begin_text(p(100, 100), &MON, 0);
        assert!(a.is_editing());
        type_text(&mut a, "ab\ncde");
        assert!(a.confirm_text());
        assert!(!a.is_editing());
        let (text, bounds) = only_text(&a);
        assert_eq!(text, "ab\ncde");
        // 字号 24：字符宽 12、行高 30，最宽一行 3 个字符、共 2 行
        assert_eq!(bounds, Rect::new(100, 100, 36, 60));
        assert!(a.can_undo());
    }

    #[test]
    fn background_adds_padding_to_bounds() {
        let mut a = text_annotator();
        a.set_text_background(true);
        a.begin_text(p(10, 10), &MON, 0);
        type_text(&mut a, "ab");
        a.confirm_text();
        // 内边距 24 / 4 = 6
        assert_eq!(only_text(&a).1, Rect::new(10, 10, 24 + 12, 30 + 12));
        assert!(matches!(
            a.document().shapes().first(),
            Some(Shape::Text {
                background: true,
                ..
            })
        ));
    }

    #[test]
    fn blank_new_box_produces_nothing() {
        for content in ["", "   ", " \n "] {
            let mut a = text_annotator();
            a.begin_text(p(5, 5), &MON, 0);
            type_text(&mut a, content);
            if content.contains('\n') {
                a.edit_key(EditKey::Newline);
            }
            assert!(!a.confirm_text(), "{content:?}");
            assert!(a.document().is_empty());
            assert!(!a.can_undo());
        }
    }

    #[test]
    fn discard_drops_new_box_and_keeps_original_when_reediting() {
        let mut a = text_annotator();
        a.begin_text(p(5, 5), &MON, 0);
        type_text(&mut a, "x");
        a.edit_key(EditKey::Discard);
        assert!(!a.is_editing() && a.document().is_empty());

        a.begin_text(p(5, 5), &MON, 0);
        type_text(&mut a, "keep");
        a.confirm_text();
        assert!(a.begin_edit_existing(0, 0));
        assert_eq!(a.edit_view().map(|v| v.hidden), Some(Some(0)));
        a.edit_key(EditKey::Backspace);
        a.discard_text();
        assert_eq!(only_text(&a).0, "keep");
    }

    #[test]
    fn reedit_replaces_in_one_undo_step() {
        let mut a = text_annotator();
        a.begin_text(p(5, 5), &MON, 0);
        type_text(&mut a, "old");
        a.confirm_text();
        assert!(a.begin_edit_existing(0, 0));
        a.edit_key(EditKey::Backspace);
        type_text(&mut a, "ER");
        assert!(a.confirm_text());
        assert_eq!(only_text(&a).0, "olER");
        assert!(a.undo());
        assert_eq!(only_text(&a).0, "old");
        assert!(a.redo());
        assert_eq!(only_text(&a).0, "olER");
    }

    #[test]
    fn reedit_without_change_is_not_an_undo_step() {
        let mut a = text_annotator();
        a.begin_text(p(5, 5), &MON, 0);
        type_text(&mut a, "same");
        a.confirm_text();
        assert!(a.begin_edit_existing(0, 0));
        assert!(!a.confirm_text());
        assert!(a.undo());
        assert!(a.document().is_empty());
    }

    #[test]
    fn reedit_to_blank_removes_and_undo_restores() {
        let mut a = text_annotator();
        a.begin_text(p(5, 5), &MON, 0);
        type_text(&mut a, "gone");
        a.confirm_text();
        assert!(a.begin_edit_existing(0, 0));
        for _ in 0..4 {
            a.edit_key(EditKey::Backspace);
        }
        assert!(a.confirm_text());
        assert!(a.document().is_empty());
        assert!(a.undo());
        assert_eq!(only_text(&a).0, "gone");
    }

    #[test]
    fn begin_edit_existing_rejects_non_text_and_loads_style() {
        let mut a = with_tool(Tool::Line);
        assert!(drag(&mut a, p(10, 10), p(100, 100)));
        assert!(!a.begin_edit_existing(0, 0));
        assert!(!a.begin_edit_existing(9, 0));
        a.toggle_tool(Tool::Text);
        a.set_color(PALETTE[4]);
        a.set_text_background(true);
        a.wheel(3);
        a.begin_text(p(5, 5), &MON, 0);
        type_text(&mut a, "s");
        a.confirm_text();
        a.set_color(PALETTE[1]);
        a.set_text_background(false);
        assert!(a.begin_edit_existing(1, 0));
        let view = a.edit_view().expect("编辑中");
        assert_eq!(view.style.color, PALETTE[4]);
        assert!(view.style.background);
        assert_eq!(view.style.size, 30);
    }

    #[test]
    fn style_changes_apply_to_the_box_being_edited() {
        let mut a = text_annotator();
        a.begin_text(p(5, 5), &MON, 0);
        type_text(&mut a, "hi");
        a.apply_preset(2);
        a.set_color(PALETTE[3]);
        a.set_text_background(true);
        a.confirm_text();
        match a.document().shapes() {
            [
                Shape::Text {
                    size,
                    color,
                    background,
                    ..
                },
            ] => {
                assert_eq!((*size, *color, *background), (36, PALETTE[3], true));
            }
            other => panic!("期望文字图元，实际为 {other:?}"),
        }
        let config = a.changed_config().expect("样式变化");
        assert_eq!(config.text.size, 36);
        assert_eq!(config.text.color, "#34C759");
        assert!(config.text.background);
    }

    #[test]
    fn edit_keys_move_and_delete() {
        let mut a = text_annotator();
        a.begin_text(p(5, 5), &MON, 0);
        type_text(&mut a, "abc");
        a.edit_key(EditKey::Left);
        a.edit_key(EditKey::Backspace);
        a.edit_key(EditKey::Delete);
        assert_eq!(a.edit_view().map(|v| v.text), Some("a".to_string()));
        a.edit_key(EditKey::Newline);
        type_text(&mut a, "xy");
        a.edit_key(EditKey::Up);
        a.edit_key(EditKey::End);
        a.edit_key(EditKey::Tab);
        a.edit_key(EditKey::Home);
        a.edit_key(EditKey::Right);
        a.edit_key(EditKey::Down);
        assert_eq!(a.edit_view().map(|v| v.text), Some("a    \nxy".to_string()));
        assert_eq!(a.edit_view().map(|v| v.caret), Some(7));
    }

    #[test]
    fn confirm_key_confirms_and_edit_keys_without_editing_do_nothing() {
        let mut a = text_annotator();
        assert!(!a.edit_key(EditKey::Left));
        assert!(!a.insert_char('x'));
        assert!(!a.insert_text("x"));
        a.begin_text(p(5, 5), &MON, 0);
        type_text(&mut a, "ok");
        a.edit_key(EditKey::Confirm);
        assert!(!a.is_editing());
        assert_eq!(only_text(&a).0, "ok");
    }

    #[test]
    fn insert_text_filters_controls_and_char_ignores_them() {
        let mut a = text_annotator();
        a.begin_text(p(5, 5), &MON, 0);
        assert!(!a.insert_char('\u{7}'));
        assert!(a.insert_text("a\nb\u{1}c"));
        assert!(!a.insert_text("\u{1}"));
        assert_eq!(a.edit_view().map(|v| v.text), Some("a\nbc".to_string()));
    }

    #[test]
    fn composition_is_displayed_inline_and_dropped_on_confirm() {
        let mut a = text_annotator();
        a.begin_text(p(5, 5), &MON, 0);
        type_text(&mut a, "ab");
        a.edit_key(EditKey::Left);
        a.set_composition("ni😀", 2);
        let view = a.edit_view().expect("编辑中");
        assert_eq!(view.text, "ani😀b");
        // 组字串从 UTF-16 偏移 1 起，长 4 个单元
        assert_eq!(view.composition, Some((1, 4)));
        // 光标在组字串内第 2 个字符后：缓冲光标 1 + 2
        assert_eq!(view.caret, 3);
        a.set_composition("", 0);
        assert_eq!(a.edit_view().map(|v| v.text), Some("ab".to_string()));
        a.set_composition("zz", 9);
        assert_eq!(a.edit_view().map(|v| v.caret), Some(3));
        a.confirm_text();
        assert_eq!(only_text(&a).0, "ab");
    }

    #[test]
    fn composition_result_inserts_like_normal_input() {
        let mut a = text_annotator();
        a.begin_text(p(5, 5), &MON, 0);
        a.set_composition("zhong", 5);
        a.insert_text("中");
        a.clear_composition();
        assert_eq!(
            a.edit_view().map(|v| (v.text, v.composition)),
            Some(("中".to_string(), None))
        );
    }

    #[test]
    fn edit_bounds_has_a_minimum_width_and_follows_text() {
        let mut a = text_annotator();
        a.begin_text(p(100, 50), &MON, 0);
        assert_eq!(a.edit_bounds(), Some(Rect::new(100, 50, 12, 30)));
        type_text(&mut a, "abcd");
        assert_eq!(a.edit_bounds(), Some(Rect::new(100, 50, 48, 30)));
        a.set_composition("xx", 2);
        assert_eq!(a.edit_bounds().map(|r| r.width), Some(72));
        a.discard_text();
        assert_eq!(a.edit_bounds(), None);
    }

    #[test]
    fn caret_line_follows_the_cursor_and_padding() {
        let mut a = text_annotator();
        assert_eq!(a.caret_line(), None);
        a.begin_text(p(100, 50), &MON, 1);
        type_text(&mut a, "ab");
        a.edit_key(EditKey::Newline);
        type_text(&mut a, "c");
        assert_eq!(a.caret_line(), Some((1, Rect::new(112, 80, 1, 30), false)));
        a.set_text_background(true);
        assert_eq!(a.caret_line(), Some((1, Rect::new(118, 86, 1, 30), false)));
    }

    #[test]
    fn text_origin_is_clamped_to_the_monitor() {
        let mut a = text_annotator();
        a.begin_text(p(-40, 5000), &MON, 0);
        assert_eq!(
            a.edit_view().map(|v| (v.bounds.x, v.bounds.y)),
            Some((0, 799))
        );
    }

    #[test]
    fn leaving_the_tool_or_undoing_ends_editing() {
        for action in 0..3 {
            let mut a = text_annotator();
            a.begin_text(p(5, 5), &MON, 0);
            type_text(&mut a, "x");
            match action {
                0 => a.toggle_tool(Tool::Pen),
                1 => {
                    a.exit_tool();
                }
                _ => {
                    a.undo();
                }
            }
            assert!(!a.is_editing(), "{action}");
            assert!(a.document().is_empty());
        }
    }

    #[test]
    fn text_tool_does_not_draw_and_shows_background_switch() {
        let mut a = text_annotator();
        a.press(p(10, 10), &MON);
        assert!(!a.is_drawing());
        let view = a.style_view().expect("样式条");
        assert_eq!(view.background, Some(false));
        assert_eq!(view.size, 24);
        assert_eq!(view.preset, Some(1));
        a.toggle_tool(Tool::Pen);
        assert_eq!(a.style_view().and_then(|v| v.background), None);
    }

    #[test]
    fn editing_blank_new_reports_only_new_blank_boxes() {
        let mut a = text_annotator();
        assert!(!a.editing_blank_new());
        a.begin_text(p(5, 5), &MON, 0);
        assert!(a.editing_blank_new());
        type_text(&mut a, "a");
        assert!(!a.editing_blank_new());
        a.confirm_text();
        a.begin_edit_existing(0, 0);
        a.edit_key(EditKey::Backspace);
        assert!(!a.editing_blank_new(), "重新编辑不算新文本框");
    }

    #[test]
    fn approx_measure_handles_empty_and_multiline() {
        let m = ApproxMeasure;
        assert_eq!(m.measure("", 24, TextLayout::default()), (0, 30));
        assert_eq!(m.measure("a\nbcd\n", 20, TextLayout::default()), (30, 75));
        assert_eq!(
            m.caret("a\nbcd", 20, TextLayout::default(), 4),
            (20, 25, 25)
        );
        assert_eq!(m.caret("abc", 20, TextLayout::default(), 99), (30, 0, 25));
    }

    #[test]
    fn color_applies_to_current_tool_only_and_mosaic_ignores_it() {
        let mut a = with_tool(Tool::Pen);
        a.set_color(PALETTE[4]);
        assert_eq!(a.style_view().and_then(|v| v.swatch), Some(4));
        a.toggle_tool(Tool::Line);
        assert_eq!(a.style_view().and_then(|v| v.swatch), Some(0));
        a.toggle_tool(Tool::Mosaic);
        a.set_color(PALETTE[5]);
        let view = a.style_view().expect("样式条");
        assert!(!view.has_color && view.swatch.is_none());
        assert_eq!(
            a.changed_config().map(|c| c.pen.color),
            Some("#007AFF".to_string())
        );
        assert_eq!(
            a.changed_config().map(|c| c.line.color),
            Some("#FF3B30".to_string())
        );
    }

    #[test]
    fn style_view_marks_number_style_and_off_palette_color() {
        let mut config = AnnotateConfig::default();
        config.number.color = "#123456".to_string();
        let mut a = Annotator::new(&config);
        a.toggle_tool(Tool::Number);
        let view = a.style_view().expect("样式条");
        assert_eq!(view.swatch, None);
        assert_eq!(view.number_style, Some(NumberStyle::Number));
        a.set_number_style(NumberStyle::Letter);
        assert_eq!(
            a.style_view().and_then(|v| v.number_style),
            Some(NumberStyle::Letter)
        );
        a.toggle_tool(Tool::Pen);
        assert_eq!(a.style_view().and_then(|v| v.number_style), None);
    }

    #[test]
    fn changed_config_only_when_styles_differ() {
        let mut a = with_tool(Tool::Rectangle);
        assert_eq!(a.changed_config(), None);
        a.wheel(1);
        a.wheel(-1);
        assert_eq!(a.changed_config(), None, "改回原值不算变化");
        a.wheel(1);
        let changed = a.changed_config().expect("样式变化");
        assert_eq!(changed.rectangle.width, 4);
        assert_eq!(changed.pen, StrokeConfig::default());
    }

    #[test]
    fn lowercase_config_colors_do_not_count_as_changes() {
        let mut config = AnnotateConfig::default();
        config.rectangle.color = "#ff3b30".to_string();
        let a = Annotator::new(&config);
        assert_eq!(a.changed_config(), None);
    }

    #[test]
    fn invalid_config_color_falls_back() {
        let mut config = AnnotateConfig::default();
        config.highlighter.color = "oops".to_string();
        let styles = Styles::from_config(&config);
        assert_eq!(styles.color(Tool::Highlighter), Some(PALETTE[2]));
    }

    #[test]
    fn undo_redo_flow_and_new_shape_clears_redo() {
        let mut a = with_tool(Tool::Line);
        assert!(!a.can_undo() && !a.can_redo());
        assert!(drag(&mut a, p(10, 10), p(100, 100)));
        assert!(drag(&mut a, p(10, 50), p(100, 150)));
        assert!(a.undo());
        assert!(a.can_redo());
        assert!(a.redo());
        assert!(a.undo());
        assert!(drag(&mut a, p(20, 20), p(120, 120)));
        assert!(!a.can_redo());
        assert_eq!(a.document().shapes().len(), 2);
    }

    #[test]
    fn exit_tool_and_undo_cancel_drawing() {
        let mut a = with_tool(Tool::Rectangle);
        a.press(p(10, 10), &MON);
        assert!(a.is_drawing());
        assert!(a.exit_tool());
        assert!(!a.is_drawing());
        assert!(a.document().is_empty());
        a.toggle_tool(Tool::Pen);
        a.press(p(10, 10), &MON);
        a.undo();
        assert!(!a.is_drawing());
    }

    #[test]
    fn preview_matches_final_shape() {
        let mut a = with_tool(Tool::Ellipse);
        a.press(p(10, 10), &MON);
        assert_eq!(a.preview(), None);
        a.drag(p(90, 70), &MON);
        let preview = a.preview().expect("预览");
        assert!(a.release(p(90, 70), &MON));
        assert_eq!(a.document().shapes().first(), Some(&preview));
        assert_eq!(a.preview(), None);
    }

    #[test]
    fn release_without_drawing_is_harmless() {
        let mut a = with_tool(Tool::Rectangle);
        assert!(!a.release(p(10, 10), &MON));
        a.drag(p(20, 20), &MON);
        assert!(a.document().is_empty());
    }

    #[test]
    fn negative_coordinate_monitor_is_respected() {
        let left = Rect::new(-1920, 0, 1920, 1080);
        let mut a = with_tool(Tool::Line);
        a.press(p(-100, 10), &left);
        a.drag(p(50, 50), &left);
        assert!(a.release(p(50, 50), &left));
        assert!(matches!(
            a.document().shapes().first(),
            Some(Shape::Line { to, .. }) if *to == p(-1, 50)
        ));
    }

    #[test]
    fn auto_selection_applies_to_shapes_except_pen_and_highlighter() {
        for tool in [
            Tool::Rectangle,
            Tool::Ellipse,
            Tool::Line,
            Tool::Arrow,
            Tool::Mosaic,
        ] {
            let mut a = with_tool(tool);
            assert!(drag(&mut a, p(100, 100), p(200, 160)), "{tool:?}");
            assert_eq!(a.selected_shape(), Some(0), "{tool:?}");
            assert!(a.selection_box().is_some(), "{tool:?}");
        }
        let mut num = with_tool(Tool::Number);
        num.press(p(150, 150), &MON);
        assert_eq!(num.selected_shape(), Some(0));
        assert!(matches!(
            num.selection_box(),
            Some(SelectionBox::Box {
                kind: BoxKind::Plain,
                ..
            })
        ));
        for tool in [Tool::Pen, Tool::Highlighter] {
            let mut a = with_tool(tool);
            assert!(drag(&mut a, p(100, 100), p(180, 140)), "{tool:?}");
            assert_eq!(a.selected_shape(), None, "{tool:?}");
            assert_eq!(a.selection_box(), None, "{tool:?}");
        }
    }

    #[test]
    fn select_shape_switches_tool_loads_style_and_keeps_rotation_out_of_config() {
        let mut a = with_tool(Tool::Rectangle);
        a.set_color(PALETTE[4]);
        a.wheel(2);
        assert!(drag(&mut a, p(100, 100), p(200, 160)));
        // 旋转一次，再换工具和样式后重新选中该矩形
        assert!(a.begin_adjust(ShapeHandle::Rotate, p(150, 76)));
        a.update_adjust(p(200, 130));
        assert!(a.finish_adjust());
        a.toggle_tool(Tool::Line);
        a.set_color(PALETTE[2]);
        assert!(a.select_shape(0));
        assert_eq!(a.tool(), Some(Tool::Rectangle));
        assert_eq!(a.selected_shape(), Some(0));
        let style = a.style_view().expect("样式条");
        assert_eq!(style.swatch, Some(4));
        assert_eq!(style.size, 5);
        assert!(!a.select_shape(99));
    }

    #[test]
    fn restyling_selected_shape_updates_document_in_one_undo_step() {
        let mut a = with_tool(Tool::Number);
        a.press(p(120, 120), &MON);
        a.set_color(PALETTE[5]);
        a.set_number_style(NumberStyle::Letter);
        a.apply_preset(2);
        assert!(matches!(
            a.document().shapes().first(),
            Some(Shape::Number {
                style: NumberStyle::Letter,
                color,
                size: 40,
                ..
            }) if *color == PALETTE[5]
        ));
        assert_eq!(a.selected_shape(), Some(0));
        assert!(a.undo());
        assert_eq!(a.selected_shape(), None);
        assert!(matches!(
            a.document().shapes().first(),
            Some(Shape::Number {
                style: NumberStyle::Letter,
                size: 28,
                ..
            })
        ));
    }

    #[test]
    fn adjust_preview_hides_original_and_commits_on_finish() {
        let mut a = with_tool(Tool::Rectangle);
        assert!(drag(&mut a, p(100, 100), p(200, 160)));
        assert!(a.begin_adjust(ShapeHandle::Body, p(100, 100)));
        // 没有移动不记撤销
        assert!(!a.finish_adjust());
        assert_eq!(a.hidden_shape(), None);

        assert!(a.begin_adjust(ShapeHandle::Body, p(120, 120)));
        a.update_adjust(p(150, 140));
        assert!(a.is_adjusting());
        assert_eq!(a.hidden_shape(), Some(0));
        assert!(a.adjust_preview().is_some());
        assert!(a.finish_adjust());
        assert!(!a.is_adjusting());
        assert_eq!(a.hidden_shape(), None);
        assert_eq!(a.adjust_preview(), None);
        assert!(matches!(
            a.document().shapes().first(),
            Some(Shape::Rectangle { from, to, .. })
                if *from == p(130, 120) && *to == p(230, 180)
        ));
    }

    #[test]
    fn rotate_and_resize_shape_through_annotator() {
        let mut a = with_tool(Tool::Ellipse);
        assert!(drag(&mut a, p(100, 100), p(200, 200)));
        a.set_shift(true);
        assert!(a.begin_adjust(ShapeHandle::Rotate, p(150, 76)));
        assert_eq!(a.adjusting_handle(), Some(ShapeHandle::Rotate));
        a.update_adjust(p(200, 130));
        assert!(a.finish_adjust());
        let rot = match a.document().shapes().first() {
            Some(Shape::Ellipse { rotation, .. }) => *rotation,
            other => panic!("期望椭圆，实际为 {other:?}"),
        };
        assert_eq!(rot % 15, 0);
        assert!(a.delete_selected());
        assert!(a.document().is_empty());
        assert_eq!(a.selected_shape(), None);
        assert!(a.undo());
        assert_eq!(a.document().shapes().len(), 1);
    }
}
