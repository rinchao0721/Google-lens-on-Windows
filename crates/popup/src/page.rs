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

const STYLE: &str = "<style>\
html,body{margin:0;height:100%;font-family:'Segoe UI','Microsoft YaHei UI',sans-serif;\
background:#fff;color:#3c4043}\
@media (prefers-color-scheme:dark){html,body{background:#202124;color:#e8eaed}}\
.box{height:100%;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:18px}\
.dots{display:flex;gap:10px}.dots i{width:12px;height:12px;border-radius:50%;\
animation:b 1s infinite ease-in-out}\
.dots i:nth-child(1){background:#4285F4}.dots i:nth-child(2){background:#EA4335;animation-delay:.15s}\
.dots i:nth-child(3){background:#FBBC05;animation-delay:.3s}.dots i:nth-child(4){background:#34A853;animation-delay:.45s}\
@keyframes b{0%,80%,100%{transform:scale(.6);opacity:.5}40%{transform:scale(1);opacity:1}}\
p{margin:0;font-size:14px;max-width:80%;text-align:center;line-height:1.6}</style>";

pub(crate) fn loading_page() -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">{STYLE}</head><body><div class=\"box\">\
         <div class=\"dots\"><i></i><i></i><i></i><i></i></div><p>正在识别…</p></div></body></html>"
    )
}

/// 出错页面，`message` 会被转义。
pub(crate) fn error_page(message: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">{STYLE}</head><body><div class=\"box\">\
         <p><b>搜索失败</b></p><p>{}</p><p>可以关闭弹窗后重试，或在设置里改用「图床 + 浏览器」方式。</p>\
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
        assert!(loading_page().contains("正在识别"));
    }
}
