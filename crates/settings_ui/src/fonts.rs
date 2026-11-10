//! 加载系统字体：中文字体（egui 自带字体不含中文）与图标字体。

use std::fs;
use std::sync::Arc;

use eframe::egui;
use lenswin_platform::fonts::{FontCandidate, chinese_font_candidates, icon_font_candidates};

const CJK_KEY: &str = "lenswin-cjk";
const ICON_KEY: &str = "lenswin-icons";
/// 图标字体族名。
pub const ICON_FAMILY: &str = "icons";

/// 字体加载结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontStatus {
    /// 中文字体是否加载成功。
    pub chinese: bool,
    /// 图标字体是否加载成功。
    pub icons: bool,
}

/// 读出第一个能读取的字体；全部失败返回 `None`。
fn first_readable(candidates: Vec<FontCandidate>) -> Option<egui::FontData> {
    candidates.into_iter().find_map(|candidate| {
        let bytes = fs::read(&candidate.path).ok()?;
        let mut data = egui::FontData::from_owned(bytes);
        data.index = candidate.index;
        Some(data)
    })
}

/// 安装中文回退字体和独立的图标字体族。
pub fn install_fonts(ctx: &egui::Context) -> FontStatus {
    let mut fonts = egui::FontDefinitions::default();
    let chinese = first_readable(chinese_font_candidates());
    let icons = first_readable(icon_font_candidates());
    let status = FontStatus {
        chinese: chinese.is_some(),
        icons: icons.is_some(),
    };
    if let Some(data) = chinese {
        fonts.font_data.insert(CJK_KEY.to_string(), Arc::new(data));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push(CJK_KEY.to_string());
        }
    }
    if let Some(data) = icons {
        fonts.font_data.insert(ICON_KEY.to_string(), Arc::new(data));
        fonts.families.insert(
            egui::FontFamily::Name(ICON_FAMILY.into()),
            vec![ICON_KEY.to_string()],
        );
    }
    ctx.set_fonts(fonts);
    status
}

/// 图标字体族。
pub fn icon_family() -> egui::FontFamily {
    egui::FontFamily::Name(ICON_FAMILY.into())
}
