//! 识别结果的过滤、行聚类与文本拼接。

use super::det::PixelBox;
use super::num::{ceil_u32, floor_u32, to_usize, u32_to_f32, usize_to_f32};
use super::rec::{DecodedChar, DecodedLine};
use super::text::{insert_script_spaces, is_cjk_letter, should_strip_between};

/// 平均置信度低于该值的片段丢弃。
pub const MIN_CONFIDENCE: f32 = 0.5;
/// 墨迹低于整段最大墨迹的该比例时视为空白列。
const INK_BLANK_RATIO: f32 = 0.3;
/// 整段最大墨迹低于该值时认为没有可用的墨迹信息。
const MIN_PEAK_INK: f32 = 0.05;
/// 汉字之间的空白至少这么多列才补空格（按 48 像素高的识别输入计，约 0.2 个字高）。
const GAP_MIN_COLUMNS: usize = 10;
/// 模型在汉字之间输出了空格时，空白只需达到这么多列就保留。
const MODEL_GAP_MIN_COLUMNS: usize = 8;
/// 字符数不超过该值的片段视为可能是图标误识别。
const SHORT_MAX_CHARS: usize = 2;
/// 短片段需要达到的置信度，低于它就丢弃。
const SHORT_CONFIDENCE: f32 = 0.8;

/// 一个文本框及其识别结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// 文本框在原图中的位置。
    pub bounds: PixelBox,
    /// CTC 解码结果。
    pub line: DecodedLine,
    /// 识别输入每列的墨迹强度（见 [`super::column_ink`]），为空表示没有墨迹信息。
    pub ink: Vec<f32>,
}

/// 过滤后、已带空格的片段。
struct Item {
    left: f32,
    center_y: f32,
    height: f32,
    text: String,
}

/// 一行文字的聚类状态。
struct Row {
    center_y: f32,
    height: f32,
    items: Vec<Item>,
}

/// 两个字符位置（占内容宽度的比例）之间最长的连续空白列数。
fn blank_run(ink: &[f32], from: f32, to: f32) -> usize {
    let peak = ink.iter().copied().fold(0.0f32, f32::max);
    if peak < MIN_PEAK_INK {
        return 0;
    }
    let len = usize_to_f32(ink.len());
    let start = to_usize(floor_u32(from * len));
    let end = to_usize(ceil_u32(to * len)).min(ink.len());
    let mut best = 0usize;
    let mut run = 0usize;
    for &v in ink.get(start..end).unwrap_or_default() {
        if v < peak * INK_BLANK_RATIO {
            run = run.saturating_add(1);
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

fn first_char(c: &DecodedChar) -> Option<char> {
    c.text.chars().next()
}

fn last_char(c: &DecodedChar) -> Option<char> {
    c.text.chars().next_back()
}

/// 把解码字符串拼成文本：汉字之间只在墨迹出现明显空白时补空格（模型给出空格时放宽门槛），
/// 其它位置沿用模型的空格。
fn segment_text(line: &DecodedLine, ink: &[f32]) -> String {
    let mut out = String::new();
    let mut prev: Option<(char, f32)> = None;
    let mut pending_space = false;
    for c in &line.chars {
        if c.text == " " {
            pending_space = true;
            continue;
        }
        let (Some(first), Some(last)) = (first_char(c), last_char(c)) else {
            continue;
        };
        if let Some((prev_last, prev_pos)) = prev {
            let insert = if is_cjk_letter(prev_last) && is_cjk_letter(first) {
                let need = if pending_space {
                    MODEL_GAP_MIN_COLUMNS
                } else {
                    GAP_MIN_COLUMNS
                };
                blank_run(ink, prev_pos, c.position) >= need
            } else {
                pending_space && !should_strip_between(prev_last, first)
            };
            if insert {
                out.push(' ');
            }
        }
        out.push_str(&c.text);
        prev = Some((last, c.position));
        pending_space = false;
    }
    out
}

/// 低置信度的片段，以及置信度不高的短片段（图标常被误识别成一两个符号或数字）要丢弃。
fn is_noise(text: &str, score: f32) -> bool {
    score < MIN_CONFIDENCE || (text.chars().count() <= SHORT_MAX_CHARS && score < SHORT_CONFIDENCE)
}

fn to_item(segment: &Segment) -> Option<Item> {
    let text = segment_text(&segment.line, &segment.ink);
    if text.trim().is_empty() || is_noise(&text, segment.line.score) {
        return None;
    }
    let b = segment.bounds;
    Some(Item {
        left: u32_to_f32(b.x),
        center_y: u32_to_f32(b.y) + u32_to_f32(b.height) / 2.0,
        height: u32_to_f32(b.height),
        text,
    })
}

/// 按纵向中心把片段聚成行；框中心距行中心小于较矮者半高时并入该行。
fn cluster_rows(mut items: Vec<Item>) -> Vec<Row> {
    items.sort_by(|a, b| a.center_y.total_cmp(&b.center_y));
    let mut rows: Vec<Row> = Vec::new();
    for item in items {
        match rows.last_mut() {
            Some(row)
                if (item.center_y - row.center_y).abs() < 0.5 * item.height.min(row.height) =>
            {
                let n = usize_to_f32(row.items.len());
                row.center_y = (row.center_y * n + item.center_y) / (n + 1.0);
                row.height = (row.height * n + item.height) / (n + 1.0);
                row.items.push(item);
            }
            _ => rows.push(Row {
                center_y: item.center_y,
                height: item.height,
                items: vec![item],
            }),
        }
    }
    rows
}

/// 把识别片段整理成逐行文本。
///
/// 丢弃低置信度片段与图标噪声；按行聚类、行内从左到右，相邻片段之间补空格，
/// 汉字与拉丁字母、数字交界处补空格。
pub fn assemble_lines(segments: &[Segment]) -> Vec<String> {
    let items: Vec<Item> = segments.iter().filter_map(to_item).collect();
    cluster_rows(items)
        .into_iter()
        .filter_map(|mut row| {
            row.items.sort_by(|a, b| a.left.total_cmp(&b.left));
            let joined = row
                .items
                .iter()
                .map(|i| i.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let line = insert_script_spaces(&joined);
            let trimmed = line.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(spec: &[(&str, f32)]) -> Vec<DecodedChar> {
        spec.iter()
            .map(|&(t, p)| DecodedChar {
                text: t.to_string(),
                position: p,
            })
            .collect()
    }

    fn seg(x: u32, y: u32, w: u32, h: u32, text: &str, score: f32) -> Segment {
        let count = text.chars().count().max(1);
        let spec: Vec<DecodedChar> = text
            .chars()
            .enumerate()
            .map(|(i, c)| DecodedChar {
                text: c.to_string(),
                position: (usize_to_f32(i) + 0.5) / usize_to_f32(count),
            })
            .collect();
        Segment {
            bounds: PixelBox {
                x,
                y,
                width: w,
                height: h,
            },
            line: DecodedLine { chars: spec, score },
            ink: Vec::new(),
        }
    }

    #[test]
    fn rows_are_clustered_and_sorted_left_to_right() {
        let segments = vec![
            seg(200, 12, 80, 20, "右", 0.99),
            seg(0, 10, 80, 20, "左", 0.99),
            seg(0, 60, 80, 20, "下一行", 0.99),
        ];
        assert_eq!(assemble_lines(&segments), vec!["左 右", "下一行"]);
    }

    #[test]
    fn low_confidence_and_icon_noise_are_dropped() {
        let segments = vec![
            seg(0, 10, 20, 20, "中", 0.18),
            seg(30, 10, 100, 20, "麦克风", 1.0),
            seg(140, 10, 20, 20, ">", 0.53),
            seg(0, 60, 80, 20, "内录", 1.0),
            seg(90, 60, 20, 20, ">", 0.48),
            seg(0, 100, 20, 20, "?", 0.95),
            seg(0, 140, 20, 20, "0", 0.68),
            seg(0, 180, 60, 20, "内录", 0.7),
        ];
        assert_eq!(assemble_lines(&segments), vec!["麦克风", "内录", "?"]);
    }

    #[test]
    fn empty_and_blank_inputs_give_no_lines() {
        assert!(assemble_lines(&[]).is_empty());
        let blank = Segment {
            bounds: PixelBox {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            line: DecodedLine {
                chars: chars(&[(" ", 0.5)]),
                score: 0.9,
            },
            ink: Vec::new(),
        };
        let none = Segment {
            bounds: PixelBox {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            line: DecodedLine {
                chars: vec![],
                score: 0.0,
            },
            ink: Vec::new(),
        };
        assert!(assemble_lines(&[blank, none]).is_empty());
    }

    #[test]
    fn cjk_latin_boundaries_get_spaces() {
        let segments = vec![seg(0, 0, 300, 20, "其他App正在", 0.97)];
        assert_eq!(assemble_lines(&segments), vec!["其他 App 正在"]);
    }

    /// 全程有墨迹、仅在给定区间留空白的墨迹轮廓。
    fn ink_with_gaps(len: usize, gaps: &[(usize, usize)]) -> Vec<f32> {
        let mut ink = vec![1.0; len];
        for &(start, count) in gaps {
            for v in ink.iter_mut().skip(start).take(count) {
                *v = 0.0;
            }
        }
        ink
    }

    fn hanzi_segment(spec: Vec<DecodedChar>, ink: Vec<f32>) -> Segment {
        Segment {
            bounds: PixelBox {
                x: 0,
                y: 0,
                width: 300,
                height: 20,
            },
            line: DecodedLine {
                chars: spec,
                score: 0.99,
            },
            ink,
        }
    }

    #[test]
    fn wide_blank_between_hanzi_inserts_a_space() {
        let spec = chars(&[
            ("真", 0.05),
            ("可", 0.15),
            ("笑", 0.25),
            ("这", 0.45),
            ("些", 0.55),
            ("情", 0.65),
        ]);
        // 笑(75) 到 这(135) 之间有 20 列空白，其它字间只有 3 列
        let ink = ink_with_gaps(300, &[(30, 3), (60, 3), (100, 20), (160, 3), (190, 3)]);
        assert_eq!(
            assemble_lines(&[hanzi_segment(spec.clone(), ink)]),
            vec!["真可笑 这些情"]
        );
        // 没有墨迹信息时不补空格
        assert_eq!(
            assemble_lines(&[hanzi_segment(spec, Vec::new())]),
            vec!["真可笑这些情"]
        );
    }

    #[test]
    fn narrow_blanks_and_model_spaces_between_hanzi_are_removed() {
        let spec = chars(&[
            ("你", 0.1),
            (" ", 0.15),
            ("好", 0.2),
            ("世", 0.3),
            ("界", 0.4),
            ("吗", 0.5),
        ]);
        let ink = ink_with_gaps(300, &[(40, 6), (70, 4), (100, 4), (130, 4)]);
        assert_eq!(
            assemble_lines(&[hanzi_segment(spec, ink)]),
            vec!["你好世界吗"]
        );
    }

    #[test]
    fn model_space_between_hanzi_needs_a_smaller_blank() {
        let spec = |with_space: bool| {
            let mut v = vec![("要", 0.10), ("是", 0.20), ("要", 0.33)];
            if with_space {
                v.insert(2, (" ", 0.27));
            }
            v.extend([("是", 0.43), ("你", 0.53)]);
            chars(&v)
        };
        // 是(60) 到 要(99) 之间 9 列空白：够保留模型的空格，但不够自行补空格
        let ink = ink_with_gaps(300, &[(70, 9)]);
        assert_eq!(
            assemble_lines(&[hanzi_segment(spec(true), ink.clone())]),
            vec!["要是 要是你"]
        );
        assert_eq!(
            assemble_lines(&[hanzi_segment(spec(false), ink)]),
            vec!["要是要是你"]
        );
    }

    #[test]
    fn blank_run_handles_edges() {
        assert_eq!(blank_run(&[], 0.0, 1.0), 0);
        // 全空白没有可用墨迹信息
        assert_eq!(blank_run(&[0.0; 50], 0.0, 1.0), 0);
        let ink = ink_with_gaps(100, &[(20, 7), (50, 12)]);
        assert_eq!(blank_run(&ink, 0.0, 1.0), 12);
        assert_eq!(blank_run(&ink, 0.0, 0.4), 7);
        assert_eq!(blank_run(&ink, 0.9, 5.0), 0);
        assert_eq!(blank_run(&ink, 0.5, 0.5), 0);
    }

    #[test]
    fn model_spaces_between_latin_words_are_kept() {
        let spec = chars(&[("I", 0.1), ("f", 0.15), (" ", 0.2), ("o", 0.3), ("n", 0.4)]);
        assert_eq!(
            assemble_lines(&[hanzi_segment(spec, Vec::new())]),
            vec!["If on"]
        );
    }

    #[test]
    fn very_long_line_is_not_truncated() {
        let text = "字".repeat(3000);
        let segments = vec![seg(0, 0, 60_000, 20, &text, 0.99)];
        let lines = assemble_lines(&segments);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].chars().count(), 3000);
    }
}
