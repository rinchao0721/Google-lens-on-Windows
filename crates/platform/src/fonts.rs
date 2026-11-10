//! 系统字体目录与中文字体候选，供设置窗口加载。

use std::path::{Path, PathBuf};

use windows::Win32::System::SystemInformation::GetWindowsDirectoryW;

/// 中文字体候选：文件名与集合内序号，按优先级排列。
const CHINESE_FONT_FILES: [(&str, u32); 6] = [
    ("msyh.ttc", 0),
    ("msyh.ttf", 0),
    ("msjh.ttc", 0),
    ("simhei.ttf", 0),
    ("simsun.ttc", 0),
    ("mingliu.ttc", 0),
];

/// 图标字体候选：Win11 的 Segoe Fluent Icons 在前，Win10 的 Segoe MDL2 Assets 在后。
const ICON_FONT_FILES: [&str; 2] = ["SegoeIcons.ttf", "segmdl2.ttf"];

/// 一个可尝试加载的字体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontCandidate {
    /// 字体文件完整路径。
    pub path: PathBuf,
    /// 字体集合（ttc）内的序号，单文件字体为 0。
    pub index: u32,
}

/// 系统字体目录，取不到时返回 `None`。
pub fn system_font_dir() -> Option<PathBuf> {
    let mut buffer = [0u16; 260];
    // SAFETY: buffer 是有效的可写缓冲区，长度由切片给出。
    let written = unsafe { GetWindowsDirectoryW(Some(&mut buffer)) };
    let len = usize::try_from(written)
        .ok()
        .filter(|n| *n > 0 && *n < buffer.len())?;
    let windows = String::from_utf16_lossy(buffer.get(..len)?);
    Some(Path::new(&windows).join("Fonts"))
}

/// 在 `dir` 下按优先级列出中文字体候选（不检查文件是否存在）。
pub fn chinese_font_candidates_in(dir: &Path) -> Vec<FontCandidate> {
    CHINESE_FONT_FILES
        .iter()
        .map(|(name, index)| FontCandidate {
            path: dir.join(name),
            index: *index,
        })
        .collect()
}

/// 系统字体目录下的中文字体候选；取不到目录时为空。
pub fn chinese_font_candidates() -> Vec<FontCandidate> {
    system_font_dir()
        .map(|dir| chinese_font_candidates_in(&dir))
        .unwrap_or_default()
}

/// 在 `dir` 下按优先级列出图标字体候选（不检查文件是否存在）。
pub fn icon_font_candidates_in(dir: &Path) -> Vec<FontCandidate> {
    ICON_FONT_FILES
        .iter()
        .map(|name| FontCandidate {
            path: dir.join(name),
            index: 0,
        })
        .collect()
}

/// 系统字体目录下的图标字体候选；取不到目录时为空。
pub fn icon_font_candidates() -> Vec<FontCandidate> {
    system_font_dir()
        .map(|dir| icon_font_candidates_in(&dir))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_start_with_yahei_collection_index_zero() {
        let list = chinese_font_candidates_in(Path::new(r"C:\Windows\Fonts"));
        let first = list.first().expect("至少一个候选");
        assert_eq!(first.path, Path::new(r"C:\Windows\Fonts\msyh.ttc"));
        assert_eq!(first.index, 0);
        assert!(list.len() >= 3);
    }

    #[test]
    fn icon_candidates_prefer_fluent_then_mdl2() {
        let list = icon_font_candidates_in(Path::new(r"C:\Windows\Fonts"));
        assert_eq!(list.len(), 2);
        assert_eq!(
            list.first().map(|c| c.path.clone()),
            Some(Path::new(r"C:\Windows\Fonts\SegoeIcons.ttf").to_path_buf())
        );
        assert_eq!(
            list.get(1).map(|c| c.path.clone()),
            Some(Path::new(r"C:\Windows\Fonts\segmdl2.ttf").to_path_buf())
        );
    }

    #[test]
    fn system_font_dir_ends_with_fonts() {
        let dir = system_font_dir().expect("Windows 目录");
        assert!(dir.ends_with("Fonts"));
    }
}
