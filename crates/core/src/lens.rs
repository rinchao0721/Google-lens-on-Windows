//! 内置弹窗直传 Google Lens 用到的纯函数：上传地址、multipart 请求体、缩图、弹窗位置、导航白名单。

use thiserror::Error;

use crate::error::FrameError;
use crate::frame::CroppedImage;
use crate::geometry::Rect;
use crate::url::url_host;

/// Lens 上传接口。
pub const LENS_UPLOAD_ENDPOINT: &str = "https://lens.google.com/v3/upload";
/// multipart 中图片字段名。
pub const LENS_FILE_FIELD: &str = "encoded_image";
/// 提交给 Lens 的文件名。
pub const LENS_FILE_NAME: &str = "lenswin.png";
/// 图片长边上限，超过时等比缩小。
pub const MAX_LONG_SIDE: u32 = 2048;
/// 请求体大小上限。
pub const MAX_BODY_BYTES: usize = 20 * 1024 * 1024;

/// 组装请求体时的错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LensError {
    /// 请求体超过上限。
    #[error("图片过大：{actual} 字节，上限 {limit} 字节")]
    BodyTooLarge {
        /// 实际字节数。
        actual: usize,
        /// 上限。
        limit: usize,
    },
    /// 找不到与图片内容不冲突的分隔串。
    #[error("无法生成 multipart 分隔串")]
    BoundaryExhausted,
    /// 字段名、文件名或类型含非法字符。
    #[error("multipart 头部含非法字符")]
    InvalidHeader,
    /// 尺寸运算溢出。
    #[error("尺寸运算溢出")]
    SizeOverflow,
    /// 缩图失败。
    #[error(transparent)]
    Frame(#[from] FrameError),
}

/// 一次直传请求：地址、Content-Type 头和完整请求体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensRequest {
    /// POST 地址。
    pub url: String,
    /// `multipart/form-data; boundary=...`。
    pub content_type: String,
    /// 请求体。
    pub body: Vec<u8>,
}

/// 带时间戳的上传地址；Lens 用 `st` 参数区分会话。
pub fn upload_url(timestamp_ms: u64) -> String {
    format!("{LENS_UPLOAD_ENDPOINT}?ep=gsbubb&re=df&st={timestamp_ms}")
}

/// 按种子生成分隔串，遇到与内容冲突时换下一个种子，最多尝试 64 次。
pub fn choose_boundary(data: &[u8], seed: u64) -> Result<String, LensError> {
    let mut current = seed;
    for _ in 0..64 {
        let candidate = format!("----lenswin{current:016x}");
        if !contains(data, candidate.as_bytes()) {
            return Ok(candidate);
        }
        current = current
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
    }
    Err(LensError::BoundaryExhausted)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

fn is_safe_header_value(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_graphic() && b != b'"')
}

/// 组装只含一个文件字段的 multipart 请求体，返回 Content-Type 头与请求体。
pub fn multipart_body(
    boundary: &str,
    field: &str,
    file_name: &str,
    mime: &str,
    data: &[u8],
) -> Result<(String, Vec<u8>), LensError> {
    if ![boundary, field, file_name, mime]
        .iter()
        .all(|s| is_safe_header_value(s))
    {
        return Err(LensError::InvalidHeader);
    }
    if contains(data, boundary.as_bytes()) {
        return Err(LensError::BoundaryExhausted);
    }
    let head = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{file_name}\"\r\nContent-Type: {mime}\r\n\r\n"
    );
    let tail = format!("\r\n--{boundary}--\r\n");
    let total = head
        .len()
        .checked_add(data.len())
        .and_then(|n| n.checked_add(tail.len()))
        .ok_or(LensError::SizeOverflow)?;
    if total > MAX_BODY_BYTES {
        return Err(LensError::BodyTooLarge {
            actual: total,
            limit: MAX_BODY_BYTES,
        });
    }
    let mut body = Vec::with_capacity(total);
    body.extend_from_slice(head.as_bytes());
    body.extend_from_slice(data);
    body.extend_from_slice(tail.as_bytes());
    Ok((format!("multipart/form-data; boundary={boundary}"), body))
}

/// 长边不超过 `max_side` 的等比尺寸，短边至少 1；输入有 0 时返回 `None`。
pub fn scaled_size(width: u32, height: u32, max_side: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 || max_side == 0 {
        return None;
    }
    let long = width.max(height);
    if long <= max_side {
        return Some((width, height));
    }
    let scale = |side: u32| -> Option<u32> {
        let scaled = u64::from(side)
            .checked_mul(u64::from(max_side))?
            .checked_div(u64::from(long))?;
        u32::try_from(scaled.max(1)).ok()
    };
    Some((scale(width)?, scale(height)?))
}

/// 区域平均缩图，长边不超过 `max_side` 时原样返回。
pub fn downscale(image: CroppedImage, max_side: u32) -> Result<CroppedImage, LensError> {
    let (sw, sh) = (image.width(), image.height());
    let (dw, dh) = scaled_size(sw, sh, max_side).ok_or(FrameError::EmptyRegion)?;
    if (dw, dh) == (sw, sh) {
        return Ok(image);
    }
    let src = image.rgba();
    let row_bytes = usize::try_from(sw)
        .ok()
        .and_then(|w| w.checked_mul(4))
        .ok_or(LensError::SizeOverflow)?;
    let out_len = usize::try_from(dw)
        .ok()
        .and_then(|w| w.checked_mul(usize::try_from(dh).ok()?))
        .and_then(|n| n.checked_mul(4))
        .ok_or(LensError::SizeOverflow)?;
    let mut out = Vec::with_capacity(out_len);
    for dy in 0..dh {
        let (y0, y1) = source_span(dy, dh, sh).ok_or(LensError::SizeOverflow)?;
        for dx in 0..dw {
            let (x0, x1) = source_span(dx, dw, sw).ok_or(LensError::SizeOverflow)?;
            let mut sum = [0u64; 4];
            let mut count = 0u64;
            for y in y0..y1 {
                let row_start = usize::try_from(y)
                    .ok()
                    .and_then(|y| y.checked_mul(row_bytes))
                    .ok_or(LensError::SizeOverflow)?;
                for x in x0..x1 {
                    let offset = usize::try_from(x)
                        .ok()
                        .and_then(|x| x.checked_mul(4))
                        .and_then(|o| o.checked_add(row_start))
                        .ok_or(LensError::SizeOverflow)?;
                    let px = src
                        .get(offset..offset.saturating_add(4))
                        .ok_or(LensError::SizeOverflow)?;
                    for (acc, &v) in sum.iter_mut().zip(px) {
                        *acc = acc.saturating_add(u64::from(v));
                    }
                    count = count.saturating_add(1);
                }
            }
            for acc in sum {
                let avg = acc.checked_div(count).unwrap_or(0);
                out.push(u8::try_from(avg).unwrap_or(u8::MAX));
            }
        }
    }
    Ok(CroppedImage::new(dw, dh, out)?)
}

/// 目标像素 `d` 对应的源区间 `[start, end)`，保证至少一个像素。
fn source_span(d: u32, dst: u32, src: u32) -> Option<(u32, u32)> {
    let start = u64::from(d)
        .checked_mul(u64::from(src))?
        .checked_div(u64::from(dst))?;
    let end = u64::from(d)
        .checked_add(1)?
        .checked_mul(u64::from(src))?
        .checked_div(u64::from(dst))?;
    let start = u32::try_from(start).ok()?;
    let end = u32::try_from(end).ok()?.max(start.checked_add(1)?).min(src);
    Some((start, end))
}

/// 弹窗相对选区的摆放方向，按优先级排列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Right,
    Left,
    Below,
    Above,
}

const SIDES: [Side; 4] = [Side::Right, Side::Left, Side::Below, Side::Above];

/// 弹窗位置：依次尝试选区右、左、下、上，取第一个能完整放进工作区且不遮挡选区的位置；
/// 都放不下时贴在剩余空间相对最大的一侧并限制在工作区内，允许部分遮挡；尺寸超出工作区时收缩。
pub fn popup_placement(
    selection: &Rect,
    work_area: &Rect,
    width: u32,
    height: u32,
    gap: u32,
) -> Option<Rect> {
    if work_area.width == 0 || work_area.height == 0 || width == 0 || height == 0 {
        return None;
    }
    let w = i64::from(width.min(work_area.width));
    let h = i64::from(height.min(work_area.height));
    let gap = i64::from(gap);
    let (wl, wt, wr, wb) = (
        i64::from(work_area.x),
        i64::from(work_area.y),
        work_area.right(),
        work_area.bottom(),
    );
    let (sl, st, sr, sb) = (
        i64::from(selection.x),
        i64::from(selection.y),
        selection.right(),
        selection.bottom(),
    );
    let free = |side: Side| match side {
        Side::Right => wr.saturating_sub(sr.saturating_add(gap)),
        Side::Left => sl.saturating_sub(gap).saturating_sub(wl),
        Side::Below => wb.saturating_sub(sb.saturating_add(gap)),
        Side::Above => st.saturating_sub(gap).saturating_sub(wt),
    };
    let need = |side: Side| match side {
        Side::Right | Side::Left => w,
        Side::Below | Side::Above => h,
    };
    let side = SIDES
        .into_iter()
        .find(|s| free(*s) >= need(*s))
        .or_else(|| {
            // 比较能放下弹窗的比例，避免宽高差异让竖直方向总是吃亏
            SIDES.into_iter().max_by(|a, b| {
                let ra = i128::from(free(*a).max(0)) * i128::from(need(*b));
                let rb = i128::from(free(*b).max(0)) * i128::from(need(*a));
                // 比例相同时保留优先级靠前的方向
                ra.cmp(&rb).then(std::cmp::Ordering::Greater)
            })
        })?;
    let center_x = sl.checked_add(i64::from(selection.width) / 2)?;
    let center_y = st.checked_add(i64::from(selection.height) / 2)?;
    let (x, y) = match side {
        Side::Right => (sr.checked_add(gap)?, center_y.checked_sub(h / 2)?),
        Side::Left => (
            sl.checked_sub(gap)?.checked_sub(w)?,
            center_y.checked_sub(h / 2)?,
        ),
        Side::Below => (center_x.checked_sub(w / 2)?, sb.checked_add(gap)?),
        Side::Above => (
            center_x.checked_sub(w / 2)?,
            st.checked_sub(gap)?.checked_sub(h)?,
        ),
    };
    let x = x.clamp(wl, wr.checked_sub(w)?);
    let y = y.clamp(wt, wb.checked_sub(h)?);
    Rect::from_edges(x, y, x.checked_add(w)?, y.checked_add(h)?)
}

/// 弹窗内一次顶层导航的处理方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationDecision {
    /// 留在弹窗内。
    Allow,
    /// 取消，并交给默认浏览器打开。
    External,
    /// 直接取消。
    Block,
}

/// 判断一个地址能否在弹窗内打开；本地加载页（`about:blank`、`data:text/html`）放行。
pub fn classify_navigation(url: &str) -> NavigationDecision {
    let lower = url.trim().to_ascii_lowercase();
    if lower == "about:blank" || lower.starts_with("data:text/html") {
        return NavigationDecision::Allow;
    }
    match url_host(url.trim()) {
        Some(host) if is_google_host(&host) => NavigationDecision::Allow,
        Some(_) => NavigationDecision::External,
        None => NavigationDecision::Block,
    }
}

/// Google 自家域名：`google.<tld>` 及子域、`gstatic.com` 子域、登录跳转用的 `accounts.youtube.com`。
pub fn is_google_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "accounts.youtube.com" || host == "gstatic.com" || host.ends_with(".gstatic.com") {
        return true;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.iter().any(|l| l.is_empty()) {
        return false;
    }
    labels
        .iter()
        .position(|&l| l == "google")
        .and_then(|i| labels.get(i.checked_add(1)?..))
        .is_some_and(is_google_suffix)
}

/// Google 实际使用的 `google.co.<cc>` 国家后缀。
const GOOGLE_CO_SUFFIXES: [&str; 24] = [
    "ao", "bw", "ck", "cr", "id", "il", "in", "jp", "ke", "kr", "ls", "ma", "mz", "nz", "th", "tz",
    "ug", "uk", "uz", "ve", "vi", "za", "zm", "zw",
];

/// Google 实际使用的 `google.com.<cc>` 国家后缀。
const GOOGLE_COM_SUFFIXES: [&str; 56] = [
    "af", "ag", "ar", "au", "bd", "bh", "bn", "bo", "br", "bz", "co", "cu", "cy", "do", "ec", "eg",
    "et", "fj", "gh", "gi", "gt", "hk", "jm", "kh", "kw", "lb", "ly", "mm", "mt", "mx", "my", "na",
    "ng", "ni", "np", "om", "pa", "pe", "pg", "ph", "pk", "pr", "py", "qa", "sa", "sb", "sg", "sl",
    "sv", "tj", "tr", "tw", "ua", "uy", "vc", "vn",
];

// 部分国家的 com.xx / co.xx 可被任何人注册，只放行 Google 实际使用的后缀
fn is_google_suffix(rest: &[&str]) -> bool {
    let is_cc = |l: &str| l.len() == 2 && l.bytes().all(|b| b.is_ascii_lowercase());
    match rest {
        ["com"] => true,
        [cc] => is_cc(cc),
        ["co", cc] => GOOGLE_CO_SUFFIXES.contains(cc),
        ["com", cc] => GOOGLE_COM_SUFFIXES.contains(cc),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32, value: u8) -> CroppedImage {
        let len = usize::try_from(w * h * 4).expect("长度");
        CroppedImage::new(w, h, vec![value; len]).expect("图像")
    }

    #[test]
    fn upload_url_carries_timestamp() {
        let url = upload_url(1_794_397_812_577);
        assert!(url.starts_with("https://lens.google.com/v3/upload?"));
        assert!(url.ends_with("st=1794397812577"));
        assert_eq!(classify_navigation(&url), NavigationDecision::Allow);
    }

    #[test]
    fn multipart_body_has_expected_layout() {
        let data = b"\x89PNG\r\n";
        let (ct, body) =
            multipart_body("----b", LENS_FILE_FIELD, LENS_FILE_NAME, "image/png", data)
                .expect("组装");
        assert_eq!(ct, "multipart/form-data; boundary=----b");
        let text = String::from_utf8_lossy(&body);
        assert!(text.starts_with("------b\r\nContent-Disposition: form-data; name=\"encoded_image\"; filename=\"lenswin.png\"\r\nContent-Type: image/png\r\n\r\n"));
        assert!(text.ends_with("\r\n------b--\r\n"));
        assert!(contains(&body, data));
    }

    #[test]
    fn multipart_rejects_bad_headers_and_conflicts() {
        assert_eq!(
            multipart_body("b", "a\"b", "f.png", "image/png", b"x"),
            Err(LensError::InvalidHeader)
        );
        assert_eq!(
            multipart_body("b", "f", "with space.png", "image/png", b"x"),
            Err(LensError::InvalidHeader)
        );
        assert_eq!(
            multipart_body("", "f", "f.png", "image/png", b"x"),
            Err(LensError::InvalidHeader)
        );
        assert_eq!(
            multipart_body("xyz", "f", "f.png", "image/png", b"..xyz.."),
            Err(LensError::BoundaryExhausted)
        );
    }

    #[test]
    fn multipart_rejects_oversized_body() {
        let data = vec![0u8; MAX_BODY_BYTES];
        assert!(matches!(
            multipart_body("b1", "f", "f.png", "image/png", &data),
            Err(LensError::BodyTooLarge { .. })
        ));
    }

    #[test]
    fn boundary_avoids_content() {
        let first = choose_boundary(b"", 7).expect("分隔串");
        let data = format!("abc{first}def");
        let second = choose_boundary(data.as_bytes(), 7).expect("换种子");
        assert_ne!(first, second);
        assert!(!data.contains(&second));
    }

    #[test]
    fn scaled_size_handles_edges() {
        assert_eq!(scaled_size(0, 10, 2048), None);
        assert_eq!(scaled_size(10, 10, 0), None);
        assert_eq!(scaled_size(2048, 100, 2048), Some((2048, 100)));
        assert_eq!(scaled_size(4096, 2160, 2048), Some((2048, 1080)));
        assert_eq!(scaled_size(100_000, 1, 2048), Some((2048, 1)));
        assert_eq!(scaled_size(u32::MAX, u32::MAX, 2048), Some((2048, 2048)));
    }

    #[test]
    fn downscale_keeps_small_images() {
        let img = image(10, 5, 9);
        assert_eq!(downscale(img.clone(), 2048).expect("缩图"), img);
    }

    #[test]
    fn downscale_averages_pixels() {
        let rgba = vec![
            0, 0, 0, 255, 200, 100, 50, 255, //
            0, 0, 0, 255, 200, 100, 50, 255,
        ];
        let img = CroppedImage::new(2, 2, rgba).expect("图像");
        let out = downscale(img, 1).expect("缩图");
        assert_eq!((out.width(), out.height()), (1, 1));
        assert_eq!(out.rgba(), &[100, 50, 25, 255]);
    }

    #[test]
    fn downscale_large_and_narrow() {
        let out = downscale(image(4100, 3, 77), 2048).expect("缩图");
        assert_eq!((out.width(), out.height()), (2048, 1));
        assert!(out.rgba().iter().all(|&v| v == 77));
    }

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    fn overlaps(a: &Rect, b: &Rect) -> bool {
        a.intersect(b).is_some()
    }

    #[test]
    fn placement_prefers_right_then_left() {
        let work = rect(0, 0, 1920, 1040);
        let sel = rect(100, 100, 200, 200);
        let right = popup_placement(&sel, &work, 460, 760, 16).expect("位置");
        assert_eq!(right.x, 316);
        assert!(!overlaps(&right, &sel));
        let sel = rect(1500, 100, 300, 200);
        let left = popup_placement(&sel, &work, 460, 760, 16).expect("位置");
        assert_eq!(left.x, 1500 - 16 - 460);
        assert!(!overlaps(&left, &sel));
    }

    #[test]
    fn placement_falls_back_to_below_then_above() {
        let work = rect(0, 0, 1920, 1080);
        // 左右都放不下，下方有空间
        let sel = rect(300, 50, 1400, 200);
        let below = popup_placement(&sel, &work, 460, 760, 16).expect("位置");
        assert_eq!(below.y, 266);
        assert_eq!(below.x, 300 + 700 - 230);
        assert!(!overlaps(&below, &sel));
        // 左右和下方都放不下，上方有空间
        let sel = rect(300, 900, 1400, 100);
        let above = popup_placement(&sel, &work, 460, 760, 16).expect("位置");
        assert_eq!(above.bottom(), 900 - 16);
        assert!(!overlaps(&above, &sel));
    }

    #[test]
    fn placement_overlaps_only_when_nothing_fits() {
        let work = rect(0, 0, 1920, 1040);
        let sel = rect(200, 100, 1500, 200);
        let p = popup_placement(&sel, &work, 460, 760, 16).expect("位置");
        // 下方剩余比例最大，贴着下方并限制在工作区内
        assert_eq!(p.bottom(), 1040);
        assert_eq!(p.x, 950 - 230);
        assert!(p.y >= 0);
        let full = popup_placement(&work, &work, 460, 760, 16).expect("位置");
        assert!(full.x >= 0 && full.right() <= 1920 && full.bottom() <= 1040);
    }

    #[test]
    fn placement_on_negative_monitor_and_small_work_area() {
        let work = rect(-1920, -200, 1920, 1080);
        let p = popup_placement(&rect(-1900, -190, 100, 100), &work, 460, 760, 16).expect("位置");
        assert_eq!(p.x, -1900 + 100 + 16);
        assert!(i64::from(p.y) >= -200 && p.bottom() <= 880);
        let tiny = popup_placement(&rect(0, 0, 10, 10), &rect(0, 0, 300, 400), 460, 760, 16)
            .expect("位置");
        assert_eq!((tiny.width, tiny.height), (300, 400));
        assert_eq!(
            popup_placement(&rect(0, 0, 1, 1), &rect(0, 0, 0, 10), 460, 760, 16),
            None
        );
        assert_eq!(popup_placement(&rect(0, 0, 1, 1), &work, 0, 760, 16), None);
    }

    #[test]
    fn google_hosts_are_recognised() {
        for host in [
            "google.com",
            "www.google.com",
            "lens.google.com",
            "consent.google.com",
            "www.google.co.jp",
            "www.google.com.hk",
            "google.de",
            "encrypted-tbn0.gstatic.com",
            "accounts.youtube.com",
            "WWW.GOOGLE.COM.",
        ] {
            assert!(is_google_host(host), "{host}");
        }
        for host in [
            "google.com.evil.com",
            "google.com.xx",
            "google.co.zz",
            "www.google.co.cn",
            "evilgoogle.com",
            "google.evil",
            "google",
            "www.youtube.com",
            "gstatic.com.evil.com",
            "google..com",
            "",
        ] {
            assert!(!is_google_host(host), "{host}");
        }
    }

    #[test]
    fn navigation_is_classified() {
        assert_eq!(
            classify_navigation("https://www.google.com/search?q=1"),
            NavigationDecision::Allow
        );
        assert_eq!(
            classify_navigation("about:blank"),
            NavigationDecision::Allow
        );
        assert_eq!(
            classify_navigation("data:text/html;charset=utf-8,hi"),
            NavigationDecision::Allow
        );
        assert_eq!(
            classify_navigation("https://www.amazon.com/x"),
            NavigationDecision::External
        );
        assert_eq!(
            classify_navigation("https://google.com.evil.com/"),
            NavigationDecision::External
        );
        assert_eq!(
            classify_navigation("file:///C:/x"),
            NavigationDecision::Block
        );
        assert_eq!(
            classify_navigation("javascript:alert(1)"),
            NavigationDecision::Block
        );
        assert_eq!(classify_navigation(""), NavigationDecision::Block);
    }
}
