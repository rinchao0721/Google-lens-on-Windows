//! 自绘控件：只负责绘制与交互，不含业务逻辑。

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, Rect, Response, RichText,
    Sense, Stroke, StrokeKind, Ui, WidgetInfo, WidgetType, pos2, vec2,
};

use crate::fonts::icon_family;
use crate::theme::{
    CARD_PAD_X, CARD_PAD_Y, CARD_RADIUS, CONTROL_RADIUS, FONT_CARD_TITLE, FONT_DESC, FONT_ICON,
    FONT_KEYCAP, FONT_TITLE, Look, NAV_BAR_WIDTH,
};

/// 提示卡片的级别。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BannerKind {
    /// 警告。
    Warn,
    /// 错误。
    Error,
}

/// 页面大标题。
pub fn page_title(ui: &mut Ui, look: &Look, text: &str) {
    ui.label(
        RichText::new(text)
            .size(FONT_TITLE)
            .color(look.palette.text),
    );
    ui.add_space(8.0);
}

/// 页面内的分组小标题。
pub fn group_label(ui: &mut Ui, look: &Look, text: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(text).size(13.0).color(look.palette.text_dim));
}

/// 带色框的提示卡片。
pub fn banner(ui: &mut Ui, look: &Look, kind: BannerKind, text: &str) {
    let p = &look.palette;
    let color = match kind {
        BannerKind::Warn => p.warn,
        BannerKind::Error => p.error,
    };
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0_f32, color))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::symmetric(CARD_PAD_X, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
        });
}

/// 导航栏的一项：整行浅底高亮，选中时左侧有强调色竖条。
pub fn nav_item(ui: &mut Ui, look: &Look, selected: bool, icon: char, label: &str) -> Response {
    let p = &look.palette;
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, label));
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let inner = rect.shrink2(vec2(6.0, 2.0));
    let fill = if selected {
        p.selected_row
    } else if response.hovered() {
        p.hover_row
    } else {
        Color32::TRANSPARENT
    };
    let painter = ui.painter();
    painter.rect_filled(inner, CornerRadius::same(CONTROL_RADIUS), fill);
    if selected {
        let bar = Rect::from_min_size(
            pos2(inner.left(), inner.center().y - 8.0),
            vec2(NAV_BAR_WIDTH, 16.0),
        );
        painter.rect_filled(bar, CornerRadius::same(2), p.accent);
    }
    let mut x = inner.left() + 14.0;
    if look.icons {
        let color = if selected { p.accent } else { p.text };
        painter.text(
            pos2(x, inner.center().y),
            Align2::LEFT_CENTER,
            icon,
            FontId::new(FONT_ICON, icon_family()),
            color,
        );
        x += 30.0;
    }
    painter.text(
        pos2(x, inner.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(crate::theme::FONT_BODY),
        p.text,
    );
    response
}

/// Win11 式开关。
pub fn toggle(ui: &mut Ui, look: &Look, on: &mut bool) -> Response {
    let p = &look.palette;
    let (rect, mut response) = ui.allocate_exact_size(vec2(40.0, 20.0), Sense::click());
    response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let enabled = ui.is_enabled();
    let value = *on;
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, value, ""));
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_with_time(response.id, value, 0.1);
        let off_stroke = if response.hovered() {
            p.text
        } else {
            p.text_dim
        };
        let fill = p.card.lerp_to_gamma(p.accent, t);
        let stroke_color = off_stroke.lerp_to_gamma(p.accent, t);
        let radius = CornerRadius::same(10);
        let painter = ui.painter();
        painter.rect_filled(rect, radius, fill);
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(1.0_f32, stroke_color),
            StrokeKind::Inside,
        );
        let knob_x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
        let knob_color = off_stroke.lerp_to_gamma(p.accent_text, t);
        painter.circle_filled(pos2(knob_x, rect.center().y), 5.5, knob_color);
    }
    response
}

/// 胶囊按钮的外观。
#[derive(Clone, Copy, PartialEq, Eq)]
enum PillStyle {
    Primary,
    Secondary,
    Chip { selected: bool },
}

fn pill(
    ui: &mut Ui,
    look: &Look,
    label: &str,
    style: PillStyle,
    height: f32,
    min_w: f32,
) -> Response {
    let p = &look.palette;
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        FontId::proportional(crate::theme::FONT_BODY),
        p.text,
    );
    let width = (galley.size().x + 28.0).max(min_w);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let pressed = response.is_pointer_button_down_on();
        let accent_fill = |base: Color32| {
            if pressed {
                base.gamma_multiply(0.8)
            } else if hovered {
                base.lerp_to_gamma(Color32::WHITE, 0.12)
            } else {
                base
            }
        };
        let neutral_fill = if hovered { p.button_hover } else { p.button };
        let (fill, text_color) = match style {
            PillStyle::Primary => (accent_fill(p.accent), p.accent_text),
            PillStyle::Chip { selected: true } => (accent_fill(p.accent), p.accent_text),
            PillStyle::Secondary | PillStyle::Chip { selected: false } => (neutral_fill, p.text),
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(CONTROL_RADIUS), fill);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(crate::theme::FONT_BODY),
            text_color,
        );
    }
    response
}

/// 强调色主按钮。
pub fn primary_button(ui: &mut Ui, look: &Look, label: &str) -> Response {
    pill(ui, look, label, PillStyle::Primary, 30.0, 76.0)
}

/// 次级按钮。
pub fn secondary_button(ui: &mut Ui, look: &Look, label: &str) -> Response {
    pill(ui, look, label, PillStyle::Secondary, 30.0, 76.0)
}

/// 修饰键开关小方块：选中时强调色底。
pub fn modifier_chip(ui: &mut Ui, look: &Look, on: &mut bool, label: &str) -> Response {
    let mut response = pill(
        ui,
        look,
        label,
        PillStyle::Chip { selected: *on },
        26.0,
        52.0,
    );
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response
}

/// 一组按键小方块，如 Ctrl、Alt、F13。
pub fn key_caps(ui: &mut Ui, look: &Look, labels: &[String]) {
    let p = &look.palette;
    ui.spacing_mut().item_spacing.x = 4.0;
    // right_to_left 布局下逆序添加，视觉顺序仍与 labels 一致
    let ordered: Vec<&String> =
        if ui.layout().main_dir().is_horizontal() && ui.layout().prefer_right_to_left() {
            labels.iter().rev().collect()
        } else {
            labels.iter().collect()
        };
    for label in ordered {
        let galley =
            ui.painter()
                .layout_no_wrap(label.clone(), FontId::monospace(FONT_KEYCAP), p.text);
        let size = vec2((galley.size().x + 16.0).max(28.0), 24.0);
        let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, label));
        if ui.is_rect_visible(rect) {
            let radius = CornerRadius::same(CONTROL_RADIUS);
            ui.painter().rect_filled(rect, radius, p.button);
            ui.painter().rect_stroke(
                rect,
                radius,
                Stroke::new(1.0_f32, p.card_border),
                StrokeKind::Inside,
            );
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::monospace(FONT_KEYCAP),
                p.text,
            );
        }
    }
}

/// 单行文本框，宽度固定。
pub fn text_input(ui: &mut Ui, text: &mut String, width: f32) -> Response {
    ui.add(
        egui::TextEdit::singleline(text)
            .desired_width(width)
            .margin(Margin::symmetric(8, 5)),
    )
}

/// 设置卡片：左侧图标、标题与说明，右侧放控件。
pub struct Card<'a> {
    title: &'a str,
    desc: Option<&'a str>,
    error: bool,
    icon: Option<char>,
    right_width: f32,
}

impl<'a> Card<'a> {
    /// 以标题新建卡片。
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            desc: None,
            error: false,
            icon: None,
            right_width: 200.0,
        }
    }

    /// 左侧图标。
    pub fn icon(mut self, icon: char) -> Self {
        self.icon = Some(icon);
        self
    }

    /// 说明文字。
    pub fn desc(mut self, desc: &'a str) -> Self {
        self.desc = Some(desc);
        self
    }

    /// 说明文字替换为错误提示。
    pub fn error(mut self, message: Option<&'a str>) -> Self {
        if let Some(message) = message {
            self.desc = Some(message);
            self.error = true;
        }
        self
    }

    /// 右侧控件预留宽度，决定说明文字的换行位置。
    pub fn right_width(mut self, width: f32) -> Self {
        self.right_width = width;
        self
    }

    /// 单行卡片：右侧放控件。
    pub fn show(self, ui: &mut Ui, look: &Look, add_right: impl FnOnce(&mut Ui)) {
        self.render(ui, look, add_right, |_| {});
    }

    /// 带整宽内容区的卡片：标题行右侧放控件，下方放整宽内容。
    pub fn show_wide(
        self,
        ui: &mut Ui,
        look: &Look,
        add_right: impl FnOnce(&mut Ui),
        add_body: impl FnOnce(&mut Ui),
    ) {
        self.render(ui, look, add_right, add_body);
    }

    fn render(
        self,
        ui: &mut Ui,
        look: &Look,
        add_right: impl FnOnce(&mut Ui),
        add_body: impl FnOnce(&mut Ui),
    ) {
        let p = &look.palette;
        egui::Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0_f32, p.card_border))
            .corner_radius(CornerRadius::same(CARD_RADIUS))
            .inner_margin(Margin::symmetric(CARD_PAD_X, CARD_PAD_Y))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical(|ui| {
                    self.header(ui, look, add_right);
                    ui.add_space(2.0);
                    add_body(ui);
                });
            });
    }

    fn header(&self, ui: &mut Ui, look: &Look, add_right: impl FnOnce(&mut Ui)) {
        let p = &look.palette;
        let show_icon = look.icons && self.icon.is_some();
        let icon_w = if show_icon { 32.0 } else { 0.0 };
        let text_max = (ui.available_width() - icon_w - self.right_width - 16.0).max(120.0);
        ui.horizontal(|ui| {
            if let (true, Some(icon)) = (show_icon, self.icon) {
                let (rect, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                ui.painter().text(
                    rect.left_center(),
                    Align2::LEFT_CENTER,
                    icon,
                    FontId::new(FONT_ICON, icon_family()),
                    p.text,
                );
            }
            let left = ui.vertical(|ui| {
                ui.set_max_width(text_max);
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(
                    RichText::new(self.title)
                        .size(FONT_CARD_TITLE)
                        .color(p.text),
                );
                if let Some(desc) = self.desc {
                    let color = if self.error { p.error } else { p.text_dim };
                    ui.add(
                        egui::Label::new(RichText::new(desc).size(FONT_DESC).color(color)).wrap(),
                    );
                }
            });
            let height = left.response.rect.height().max(28.0);
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), height),
                Layout::right_to_left(Align::Center),
                add_right,
            );
        });
    }
}
