//! 加载系统中文字体；egui 自带字体不含中文。

use std::fs;
use std::sync::Arc;

use eframe::egui;
use lenswin_platform::fonts::chinese_font_candidates;

const FONT_KEY: &str = "lenswin-cjk";

/// 读出第一个能读取的中文字体；全部失败返回 `None`。
fn first_readable_font() -> Option<egui::FontData> {
    chinese_font_candidates().into_iter().find_map(|candidate| {
        let bytes = fs::read(&candidate.path).ok()?;
        let mut data = egui::FontData::from_owned(bytes);
        data.index = candidate.index;
        Some(data)
    })
}

/// 把中文字体加到各字体族末尾作为回退，返回是否成功。
pub fn install_chinese_font(ctx: &egui::Context) -> bool {
    let Some(data) = first_readable_font() else {
        return false;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(FONT_KEY.to_string(), Arc::new(data));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push(FONT_KEY.to_string());
    }
    ctx.set_fonts(fonts);
    true
}
