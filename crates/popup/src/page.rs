//! 弹窗内的本地页面：加载中与出错。

pub(crate) fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

const STYLE: &str = "<style>:root{--bg:#fff;--fg:#202124;--sub:#5f6368;--spin:#1a73e8;--track:#e8eaed;--err:#d93025;--err-bg:#fce8e6}@media (prefers-color-scheme:dark){:root{--bg:#202124;--fg:#e8eaed;--sub:#9aa0a6;--spin:#8ab4f8;--track:#3c4043;--err:#f28b82;--err-bg:rgba(242,139,130,.16)}}html,body{margin:0;height:100%;font-family:'Segoe UI Variable Text','Segoe UI','Microsoft YaHei UI',sans-serif;background:var(--bg);color:var(--fg)}.box{height:100%;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:12px;padding:0 24px;box-sizing:border-box;text-align:center}.bar{position:fixed;top:0;left:0;right:0;height:3px;background:linear-gradient(90deg,#4285F4 0 25%,#EA4335 25% 50%,#FBBC05 50% 75%,#34A853 75% 100%)}.spin{width:28px;height:28px;box-sizing:border-box;border:3px solid var(--track);border-top-color:var(--spin);border-radius:50%;animation:r .9s linear infinite;margin-bottom:6px}@keyframes r{to{transform:rotate(360deg)}}.icon{width:56px;height:56px;border-radius:50%;background:var(--err-bg);color:var(--err);display:flex;align-items:center;justify-content:center;margin-bottom:6px}h1{margin:0;font-size:18px;font-weight:600}p{margin:0;font-size:14px;line-height:1.6;max-width:320px;overflow-wrap:anywhere}.sub{font-size:12px;color:var(--sub)}</style>";

pub(crate) fn loading_page() -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">{STYLE}</head><body>\
         <div class=\"bar\"></div><div class=\"box\"><div class=\"spin\"></div>\
         <p>正在识别图片…</p><p class=\"sub\">图片直接发送给 Google 智能镜头</p></div></body></html>"
    )
}

/// 出错页面，`message` 会被转义。
pub(crate) fn error_page(message: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">{STYLE}</head><body><div class=\"box\">\
         <div class=\"icon\"><svg width=\"28\" height=\"28\" viewBox=\"0 0 24 24\" aria-hidden=\"true\">\
         <path fill=\"currentColor\" d=\"M11 6h2v8h-2zM11 16h2v2h-2z\"/></svg></div>\
         <h1>搜索失败</h1><p>{}</p>\
         <p class=\"sub\">可以关闭弹窗后重试，或在设置里改用「图床 + 浏览器」方式。</p>\
         </div></body></html>",
        escape_html(message)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_markup() {
        assert_eq!(
            escape_html("<a href=\"x\">'&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
        assert_eq!(escape_html(""), "");
        assert_eq!(escape_html("网络错误"), "网络错误");
    }

    #[test]
    fn error_page_never_contains_raw_markup() {
        let page = error_page("<script>alert(1)</script>");
        assert!(!page.contains("<script>"));
        assert!(page.contains("&lt;script&gt;"));
    }

    #[test]
    fn loading_page_has_progress_bar_and_hint() {
        let page = loading_page();
        assert!(page.contains("正在识别图片…"));
        assert!(page.contains("图片直接发送给 Google 智能镜头"));
        assert!(page.contains("#4285F4"));
        assert!(page.contains("prefers-color-scheme:dark"));
    }

    #[test]
    fn error_page_has_title_icon_and_hint() {
        let page = error_page("连接超时");
        assert!(page.contains("搜索失败"));
        assert!(page.contains("连接超时"));
        assert!(page.contains("<svg"));
        assert!(page.contains("可以关闭弹窗后重试，或在设置里改用「图床 + 浏览器」方式。"));
        assert!(!page.contains("<button"));
    }
}
