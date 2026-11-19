//! 原地标注的纯数据：工具、颜色、图元、撤销重做，以及约束、箭头、马赛克、序号的几何计算。
//!
//! 图元坐标一律为虚拟桌面物理像素，线宽以成图物理像素为单位。

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use crate::config::{
    ANNOTATE_WIDTH_RANGE, HIGHLIGHTER_WIDTH_RANGE, MOSAIC_BLOCK_RANGE, NUMBER_SIZE_RANGE,
    TEXT_SIZE_RANGE,
};
use crate::geometry::{Point, Rect};
use crate::text_box::{
    BoxCorner, BoxEdge, PointF, RotatedBox, TextBox, TextHandle, rotation_from_pointer,
    round_to_i32, snap_rotation,
};

/// 起止点距离小于该像素数时不生成拖拽类图元。
pub const MIN_DRAG_DISTANCE: i64 = 2;
/// 缩放矩形类图元时允许的最小边长（像素）。
pub const MIN_SHAPE_SIZE: u32 = 2;
/// 荧光笔颜色的不透明度。
pub const HIGHLIGHTER_ALPHA: f32 = 0.4;
/// 箭头头部的最小长度（像素）。
pub const ARROW_MIN_HEAD: u32 = 10;
/// 箭头头部长度相对线宽的倍数。
pub const ARROW_HEAD_SCALE: u32 = 4;
/// 滚轮调整粗细的步长：普通线宽。
pub const ANNOTATE_WIDTH_STEP: u32 = 1;
/// 滚轮调整粗细的步长：荧光笔。
pub const HIGHLIGHTER_WIDTH_STEP: u32 = 2;
/// 滚轮调整粗细的步长：马赛克块。
pub const MOSAIC_BLOCK_STEP: u32 = 2;
/// 滚轮调整粗细的步长：序号直径。
pub const NUMBER_SIZE_STEP: u32 = 2;
/// 滚轮调整粗细的步长：文字字号。
pub const TEXT_SIZE_STEP: u32 = 2;
/// 底色文字的底色不透明度。
pub const TEXT_BACKGROUND_ALPHA: f32 = 0.85;
/// 底色文字的内边距相对字号的比例，用整数比表示：`字号 / 4`。
pub const TEXT_PADDING_DIVISOR: u32 = 4;

/// 标注工具，顺序即工具栏顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// 矩形。
    Rectangle,
    /// 椭圆。
    Ellipse,
    /// 直线。
    Line,
    /// 箭头。
    Arrow,
    /// 画笔。
    Pen,
    /// 荧光笔。
    Highlighter,
    /// 马赛克。
    Mosaic,
    /// 序号。
    Number,
    /// 文字。
    Text,
}

impl Tool {
    /// 全部工具。
    pub const ALL: [Tool; 9] = [
        Tool::Rectangle,
        Tool::Ellipse,
        Tool::Line,
        Tool::Arrow,
        Tool::Pen,
        Tool::Highlighter,
        Tool::Mosaic,
        Tool::Number,
        Tool::Text,
    ];

    /// 面向用户的中文名称。
    pub const fn label(self) -> &'static str {
        match self {
            Tool::Rectangle => "矩形",
            Tool::Ellipse => "椭圆",
            Tool::Line => "直线",
            Tool::Arrow => "箭头",
            Tool::Pen => "画笔",
            Tool::Highlighter => "荧光笔",
            Tool::Mosaic => "马赛克",
            Tool::Number => "序号",
            Tool::Text => "文字",
        }
    }

    /// 是否有颜色；马赛克没有。
    pub const fn has_color(self) -> bool {
        !matches!(self, Tool::Mosaic)
    }

    /// 样式条上「细 / 中 / 粗」三档预设，都在 [`Tool::size_spec`] 的范围内。
    pub const fn size_presets(self) -> [u32; 3] {
        match self {
            Tool::Rectangle | Tool::Ellipse | Tool::Line | Tool::Arrow | Tool::Pen => [2, 4, 8],
            Tool::Highlighter => [12, 20, 32],
            Tool::Mosaic => [8, 16, 32],
            Tool::Number => [20, 28, 40],
            Tool::Text => [16, 24, 36],
        }
    }

    /// 粗细（线宽、块大小或序号直径）的取值范围与滚轮步长。
    pub fn size_spec(self) -> SizeSpec {
        match self {
            Tool::Rectangle | Tool::Ellipse | Tool::Line | Tool::Arrow | Tool::Pen => SizeSpec {
                range: ANNOTATE_WIDTH_RANGE,
                step: ANNOTATE_WIDTH_STEP,
            },
            Tool::Highlighter => SizeSpec {
                range: HIGHLIGHTER_WIDTH_RANGE,
                step: HIGHLIGHTER_WIDTH_STEP,
            },
            Tool::Mosaic => SizeSpec {
                range: MOSAIC_BLOCK_RANGE,
                step: MOSAIC_BLOCK_STEP,
            },
            Tool::Number => SizeSpec {
                range: NUMBER_SIZE_RANGE,
                step: NUMBER_SIZE_STEP,
            },
            Tool::Text => SizeSpec {
                range: TEXT_SIZE_RANGE,
                step: TEXT_SIZE_STEP,
            },
        }
    }
}

/// 粗细的取值范围与滚轮步长。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizeSpec {
    /// 合法范围。
    pub range: RangeInclusive<u32>,
    /// 滚轮每格的变化量。
    pub step: u32,
}

/// 滚轮调整后的粗细：`notches` 为正表示增大，结果限制在范围内。
pub fn step_size(current: u32, notches: i32, spec: &SizeSpec) -> u32 {
    let delta = i64::from(notches).saturating_mul(i64::from(spec.step));
    let next = i64::from(current).saturating_add(delta);
    let low = i64::from(*spec.range.start());
    let high = i64::from(*spec.range.end());
    u32::try_from(next.clamp(low, high)).unwrap_or(*spec.range.start())
}

/// RGB 颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    /// 红。
    pub r: u8,
    /// 绿。
    pub g: u8,
    /// 蓝。
    pub b: u8,
}

impl Color {
    /// 构造颜色。
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// 解析 `#RRGGBB`，大小写均可；其它写法返回 `None`。
    pub fn from_hex(text: &str) -> Option<Self> {
        let digits = text.strip_prefix('#')?;
        if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let channel = |range: std::ops::Range<usize>| {
            digits
                .get(range)
                .and_then(|s| u8::from_str_radix(s, 16).ok())
        };
        Some(Self {
            r: channel(0..2)?,
            g: channel(2..4)?,
            b: channel(4..6)?,
        })
    }

    /// 输出 `#RRGGBB`（大写）。
    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// 序号圆内文字的颜色：背景接近白色时用黑色，否则用白色。
    pub fn label_color(self) -> Color {
        let luma = (2126 * u32::from(self.r) + 7152 * u32::from(self.g) + 722 * u32::from(self.b))
            / 10_000;
        if luma >= 230 {
            Color::rgb(0, 0, 0)
        } else {
            Color::rgb(255, 255, 255)
        }
    }
}

/// 固定色板：红、橙、黄、绿、蓝、紫、黑、白。
pub const PALETTE: [Color; 8] = [
    Color::rgb(0xFF, 0x3B, 0x30),
    Color::rgb(0xFF, 0x95, 0x00),
    Color::rgb(0xFF, 0xCC, 0x00),
    Color::rgb(0x34, 0xC7, 0x59),
    Color::rgb(0x00, 0x7A, 0xFF),
    Color::rgb(0xAF, 0x52, 0xDE),
    Color::rgb(0x00, 0x00, 0x00),
    Color::rgb(0xFF, 0xFF, 0xFF),
];

/// 颜色在色板中的序号；不在色板内返回 `None`。
pub fn palette_index(color: Color) -> Option<usize> {
    PALETTE.iter().position(|c| *c == color)
}

/// 序号的编号风格。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NumberStyle {
    /// 数字 1、2、3。
    #[default]
    Number,
    /// 字母 A、B、…、Z、AA。
    Letter,
}

impl NumberStyle {
    /// 全部取值，供界面生成选项。
    pub const ALL: [NumberStyle; 2] = [NumberStyle::Number, NumberStyle::Letter];

    /// 面向用户的中文名称。
    pub const fn label(self) -> &'static str {
        match self {
            NumberStyle::Number => "数字",
            NumberStyle::Letter => "字母",
        }
    }

    /// 样式条上的短标记。
    pub const fn mark(self) -> &'static str {
        match self {
            NumberStyle::Number => "1",
            NumberStyle::Letter => "A",
        }
    }
}

/// 序号标签：数字为十进制，字母为 A–Z 后接 AA、AB…；0 按 1 处理。
pub fn number_label(value: u32, style: NumberStyle) -> String {
    let value = value.max(1);
    match style {
        NumberStyle::Number => value.to_string(),
        NumberStyle::Letter => {
            let mut rest = value;
            let mut letters: Vec<char> = Vec::new();
            while rest > 0 {
                rest -= 1;
                let offset = u8::try_from(rest % 26).unwrap_or(0);
                letters.push(char::from(b'A'.saturating_add(offset)));
                rest /= 26;
            }
            letters.iter().rev().collect()
        }
    }
}

/// 一个已完成的图元。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// 矩形描边，`from`、`to` 为对角点。
    Rectangle {
        /// 起点。
        from: Point,
        /// 终点。
        to: Point,
        /// 颜色。
        color: Color,
        /// 线宽。
        width: u32,
        /// 顺时针旋转角度（整数度，`0..360`）。
        rotation: i32,
    },
    /// 椭圆描边，外接矩形的对角点。
    Ellipse {
        /// 起点。
        from: Point,
        /// 终点。
        to: Point,
        /// 颜色。
        color: Color,
        /// 线宽。
        width: u32,
        /// 顺时针旋转角度（整数度，`0..360`）。
        rotation: i32,
    },
    /// 直线。
    Line {
        /// 起点。
        from: Point,
        /// 终点。
        to: Point,
        /// 颜色。
        color: Color,
        /// 线宽。
        width: u32,
    },
    /// 箭头，箭头尖在 `to`。
    Arrow {
        /// 起点。
        from: Point,
        /// 终点（箭头尖）。
        to: Point,
        /// 颜色。
        color: Color,
        /// 线宽。
        width: u32,
    },
    /// 画笔轨迹。
    Pen {
        /// 轨迹点。
        points: Vec<Point>,
        /// 颜色。
        color: Color,
        /// 线宽。
        width: u32,
    },
    /// 荧光笔轨迹，绘制时颜色半透明。
    Highlighter {
        /// 轨迹点。
        points: Vec<Point>,
        /// 颜色。
        color: Color,
        /// 线宽。
        width: u32,
    },
    /// 马赛克，对角点围出的矩形区域。
    Mosaic {
        /// 起点。
        from: Point,
        /// 终点。
        to: Point,
        /// 块边长。
        block: u32,
    },
    /// 序号：圆心、编号、风格。
    Number {
        /// 圆心。
        center: Point,
        /// 编号，从 1 起。
        value: u32,
        /// 编号风格。
        style: NumberStyle,
        /// 圆的颜色。
        color: Color,
        /// 圆的直径。
        size: u32,
    },
    /// 文字，可多行，不自动换行。
    Text {
        /// 文本框左上角，底色模式下含内边距。
        origin: Point,
        /// 内容。
        text: String,
        /// 文字颜色；底色模式下为底色。
        color: Color,
        /// 字号（像素）。
        size: u32,
        /// 是否带底色。
        background: bool,
        /// 未旋转的本地外接框（左上角加宽高），旋转中心为框中心，命中测试也用它。
        bounds: Rect,
        /// 顺时针旋转角度（整数度，`0..360`）。
        rotation: i32,
        /// 换行长度（像素，不含内边距）：横排是宽、竖排是高；`None` 表示不自动换行。
        wrap: Option<u32>,
        /// 竖排：列从右往左排。
        vertical: bool,
    },
}

impl Shape {
    /// 图元对应的工具。
    pub const fn tool(&self) -> Tool {
        match self {
            Shape::Rectangle { .. } => Tool::Rectangle,
            Shape::Ellipse { .. } => Tool::Ellipse,
            Shape::Line { .. } => Tool::Line,
            Shape::Arrow { .. } => Tool::Arrow,
            Shape::Pen { .. } => Tool::Pen,
            Shape::Highlighter { .. } => Tool::Highlighter,
            Shape::Mosaic { .. } => Tool::Mosaic,
            Shape::Number { .. } => Tool::Number,
            Shape::Text { .. } => Tool::Text,
        }
    }
}

/// 撤销一步所需的信息。
#[derive(Debug, Clone, PartialEq, Eq)]
enum UndoStep {
    /// 撤销一次添加：弹出最后一个图元。
    Push,
    /// 撤销一次替换：把 `index` 处恢复为 `before`。
    Replace { index: usize, before: Shape },
    /// 撤销一次删除：把 `shape` 插回 `index`。
    Remove { index: usize, shape: Shape },
}

/// 重做一步所需的信息。
#[derive(Debug, Clone, PartialEq, Eq)]
enum RedoStep {
    Push(Shape),
    Replace { index: usize, after: Shape },
    Remove { index: usize },
}

/// 图元列表与撤销重做栈；`revision` 在内容变化时递增，供绘制缓存判断是否失效。
///
/// 添加、替换、删除各占一步撤销；任何新操作都会清空重做栈。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Document {
    shapes: Vec<Shape>,
    undo: Vec<UndoStep>,
    redo: Vec<RedoStep>,
    revision: u64,
}

impl Document {
    /// 空文档。
    pub fn new() -> Self {
        Self::default()
    }

    /// 已完成的图元，按添加顺序。
    pub fn shapes(&self) -> &[Shape] {
        &self.shapes
    }

    /// 内容版本号。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 没有图元。
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// 添加图元。
    pub fn push(&mut self, shape: Shape) {
        self.shapes.push(shape);
        self.record(UndoStep::Push);
    }

    /// 把 `index` 处的图元换成 `shape`；序号越界返回 `false`。
    pub fn replace(&mut self, index: usize, shape: Shape) -> bool {
        let Some(slot) = self.shapes.get_mut(index) else {
            return false;
        };
        let before = std::mem::replace(slot, shape);
        self.record(UndoStep::Replace { index, before });
        true
    }

    /// 删除 `index` 处的图元；序号越界返回 `false`。
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.shapes.len() {
            return false;
        }
        let shape = self.shapes.remove(index);
        self.record(UndoStep::Remove { index, shape });
        true
    }

    /// 撤销上一步；没有可撤销的返回 `false`。
    pub fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop() else {
            return false;
        };
        match step {
            UndoStep::Push => {
                if let Some(shape) = self.shapes.pop() {
                    self.redo.push(RedoStep::Push(shape));
                }
            }
            UndoStep::Replace { index, before } => {
                if let Some(slot) = self.shapes.get_mut(index) {
                    let after = std::mem::replace(slot, before);
                    self.redo.push(RedoStep::Replace { index, after });
                }
            }
            UndoStep::Remove { index, shape } => {
                self.shapes.insert(index.min(self.shapes.len()), shape);
                self.redo.push(RedoStep::Remove { index });
            }
        }
        self.bump();
        true
    }

    /// 重做最近撤销的一步；没有可重做的返回 `false`。
    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else {
            return false;
        };
        match step {
            RedoStep::Push(shape) => {
                self.shapes.push(shape);
                self.undo.push(UndoStep::Push);
            }
            RedoStep::Replace { index, after } => {
                if let Some(slot) = self.shapes.get_mut(index) {
                    let before = std::mem::replace(slot, after);
                    self.undo.push(UndoStep::Replace { index, before });
                }
            }
            RedoStep::Remove { index } => {
                if index < self.shapes.len() {
                    let shape = self.shapes.remove(index);
                    self.undo.push(UndoStep::Remove { index, shape });
                }
            }
        }
        self.bump();
        true
    }

    /// 能否撤销。
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// 能否重做。
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// 下一个序号：同风格现有序号的最大值加 1，数字与字母各自独立计数。
    pub fn next_number(&self, style: NumberStyle) -> u32 {
        self.shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Number {
                    value, style: s, ..
                } if *s == style => Some(*value),
                _ => None,
            })
            .max()
            .map_or(1, |max| max.saturating_add(1))
    }

    /// 命中点的文字图元序号：按外接框判断，后添加的优先。
    pub fn text_at(&self, p: Point) -> Option<usize> {
        let point = (f64::from(p.x), f64::from(p.y));
        self.shapes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, shape)| match shape {
                Shape::Text {
                    bounds,
                    rotation,
                    vertical,
                    ..
                } => TextBox {
                    bounds: *bounds,
                    rotation: *rotation,
                    vertical: *vertical,
                }
                .contains(point)
                .then_some(i),
                _ => None,
            })
    }

    /// 命中点的任意图元序号：描边类按描边距离判断，马赛克与序号认整个区域，后添加的优先。
    pub fn shape_at(&self, p: Point, tolerance: f64) -> Option<usize> {
        self.shapes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, shape)| shape_hits(shape, p, tolerance).then_some(i))
    }

    fn record(&mut self, step: UndoStep) {
        self.undo.push(step);
        self.redo.clear();
        self.bump();
    }

    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

fn sign(v: i64) -> i64 {
    if v < 0 { -1 } else { 1 }
}

fn offset(from: Point, dx: i64, dy: i64) -> Point {
    let x = i64::from(from.x).saturating_add(dx);
    let y = i64::from(from.y).saturating_add(dy);
    Point::new(
        i32::try_from(x).unwrap_or(from.x),
        i32::try_from(y).unwrap_or(from.y),
    )
}

/// 起止点距离是否足以生成图元。
pub fn drag_is_valid(from: Point, to: Point) -> bool {
    let dx = i128::from(to.x) - i128::from(from.x);
    let dy = i128::from(to.y) - i128::from(from.y);
    dx * dx + dy * dy >= i128::from(MIN_DRAG_DISTANCE * MIN_DRAG_DISTANCE)
}

/// Shift 约束：矩形变正方形、椭圆变正圆，边长取两个方向中较长的一边，方向保持不变。
pub fn constrain_square(from: Point, to: Point) -> Point {
    let dx = i64::from(to.x) - i64::from(from.x);
    let dy = i64::from(to.y) - i64::from(from.y);
    let side = dx.abs().max(dy.abs());
    offset(from, sign(dx) * side, sign(dy) * side)
}

/// Shift 约束：直线和箭头的角度吸附到 45° 的倍数，长度尽量保持。
pub fn snap_angle(from: Point, to: Point) -> Point {
    let dx = i64::from(to.x) - i64::from(from.x);
    let dy = i64::from(to.y) - i64::from(from.y);
    let (ax, ay) = (dx.abs(), dy.abs());
    // tan(22.5°) ≈ 0.41421，用整数比较判断落在哪个 45° 扇区
    if ay * 1000 < ax * 414 {
        return offset(from, dx, 0);
    }
    if ax * 1000 < ay * 414 {
        return offset(from, 0, dy);
    }
    // 斜向每个方向的位移为长度除以根号 2（乘 0.7071）
    let length_sq = i128::from(dx) * i128::from(dx) + i128::from(dy) * i128::from(dy);
    let diagonal = i64::try_from((length_sq.isqrt() * 7071 + 5_000) / 10_000).unwrap_or(0);
    offset(from, sign(dx) * diagonal, sign(dy) * diagonal)
}

/// 箭头头部几何：以下所有偏移都相对箭头尖。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrowHead {
    /// 头部底边一端。
    pub left: (f32, f32),
    /// 头部底边另一端。
    pub right: (f32, f32),
    /// 杆身终点，略深入头部以免线帽和三角之间露缝。
    pub shaft_end: (f32, f32),
}

fn to_f32(value: i64) -> f32 {
    let clamped = value.clamp(i64::from(i16::MIN), i64::from(i16::MAX));
    f32::from(i16::try_from(clamped).unwrap_or(0))
}

/// 箭头头部：长约线宽×4，最少 10 像素，线段太短时缩到线段长度；起止点太近返回 `None`。
pub fn arrow_head(from: Point, to: Point, width: u32) -> Option<ArrowHead> {
    let dx = to_f32(i64::from(to.x) - i64::from(from.x));
    let dy = to_f32(i64::from(to.y) - i64::from(from.y));
    let length = dx.hypot(dy);
    if length < 1.0 {
        return None;
    }
    let (ux, uy) = (dx / length, dy / length);
    let wanted = width.saturating_mul(ARROW_HEAD_SCALE).max(ARROW_MIN_HEAD);
    let head = to_f32(i64::from(wanted)).min(length);
    let half = head * 0.45;
    // 底边中心在尖端反方向 head 处，法向量为 (-uy, ux)
    let (bx, by) = (-ux * head, -uy * head);
    Some(ArrowHead {
        left: (bx - uy * half, by + ux * half),
        right: (bx + uy * half, by - ux * half),
        shaft_end: (-ux * head * 0.8, -uy * head * 0.8),
    })
}

/// 像素缓冲区的通道排列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelOrder {
    /// R、G、B、A。
    Rgba,
    /// B、G、R、A。
    Bgra,
}

/// 马赛克的块色网格：按块大小对原图取平均色，行优先排列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MosaicGrid {
    /// 实际覆盖的区域（图像坐标，已与图像求交）。
    pub area: Rect,
    /// 块边长。
    pub block: u32,
    /// 列数。
    pub cols: u32,
    /// 行数。
    pub rows: u32,
    /// 每块的平均色，长度为 `cols * rows`。
    pub colors: Vec<Color>,
}

/// 计算马赛克块色。
///
/// `pixels` 是整幅图像（每像素 4 字节），`area` 为图像坐标下的区域；
/// 块从区域左上角起排列，右、下边缘不足一块的按实际大小取平均。
/// 缓冲区长度不符、块为 0 或区域与图像无交集时返回 `None`。
pub fn mosaic_grid(
    pixels: &[u8],
    image: (u32, u32),
    order: PixelOrder,
    area: &Rect,
    block: u32,
) -> Option<MosaicGrid> {
    let (width, height) = image;
    let expected = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(4)?;
    if block == 0 || pixels.len() != expected {
        return None;
    }
    let area = area.intersect(&Rect::new(0, 0, width, height))?;
    let cols = area.width.div_ceil(block);
    let rows = area.height.div_ceil(block);
    let stride = usize::try_from(width).ok()?;
    let (ri, bi) = match order {
        PixelOrder::Rgba => (0, 2),
        PixelOrder::Bgra => (2, 0),
    };
    let mut colors = Vec::with_capacity(usize::try_from(u64::from(cols) * u64::from(rows)).ok()?);
    for row in 0..rows {
        let y0 = i64::from(area.y) + i64::from(row) * i64::from(block);
        let y1 = (y0 + i64::from(block)).min(area.bottom());
        for col in 0..cols {
            let x0 = i64::from(area.x) + i64::from(col) * i64::from(block);
            let x1 = (x0 + i64::from(block)).min(area.right());
            let (sx0, sx1) = (usize::try_from(x0).ok()?, usize::try_from(x1).ok()?);
            let mut sum = [0u64; 3];
            let mut count = 0u64;
            for y in usize::try_from(y0).ok()?..usize::try_from(y1).ok()? {
                let start = y.checked_mul(stride)?.checked_add(sx0)?.checked_mul(4)?;
                let end = y.checked_mul(stride)?.checked_add(sx1)?.checked_mul(4)?;
                for px in pixels.get(start..end)?.as_chunks::<4>().0 {
                    sum[0] += u64::from(*px.get(ri)?);
                    sum[1] += u64::from(*px.get(1)?);
                    sum[2] += u64::from(*px.get(bi)?);
                    count += 1;
                }
            }
            let avg = |total: u64| u8::try_from((total + count / 2) / count.max(1)).unwrap_or(255);
            colors.push(Color::rgb(avg(sum[0]), avg(sum[1]), avg(sum[2])));
        }
    }
    Some(MosaicGrid {
        area,
        block,
        cols,
        rows,
        colors,
    })
}

/// 直线或箭头的端点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    /// 起点。
    From,
    /// 终点（箭头尖）。
    To,
}

/// 已选中图元上可操作的部位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeHandle {
    /// 图元本体。
    Body,
    /// 选中框边缘一带。
    Border,
    /// 四角缩放手柄。
    Corner(BoxCorner),
    /// 四边中点缩放手柄。
    Edge(BoxEdge),
    /// 旋转手柄。
    Rotate,
    /// 直线或箭头端点手柄。
    Endpoint(Endpoint),
}

impl From<TextHandle> for ShapeHandle {
    fn from(handle: TextHandle) -> Self {
        match handle {
            TextHandle::Body => ShapeHandle::Body,
            TextHandle::Border => ShapeHandle::Border,
            TextHandle::Corner(c) => ShapeHandle::Corner(c),
            TextHandle::Edge(e) => ShapeHandle::Edge(e),
            TextHandle::Rotate => ShapeHandle::Rotate,
        }
    }
}

fn point_f(p: Point) -> PointF {
    (f64::from(p.x), f64::from(p.y))
}

fn rect_end(rect: &Rect) -> (Point, Point) {
    let right = i32::try_from(rect.right()).unwrap_or(i32::MAX);
    let bottom = i32::try_from(rect.bottom()).unwrap_or(i32::MAX);
    (Point::new(rect.x, rect.y), Point::new(right, bottom))
}

/// 图元在自身本地坐标系下的外接矩形；轨迹点为空时返回 `None`。
pub fn shape_bounds(shape: &Shape) -> Option<Rect> {
    match shape {
        Shape::Rectangle { from, to, .. }
        | Shape::Ellipse { from, to, .. }
        | Shape::Line { from, to, .. }
        | Shape::Arrow { from, to, .. }
        | Shape::Mosaic { from, to, .. } => Rect::bounding(&[*from, *to]),
        Shape::Pen { points, .. } | Shape::Highlighter { points, .. } => Rect::bounding(points),
        Shape::Number { center, size, .. } => {
            let radius = i64::from(*size) / 2;
            let left = i64::from(center.x) - radius;
            let top = i64::from(center.y) - radius;
            Rect::from_edges(left, top, left + i64::from(*size), top + i64::from(*size))
        }
        Shape::Text { bounds, .. } => Some(*bounds),
    }
}

/// 图元的旋转框：直线与箭头使用两端点手柄，不显示外接框，返回 `None`。
pub fn shape_box(shape: &Shape) -> Option<RotatedBox> {
    let rotation = match shape {
        Shape::Rectangle { rotation, .. }
        | Shape::Ellipse { rotation, .. }
        | Shape::Text { rotation, .. } => *rotation,
        Shape::Line { .. } | Shape::Arrow { .. } => return None,
        Shape::Pen { .. }
        | Shape::Highlighter { .. }
        | Shape::Mosaic { .. }
        | Shape::Number { .. } => 0,
    };
    Some(RotatedBox::new(shape_bounds(shape)?, rotation))
}

fn point_to_segment(p: PointF, a: PointF, b: PointF) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len_sq = dx * dx + dy * dy;
    if len_sq <= 0.0 {
        return (p.0 - a.0).hypot(p.1 - a.1);
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len_sq).clamp(0.0, 1.0);
    (p.0 - (a.0 + t * dx)).hypot(p.1 - (a.1 + t * dy))
}

fn point_in_triangle(p: PointF, a: PointF, b: PointF, c: PointF) -> bool {
    let cross =
        |u: PointF, v: PointF, q: PointF| (v.0 - u.0) * (q.1 - u.1) - (v.1 - u.1) * (q.0 - u.0);
    let (c1, c2, c3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
    let has_neg = c1 < 0.0 || c2 < 0.0 || c3 < 0.0;
    let has_pos = c1 > 0.0 || c2 > 0.0 || c3 > 0.0;
    !(has_neg && has_pos)
}

fn ellipse_border_distance(rbox: &RotatedBox, p: PointF) -> f64 {
    let local = rbox.to_local(p);
    let (px, py) = (local.0.abs(), local.1.abs());
    let (rx, ry) = rbox.half();
    if rx <= 0.0 || ry <= 0.0 {
        return (px - rx).max(0.0).hypot((py - ry).max(0.0));
    }
    let dist_sq = |angle: f64| {
        let (s, c) = angle.sin_cos();
        let dx = rx * c - px;
        let dy = ry * s - py;
        dx * dx + dy * dy
    };
    let samples = 24usize;
    let step = std::f64::consts::FRAC_PI_2 / 24.0;
    let mut best_i = 0usize;
    let mut best_d = f64::INFINITY;
    for i in 0..=samples {
        let idx = i32::try_from(i).unwrap_or(0);
        let d = dist_sq(f64::from(idx) * step);
        if d < best_d {
            best_d = d;
            best_i = i;
        }
    }
    let lo_idx = i32::try_from(best_i.saturating_sub(1)).unwrap_or(0);
    let hi_idx = i32::try_from((best_i + 1).min(samples)).unwrap_or(24);
    let (mut lo, mut hi) = (f64::from(lo_idx) * step, f64::from(hi_idx) * step);
    for _ in 0..32 {
        let m1 = lo + (hi - lo) / 3.0;
        let m2 = hi - (hi - lo) / 3.0;
        if dist_sq(m1) <= dist_sq(m2) {
            hi = m2;
        } else {
            lo = m1;
        }
    }
    dist_sq((lo + hi) / 2.0).sqrt()
}

fn trail_distance(points: &[Point], p: PointF) -> f64 {
    match points {
        [] => f64::INFINITY,
        [only] => (p.0 - f64::from(only.x)).hypot(p.1 - f64::from(only.y)),
        _ => points
            .windows(2)
            .filter_map(|pair| {
                Some(point_to_segment(
                    p,
                    point_f(*pair.first()?),
                    point_f(*pair.get(1)?),
                ))
            })
            .fold(f64::INFINITY, f64::min),
    }
}

/// 点 `p` 是否命中图元：描边类只认描边附近（半线宽 + `tolerance`），马赛克与序号认整个区域。
pub fn shape_hits(shape: &Shape, p: Point, tolerance: f64) -> bool {
    let pf = point_f(p);
    let margin = |width: u32| f64::from(width) / 2.0 + tolerance;
    match shape {
        Shape::Rectangle { .. } => shape_box(shape)
            .zip(match shape {
                Shape::Rectangle { width, .. } => Some(*width),
                _ => None,
            })
            .is_some_and(|(rbox, width)| rbox.border_distance(pf) <= margin(width)),
        Shape::Ellipse { width, .. } => shape_box(shape)
            .is_some_and(|rbox| ellipse_border_distance(&rbox, pf) <= margin(*width)),
        Shape::Line {
            from, to, width, ..
        } => point_to_segment(pf, point_f(*from), point_f(*to)) <= margin(*width),
        Shape::Arrow {
            from, to, width, ..
        } => {
            if point_to_segment(pf, point_f(*from), point_f(*to)) <= margin(*width) {
                return true;
            }
            let Some(head) = arrow_head(*from, *to, *width) else {
                return false;
            };
            let tip = point_f(*to);
            let left = (
                tip.0 + f64::from(head.left.0),
                tip.1 + f64::from(head.left.1),
            );
            let right = (
                tip.0 + f64::from(head.right.0),
                tip.1 + f64::from(head.right.1),
            );
            point_in_triangle(pf, tip, left, right)
                || point_to_segment(pf, tip, left) <= tolerance
                || point_to_segment(pf, left, right) <= tolerance
                || point_to_segment(pf, right, tip) <= tolerance
        }
        Shape::Pen { points, width, .. } | Shape::Highlighter { points, width, .. } => {
            trail_distance(points, pf) <= margin(*width)
        }
        Shape::Mosaic { from, to, .. } => {
            Rect::bounding(&[*from, *to]).is_some_and(|r| r.contains(p))
        }
        Shape::Number { center, size, .. } => {
            (pf.0 - f64::from(center.x)).hypot(pf.1 - f64::from(center.y)) <= f64::from(*size) / 2.0
        }
        Shape::Text {
            bounds,
            rotation,
            vertical,
            ..
        } => TextBox {
            bounds: *bounds,
            rotation: *rotation,
            vertical: *vertical,
        }
        .contains(pf),
    }
}

/// 命中已选中图元的哪个部位；手柄优先于本体，本体按外接框（旋转后）命中。
pub fn hit_selected_shape(
    shape: &Shape,
    p: Point,
    tolerance: f64,
    rotate_distance: f64,
) -> Option<ShapeHandle> {
    let pf = point_f(p);
    match shape {
        Shape::Rectangle { .. } | Shape::Ellipse { .. } => shape_box(shape)?
            .hit_handles(pf, tolerance, Some(rotate_distance), &BoxEdge::ALL, false)
            .map(ShapeHandle::from),
        Shape::Pen { .. } | Shape::Highlighter { .. } | Shape::Mosaic { .. } => shape_box(shape)?
            .hit_handles(pf, tolerance, None, &BoxEdge::ALL, false)
            .map(ShapeHandle::from),
        Shape::Line { from, to, .. } | Shape::Arrow { from, to, .. } => {
            let d_from = (pf.0 - f64::from(from.x)).hypot(pf.1 - f64::from(from.y));
            let d_to = (pf.0 - f64::from(to.x)).hypot(pf.1 - f64::from(to.y));
            if d_from <= tolerance || d_to <= tolerance {
                let endpoint = if d_to <= d_from {
                    Endpoint::To
                } else {
                    Endpoint::From
                };
                return Some(ShapeHandle::Endpoint(endpoint));
            }
            shape_hits(shape, p, tolerance).then_some(ShapeHandle::Body)
        }
        Shape::Number { .. } => shape_hits(shape, p, tolerance).then_some(ShapeHandle::Body),
        Shape::Text {
            bounds,
            rotation,
            vertical,
            ..
        } => TextBox {
            bounds: *bounds,
            rotation: *rotation,
            vertical: *vertical,
        }
        .hit(pf, tolerance, rotate_distance)
        .map(ShapeHandle::from),
    }
}

fn shift_point(p: Point, dx: i32, dy: i32) -> Point {
    Point::new(p.x.saturating_add(dx), p.y.saturating_add(dy))
}

/// 平移图元 `(dx, dy)` 像素（饱和加法，不溢出）。
pub fn translate_shape(shape: &Shape, dx: i32, dy: i32) -> Shape {
    let mut moved = shape.clone();
    match &mut moved {
        Shape::Rectangle { from, to, .. }
        | Shape::Ellipse { from, to, .. }
        | Shape::Line { from, to, .. }
        | Shape::Arrow { from, to, .. }
        | Shape::Mosaic { from, to, .. } => {
            *from = shift_point(*from, dx, dy);
            *to = shift_point(*to, dx, dy);
        }
        Shape::Pen { points, .. } | Shape::Highlighter { points, .. } => {
            for pt in points {
                *pt = shift_point(*pt, dx, dy);
            }
        }
        Shape::Number { center, .. } => *center = shift_point(*center, dx, dy),
        Shape::Text { origin, bounds, .. } => {
            *origin = shift_point(*origin, dx, dy);
            bounds.x = bounds.x.saturating_add(dx);
            bounds.y = bounds.y.saturating_add(dy);
        }
    }
    moved
}

/// 按外接框从 `from` 到 `to` 缩放全部轨迹点，`flip_x` / `flip_y` 为真时沿对应轴翻转。
pub fn scale_points(
    points: &[Point],
    from: &Rect,
    to: &Rect,
    flip_x: bool,
    flip_y: bool,
) -> Vec<Point> {
    points
        .iter()
        .map(|p| {
            let x = if from.width == 0 {
                f64::from(to.x) + f64::from(to.width) / 2.0
            } else {
                let ratio = (f64::from(p.x) - f64::from(from.x)) / f64::from(from.width);
                let t = if flip_x { 1.0 - ratio } else { ratio };
                f64::from(to.x) + t * f64::from(to.width)
            };
            let y = if from.height == 0 {
                f64::from(to.y) + f64::from(to.height) / 2.0
            } else {
                let ratio = (f64::from(p.y) - f64::from(from.y)) / f64::from(from.height);
                let t = if flip_y { 1.0 - ratio } else { ratio };
                f64::from(to.y) + t * f64::from(to.height)
            };
            Point::new(round_to_i32(x), round_to_i32(y))
        })
        .collect()
}

fn clamp_endpoint(fixed: Point, target: Point, fallback: Point) -> Point {
    if drag_is_valid(fixed, target) {
        return target;
    }
    let dx = f64::from(target.x) - f64::from(fixed.x);
    let dy = f64::from(target.y) - f64::from(fixed.y);
    let len = dx.hypot(dy);
    if len <= 0.0 {
        return fallback;
    }
    let pushed = Point::new(
        fixed.x.saturating_add(round_to_i32(dx * 2.5 / len)),
        fixed.y.saturating_add(round_to_i32(dy * 2.5 / len)),
    );
    if drag_is_valid(fixed, pushed) {
        pushed
    } else {
        fallback
    }
}

fn resize_box(
    bounds: Rect,
    rotation: i32,
    handle: ShapeHandle,
    pointer: Point,
    shift: bool,
    min_size: (u32, u32),
) -> (Rect, i32, bool, bool) {
    let rbox = RotatedBox::new(bounds, rotation);
    let pf = point_f(pointer);
    match handle {
        ShapeHandle::Corner(corner) => {
            let (next, fx, fy) = rbox.resize_corner(corner, pf, shift, min_size);
            (next, rbox.rotation, fx, fy)
        }
        ShapeHandle::Edge(edge) => {
            let (next, fx, fy) = rbox.resize_edge(edge, pf, min_size);
            (next, rbox.rotation, fx, fy)
        }
        ShapeHandle::Rotate => {
            let next_rot = match rotation_from_pointer(rbox.center(), pf, false) {
                Some(raw) if shift => snap_rotation(raw),
                Some(raw) => raw,
                None => rbox.rotation,
            };
            (bounds, next_rot, false, false)
        }
        _ => (bounds, rbox.rotation, false, false),
    }
}

fn resize_rect_endpoints(
    from: Point,
    to: Point,
    rotation: i32,
    handle: ShapeHandle,
    pointer: Point,
    shift: bool,
) -> Option<(Point, Point, i32)> {
    let bounds = Rect::bounding(&[from, to])?;
    let (next, next_rot, _, _) = resize_box(
        bounds,
        rotation,
        handle,
        pointer,
        shift,
        (MIN_SHAPE_SIZE, MIN_SHAPE_SIZE),
    );
    let (nf, nt) = rect_end(&next);
    Some((nf, nt, next_rot))
}

fn resize_segment_endpoints(
    from: Point,
    to: Point,
    handle: ShapeHandle,
    pointer: Point,
    shift: bool,
) -> (Point, Point) {
    let ShapeHandle::Endpoint(endpoint) = handle else {
        return (from, to);
    };
    let (fixed, old_moving) = match endpoint {
        Endpoint::From => (to, from),
        Endpoint::To => (from, to),
    };
    let raw = if shift {
        snap_angle(fixed, pointer)
    } else {
        pointer
    };
    let moved = clamp_endpoint(fixed, raw, old_moving);
    match endpoint {
        Endpoint::From => (moved, fixed),
        Endpoint::To => (fixed, moved),
    }
}

fn resize_trail_points(
    points: &[Point],
    handle: ShapeHandle,
    pointer: Point,
    shift: bool,
) -> Option<Vec<Point>> {
    if matches!(handle, ShapeHandle::Rotate) {
        return None;
    }
    let bounds = Rect::bounding(points)?;
    let min_w = if bounds.width == 0 { 0 } else { MIN_SHAPE_SIZE };
    let min_h = if bounds.height == 0 {
        0
    } else {
        MIN_SHAPE_SIZE
    };
    let (next, _, fx, fy) = resize_box(bounds, 0, handle, pointer, shift, (min_w, min_h));
    Some(scale_points(points, &bounds, &next, fx, fy))
}

/// 按选中的手柄拖动到 `pointer` 后得到的新图元；`shift` 控制等比、15° 旋转或 45° 线角吸附。
pub fn resize_shape(shape: &Shape, handle: ShapeHandle, pointer: Point, shift: bool) -> Shape {
    let mut updated = shape.clone();
    match &mut updated {
        Shape::Rectangle {
            from, to, rotation, ..
        }
        | Shape::Ellipse {
            from, to, rotation, ..
        } => {
            if let Some((nf, nt, next_rot)) =
                resize_rect_endpoints(*from, *to, *rotation, handle, pointer, shift)
            {
                *from = nf;
                *to = nt;
                *rotation = next_rot;
            }
        }
        Shape::Mosaic { from, to, .. } if !matches!(handle, ShapeHandle::Rotate) => {
            if let Some((nf, nt, _)) = resize_rect_endpoints(*from, *to, 0, handle, pointer, shift)
            {
                *from = nf;
                *to = nt;
            }
        }
        Shape::Line { from, to, .. } | Shape::Arrow { from, to, .. } => {
            (*from, *to) = resize_segment_endpoints(*from, *to, handle, pointer, shift);
        }
        Shape::Pen { points, .. } | Shape::Highlighter { points, .. } => {
            if let Some(scaled) = resize_trail_points(points, handle, pointer, shift) {
                *points = scaled;
            }
        }
        Shape::Mosaic { .. } | Shape::Number { .. } | Shape::Text { .. } => {}
    }
    updated
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: i32, y: i32) -> Point {
        Point::new(x, y)
    }

    fn number(value: u32, style: NumberStyle) -> Shape {
        Shape::Number {
            center: p(0, 0),
            value,
            style,
            color: PALETTE[0],
            size: 28,
        }
    }

    fn line() -> Shape {
        Shape::Line {
            from: p(0, 0),
            to: p(10, 10),
            color: PALETTE[0],
            width: 3,
        }
    }

    #[test]
    fn undo_redo_roundtrip_and_new_shape_clears_redo() {
        let mut doc = Document::new();
        assert!(!doc.can_undo() && !doc.can_redo());
        assert!(!doc.undo() && !doc.redo());
        doc.push(line());
        doc.push(number(1, NumberStyle::Number));
        assert!(doc.can_undo() && !doc.can_redo());
        assert!(doc.undo());
        assert_eq!(doc.shapes().len(), 1);
        assert!(doc.can_redo());
        assert!(doc.redo());
        assert_eq!(doc.shapes().len(), 2);
        assert!(doc.undo());
        doc.push(line());
        assert!(!doc.can_redo(), "新图元应清空重做栈");
        assert!(!doc.redo());
    }

    fn text(value: &str, x: i32) -> Shape {
        Shape::Text {
            origin: p(x, 0),
            text: value.to_string(),
            color: PALETTE[0],
            size: 24,
            background: false,
            bounds: Rect::new(x, 0, 20, 20),
            rotation: 0,
            wrap: None,
            vertical: false,
        }
    }

    #[test]
    fn replace_is_one_undo_step_and_restores_original() {
        let mut doc = Document::new();
        doc.push(text("a", 0));
        doc.push(line());
        assert!(doc.replace(0, text("b", 0)));
        assert_eq!(doc.shapes().len(), 2);
        assert!(matches!(doc.shapes().first(), Some(Shape::Text { text, .. }) if text == "b"));
        assert!(doc.undo());
        assert!(matches!(doc.shapes().first(), Some(Shape::Text { text, .. }) if text == "a"));
        assert_eq!(doc.shapes().len(), 2);
        assert!(doc.redo());
        assert!(matches!(doc.shapes().first(), Some(Shape::Text { text, .. }) if text == "b"));
        assert!(!doc.replace(5, text("x", 0)));
    }

    #[test]
    fn remove_is_one_undo_step_and_reinserts_in_place() {
        let mut doc = Document::new();
        doc.push(text("a", 0));
        doc.push(line());
        doc.push(text("c", 40));
        assert!(doc.remove(1));
        assert_eq!(doc.shapes().len(), 2);
        assert!(doc.undo());
        assert_eq!(doc.shapes().len(), 3);
        assert_eq!(doc.shapes().get(1), Some(&line()));
        assert!(doc.redo());
        assert_eq!(doc.shapes().len(), 2);
        assert!(!doc.remove(9));
        for _ in 0..4 {
            assert!(doc.undo());
        }
        assert!(doc.shapes().is_empty());
        assert!(!doc.can_undo());
    }

    #[test]
    fn mixed_operations_unwind_in_reverse_order() {
        let mut doc = Document::new();
        doc.push(text("a", 0));
        doc.replace(0, text("b", 0));
        doc.push(line());
        doc.remove(0);
        assert_eq!(doc.shapes(), &[line()]);
        for _ in 0..4 {
            assert!(doc.undo());
        }
        assert!(doc.shapes().is_empty());
        for _ in 0..4 {
            assert!(doc.redo());
        }
        assert_eq!(doc.shapes(), &[line()]);
        assert!(doc.undo());
        doc.replace(0, text("z", 0));
        assert!(!doc.can_redo(), "新操作应清空重做栈");
    }

    #[test]
    fn text_hit_test_follows_rotation() {
        let mut doc = Document::new();
        doc.push(Shape::Text {
            origin: p(0, 0),
            text: "r".to_string(),
            color: PALETTE[0],
            size: 24,
            background: false,
            bounds: Rect::new(0, 0, 100, 20),
            rotation: 90,
            wrap: None,
            vertical: false,
        });
        // 旋转 90° 后是围绕 (50, 10) 的竖条
        assert_eq!(doc.text_at(p(50, 50)), Some(0));
        assert_eq!(doc.text_at(p(95, 10)), None);
    }

    #[test]
    fn text_hit_test_prefers_the_latest_shape() {
        let mut doc = Document::new();
        doc.push(text("old", 0));
        doc.push(line());
        doc.push(text("new", 10));
        assert_eq!(doc.text_at(p(15, 5)), Some(2));
        assert_eq!(doc.text_at(p(5, 5)), Some(0));
        assert_eq!(doc.text_at(p(100, 5)), None);
        assert_eq!(doc.text_at(p(5, 25)), None);
    }

    #[test]
    fn revision_changes_on_every_mutation_only() {
        let mut doc = Document::new();
        let r0 = doc.revision();
        assert!(!doc.undo());
        assert_eq!(doc.revision(), r0);
        doc.push(line());
        let r1 = doc.revision();
        assert_ne!(r1, r0);
        assert!(doc.undo());
        assert_ne!(doc.revision(), r1);
    }

    #[test]
    fn numbers_count_per_style_and_fall_back_after_undo() {
        let mut doc = Document::new();
        assert_eq!(doc.next_number(NumberStyle::Number), 1);
        doc.push(number(1, NumberStyle::Number));
        doc.push(number(2, NumberStyle::Number));
        assert_eq!(doc.next_number(NumberStyle::Number), 3);
        assert_eq!(doc.next_number(NumberStyle::Letter), 1);
        doc.push(number(1, NumberStyle::Letter));
        assert_eq!(doc.next_number(NumberStyle::Letter), 2);
        assert_eq!(doc.next_number(NumberStyle::Number), 3);
        assert!(doc.undo());
        assert_eq!(doc.next_number(NumberStyle::Letter), 1);
        assert!(doc.undo());
        assert_eq!(doc.next_number(NumberStyle::Number), 2);
    }

    #[test]
    fn letter_labels_carry_over() {
        let labels: Vec<_> = [1, 2, 26, 27, 28, 52, 53, 702, 703]
            .iter()
            .map(|v| number_label(*v, NumberStyle::Letter))
            .collect();
        assert_eq!(labels, ["A", "B", "Z", "AA", "AB", "AZ", "BA", "ZZ", "AAA"]);
        assert_eq!(number_label(7, NumberStyle::Number), "7");
        assert_eq!(number_label(0, NumberStyle::Number), "1");
        assert_eq!(number_label(0, NumberStyle::Letter), "A");
    }

    #[test]
    fn color_hex_roundtrip_and_rejection() {
        assert_eq!(Color::from_hex("#FF3B30"), Some(PALETTE[0]));
        assert_eq!(Color::from_hex("#ff3b30"), Some(PALETTE[0]));
        assert_eq!(PALETTE[4].to_hex(), "#007AFF");
        for bad in [
            "", "#", "FF3B30", "#FF3B3", "#FF3B300", "#GG0000", "#+F0000", " #FF3B30",
        ] {
            assert_eq!(Color::from_hex(bad), None, "{bad:?}");
        }
        for c in PALETTE {
            assert_eq!(Color::from_hex(&c.to_hex()), Some(c));
        }
    }

    #[test]
    fn palette_lookup_and_label_color() {
        assert_eq!(palette_index(PALETTE[3]), Some(3));
        assert_eq!(palette_index(Color::rgb(1, 2, 3)), None);
        assert_eq!(PALETTE[0].label_color(), Color::rgb(255, 255, 255));
        assert_eq!(PALETTE[2].label_color(), Color::rgb(255, 255, 255));
        assert_eq!(PALETTE[7].label_color(), Color::rgb(0, 0, 0));
    }

    #[test]
    fn size_stepping_clamps_to_range() {
        let spec = Tool::Rectangle.size_spec();
        assert_eq!(step_size(3, 1, &spec), 4);
        assert_eq!(step_size(3, -1, &spec), 2);
        assert_eq!(step_size(1, -5, &spec), 1);
        assert_eq!(step_size(32, 3, &spec), 32);
        assert_eq!(step_size(3, i32::MAX, &spec), 32);
        let hl = Tool::Highlighter.size_spec();
        assert_eq!(step_size(16, 1, &hl), 18);
        assert_eq!(step_size(4, -1, &hl), 4);
        let num = Tool::Number.size_spec();
        assert_eq!(step_size(28, -1, &num), 26);
        assert_eq!(step_size(16, -1, &num), 16);
        let mosaic = Tool::Mosaic.size_spec();
        assert_eq!(step_size(12, 2, &mosaic), 16);
        assert_eq!(step_size(64, 1, &mosaic), 64);
    }

    #[test]
    fn size_presets_are_ascending_and_inside_the_config_range() {
        for tool in Tool::ALL {
            let presets = tool.size_presets();
            let spec = tool.size_spec();
            assert!(presets.windows(2).all(|w| w[0] < w[1]), "{tool:?}");
            for value in presets {
                assert!(spec.range.contains(&value), "{tool:?} 预设 {value} 越界");
            }
        }
        assert_eq!(Tool::Rectangle.size_presets(), [2, 4, 8]);
        assert_eq!(Tool::Highlighter.size_presets(), [12, 20, 32]);
        assert_eq!(Tool::Mosaic.size_presets(), [8, 16, 32]);
        assert_eq!(Tool::Number.size_presets(), [20, 28, 40]);
    }

    #[test]
    fn text_tool_presets_are_16_24_36() {
        assert_eq!(Tool::Text.size_presets(), [16, 24, 36]);
        assert_eq!(Tool::Text.label(), "文字");
        assert!(Tool::Text.has_color());
        assert_eq!(*Tool::Text.size_spec().range.start(), 12);
        assert_eq!(*Tool::Text.size_spec().range.end(), 16_384);
        assert_eq!(Tool::Text.size_spec().step, 2);
        assert_eq!(step_size(24, 1, &Tool::Text.size_spec()), 26);
        assert_eq!(step_size(12, -1, &Tool::Text.size_spec()), 12);
    }

    #[test]
    fn every_tool_has_label_and_size_spec() {
        let mut labels: Vec<_> = Tool::ALL.iter().map(|t| t.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), Tool::ALL.len());
        for tool in Tool::ALL {
            let spec = tool.size_spec();
            assert!(spec.step > 0 && spec.range.start() <= spec.range.end());
            assert_eq!(tool.has_color(), tool != Tool::Mosaic);
        }
    }

    #[test]
    fn shape_reports_its_tool() {
        assert_eq!(line().tool(), Tool::Line);
        assert_eq!(number(1, NumberStyle::Number).tool(), Tool::Number);
        let mosaic = Shape::Mosaic {
            from: p(0, 0),
            to: p(5, 5),
            block: 8,
        };
        assert_eq!(mosaic.tool(), Tool::Mosaic);
    }

    #[test]
    fn tiny_drags_are_rejected() {
        assert!(!drag_is_valid(p(5, 5), p(5, 5)));
        assert!(!drag_is_valid(p(5, 5), p(6, 5)));
        assert!(!drag_is_valid(p(5, 5), p(6, 6)));
        assert!(drag_is_valid(p(5, 5), p(7, 5)));
        assert!(drag_is_valid(p(5, 5), p(5, 3)));
        assert!(drag_is_valid(p(0, 0), p(i32::MAX, i32::MIN)));
    }

    #[test]
    fn square_constraint_keeps_direction() {
        assert_eq!(constrain_square(p(10, 10), p(40, 25)), p(40, 40));
        assert_eq!(constrain_square(p(10, 10), p(15, -20)), p(40, -20));
        assert_eq!(constrain_square(p(10, 10), p(-5, 12)), p(-5, 25));
        assert_eq!(constrain_square(p(10, 10), p(10, 10)), p(10, 10));
        assert_eq!(constrain_square(p(0, 0), p(0, 30)), p(30, 30));
    }

    #[test]
    fn angle_snaps_to_multiples_of_45_degrees() {
        let o = p(100, 100);
        assert_eq!(snap_angle(o, p(200, 110)), p(200, 100));
        assert_eq!(snap_angle(o, p(110, 200)), p(100, 200));
        assert_eq!(snap_angle(o, p(0, 95)), p(0, 100));
        assert_eq!(snap_angle(o, p(95, 0)), p(100, 0));
        assert_eq!(snap_angle(o, o), o);
        // 斜向：长度 √(60²+80²)=100，每轴约 71
        assert_eq!(snap_angle(o, p(160, 180)), p(171, 171));
        assert_eq!(snap_angle(o, p(40, 20)), p(29, 29));
        assert_eq!(snap_angle(o, p(40, 180)), p(29, 171));
        assert_eq!(snap_angle(o, p(160, 20)), p(171, 29));
    }

    #[test]
    fn angle_snap_survives_extreme_coordinates() {
        let r = snap_angle(p(i32::MIN, i32::MIN), p(i32::MAX, i32::MAX));
        assert_eq!(
            i64::from(r.x) - i64::from(i32::MIN),
            i64::from(r.y) - i64::from(i32::MIN)
        );
        let r = snap_angle(p(i32::MAX, 0), p(i32::MIN, 10));
        assert_eq!(r.y, 0);
    }

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
    }

    #[test]
    fn arrow_head_scales_with_width() {
        // 向右、线宽 3：头长 max(12, 10) = 12，半宽 5.4
        let head = arrow_head(p(0, 0), p(100, 0), 3).expect("箭头");
        assert!(close(head.left, (-12.0, 5.4)), "{:?}", head.left);
        assert!(close(head.right, (-12.0, -5.4)), "{:?}", head.right);
        assert!(close(head.shaft_end, (-9.6, 0.0)), "{:?}", head.shaft_end);
        // 线宽 1：4 小于下限，头长取 10
        let thin = arrow_head(p(0, 0), p(0, 100), 1).expect("箭头");
        assert!(close(thin.left, (-4.5, -10.0)), "{:?}", thin.left);
        assert!(close(thin.right, (4.5, -10.0)), "{:?}", thin.right);
        // 线宽 32：头长 128
        let wide = arrow_head(p(0, 0), p(300, 0), 32).expect("箭头");
        assert!(close(wide.left, (-128.0, 57.6)), "{:?}", wide.left);
    }

    #[test]
    fn arrow_head_shrinks_on_short_segment_and_rejects_zero() {
        let short = arrow_head(p(0, 0), p(6, 0), 3).expect("箭头");
        assert!(close(short.left, (-6.0, 2.7)), "{:?}", short.left);
        assert_eq!(arrow_head(p(5, 5), p(5, 5), 3), None);
        let diagonal = arrow_head(p(0, 0), p(50, 50), 3).expect("箭头");
        assert!(diagonal.left.0 < 0.0 || diagonal.left.1 < 0.0);
        let back = arrow_head(p(0, 0), p(-100, 0), 3).expect("箭头");
        assert!(close(back.left, (12.0, -5.4)), "{:?}", back.left);
    }

    /// 每个像素的 R 记为 x，G 记为 y，B 固定 7，便于核对平均值。
    fn gradient(width: u32, height: u32, order: PixelOrder) -> Vec<u8> {
        let mut out = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let (x, y) = (u8::try_from(x).expect("小"), u8::try_from(y).expect("小"));
                match order {
                    PixelOrder::Rgba => out.extend_from_slice(&[x, y, 7, 255]),
                    PixelOrder::Bgra => out.extend_from_slice(&[7, y, x, 255]),
                }
            }
        }
        out
    }

    #[test]
    fn mosaic_averages_blocks_and_handles_partial_edges() {
        let pixels = gradient(8, 8, PixelOrder::Rgba);
        // 区域 (1,1) 起 5×5，块 2 → 3×3 块，右、下边缘各有一列/行只有 1 像素
        let grid = mosaic_grid(&pixels, (8, 8), PixelOrder::Rgba, &Rect::new(1, 1, 5, 5), 2)
            .expect("网格");
        assert_eq!((grid.cols, grid.rows), (3, 3));
        assert_eq!(grid.colors.len(), 9);
        assert_eq!(grid.area, Rect::new(1, 1, 5, 5));
        // 第一块覆盖 x∈{1,2}、y∈{1,2}：R 平均 (1+2+1+2)/4 = 1.5 → 2，G 同理
        assert_eq!(grid.colors.first(), Some(&Color::rgb(2, 2, 7)));
        // 最后一块只覆盖 (5,5)
        assert_eq!(grid.colors.last(), Some(&Color::rgb(5, 5, 7)));
        // 中间行最右一块覆盖 x=5、y∈{3,4}：G 平均 3.5 → 4
        assert_eq!(grid.colors.get(5), Some(&Color::rgb(5, 4, 7)));
    }

    #[test]
    fn mosaic_respects_channel_order() {
        let rgba = gradient(4, 4, PixelOrder::Rgba);
        let bgra = gradient(4, 4, PixelOrder::Bgra);
        let area = Rect::new(0, 0, 4, 4);
        let a = mosaic_grid(&rgba, (4, 4), PixelOrder::Rgba, &area, 4).expect("网格");
        let b = mosaic_grid(&bgra, (4, 4), PixelOrder::Bgra, &area, 4).expect("网格");
        assert_eq!(a, b);
        assert_eq!(a.colors, vec![Color::rgb(2, 2, 7)]);
    }

    #[test]
    fn mosaic_clips_area_to_image_and_rejects_bad_input() {
        let pixels = gradient(4, 4, PixelOrder::Rgba);
        let grid = mosaic_grid(
            &pixels,
            (4, 4),
            PixelOrder::Rgba,
            &Rect::new(-3, -3, 20, 20),
            3,
        )
        .expect("网格");
        assert_eq!(grid.area, Rect::new(0, 0, 4, 4));
        assert_eq!((grid.cols, grid.rows), (2, 2));
        assert!(
            mosaic_grid(
                &pixels,
                (4, 4),
                PixelOrder::Rgba,
                &Rect::new(10, 10, 5, 5),
                2
            )
            .is_none()
        );
        assert!(
            mosaic_grid(&pixels, (4, 4), PixelOrder::Rgba, &Rect::new(0, 0, 4, 4), 0).is_none()
        );
        assert!(
            mosaic_grid(&pixels, (4, 5), PixelOrder::Rgba, &Rect::new(0, 0, 4, 4), 2).is_none()
        );
        assert!(
            mosaic_grid(
                &pixels[..8],
                (4, 4),
                PixelOrder::Rgba,
                &Rect::new(0, 0, 1, 1),
                2
            )
            .is_none()
        );
        assert!(
            mosaic_grid(&pixels, (4, 4), PixelOrder::Rgba, &Rect::new(0, 0, 0, 4), 2).is_none()
        );
    }

    #[test]
    fn mosaic_block_larger_than_area_gives_single_block() {
        let pixels = gradient(8, 8, PixelOrder::Rgba);
        let grid = mosaic_grid(
            &pixels,
            (8, 8),
            PixelOrder::Rgba,
            &Rect::new(2, 2, 3, 2),
            64,
        )
        .expect("网格");
        assert_eq!((grid.cols, grid.rows), (1, 1));
        // x∈{2,3,4} 平均 3，y∈{2,3} 平均 2.5 → 3
        assert_eq!(grid.colors, vec![Color::rgb(3, 3, 7)]);
    }

    fn rect_shape(from: Point, to: Point, width: u32, rotation: i32) -> Shape {
        Shape::Rectangle {
            from,
            to,
            color: PALETTE[0],
            width,
            rotation,
        }
    }

    fn ellipse_shape(from: Point, to: Point, width: u32, rotation: i32) -> Shape {
        Shape::Ellipse {
            from,
            to,
            color: PALETTE[1],
            width,
            rotation,
        }
    }

    #[test]
    fn shape_bounds_and_box_handle_all_shapes_and_edges() {
        let r = rect_shape(p(200, 150), p(100, 100), 4, 359);
        assert_eq!(shape_bounds(&r), Some(Rect::new(100, 100, 100, 50)));
        assert_eq!(
            shape_box(&r),
            Some(RotatedBox::new(Rect::new(100, 100, 100, 50), 359))
        );
        let l = Shape::Line {
            from: p(10, 20),
            to: p(50, 60),
            color: PALETTE[0],
            width: 3,
        };
        assert_eq!(shape_bounds(&l), Some(Rect::new(10, 20, 40, 40)));
        assert_eq!(shape_box(&l), None);
        let a = Shape::Arrow {
            from: p(-50, -20),
            to: p(-10, -40),
            color: PALETTE[0],
            width: 3,
        };
        assert_eq!(shape_bounds(&a), Some(Rect::new(-50, -40, 40, 20)));
        assert_eq!(shape_box(&a), None);
        let empty_pen = Shape::Pen {
            points: Vec::new(),
            color: PALETTE[0],
            width: 2,
        };
        assert_eq!(shape_bounds(&empty_pen), None);
        let num = Shape::Number {
            center: p(100, 100),
            value: 1,
            style: NumberStyle::Number,
            color: PALETTE[0],
            size: 20,
        };
        assert_eq!(shape_bounds(&num), Some(Rect::new(90, 90, 20, 20)));
        assert_eq!(
            shape_box(&num),
            Some(RotatedBox::new(Rect::new(90, 90, 20, 20), 0))
        );
    }

    #[test]
    fn shape_hits_strokes_only_near_border_and_fills_mosaic_number() {
        let rect = rect_shape(p(100, 100), p(200, 180), 4, 0);
        // 空心内部不命中，边线附近命中
        assert!(!shape_hits(&rect, p(150, 140), 6.0));
        assert!(shape_hits(&rect, p(100, 140), 6.0));
        assert!(shape_hits(&rect, p(93, 140), 6.0));
        assert!(!shape_hits(&rect, p(90, 140), 6.0));

        // 旋转 90° 的矩形（中心 150, 140，原 100x80 旋转后上下边在 y = 140 ± 50）
        let rot_rect = rect_shape(p(100, 100), p(200, 180), 2, 90);
        assert!(shape_hits(&rot_rect, p(150, 90), 6.0));
        assert!(!shape_hits(&rot_rect, p(150, 140), 6.0));

        let ell = ellipse_shape(p(100, 100), p(220, 160), 4, 0);
        assert!(!shape_hits(&ell, p(160, 130), 6.0));
        assert!(shape_hits(&ell, p(160, 100), 6.0));
        assert!(shape_hits(&ell, p(220, 130), 6.0));
        // 旋转 90° 后椭圆顶端点落在 (160, 70) 与 (160, 190)
        let rot_ell = ellipse_shape(p(100, 100), p(220, 160), 4, 90);
        assert!(shape_hits(&rot_ell, p(160, 70), 6.0));
        assert!(!shape_hits(&rot_ell, p(220, 130), 6.0));

        let arrow = Shape::Arrow {
            from: p(0, 0),
            to: p(100, 0),
            color: PALETTE[0],
            width: 2,
        };
        assert!(shape_hits(&arrow, p(50, 5), 6.0));
        assert!(shape_hits(&arrow, p(92, 4), 2.0));
        assert!(!shape_hits(&arrow, p(50, 20), 6.0));

        let pen = Shape::Pen {
            points: vec![p(0, 0), p(50, 0), p(50, 50)],
            color: PALETTE[0],
            width: 4,
        };
        assert!(shape_hits(&pen, p(50, 25), 6.0));
        assert!(!shape_hits(&pen, p(10, 40), 6.0));

        let mosaic = Shape::Mosaic {
            from: p(-40, -40),
            to: p(-10, -10),
            block: 8,
        };
        assert!(shape_hits(&mosaic, p(-25, -25), 6.0));
        assert!(!shape_hits(&mosaic, p(-10, -10), 6.0));

        let num = Shape::Number {
            center: p(50, 50),
            value: 1,
            style: NumberStyle::Number,
            color: PALETTE[0],
            size: 20,
        };
        assert!(shape_hits(&num, p(50, 50), 6.0));
        assert!(shape_hits(&num, p(60, 50), 6.0));
        assert!(!shape_hits(&num, p(61, 50), 6.0));
    }

    #[test]
    fn document_shape_at_picks_topmost_shape() {
        let mut doc = Document::new();
        doc.push(rect_shape(p(0, 0), p(100, 100), 4, 0));
        doc.push(Shape::Line {
            from: p(0, 50),
            to: p(100, 50),
            color: PALETTE[1],
            width: 4,
        });
        assert_eq!(doc.shape_at(p(0, 50), 6.0), Some(1));
        assert_eq!(doc.shape_at(p(50, 0), 6.0), Some(0));
        // 矩形空心处不命中
        assert_eq!(doc.shape_at(p(50, 20), 6.0), None);
    }

    #[test]
    fn hit_selected_shape_prioritizes_handles_and_allows_interior_body_drag() {
        let rect = rect_shape(p(100, 100), p(200, 160), 4, 0);
        // 选中图元本体按外接框命中，空心内部也可以直接拖动
        assert_eq!(
            hit_selected_shape(&rect, p(150, 130), 6.0, 24.0),
            Some(ShapeHandle::Body)
        );
        assert_eq!(
            hit_selected_shape(&rect, p(100, 100), 6.0, 24.0),
            Some(ShapeHandle::Corner(BoxCorner::TopLeft))
        );
        assert_eq!(
            hit_selected_shape(&rect, p(200, 130), 6.0, 24.0),
            Some(ShapeHandle::Edge(BoxEdge::Right))
        );
        assert_eq!(
            hit_selected_shape(&rect, p(150, 76), 6.0, 24.0),
            Some(ShapeHandle::Rotate)
        );
        // 马赛克没有旋转手柄
        let mosaic = Shape::Mosaic {
            from: p(100, 100),
            to: p(200, 160),
            block: 8,
        };
        assert_eq!(hit_selected_shape(&mosaic, p(150, 76), 6.0, 24.0), None);
        // 直线/箭头只命中两端点或本体
        let arrow = Shape::Arrow {
            from: p(10, 10),
            to: p(100, 10),
            color: PALETTE[0],
            width: 4,
        };
        assert_eq!(
            hit_selected_shape(&arrow, p(10, 10), 6.0, 24.0),
            Some(ShapeHandle::Endpoint(Endpoint::From))
        );
        assert_eq!(
            hit_selected_shape(&arrow, p(100, 10), 6.0, 24.0),
            Some(ShapeHandle::Endpoint(Endpoint::To))
        );
        assert_eq!(
            hit_selected_shape(&arrow, p(50, 10), 6.0, 24.0),
            Some(ShapeHandle::Body)
        );
    }

    #[test]
    fn translate_and_scale_points_cover_negative_zero_and_flips() {
        let moved = translate_shape(&rect_shape(p(10, 20), p(50, 60), 2, 45), -30, -40);
        assert_eq!(moved, rect_shape(p(-20, -20), p(20, 20), 2, 45));

        let moved_text = translate_shape(&text("hi", 10), -5, 15);
        assert_eq!(shape_bounds(&moved_text), Some(Rect::new(5, 15, 20, 20)));

        let pts = vec![p(100, 100), p(120, 140), p(200, 180)];
        let from = Rect::new(100, 100, 100, 80);
        let to = Rect::new(0, 0, 200, 160);
        assert_eq!(
            scale_points(&pts, &from, &to, false, false),
            vec![p(0, 0), p(40, 80), p(200, 160)]
        );
        // 水平翻转
        assert_eq!(
            scale_points(&pts, &from, &to, true, false),
            vec![p(200, 0), p(160, 80), p(0, 160)]
        );
        // 零宽度轨迹缩放不出现 NaN
        let flat = vec![p(50, 10), p(50, 50)];
        let flat_bounds = Rect::new(50, 10, 0, 40);
        assert_eq!(
            scale_points(&flat, &flat_bounds, &Rect::new(80, 20, 0, 80), false, true),
            vec![p(80, 100), p(80, 20)]
        );
    }

    #[test]
    fn resize_shape_handles_rotation_flip_shift_and_min_size() {
        let r = rect_shape(p(100, 100), p(200, 150), 4, 0);
        let rotated = resize_shape(&r, ShapeHandle::Rotate, p(250, 125), false);
        assert!(matches!(rotated, Shape::Rectangle { rotation: 90, .. }));
        let snapped = resize_shape(&r, ShapeHandle::Rotate, p(250, 95), true);
        assert!(matches!(snapped, Shape::Rectangle { rotation: 75, .. }));

        // Shift 拖角保持宽高比 (100x50 = 2:1)
        let kept = resize_shape(
            &r,
            ShapeHandle::Corner(BoxCorner::BottomRight),
            p(300, 160),
            true,
        );
        assert_eq!(shape_bounds(&kept), Some(Rect::new(100, 100, 200, 100)));

        // 拖过对边翻转且夹紧最小尺寸 2x2
        let flipped = resize_shape(
            &r,
            ShapeHandle::Corner(BoxCorner::BottomRight),
            p(60, 70),
            false,
        );
        assert_eq!(shape_bounds(&flipped), Some(Rect::new(60, 70, 40, 30)));
        let min_clamped = resize_shape(
            &r,
            ShapeHandle::Corner(BoxCorner::BottomRight),
            p(100, 100),
            false,
        );
        assert_eq!(shape_bounds(&min_clamped), Some(Rect::new(100, 100, 2, 2)));

        // 直线端点拖拽支持 Shift 45° 吸附且不会缩成零长度
        let l = Shape::Line {
            from: p(100, 100),
            to: p(200, 100),
            color: PALETTE[0],
            width: 3,
        };
        let snapped_line = resize_shape(&l, ShapeHandle::Endpoint(Endpoint::To), p(200, 110), true);
        assert!(matches!(
            snapped_line,
            Shape::Line { from, to, .. } if from == p(100, 100) && to == p(200, 100)
        ));
        let collapsed = resize_shape(&l, ShapeHandle::Endpoint(Endpoint::To), p(100, 100), false);
        assert!(matches!(collapsed, Shape::Line { from, to, .. } if drag_is_valid(from, to)));

        // 画笔按外接框缩放与翻转
        let pen = Shape::Pen {
            points: vec![p(100, 100), p(120, 140), p(180, 140)],
            color: PALETTE[0],
            width: 4,
        };
        let flipped_pen = resize_shape(&pen, ShapeHandle::Edge(BoxEdge::Right), p(20, 120), false);
        assert!(matches!(
            flipped_pen,
            Shape::Pen { points, .. } if points == vec![p(100, 100), p(80, 140), p(20, 140)]
        ));
    }
}
