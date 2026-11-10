//! 主题：配色、字号、间距常量，以及把它们写进 egui 样式。

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Theme, Visuals,
};

/// 导航栏宽度。
pub const NAV_WIDTH: f32 = 170.0;
/// 导航项选中竖条宽度。
pub const NAV_BAR_WIDTH: f32 = 3.0;
/// 卡片圆角。
pub const CARD_RADIUS: u8 = 6;
/// 卡片之间的垂直间距。
pub const CARD_GAP: f32 = 6.0;
/// 卡片水平内边距。
pub const CARD_PAD_X: i8 = 14;
/// 卡片垂直内边距。
pub const CARD_PAD_Y: i8 = 11;
/// 控件圆角。
pub const CONTROL_RADIUS: u8 = 4;
/// 页面左右留白。
pub const PAGE_MARGIN_X: i8 = 24;
/// 页面标题字号。
pub const FONT_TITLE: f32 = 22.0;
/// 正文字号。
pub const FONT_BODY: f32 = 13.5;
/// 卡片标题字号。
pub const FONT_CARD_TITLE: f32 = 14.0;
/// 说明文字字号。
pub const FONT_DESC: f32 = 11.5;
/// 按键小方块字号。
pub const FONT_KEYCAP: f32 = 12.0;
/// 图标字号。
pub const FONT_ICON: f32 = 16.0;

/// 深浅两套配色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeKind {
    /// 深色。
    Dark,
    /// 浅色。
    Light,
}

/// 由系统是否深色得出主题；读不到时用深色。
pub fn resolve(system_is_dark: Option<bool>) -> ThemeKind {
    if system_is_dark.unwrap_or(true) {
        ThemeKind::Dark
    } else {
        ThemeKind::Light
    }
}

/// egui 报告的系统主题转成本模块的主题。
pub fn resolve_system(system: Option<Theme>) -> ThemeKind {
    resolve(system.map(|t| t == Theme::Dark))
}

/// 一套配色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// 窗口底部栏背景。
    pub window: Color32,
    /// 导航栏背景。
    pub nav: Color32,
    /// 内容区背景。
    pub content: Color32,
    /// 卡片背景。
    pub card: Color32,
    /// 卡片边框。
    pub card_border: Color32,
    /// 次级按钮与控件底色。
    pub button: Color32,
    /// 次级按钮悬停底色。
    pub button_hover: Color32,
    /// 输入框底色。
    pub input: Color32,
    /// 导航选中行底色。
    pub selected_row: Color32,
    /// 导航悬停行底色。
    pub hover_row: Color32,
    /// 正文。
    pub text: Color32,
    /// 说明文字。
    pub text_dim: Color32,
    /// 强调色。
    pub accent: Color32,
    /// 强调色底上的文字。
    pub accent_text: Color32,
    /// 错误。
    pub error: Color32,
    /// 警告。
    pub warn: Color32,
}

impl Palette {
    /// 深色配色。
    pub const DARK: Palette = Palette {
        window: Color32::from_rgb(0x20, 0x20, 0x20),
        nav: Color32::from_rgb(0x20, 0x20, 0x20),
        content: Color32::from_rgb(0x27, 0x27, 0x27),
        card: Color32::from_rgb(0x2d, 0x2d, 0x2d),
        card_border: Color32::from_rgb(0x35, 0x35, 0x35),
        button: Color32::from_rgb(0x3a, 0x3a, 0x3a),
        button_hover: Color32::from_rgb(0x45, 0x45, 0x45),
        input: Color32::from_rgb(0x1f, 0x1f, 0x1f),
        selected_row: Color32::from_rgb(0x32, 0x32, 0x32),
        hover_row: Color32::from_rgb(0x2a, 0x2a, 0x2a),
        text: Color32::from_rgb(0xf2, 0xf2, 0xf2),
        text_dim: Color32::from_rgb(0xaa, 0xaa, 0xaa),
        accent: Color32::from_rgb(0x4c, 0xc2, 0xff),
        accent_text: Color32::BLACK,
        error: Color32::from_rgb(0xff, 0x99, 0xa4),
        warn: Color32::from_rgb(0xff, 0xd3, 0x7a),
    };

    /// 浅色配色。
    pub const LIGHT: Palette = Palette {
        window: Color32::from_rgb(0xf3, 0xf3, 0xf3),
        nav: Color32::from_rgb(0xf3, 0xf3, 0xf3),
        content: Color32::from_rgb(0xf9, 0xf9, 0xf9),
        card: Color32::WHITE,
        card_border: Color32::from_rgb(0xe5, 0xe5, 0xe5),
        button: Color32::from_rgb(0xec, 0xec, 0xec),
        button_hover: Color32::from_rgb(0xe0, 0xe0, 0xe0),
        input: Color32::WHITE,
        selected_row: Color32::from_rgb(0xe6, 0xe6, 0xe6),
        hover_row: Color32::from_rgb(0xec, 0xec, 0xec),
        text: Color32::from_rgb(0x1a, 0x1a, 0x1a),
        text_dim: Color32::from_rgb(0x5f, 0x5f, 0x5f),
        accent: Color32::from_rgb(0x00, 0x67, 0xc0),
        accent_text: Color32::WHITE,
        error: Color32::from_rgb(0xc4, 0x2b, 0x1c),
        warn: Color32::from_rgb(0x9d, 0x5d, 0x00),
    };

    /// 取主题对应的配色。
    pub const fn of(kind: ThemeKind) -> Palette {
        match kind {
            ThemeKind::Dark => Self::DARK,
            ThemeKind::Light => Self::LIGHT,
        }
    }
}

/// 界面绘制时用到的外观信息。
#[derive(Debug, Clone, Copy)]
pub struct Look {
    /// 当前配色。
    pub palette: Palette,
    /// 图标字体是否可用。
    pub icons: bool,
}

fn corner(radius: u8) -> CornerRadius {
    CornerRadius::same(radius)
}

fn visuals_for(kind: ThemeKind, p: &Palette) -> Visuals {
    let mut v = match kind {
        ThemeKind::Dark => Visuals::dark(),
        ThemeKind::Light => Visuals::light(),
    };
    v.panel_fill = p.content;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0_f32, p.card_border);
    v.window_corner_radius = corner(CARD_RADIUS);
    v.menu_corner_radius = corner(CARD_RADIUS);
    v.extreme_bg_color = p.input;
    v.faint_bg_color = p.card;
    v.error_fg_color = p.error;
    v.warn_fg_color = p.warn;
    v.hyperlink_color = p.accent;
    v.override_text_color = Some(p.text);
    v.selection.bg_fill = p.accent.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0_f32, p.accent);
    let radius = corner(CONTROL_RADIUS);
    let widgets = &mut v.widgets;
    widgets.noninteractive.bg_fill = p.card;
    widgets.noninteractive.weak_bg_fill = p.card;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.card_border);
    for (state, fill) in [
        (&mut widgets.inactive, p.button),
        (&mut widgets.hovered, p.button_hover),
        (&mut widgets.active, p.button_hover),
        (&mut widgets.open, p.button_hover),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::NONE;
        state.fg_stroke = Stroke::new(1.0_f32, p.text);
    }
    widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.accent);
    widgets.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
    for state in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        state.corner_radius = radius;
    }
    v
}

/// 构造某主题的完整样式（字号、间距、颜色）。
pub fn style_for(kind: ThemeKind, p: &Palette) -> egui::Style {
    let mut style = egui::Style {
        visuals: visuals_for(kind, p),
        ..egui::Style::default()
    };
    let proportional = |size: f32| FontId::new(size, FontFamily::Proportional);
    style.text_styles = [
        (TextStyle::Heading, proportional(FONT_TITLE)),
        (TextStyle::Body, proportional(FONT_BODY)),
        (TextStyle::Button, proportional(FONT_BODY)),
        (TextStyle::Small, proportional(FONT_DESC)),
        (
            TextStyle::Monospace,
            FontId::new(FONT_KEYCAP + 0.5, FontFamily::Monospace),
        ),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, CARD_GAP);
    style.spacing.interact_size.y = 28.0;
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    style.spacing.combo_width = 160.0;
    style
}

/// 把某主题的样式装进 egui，并让 egui 切到该主题。
pub fn install(ctx: &egui::Context, kind: ThemeKind) {
    let p = Palette::of(kind);
    let (theme, preference) = match kind {
        ThemeKind::Dark => (Theme::Dark, egui::ThemePreference::Dark),
        ThemeKind::Light => (Theme::Light, egui::ThemePreference::Light),
    };
    ctx.set_theme(preference);
    ctx.set_style_of(theme, style_for(kind, &p));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单个颜色通道的相对亮度分量。
    fn channel_luminance(c: u8) -> f32 {
        let v = f32::from(c) / 255.0;
        if v <= 0.039_28 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }

    /// WCAG 对比度，范围 1–21。
    pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
        let lum = |c: Color32| {
            0.2126 * channel_luminance(c.r())
                + 0.7152 * channel_luminance(c.g())
                + 0.0722 * channel_luminance(c.b())
        };
        let (la, lb) = (lum(a), lum(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn unknown_system_theme_defaults_to_dark() {
        assert_eq!(resolve(None), ThemeKind::Dark);
        assert_eq!(resolve(Some(true)), ThemeKind::Dark);
        assert_eq!(resolve(Some(false)), ThemeKind::Light);
        assert_eq!(resolve_system(None), ThemeKind::Dark);
        assert_eq!(resolve_system(Some(Theme::Light)), ThemeKind::Light);
        assert_eq!(resolve_system(Some(Theme::Dark)), ThemeKind::Dark);
    }

    #[test]
    fn palettes_match_reference_colors() {
        let dark = Palette::of(ThemeKind::Dark);
        assert_eq!(dark.nav, Color32::from_rgb(0x20, 0x20, 0x20));
        assert_eq!(dark.content, Color32::from_rgb(0x27, 0x27, 0x27));
        assert_eq!(dark.card, Color32::from_rgb(0x2d, 0x2d, 0x2d));
        assert_eq!(dark.accent, Color32::from_rgb(0x4c, 0xc2, 0xff));
        assert_eq!(dark.accent_text, Color32::BLACK);
        let light = Palette::of(ThemeKind::Light);
        assert_eq!(light.nav, Color32::from_rgb(0xf3, 0xf3, 0xf3));
        assert_eq!(light.accent, Color32::from_rgb(0x00, 0x67, 0xc0));
        assert_eq!(light.accent_text, Color32::WHITE);
        assert_ne!(dark, light);
    }

    #[test]
    fn text_is_readable_on_its_backgrounds() {
        for kind in [ThemeKind::Dark, ThemeKind::Light] {
            let p = Palette::of(kind);
            assert!(
                contrast_ratio(p.accent_text, p.accent) >= 4.5,
                "{kind:?} 主按钮"
            );
            assert!(contrast_ratio(p.text, p.card) >= 7.0, "{kind:?} 正文");
            assert!(contrast_ratio(p.text_dim, p.card) >= 4.5, "{kind:?} 说明");
            assert!(contrast_ratio(p.error, p.card) >= 4.5, "{kind:?} 错误");
            assert!(contrast_ratio(p.warn, p.card) >= 4.5, "{kind:?} 警告");
            assert!(contrast_ratio(p.text, p.button) >= 4.5, "{kind:?} 次级按钮");
        }
    }

    #[test]
    fn contrast_ratio_extremes() {
        let max = contrast_ratio(Color32::BLACK, Color32::WHITE);
        assert!((max - 21.0).abs() < 0.01);
        assert!((contrast_ratio(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.001);
    }

    #[test]
    fn style_uses_scale_from_constants() {
        let style = style_for(ThemeKind::Dark, &Palette::DARK);
        let body = style.text_styles.get(&TextStyle::Body).map(|f| f.size);
        let title = style.text_styles.get(&TextStyle::Heading).map(|f| f.size);
        assert_eq!(body, Some(FONT_BODY));
        assert_eq!(title, Some(FONT_TITLE));
        assert_eq!(style.visuals.panel_fill, Palette::DARK.content);
    }
}
