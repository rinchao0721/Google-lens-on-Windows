//! 旧版本配置的迁移，只做文本层面的纯转换。

use std::borrow::Cow;

use toml::Table;

/// 剥掉 TOML 顶层的 `ocr` 表。
///
/// S4 的 `[ocr]`（识别语言与策略）在换用 PP-OCRv5 后不再使用。文本无法解析为 TOML 时原样返回，
/// 交给后续反序列化报错；`hotkeys.ocr` 在 `[hotkeys]` 表内，不受影响。
pub fn strip_legacy_ocr(text: &str) -> Cow<'_, str> {
    if !text.contains("ocr") {
        return Cow::Borrowed(text);
    }
    let Ok(mut table) = text.parse::<Table>() else {
        return Cow::Borrowed(text);
    };
    if table.remove("ocr").is_none() {
        return Cow::Borrowed(text);
    }
    match toml::to_string(&table) {
        Ok(stripped) => Cow::Owned(stripped),
        Err(_) => Cow::Borrowed(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn text_without_ocr_table_is_returned_untouched() {
        for text in [
            "",
            "[general]\nnotifications = false\n",
            "[hotkeys]\nocr = \"Ctrl+Alt+O\"\n",
        ] {
            assert!(
                matches!(strip_legacy_ocr(text), Cow::Borrowed(t) if t == text),
                "{text}"
            );
        }
    }

    #[test]
    fn legacy_ocr_table_is_removed_and_other_settings_kept() {
        let text = "[general]\nnotifications = false\n\n[ocr]\nlanguages = [\"zh-Hans-CN\"]\nstrategy = \"best\"\n\n[hotkeys]\nocr = \"Ctrl+Alt+O\"\n";
        let config = Config::from_toml_str(text).expect("旧配置应能加载");
        assert!(!config.general.notifications);
        assert_eq!(config.hotkeys.ocr, "Ctrl+Alt+O");
        let saved = config.to_toml_string().expect("序列化");
        assert!(!saved.contains("[ocr]"), "{saved}");
        assert!(Config::from_toml_str(&saved).is_ok());
    }

    #[test]
    fn invalid_ocr_content_is_dropped_with_the_table() {
        for text in [
            "[ocr]\nstrategy = \"fast\"\nextra = 1\n",
            "[ocr]\nlanguages = 5\n",
            "ocr = \"x\"\n[general]\nnotifications = false\n",
        ] {
            assert!(Config::from_toml_str(text).is_ok(), "{text}");
        }
    }

    #[test]
    fn unparsable_text_is_left_for_the_normal_error_path() {
        let text = "[ocr\nbroken = ";
        assert!(matches!(strip_legacy_ocr(text), Cow::Borrowed(_)));
        assert!(Config::from_toml_str(text).is_err());
        assert!(Config::from_toml_str("[general]\nbogus = 1\n[ocr]\n").is_err());
    }
}
