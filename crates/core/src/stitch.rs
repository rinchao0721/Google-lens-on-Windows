//! 长截图纯函数拼接：固定头尾栏检测与排除、行列签名位移匹配、条带累积与合成。

use thiserror::Error;

use crate::error::FrameError;
use crate::frame::CroppedImage;

/// 长截图沿滚动方向的最大输出像素数。
pub const MAX_LONG_CAPTURE_PX: u32 = 30_000;

/// 判定两帧存在有效重叠所需的最小重叠像素数。
pub const MIN_OVERLAP_PX: u32 = 32;

/// 签名分桶数量（沿垂直于滚动方向均匀划分）。
const SIG_BUCKETS: usize = 16;

/// 固定栏逐行匹配的最大平均亮度差（千分位，2000 = 2.0 灰度级）。
const FIXED_LINE_MAD_MILLI: u64 = 2_000;

/// 固定栏逐行允许的最大强变化像素比例（千分位，20 = 2%）。
const FIXED_LINE_OUTLIER_MILLI: u64 = 20;

/// 判定内容静止（完全无纹理时回退）的最大平均亮度差（千分位，2500 = 2.5 灰度级）。
const NO_MOVE_MAD_MILLI: u64 = 2_500;

/// 逐行鲁棒匹配允许的单个像素亮度差容限（约 6% 满量程，可容忍抗锯齿与轻度渲染抖动）。
const PX_DIFF_TOLERANCE: u8 = 16;

/// 单行内允许超过 [`PX_DIFF_TOLERANCE`] 的离群像素最大占比（千分位，50 = 5%）。
/// 常规滚动条（约占行宽 1%~3%）与闪烁光标（约 0.2%~0.5%）均低于此阈值，可在逐行匹配中被容忍。
const LINE_OUTLIER_MAX_MILLI: u64 = 50;

/// 采纳候选位移所需的最少有纹理条带数（约一整行文字高度，避免纯白边距或偶然短条纹误匹配）。
const MIN_TEXTURED_LINES: usize = 16;

/// 采纳候选位移所需的有纹理条带最低匹配比例（千分位，800 = 80%）。
/// 因参考帧本身也保留了上一帧屏幕固定位置的悬停高亮条，相邻帧滚动时新旧两处约 20~30 px
/// 高亮条在文档坐标系中往往互不重叠，会在中等高度选区（300 px 级）产生最高约 20% 的变化行；
/// 80% 下限可稳定容忍此类双边局部遮挡，而单行 95% 像素吻合约束仍可有效杜绝错位误匹配。
const MATCH_RATIO_MIN_MILLI: u64 = 800;

/// 零位移下判定为画面未动（NoMove）的有纹理条带最低匹配比例（千分位，900 = 90%）。
/// 防止光标闪烁、悬停高亮等局部变化被误判为已滚动或滚动太快。
const NO_MOVE_RATIO_MILLI: u64 = 900;

/// 粗筛单行签名误差封顶值（16 桶 × 16 灰度级），避免强局部遮挡拖垮正确位移的粗筛排名。
const ROUGH_SIG_LINE_CAP: u32 = 256;

/// 粗筛阶段每个候选位移最多抽取的有纹理锚点条带数。
/// 当有纹理条带不超过该值时全量参与；超出时用非周期分桶抖动抽样，避免与固定行距混叠并把 1080p 单帧耗时压到 30 ms 以内。
const ROUGH_MAX_SAMPLES: usize = 64;

/// 粗筛保留进入逐行精验证的候选位移个数，留足余量以免文本行距相似时漏掉真值。
const TOP_CANDIDATES: usize = 32;

/// 多个合法候选匹配比例相近时优先选较大重叠区的容差（千分位，20 = 2%）。
const RATIO_TIE_TOLERANCE_MILLI: u64 = 20;

/// 未锁轴且双轴均匹配时，横向必须高出竖向的最小匹配比例优势（千分位，50 = 5%），避免悬停或噪点误锁横向。
const AXIS_LOCK_ADVANTAGE_MILLI: u64 = 50;

/// 滚动方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollAxis {
    /// 竖向滚动（固定宽度，高度累加）。
    Vertical,
    /// 横向滚动（固定高度，宽度累加）。
    Horizontal,
}

/// 拼接过程中的合法性或合成错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StitchError {
    /// 帧宽或高为 0。
    #[error("帧尺寸不能为 0")]
    EmptyFrame,
    /// RGBA 字节长度与尺寸不匹配。
    #[error("像素字节数不匹配：期望 {expected}，实际 {actual}")]
    InvalidBytes {
        /// 期望字节数。
        expected: usize,
        /// 实际字节数。
        actual: usize,
    },
    /// 后续帧尺寸与首帧不一致。
    #[error("帧尺寸不一致：期望 {expected:?}，实际 {actual:?}")]
    SizeMismatch {
        /// 首帧尺寸。
        expected: (u32, u32),
        /// 传入尺寸。
        actual: (u32, u32),
    },
    /// 尚未收入任何帧。
    #[error("尚未收入任何画面")]
    NoFrames,
    /// 构造输出图像失败。
    #[error("合成图像失败：{0}")]
    Frame(#[from] FrameError),
}

/// 推入一帧后的拼接状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StitchStatus {
    /// 首帧已收入，等待滚动。
    FirstFrame,
    /// 本帧已匹配并采纳。
    Adopted {
        /// 当前沿滚动方向的总像素数（含固定尾栏）。
        total_px: u32,
        /// 本次锁定的滚动方向。
        axis: ScrollAxis,
        /// 是否已达到最大长度上限（若已到达，本次条带已截断）。
        reached_limit: bool,
    },
    /// 画面未发生明显位移。
    NoMove,
    /// 检测到往回滚动，已忽略本帧。
    IgnoredBackward,
    /// 滚动过快或画面剧变，未找到满足最小重叠的匹配。
    TooFast,
}

/// 沿指定滚动方向重新排列的只读分析平面（按「条带 × 条带长度」存储）。
#[derive(Clone)]
struct OrientedPlane {
    /// 沿滚动方向的条带数（竖向为高，横向为宽）。
    lines: usize,
    /// 每条带的像素数（竖向为宽，横向为高）。
    line_len: usize,
    /// 逐条带紧凑存放的灰度值，长度等于 `lines * line_len`。
    luma: Vec<u8>,
    /// 每条带的 16 段均值签名，供位移粗筛。
    sigs: Vec<[u8; SIG_BUCKETS]>,
    /// 每条带自身是否近似纯色，以及纯色均值（用于剔除空白行与回退纯色边距）。
    uniform_luma: Vec<Option<u8>>,
}

impl OrientedPlane {
    fn from_rgba(width: usize, height: usize, rgba: &[u8], axis: ScrollAxis) -> Self {
        let (lines, line_len) = match axis {
            ScrollAxis::Vertical => (height, width),
            ScrollAxis::Horizontal => (width, height),
        };
        let mut luma = vec![0u8; lines.saturating_mul(line_len)];
        let px_chunks = rgba.as_chunks::<4>().0;
        match axis {
            ScrollAxis::Vertical => {
                for (dst, px) in luma.iter_mut().zip(px_chunks) {
                    *dst = rgb_luma(px[0], px[1], px[2]);
                }
            }
            ScrollAxis::Horizontal => {
                for y in 0..height {
                    let row_offset = y.saturating_mul(width);
                    for x in 0..width {
                        if let (Some(px), Some(slot)) = (
                            px_chunks.get(row_offset.saturating_add(x)),
                            luma.get_mut(x.saturating_mul(height).saturating_add(y)),
                        ) {
                            *slot = rgb_luma(px[0], px[1], px[2]);
                        }
                    }
                }
            }
        }
        Self::with_luma(lines, line_len, luma)
    }

    fn with_luma(lines: usize, line_len: usize, luma: Vec<u8>) -> Self {
        let mut sigs = Vec::with_capacity(lines);
        let mut uniform_luma = Vec::with_capacity(lines);
        for line in 0..lines {
            let start = line.saturating_mul(line_len);
            let row = luma
                .get(start..start.saturating_add(line_len))
                .unwrap_or_default();
            sigs.push(line_signature(row));
            uniform_luma.push(uniform_row_value(row));
        }

        Self {
            lines,
            line_len,
            luma,
            sigs,
            uniform_luma,
        }
    }

    /// 由已计算的竖向灰度平面分块转置得到横向平面，避免重复扫描 RGBA 与缓存抖动。
    fn transpose_from(&self) -> Self {
        let lines = self.line_len;
        let line_len = self.lines;
        let mut luma = vec![0u8; lines.saturating_mul(line_len)];
        const TILE: usize = 32;
        for y0 in (0..line_len).step_by(TILE) {
            let y_end = y0.saturating_add(TILE).min(line_len);
            for x0 in (0..lines).step_by(TILE) {
                let x_end = x0.saturating_add(TILE).min(lines);
                for y in y0..y_end {
                    let src_row = y.saturating_mul(lines);
                    for x in x0..x_end {
                        if let (Some(&val), Some(dst)) = (
                            self.luma.get(src_row.saturating_add(x)),
                            luma.get_mut(x.saturating_mul(line_len).saturating_add(y)),
                        ) {
                            *dst = val;
                        }
                    }
                }
            }
        }
        Self::with_luma(lines, line_len, luma)
    }

    fn line_luma(&self, index: usize) -> &[u8] {
        let start = index.saturating_mul(self.line_len);
        self.luma
            .get(start..start.saturating_add(self.line_len))
            .unwrap_or_default()
    }
}

fn rgb_luma(r: u8, g: u8, b: u8) -> u8 {
    let val = (u32::from(r) * 77 + u32::from(g) * 150 + u32::from(b) * 29) >> 8;
    u8::try_from(val).unwrap_or(255)
}

fn line_signature(row: &[u8]) -> [u8; SIG_BUCKETS] {
    let mut sig = [0u8; SIG_BUCKETS];
    let len = row.len();
    if len == 0 {
        return sig;
    }
    for (bucket, slot) in sig.iter_mut().enumerate() {
        let start = bucket.saturating_mul(len) / SIG_BUCKETS;
        let end = (bucket.saturating_add(1).saturating_mul(len) / SIG_BUCKETS).max(start + 1);
        let slice = row.get(start..end.min(len)).unwrap_or_default();
        if !slice.is_empty() {
            let sum: u64 = slice.iter().map(|&v| u64::from(v)).sum();
            let count = u64::try_from(slice.len()).unwrap_or(1);
            *slot = u8::try_from(sum / count).unwrap_or(255);
        } else if let Some(&fallback) = row.get(start.min(len - 1)) {
            *slot = fallback;
        }
    }
    sig
}

fn uniform_row_value(row: &[u8]) -> Option<u8> {
    let (&first, rest) = row.split_first()?;
    let mut min_v = first;
    let mut max_v = first;
    for &v in rest {
        min_v = min_v.min(v);
        max_v = max_v.max(v);
    }
    (max_v.saturating_sub(min_v) <= 2).then_some(first)
}

struct MatchCandidate {
    /// 位移（正为向前滚，负为向回滚）。
    shift: isize,
    /// 重叠区有纹理条带的匹配比例（千分位）。
    match_ratio_milli: u64,
    /// 重叠长度（条带数）。
    overlap: usize,
    /// 首部固定栏条带数。
    head: usize,
    /// 尾部固定栏条带数。
    tail: usize,
}

enum AxisSearch {
    NoMove,
    Matched(MatchCandidate),
    TooFast,
}

/// 累计条带缓存（竖向为逐行 RGBA，横向为逐列 RGBA）。
enum Accumulated {
    /// 尚未锁轴，只保留首帧原图。
    Unset(Vec<u8>),
    /// 锁轴后的主干条带与当前固定尾栏。
    Locked {
        axis: ScrollAxis,
        /// 已写入的主干条带（行主序 RGBA 或列主序 RGBA）。
        body: Vec<u8>,
        /// 已写入的条带数。
        body_lines: usize,
        /// 当前帧末尾的固定尾栏，合成或预览时追加一次。
        footer: Vec<u8>,
        /// 固定尾栏条带数。
        footer_lines: usize,
        /// 在最新已采纳帧坐标系中，下一条待写入条带的索引。
        written_end: usize,
    },
}

/// 长截图拼接器：逐帧比较、排除固定头尾栏、增量累积条带并在结束时合成。
pub struct Stitcher {
    frame_size: Option<(u32, u32)>,
    axis: Option<ScrollAxis>,
    prev_vert: Option<OrientedPlane>,
    prev_horiz: Option<OrientedPlane>,
    accumulated: Option<Accumulated>,
    reached_limit: bool,
}

impl Default for Stitcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Stitcher {
    /// 创建空的拼接器。
    pub fn new() -> Self {
        Self {
            frame_size: None,
            axis: None,
            prev_vert: None,
            prev_horiz: None,
            accumulated: None,
            reached_limit: false,
        }
    }

    /// 当前锁定的滚动方向；尚未发生有效滚动前为 `None`。
    pub fn axis(&self) -> Option<ScrollAxis> {
        self.axis
    }

    /// 当前已拼接的总长度（像素）；无帧时为 0。
    pub fn total_px(&self) -> u32 {
        match (&self.frame_size, &self.accumulated) {
            (Some((_, h)), Some(Accumulated::Unset(_))) => (*h).min(MAX_LONG_CAPTURE_PX),
            (
                _,
                Some(Accumulated::Locked {
                    body_lines,
                    footer_lines,
                    ..
                }),
            ) => {
                let total = body_lines.saturating_add(*footer_lines);
                u32::try_from(total)
                    .unwrap_or(MAX_LONG_CAPTURE_PX)
                    .min(MAX_LONG_CAPTURE_PX)
            }
            _ => 0,
        }
    }

    /// 推入一帧图像并尝试与上一采纳帧拼接。
    pub fn push(&mut self, frame: &CroppedImage) -> Result<StitchStatus, StitchError> {
        self.push_raw(frame.width(), frame.height(), frame.rgba())
    }

    /// 推入原始 RGBA 帧并尝试拼接，供单元测试直接传入边界与非法尺寸。
    pub fn push_raw(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<StitchStatus, StitchError> {
        if width == 0 || height == 0 {
            return Err(StitchError::EmptyFrame);
        }
        let w = usize::try_from(width).map_err(|_| StitchError::EmptyFrame)?;
        let h = usize::try_from(height).map_err(|_| StitchError::EmptyFrame)?;
        let expected_bytes = w
            .checked_mul(h)
            .and_then(|px| px.checked_mul(4))
            .ok_or(StitchError::EmptyFrame)?;
        if rgba.len() != expected_bytes {
            return Err(StitchError::InvalidBytes {
                expected: expected_bytes,
                actual: rgba.len(),
            });
        }
        if let Some(expected) = self.frame_size {
            if expected != (width, height) {
                return Err(StitchError::SizeMismatch {
                    expected,
                    actual: (width, height),
                });
            }
        } else {
            self.frame_size = Some((width, height));
            let vert = OrientedPlane::from_rgba(w, h, rgba, ScrollAxis::Vertical);
            let horiz = vert.transpose_from();
            self.prev_vert = Some(vert);
            self.prev_horiz = Some(horiz);
            self.accumulated = Some(Accumulated::Unset(rgba.to_vec()));
            return Ok(StitchStatus::FirstFrame);
        }

        if self.reached_limit
            && let Some(axis) = self.axis
        {
            return Ok(StitchStatus::Adopted {
                total_px: self.total_px(),
                axis,
                reached_limit: true,
            });
        }

        match self.axis {
            None => self.push_unlocked(w, h, rgba),
            Some(axis) => self.push_locked(w, h, rgba, axis),
        }
    }

    /// 尚未锁定滚动轴时，同时尝试竖向与横向匹配。
    fn push_unlocked(
        &mut self,
        w: usize,
        h: usize,
        rgba: &[u8],
    ) -> Result<StitchStatus, StitchError> {
        let Some(prev_v) = self.prev_vert.as_ref() else {
            return Err(StitchError::NoFrames);
        };
        let Some(prev_h) = self.prev_horiz.as_ref() else {
            return Err(StitchError::NoFrames);
        };
        let curr_v = OrientedPlane::from_rgba(w, h, rgba, ScrollAxis::Vertical);
        // 快速鲁棒判定整帧未动（容忍光标闪烁、悬停高亮等局部变化），静止时直接跳过横向转置与搜索
        if is_stationary(prev_v, 0, &curr_v, 0, prev_v.lines) {
            return Ok(StitchStatus::NoMove);
        }
        let search_v = search_axis(prev_v, &curr_v);
        let vert_locks_out_horiz = matches!(
            &search_v,
            AxisSearch::Matched(cv)
                if cv.shift > 0
                    && cv.match_ratio_milli.saturating_add(AXIS_LOCK_ADVANTAGE_MILLI) > 1000
        );
        let (curr_h, search_h) = if vert_locks_out_horiz {
            (None, AxisSearch::TooFast)
        } else {
            let h_plane = curr_v.transpose_from();
            let h_res = search_axis(prev_h, &h_plane);
            (Some(h_plane), h_res)
        };

        let chosen = match (&search_v, &search_h) {
            (AxisSearch::Matched(cv), AxisSearch::Matched(ch)) if cv.shift > 0 && ch.shift > 0 => {
                if ch.match_ratio_milli
                    >= cv
                        .match_ratio_milli
                        .saturating_add(AXIS_LOCK_ADVANTAGE_MILLI)
                {
                    Some((ScrollAxis::Horizontal, ch))
                } else {
                    Some((ScrollAxis::Vertical, cv))
                }
            }
            (AxisSearch::Matched(cv), _) if cv.shift > 0 => Some((ScrollAxis::Vertical, cv)),
            (_, AxisSearch::Matched(ch)) if ch.shift > 0 => Some((ScrollAxis::Horizontal, ch)),
            _ => None,
        };

        if let Some((axis, candidate)) = chosen {
            let head = candidate.head;
            let tail = candidate.tail;
            let shift = usize::try_from(candidate.shift).unwrap_or(0);
            let frame0_rgba = match self.accumulated.take() {
                Some(Accumulated::Unset(buf)) => buf,
                _ => return Err(StitchError::NoFrames),
            };
            let (lines, line_bytes) = match axis {
                ScrollAxis::Vertical => (h, w.saturating_mul(4)),
                ScrollAxis::Horizontal => (w, h.saturating_mul(4)),
            };
            let first_end = lines.saturating_sub(tail);
            let mut body = Vec::new();
            copy_oriented_range(&frame0_rgba, w, h, axis, 0, first_end, &mut body);
            let mut body_lines = first_end;

            let start_in_curr = first_end.saturating_sub(shift).max(head);
            let copy_end = lines.saturating_sub(tail).max(start_in_curr);
            let footer_lines = lines.saturating_sub(copy_end);
            let max_total = usize::try_from(MAX_LONG_CAPTURE_PX).unwrap_or(30_000);
            let allowed_new = max_total
                .saturating_sub(body_lines)
                .saturating_sub(footer_lines);
            let want_new = copy_end.saturating_sub(start_in_curr);
            let take_new = want_new.min(allowed_new);
            let reached =
                want_new > allowed_new || body_lines + take_new + footer_lines >= max_total;
            copy_oriented_range(
                rgba,
                w,
                h,
                axis,
                start_in_curr,
                start_in_curr.saturating_add(take_new),
                &mut body,
            );
            body_lines = body_lines.saturating_add(take_new);

            let mut footer = Vec::with_capacity(footer_lines.saturating_mul(line_bytes));
            copy_oriented_range(rgba, w, h, axis, copy_end, lines, &mut footer);

            self.axis = Some(axis);
            self.reached_limit = reached;
            self.accumulated = Some(Accumulated::Locked {
                axis,
                body,
                body_lines,
                footer,
                footer_lines,
                written_end: copy_end,
            });
            match axis {
                ScrollAxis::Vertical => {
                    self.prev_vert = Some(curr_v);
                    self.prev_horiz = None;
                }
                ScrollAxis::Horizontal => {
                    self.prev_horiz = curr_h;
                    self.prev_vert = None;
                }
            }
            return Ok(StitchStatus::Adopted {
                total_px: self.total_px(),
                axis,
                reached_limit: reached,
            });
        }

        // 未找到向前匹配：区分回滚、静止与太快
        let backward = matches!(&search_v, AxisSearch::Matched(c) if c.shift < 0)
            || matches!(&search_h, AxisSearch::Matched(c) if c.shift < 0);
        if backward {
            return Ok(StitchStatus::IgnoredBackward);
        }
        if matches!(search_v, AxisSearch::NoMove) || matches!(search_h, AxisSearch::NoMove) {
            return Ok(StitchStatus::NoMove);
        }
        Ok(StitchStatus::TooFast)
    }

    /// 轴已锁定时，只沿锁定方向分析与拼接。
    fn push_locked(
        &mut self,
        w: usize,
        h: usize,
        rgba: &[u8],
        axis: ScrollAxis,
    ) -> Result<StitchStatus, StitchError> {
        let curr = OrientedPlane::from_rgba(w, h, rgba, axis);
        let prev = match axis {
            ScrollAxis::Vertical => self.prev_vert.as_ref(),
            ScrollAxis::Horizontal => self.prev_horiz.as_ref(),
        }
        .ok_or(StitchError::NoFrames)?;

        let result = search_axis(prev, &curr);
        let candidate = match result {
            AxisSearch::NoMove => return Ok(StitchStatus::NoMove),
            AxisSearch::TooFast => return Ok(StitchStatus::TooFast),
            AxisSearch::Matched(c) if c.shift < 0 => return Ok(StitchStatus::IgnoredBackward),
            AxisSearch::Matched(c) => c,
        };

        let shift = usize::try_from(candidate.shift).unwrap_or(0);
        let (lines, line_bytes) = match axis {
            ScrollAxis::Vertical => (h, w.saturating_mul(4)),
            ScrollAxis::Horizontal => (w, h.saturating_mul(4)),
        };

        let Some(Accumulated::Locked {
            body,
            body_lines,
            footer,
            footer_lines,
            written_end,
            ..
        }) = self.accumulated.as_mut()
        else {
            return Err(StitchError::NoFrames);
        };

        let start_in_curr = written_end.saturating_sub(shift).max(candidate.head);
        let copy_end = lines.saturating_sub(candidate.tail).max(start_in_curr);
        let new_footer_lines = lines.saturating_sub(copy_end);
        let max_total = usize::try_from(MAX_LONG_CAPTURE_PX).unwrap_or(30_000);
        let want_new = copy_end.saturating_sub(start_in_curr);
        let allowed_new = max_total
            .saturating_sub(*body_lines)
            .saturating_sub(new_footer_lines);
        let take_new = want_new.min(allowed_new);
        let reached =
            want_new > allowed_new || *body_lines + take_new + new_footer_lines >= max_total;

        copy_oriented_range(
            rgba,
            w,
            h,
            axis,
            start_in_curr,
            start_in_curr.saturating_add(take_new),
            body,
        );
        *body_lines = body_lines.saturating_add(take_new);

        footer.clear();
        footer.reserve(new_footer_lines.saturating_mul(line_bytes));
        copy_oriented_range(rgba, w, h, axis, copy_end, lines, footer);
        *footer_lines = new_footer_lines;
        *written_end = copy_end;
        self.reached_limit = reached;

        match axis {
            ScrollAxis::Vertical => self.prev_vert = Some(curr),
            ScrollAxis::Horizontal => self.prev_horiz = Some(curr),
        }

        Ok(StitchStatus::Adopted {
            total_px: self.total_px(),
            axis,
            reached_limit: reached,
        })
    }

    /// 导出最新一端（尾部或右侧）不超过 `max_px` 像素的预览图像，避免预览阶段全图拷贝。
    pub fn tail_preview(&self, max_px: u32) -> Result<CroppedImage, StitchError> {
        let max_len = usize::try_from(max_px.max(1)).unwrap_or(400);
        let (w_u32, h_u32) = self.frame_size.ok_or(StitchError::NoFrames)?;
        let w = usize::try_from(w_u32).map_err(|_| StitchError::EmptyFrame)?;
        let h = usize::try_from(h_u32).map_err(|_| StitchError::EmptyFrame)?;

        match self.accumulated.as_ref() {
            Some(Accumulated::Unset(rgba)) => {
                let take = h.min(max_len);
                let skip = h.saturating_sub(take);
                let start = skip.saturating_mul(w).saturating_mul(4);
                let out = rgba.get(start..).unwrap_or_default().to_vec();
                let out_h = u32::try_from(take).unwrap_or(1);
                Ok(CroppedImage::new(w_u32, out_h, out)?)
            }
            Some(Accumulated::Locked {
                axis,
                body,
                body_lines,
                footer,
                footer_lines,
                ..
            }) => {
                let total = body_lines.saturating_add(*footer_lines);
                let take = total.min(max_len);
                let skip_total = total.saturating_sub(take);
                let line_bytes = match axis {
                    ScrollAxis::Vertical => w.saturating_mul(4),
                    ScrollAxis::Horizontal => h.saturating_mul(4),
                };
                let mut oriented = Vec::with_capacity(take.saturating_mul(line_bytes));
                if skip_total < *body_lines {
                    let body_start = skip_total.saturating_mul(line_bytes);
                    oriented.extend_from_slice(body.get(body_start..).unwrap_or_default());
                    oriented.extend_from_slice(footer);
                } else {
                    let footer_skip = skip_total.saturating_sub(*body_lines);
                    let footer_start = footer_skip.saturating_mul(line_bytes);
                    oriented.extend_from_slice(footer.get(footer_start..).unwrap_or_default());
                }
                match axis {
                    ScrollAxis::Vertical => {
                        let out_h = u32::try_from(take).unwrap_or(1);
                        Ok(CroppedImage::new(w_u32, out_h, oriented)?)
                    }
                    ScrollAxis::Horizontal => {
                        let out_w = u32::try_from(take).unwrap_or(1);
                        let row_major = columns_to_row_major(&oriented, take, h);
                        Ok(CroppedImage::new(out_w, h_u32, row_major)?)
                    }
                }
            }
            None => Err(StitchError::NoFrames),
        }
    }

    /// 一次性合成最终拼接结果。
    pub fn compose(&self) -> Result<CroppedImage, StitchError> {
        let (w_u32, h_u32) = self.frame_size.ok_or(StitchError::NoFrames)?;
        let w = usize::try_from(w_u32).map_err(|_| StitchError::EmptyFrame)?;
        let h = usize::try_from(h_u32).map_err(|_| StitchError::EmptyFrame)?;

        match self.accumulated.as_ref() {
            Some(Accumulated::Unset(rgba)) => {
                let cap_h = h_u32.min(MAX_LONG_CAPTURE_PX);
                let cap_bytes = usize::try_from(cap_h)
                    .unwrap_or(h)
                    .saturating_mul(w)
                    .saturating_mul(4);
                let out = rgba.get(..cap_bytes).unwrap_or(rgba).to_vec();
                Ok(CroppedImage::new(w_u32, cap_h, out)?)
            }
            Some(Accumulated::Locked {
                axis,
                body,
                body_lines,
                footer,
                footer_lines,
                ..
            }) => {
                let total = body_lines.saturating_add(*footer_lines);
                let cap_lines = total.min(usize::try_from(MAX_LONG_CAPTURE_PX).unwrap_or(30_000));
                match axis {
                    ScrollAxis::Vertical => {
                        let mut out =
                            Vec::with_capacity(cap_lines.saturating_mul(w).saturating_mul(4));
                        let body_take = (*body_lines).min(cap_lines);
                        let body_bytes = body_take.saturating_mul(w).saturating_mul(4);
                        out.extend_from_slice(body.get(..body_bytes).unwrap_or(body));
                        let rem = cap_lines.saturating_sub(body_take);
                        if rem > 0 {
                            let footer_bytes = rem.saturating_mul(w).saturating_mul(4);
                            out.extend_from_slice(footer.get(..footer_bytes).unwrap_or(footer));
                        }
                        let out_h = u32::try_from(cap_lines).unwrap_or(MAX_LONG_CAPTURE_PX);
                        Ok(CroppedImage::new(w_u32, out_h, out)?)
                    }
                    ScrollAxis::Horizontal => {
                        let line_bytes = h.saturating_mul(4);
                        let mut cols = Vec::with_capacity(cap_lines.saturating_mul(line_bytes));
                        let body_take = (*body_lines).min(cap_lines);
                        let body_bytes = body_take.saturating_mul(line_bytes);
                        cols.extend_from_slice(body.get(..body_bytes).unwrap_or(body));
                        let rem = cap_lines.saturating_sub(body_take);
                        if rem > 0 {
                            let footer_bytes = rem.saturating_mul(line_bytes);
                            cols.extend_from_slice(footer.get(..footer_bytes).unwrap_or(footer));
                        }
                        let out_w = u32::try_from(cap_lines).unwrap_or(MAX_LONG_CAPTURE_PX);
                        let row_major = columns_to_row_major(&cols, cap_lines, h);
                        Ok(CroppedImage::new(out_w, h_u32, row_major)?)
                    }
                }
            }
            None => Err(StitchError::NoFrames),
        }
    }
}

fn copy_oriented_range(
    rgba: &[u8],
    width: usize,
    height: usize,
    axis: ScrollAxis,
    start: usize,
    end: usize,
    dst: &mut Vec<u8>,
) {
    if start >= end {
        return;
    }
    match axis {
        ScrollAxis::Vertical => {
            let row_bytes = width.saturating_mul(4);
            let b_start = start.saturating_mul(row_bytes);
            let b_end = end.saturating_mul(row_bytes);
            if let Some(slice) = rgba.get(b_start..b_end) {
                dst.extend_from_slice(slice);
            }
        }
        ScrollAxis::Horizontal => {
            let col_bytes = height.saturating_mul(4);
            dst.reserve(end.saturating_sub(start).saturating_mul(col_bytes));
            for x in start..end {
                for y in 0..height {
                    let offset = y.saturating_mul(width).saturating_add(x).saturating_mul(4);
                    if let Some(px) = rgba.get(offset..offset.saturating_add(4)) {
                        dst.extend_from_slice(px);
                    }
                }
            }
        }
    }
}

fn columns_to_row_major(cols: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0u8; width.saturating_mul(height).saturating_mul(4)];
    for y in 0..height {
        let row_base = y.saturating_mul(width);
        for x in 0..width {
            let src_off = x.saturating_mul(height).saturating_add(y).saturating_mul(4);
            let dst_off = row_base.saturating_add(x).saturating_mul(4);
            if let (Some(src_px), Some(dst_px)) = (
                cols.get(src_off..src_off.saturating_add(4)),
                out.get_mut(dst_off..dst_off.saturating_add(4)),
            ) {
                dst_px.copy_from_slice(src_px);
            }
        }
    }
    out
}

fn range_mad_milli(
    a: &OrientedPlane,
    a_start: usize,
    b: &OrientedPlane,
    b_start: usize,
    line_count: usize,
) -> u64 {
    let px_count = line_count.saturating_mul(a.line_len);
    if px_count == 0 {
        return u64::MAX;
    }
    let mut sum: u64 = 0;
    for offset in 0..line_count {
        let la = a.line_luma(a_start.saturating_add(offset));
        let lb = b.line_luma(b_start.saturating_add(offset));
        for (&va, &vb) in la.iter().zip(lb) {
            sum = sum.saturating_add(u64::from(va.abs_diff(vb)));
        }
    }
    let denom = u64::try_from(px_count).unwrap_or(1);
    sum.saturating_mul(1000) / denom
}

fn lines_match(a: &OrientedPlane, b: &OrientedPlane, index: usize) -> bool {
    let la = a.line_luma(index);
    let lb = b.line_luma(index);
    let len = la.len().min(lb.len());
    if len == 0 {
        return false;
    }
    let mut sum: u64 = 0;
    let mut outliers: u64 = 0;
    for (&va, &vb) in la.iter().zip(lb) {
        let diff = va.abs_diff(vb);
        sum = sum.saturating_add(u64::from(diff));
        if diff > 12 {
            outliers = outliers.saturating_add(1);
        }
    }
    let denom = u64::try_from(len).unwrap_or(1);
    let mad = sum.saturating_mul(1000) / denom;
    let outlier_rate = outliers.saturating_mul(1000) / denom;
    mad <= FIXED_LINE_MAD_MILLI && outlier_rate <= FIXED_LINE_OUTLIER_MILLI
}

/// 检测上下（或左右）固定不变区域，并回退末尾与邻接滚动区同色的纯空白行，避免把纯白边距误当固定栏。
fn detect_fixed_bars(prev: &OrientedPlane, curr: &OrientedPlane) -> (usize, usize) {
    let total = prev.lines;
    let mut head = 0usize;
    while head < total && lines_match(prev, curr, head) {
        head = head.saturating_add(1);
    }
    let mut tail = 0usize;
    while head.saturating_add(tail) < total
        && lines_match(prev, curr, total.saturating_sub(1 + tail))
    {
        tail = tail.saturating_add(1);
    }
    if head.saturating_add(tail) >= total {
        return (total, 0);
    }

    // 回退纯色边距：若 head 紧贴的第一行滚动区也是纯色，且 head 末尾连续多行与之同色，说明属于内容留白
    while head > 0 {
        let idx = head - 1;
        let prev_u = prev.uniform_luma.get(idx).copied().flatten();
        let curr_u = curr.uniform_luma.get(idx).copied().flatten();
        let next_prev = prev.uniform_luma.get(head).copied().flatten();
        let next_curr = curr.uniform_luma.get(head).copied().flatten();
        let same_as_next = match (prev_u, curr_u) {
            (Some(pu), Some(cu)) if pu.abs_diff(cu) <= 2 => {
                next_prev.is_some_and(|n| n.abs_diff(pu) <= 2)
                    || next_curr.is_some_and(|n| n.abs_diff(cu) <= 2)
            }
            _ => false,
        };
        if same_as_next {
            head -= 1;
        } else {
            break;
        }
    }
    while tail > 0 {
        let idx = total.saturating_sub(tail);
        let adj = idx.saturating_sub(1);
        let prev_u = prev.uniform_luma.get(idx).copied().flatten();
        let curr_u = curr.uniform_luma.get(idx).copied().flatten();
        let adj_prev = prev.uniform_luma.get(adj).copied().flatten();
        let adj_curr = curr.uniform_luma.get(adj).copied().flatten();
        let same_as_adj = match (prev_u, curr_u) {
            (Some(pu), Some(cu)) if pu.abs_diff(cu) <= 2 => {
                adj_prev.is_some_and(|n| n.abs_diff(pu) <= 2)
                    || adj_curr.is_some_and(|n| n.abs_diff(cu) <= 2)
            }
            _ => false,
        };
        if same_as_adj {
            tail -= 1;
        } else {
            break;
        }
    }
    (head, tail)
}

/// 单行像素级鲁棒匹配：超过 [`PX_DIFF_TOLERANCE`] 的离群像素占比不超过 [`LINE_OUTLIER_MAX_MILLI`]。
fn line_robust_matches(la: &[u8], lb: &[u8]) -> bool {
    let len = la.len().min(lb.len());
    if len == 0 {
        return false;
    }
    let mut outliers: u64 = 0;
    for (&va, &vb) in la.iter().zip(lb) {
        if va.abs_diff(vb) > PX_DIFF_TOLERANCE {
            outliers = outliers.saturating_add(1);
        }
    }
    let denom = u64::try_from(len).unwrap_or(1);
    outliers.saturating_mul(1000) <= LINE_OUTLIER_MAX_MILLI.saturating_mul(denom)
}

/// 统计重叠区内有纹理条带的鲁棒匹配比例（千分位）以及有纹理条带总数；
/// 当失败条带数已不可能达到 `min_ratio_milli` 时提前结束，避免在错误候选上扫描全图。
fn robust_match_ratio(
    a: &OrientedPlane,
    a_start: usize,
    b: &OrientedPlane,
    b_start: usize,
    line_count: usize,
    min_ratio_milli: u64,
) -> (u64, usize) {
    let Some(a_end) = a_start.checked_add(line_count) else {
        return (0, 0);
    };
    let Some(b_end) = b_start.checked_add(line_count) else {
        return (0, 0);
    };
    let (Some(a_uni), Some(b_uni)) = (
        a.uniform_luma.get(a_start..a_end),
        b.uniform_luma.get(b_start..b_end),
    ) else {
        return (0, 0);
    };
    let total_textured = a_uni
        .iter()
        .zip(b_uni)
        .filter(|(ua, ub)| !(ua.is_some() && ub.is_some()))
        .count();
    if total_textured == 0 {
        return (0, 0);
    }
    let min_matched_u64 = u64::try_from(total_textured)
        .unwrap_or(0)
        .saturating_mul(min_ratio_milli)
        .div_ceil(1000);
    let min_matched = usize::try_from(min_matched_u64).unwrap_or(total_textured);
    let max_mismatches = total_textured.saturating_sub(min_matched);

    let mut matched: usize = 0;
    let mut mismatched: usize = 0;
    for (offset, (ua, ub)) in a_uni.iter().zip(b_uni).enumerate() {
        if ua.is_some() && ub.is_some() {
            continue;
        }
        let la = a.line_luma(a_start.saturating_add(offset));
        let lb = b.line_luma(b_start.saturating_add(offset));
        if line_robust_matches(la, lb) {
            matched = matched.saturating_add(1);
        } else {
            mismatched = mismatched.saturating_add(1);
            if mismatched > max_mismatches {
                return (0, total_textured);
            }
        }
    }
    let ratio = u64::try_from(matched).unwrap_or(0).saturating_mul(1000)
        / u64::try_from(total_textured).unwrap_or(1);
    (ratio, total_textured)
}

/// 鲁棒静止判定：有纹理条带零位移匹配比例达到 [`NO_MOVE_RATIO_MILLI`]（或全区无纹理且亮度几乎一致）即视为未动。
fn is_stationary(
    a: &OrientedPlane,
    a_start: usize,
    b: &OrientedPlane,
    b_start: usize,
    line_count: usize,
) -> bool {
    let (ratio, textured) =
        robust_match_ratio(a, a_start, b, b_start, line_count, NO_MOVE_RATIO_MILLI);
    if textured > 0 {
        ratio >= NO_MOVE_RATIO_MILLI
    } else {
        range_mad_milli(a, a_start, b, b_start, line_count) <= NO_MOVE_MAD_MILLI
    }
}

/// 粗筛打分：从锚点帧重叠区内的有纹理条带中取点（<= [`ROUGH_MAX_SAMPLES`] 时全量，超出时分桶抖动抽样），
/// 使用封顶签名误差避免等距抽样混叠及局部遮挡失真。
fn rough_sig_score(
    fixed: &OrientedPlane,
    fixed_start: usize,
    shifted: &OrientedPlane,
    shifted_start: usize,
    anchors: &[usize],
    overlap: usize,
) -> Option<u64> {
    let min_anchors = (MIN_TEXTURED_LINES / 2).max(1);
    let len = anchors.len();
    if len < min_anchors {
        return None;
    }

    let mut sum: u64 = 0;
    let mut count: usize = 0;
    let mut accumulate = |off: usize| {
        if let (Some(sf), Some(ss)) = (
            fixed.sigs.get(fixed_start.saturating_add(off)),
            shifted.sigs.get(shifted_start.saturating_add(off)),
        ) {
            let mut line_err: u32 = 0;
            for (&va, &vb) in sf.iter().zip(ss) {
                line_err = line_err.saturating_add(u32::from(va.abs_diff(vb)));
            }
            sum = sum.saturating_add(u64::from(line_err.min(ROUGH_SIG_LINE_CAP)));
            count = count.saturating_add(1);
        }
    };

    if len <= ROUGH_MAX_SAMPLES {
        for &off in anchors {
            accumulate(off);
        }
    } else {
        for i in 0..ROUGH_MAX_SAMPLES {
            let lo = i.saturating_mul(len) / ROUGH_MAX_SAMPLES;
            let hi = (i.saturating_add(1).saturating_mul(len) / ROUGH_MAX_SAMPLES)
                .max(lo.saturating_add(1));
            let span = hi.saturating_sub(lo).max(1);
            // 桶内相位随桶序号与当前重叠长度非周期变化，避免与文本固定行距产生混叠共振
            let jitter = i.saturating_mul(7).saturating_add(overlap % 11) % span;
            let off = anchors.get(lo.saturating_add(jitter)).copied().unwrap_or(0);
            accumulate(off);
        }
    }
    if count == 0 {
        return None;
    }
    let denom = u64::try_from(count).unwrap_or(1);
    Some(sum.saturating_mul(1000) / denom)
}

fn textured_offsets(plane: &OrientedPlane, head: usize, mid_len: usize) -> Vec<usize> {
    let end = head.saturating_add(mid_len);
    plane
        .uniform_luma
        .get(head..end)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(off, uni)| uni.is_none().then_some(off))
        .collect()
}

fn search_axis(prev: &OrientedPlane, curr: &OrientedPlane) -> AxisSearch {
    let total = prev.lines;
    if is_stationary(prev, 0, curr, 0, total) {
        return AxisSearch::NoMove;
    }
    let (head, tail) = detect_fixed_bars(prev, curr);
    let mid_end = total.saturating_sub(tail);
    let mid_len = mid_end.saturating_sub(head);
    let min_overlap = usize::try_from(MIN_OVERLAP_PX).unwrap_or(32);

    if mid_len < min_overlap {
        return AxisSearch::TooFast;
    }

    if is_stationary(prev, head, curr, head, mid_len) {
        return AxisSearch::NoMove;
    }

    let curr_tex = textured_offsets(curr, head, mid_len);
    let prev_tex = textured_offsets(prev, head, mid_len);
    let max_shift = mid_len.saturating_sub(min_overlap);
    // (粗筛签名误差, 有符号位移)
    let mut rough: Vec<(u64, isize)> = Vec::new();
    for d in 1..=max_shift {
        let overlap = mid_len.saturating_sub(d);
        // 正向：curr[head..head+overlap] 对齐 prev[head+d..head+d+overlap]
        let curr_n = curr_tex.partition_point(|&off| off < overlap);
        if let Some(anchors) = curr_tex.get(..curr_n)
            && let Some(score) =
                rough_sig_score(curr, head, prev, head.saturating_add(d), anchors, overlap)
            && let Ok(shift) = isize::try_from(d)
        {
            rough.push((score, shift));
        }
        // 反向：prev[head..head+overlap] 对齐 curr[head+d..head+d+overlap]
        let prev_n = prev_tex.partition_point(|&off| off < overlap);
        if let Some(anchors) = prev_tex.get(..prev_n)
            && let Some(score) =
                rough_sig_score(prev, head, curr, head.saturating_add(d), anchors, overlap)
            && let Ok(shift) = isize::try_from(d)
        {
            rough.push((score, -shift));
        }
    }

    if rough.is_empty() {
        return if is_stationary(prev, head, curr, head, mid_len) {
            AxisSearch::NoMove
        } else {
            AxisSearch::TooFast
        };
    }

    rough.sort_unstable_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.unsigned_abs().cmp(&b.1.unsigned_abs()))
    });

    let mut verified: Vec<MatchCandidate> = Vec::with_capacity(TOP_CANDIDATES);
    let mut best_ratio: u64 = 0;
    for &(_, shift) in rough.iter().take(TOP_CANDIDATES) {
        let abs_d = shift.unsigned_abs();
        let overlap = mid_len.saturating_sub(abs_d);
        let (p_start, c_start) = if shift > 0 {
            (head.saturating_add(abs_d), head)
        } else {
            (head, head.saturating_add(abs_d))
        };
        let min_req = best_ratio
            .saturating_sub(RATIO_TIE_TOLERANCE_MILLI)
            .max(MATCH_RATIO_MIN_MILLI);
        let (match_ratio_milli, textured) =
            robust_match_ratio(prev, p_start, curr, c_start, overlap, min_req);
        if textured >= MIN_TEXTURED_LINES && match_ratio_milli >= MATCH_RATIO_MIN_MILLI {
            best_ratio = best_ratio.max(match_ratio_milli);
            verified.push(MatchCandidate {
                shift,
                match_ratio_milli,
                overlap,
                head,
                tail,
            });
        }
    }

    let max_ratio = verified.iter().map(|c| c.match_ratio_milli).max();
    let best = max_ratio.and_then(|max_r| {
        verified
            .into_iter()
            .filter(|c| c.match_ratio_milli >= max_r.saturating_sub(RATIO_TIE_TOLERANCE_MILLI))
            .max_by(|a, b| {
                a.overlap
                    .cmp(&b.overlap)
                    .then_with(|| a.match_ratio_milli.cmp(&b.match_ratio_milli))
            })
    });

    match best {
        Some(cand) => AxisSearch::Matched(cand),
        None => AxisSearch::TooFast,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一张高 `doc_h`、宽 `w` 的测试长图，每行用独一无二的彩色条纹编码行号。
    fn make_vertical_doc(w: u32, doc_h: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(usize::try_from(w * doc_h * 4).expect("大小"));
        for y in 0..doc_h {
            // 非周期条纹：不同行具有明显差异的 RGB 与空间分布
            let r_base = u8::try_from((y * 17) % 251).expect("u8");
            let g_base = u8::try_from((y * 37 + y / 7 * 13) % 241).expect("u8");
            let b_base = u8::try_from((y * 53 + (y * y) % 97) % 239).expect("u8");
            for x in 0..w {
                let band = if ((x + y * 3) / 11) % 3 == 0 { 40 } else { 0 };
                rgba.push(r_base.saturating_add(band));
                rgba.push(g_base);
                rgba.push(b_base.wrapping_add(u8::try_from(x % 64).expect("u8")));
                rgba.push(255);
            }
        }
        rgba
    }

    fn crop_vertical_doc(doc: &[u8], w: u32, y0: u32, view_h: u32) -> CroppedImage {
        let row_bytes = usize::try_from(w * 4).expect("row");
        let start = usize::try_from(y0).expect("y0") * row_bytes;
        let len = usize::try_from(view_h).expect("h") * row_bytes;
        CroppedImage::new(w, view_h, doc[start..start + len].to_vec()).expect("构造测试帧")
    }

    fn make_horizontal_doc(doc_w: u32, h: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(usize::try_from(doc_w * h * 4).expect("大小"));
        for y in 0..h {
            for x in 0..doc_w {
                let r = u8::try_from((x * 19 + (x * x) % 89) % 241).expect("u8");
                let g = u8::try_from((x * 41 + y * 7) % 251).expect("u8");
                let b = u8::try_from((x * 61 + x / 5 * 11) % 233).expect("u8");
                let band = if ((y + x * 2) / 9) % 2 == 0 { 35 } else { 0 };
                rgba.push(r.saturating_add(band));
                rgba.push(g);
                rgba.push(b);
                rgba.push(255);
            }
        }
        rgba
    }

    fn crop_horizontal_doc(doc: &[u8], doc_w: u32, h: u32, x0: u32, view_w: u32) -> CroppedImage {
        let mut out = Vec::with_capacity(usize::try_from(view_w * h * 4).expect("大小"));
        for y in 0..h {
            let start = usize::try_from(y * doc_w + x0).expect("off") * 4;
            let len = usize::try_from(view_w).expect("vw") * 4;
            out.extend_from_slice(&doc[start..start + len]);
        }
        CroppedImage::new(view_w, h, out).expect("构造横向帧")
    }

    #[test]
    fn vertical_stitch_produces_exact_pixels() {
        let w = 80;
        let view_h = 120;
        let doc_h = 260;
        let doc = make_vertical_doc(w, doc_h);

        let mut s = Stitcher::new();
        assert_eq!(
            s.push(&crop_vertical_doc(&doc, w, 0, view_h)),
            Ok(StitchStatus::FirstFrame)
        );
        assert_eq!(
            s.push(&crop_vertical_doc(&doc, w, 50, view_h)),
            Ok(StitchStatus::Adopted {
                total_px: 170,
                axis: ScrollAxis::Vertical,
                reached_limit: false
            })
        );
        assert_eq!(
            s.push(&crop_vertical_doc(&doc, w, 110, view_h)),
            Ok(StitchStatus::Adopted {
                total_px: 230,
                axis: ScrollAxis::Vertical,
                reached_limit: false
            })
        );
        assert_eq!(
            s.push(&crop_vertical_doc(&doc, w, 140, view_h)),
            Ok(StitchStatus::Adopted {
                total_px: 260,
                axis: ScrollAxis::Vertical,
                reached_limit: false
            })
        );

        let composed = s.compose().expect("合成");
        assert_eq!((composed.width(), composed.height()), (w, doc_h));
        assert_eq!(composed.rgba(), doc.as_slice());
    }

    #[test]
    fn horizontal_stitch_produces_exact_pixels() {
        let doc_w = 250;
        let h = 80;
        let view_w = 120;
        let doc = make_horizontal_doc(doc_w, h);

        let mut s = Stitcher::new();
        assert_eq!(
            s.push(&crop_horizontal_doc(&doc, doc_w, h, 0, view_w)),
            Ok(StitchStatus::FirstFrame)
        );
        assert_eq!(
            s.push(&crop_horizontal_doc(&doc, doc_w, h, 45, view_w)),
            Ok(StitchStatus::Adopted {
                total_px: 165,
                axis: ScrollAxis::Horizontal,
                reached_limit: false
            })
        );
        assert_eq!(
            s.push(&crop_horizontal_doc(&doc, doc_w, h, 130, view_w)),
            Ok(StitchStatus::Adopted {
                total_px: 250,
                axis: ScrollAxis::Horizontal,
                reached_limit: false
            })
        );

        let composed = s.compose().expect("合成");
        assert_eq!((composed.width(), composed.height()), (doc_w, h));
        assert_eq!(composed.rgba(), doc.as_slice());

        let tail = s.tail_preview(60).expect("尾部预览");
        assert_eq!((tail.width(), tail.height()), (60, h));
        let expected_tail = crop_horizontal_doc(&doc, doc_w, h, 190, 60);
        assert_eq!(tail.rgba(), expected_tail.rgba());
    }

    #[test]
    fn fixed_header_and_footer_are_excluded_and_kept_once() {
        let w = 80;
        let head_h = 20;
        let mid_view = 100;
        let foot_h = 18;
        let doc_mid_h = 220;
        let mid_doc = make_vertical_doc(w, doc_mid_h);

        let header = |y: u32| -> u8 { u8::try_from((y * 9 + 15) % 200).expect("h") };
        let footer = |y: u32| -> u8 { u8::try_from((y * 13 + 70) % 210).expect("f") };

        let build_frame = |scroll_y: u32| -> CroppedImage {
            let mut rgba = Vec::new();
            for y in 0..head_h {
                let v = header(y);
                for x in 0..w {
                    rgba.extend_from_slice(&[v, 40, u8::try_from(x % 100).expect("x"), 255]);
                }
            }
            let start = usize::try_from(scroll_y * w * 4).expect("s");
            let len = usize::try_from(mid_view * w * 4).expect("l");
            rgba.extend_from_slice(&mid_doc[start..start + len]);
            for y in 0..foot_h {
                let v = footer(y);
                for x in 0..w {
                    rgba.extend_from_slice(&[20, v, u8::try_from((x * 3) % 120).expect("x"), 255]);
                }
            }
            CroppedImage::new(w, head_h + mid_view + foot_h, rgba).expect("构造带固定栏帧")
        };

        let mut s = Stitcher::new();
        assert_eq!(s.push(&build_frame(0)), Ok(StitchStatus::FirstFrame));
        assert_eq!(
            s.push(&build_frame(50)),
            Ok(StitchStatus::Adopted {
                total_px: head_h + 150 + foot_h,
                axis: ScrollAxis::Vertical,
                reached_limit: false
            })
        );
        assert_eq!(
            s.push(&build_frame(110)),
            Ok(StitchStatus::Adopted {
                total_px: head_h + 210 + foot_h,
                axis: ScrollAxis::Vertical,
                reached_limit: false
            })
        );

        let composed = s.compose().expect("合成");
        assert_eq!(
            (composed.width(), composed.height()),
            (w, head_h + 210 + foot_h)
        );
        let row_bytes = usize::try_from(w * 4).expect("rb");
        let mid_start = usize::try_from(head_h).expect("hs") * row_bytes;
        let mid_end = mid_start + usize::try_from(210).expect("mh") * row_bytes;
        assert_eq!(
            &composed.rgba()[mid_start..mid_end],
            &mid_doc[..usize::try_from(210 * w * 4).expect("b")]
        );
    }

    #[test]
    fn stationary_backward_and_too_fast_are_classified() {
        let w = 80;
        let view_h = 120;
        let doc = make_vertical_doc(w, 300);

        let mut s = Stitcher::new();
        assert_eq!(
            s.push(&crop_vertical_doc(&doc, w, 40, view_h)),
            Ok(StitchStatus::FirstFrame)
        );
        // 静止
        assert_eq!(
            s.push(&crop_vertical_doc(&doc, w, 40, view_h)),
            Ok(StitchStatus::NoMove)
        );
        // 往回滚被忽略（且不锁轴）
        assert_eq!(
            s.push(&crop_vertical_doc(&doc, w, 10, view_h)),
            Ok(StitchStatus::IgnoredBackward)
        );
        assert_eq!(s.axis(), None);
        // 滚太快（剩余重叠 < 32 px）
        assert_eq!(
            s.push(&crop_vertical_doc(&doc, w, 140, view_h)),
            Ok(StitchStatus::TooFast)
        );
        // 正常向前滚
        assert_eq!(
            s.push(&crop_vertical_doc(&doc, w, 90, view_h)),
            Ok(StitchStatus::Adopted {
                total_px: 170,
                axis: ScrollAxis::Vertical,
                reached_limit: false
            })
        );
        // 锁轴后再往回滚仍被忽略且不破坏状态
        assert_eq!(
            s.push(&crop_vertical_doc(&doc, w, 60, view_h)),
            Ok(StitchStatus::IgnoredBackward)
        );
        assert_eq!(s.total_px(), 170);
    }

    #[test]
    fn enforces_max_long_capture_limit_and_truncates() {
        let w = 16;
        let view_h = 10_500;
        let doc_h = 31_000;
        let doc = make_vertical_doc(w, doc_h);

        let mut s = Stitcher::new();
        assert_eq!(
            s.push(&crop_vertical_doc(&doc, w, 0, view_h)),
            Ok(StitchStatus::FirstFrame)
        );
        assert_eq!(
            s.push(&crop_vertical_doc(&doc, w, 10_000, view_h)),
            Ok(StitchStatus::Adopted {
                total_px: 20_500,
                axis: ScrollAxis::Vertical,
                reached_limit: false
            })
        );
        assert_eq!(
            s.push(&crop_vertical_doc(&doc, w, 20_000, view_h)),
            Ok(StitchStatus::Adopted {
                total_px: MAX_LONG_CAPTURE_PX,
                axis: ScrollAxis::Vertical,
                reached_limit: true
            })
        );
        let composed = s.compose().expect("截断合成");
        assert_eq!(composed.height(), MAX_LONG_CAPTURE_PX);
    }

    #[test]
    fn rejects_invalid_inputs() {
        let mut s = Stitcher::new();
        assert_eq!(s.compose(), Err(StitchError::NoFrames));
        assert_eq!(s.tail_preview(100), Err(StitchError::NoFrames));
        assert_eq!(s.push_raw(0, 10, &[]), Err(StitchError::EmptyFrame));
        assert_eq!(s.push_raw(10, 0, &[]), Err(StitchError::EmptyFrame));
        assert_eq!(
            s.push_raw(4, 4, &[0u8; 10]),
            Err(StitchError::InvalidBytes {
                expected: 64,
                actual: 10
            })
        );

        let ok = CroppedImage::new(40, 40, vec![120; 40 * 40 * 4]).expect("ok");
        assert_eq!(s.push(&ok), Ok(StitchStatus::FirstFrame));
        let wrong_size = CroppedImage::new(40, 41, vec![120; 40 * 41 * 4]).expect("mismatch");
        assert_eq!(
            s.push(&wrong_size),
            Err(StitchError::SizeMismatch {
                expected: (40, 40),
                actual: (40, 41)
            })
        );
    }

    fn overlay_hover_bar(img: &mut [u8], w: u32, y_start: u32, height: u32) {
        let row_bytes = usize::try_from(w * 4).expect("row");
        let start = usize::try_from(y_start).expect("ys") * row_bytes;
        let len = usize::try_from(height).expect("h") * row_bytes;
        if let Some(slice) = img.get_mut(start..start + len) {
            for px in slice.as_chunks_mut::<4>().0 {
                px[0] = px[0].saturating_sub(24);
                px[1] = px[1].saturating_sub(24);
            }
        }
    }

    fn overlay_caret(img: &mut [u8], w: u32, x0: u32, y0: u32, height: u32) {
        for y in y0..y0 + height {
            for x in x0..x0 + 2 {
                let idx = usize::try_from(y * w + x).expect("idx") * 4;
                if let Some(px) = img.get_mut(idx..idx + 3) {
                    px.copy_from_slice(&[0, 0, 0]);
                }
            }
        }
    }

    fn overlay_scrollbar(img: &mut [u8], w: u32, view_h: u32, doc_h: u32, y0: u32) {
        let thumb_h = (view_h * view_h / doc_h).max(8);
        let thumb_y = y0 * view_h / doc_h;
        for y in 0..view_h {
            let shade = if y >= thumb_y && y < thumb_y + thumb_h {
                140u8
            } else {
                235u8
            };
            for x in (w - 10)..w {
                let idx = usize::try_from(y * w + x).expect("idx") * 4;
                if let Some(px) = img.get_mut(idx..idx + 3) {
                    px.copy_from_slice(&[shade, shade, shade]);
                }
            }
        }
    }

    #[test]
    fn hover_highlight_keeps_vertical_axis_and_detects_no_move() {
        let w = 200;
        let view_h = 300;
        let doc = make_vertical_doc(w, 800);

        let frame_with_hover = |y0: u32, hover_y: Option<u32>| -> CroppedImage {
            let mut raw = crop_vertical_doc(&doc, w, y0, view_h).rgba().to_vec();
            if let Some(hy) = hover_y {
                overlay_hover_bar(&mut raw, w, hy, 22);
            }
            CroppedImage::new(w, view_h, raw).expect("hover帧")
        };

        let mut s = Stitcher::new();
        assert_eq!(
            s.push(&frame_with_hover(0, None)),
            Ok(StitchStatus::FirstFrame)
        );
        // 画面未滚动，仅悬停条移动 -> 应判为 NoMove 而非已移动或误锁横向
        assert_eq!(
            s.push(&frame_with_hover(0, Some(120))),
            Ok(StitchStatus::NoMove)
        );
        assert_eq!(s.axis(), None);

        // 伴随悬停条变化的小幅向下滚动 -> 正确锁定竖向并采纳
        assert_eq!(
            s.push(&frame_with_hover(40, Some(125))),
            Ok(StitchStatus::Adopted {
                total_px: view_h + 40,
                axis: ScrollAxis::Vertical,
                reached_limit: false,
            })
        );
        assert_eq!(
            s.push(&frame_with_hover(100, Some(130))),
            Ok(StitchStatus::Adopted {
                total_px: view_h + 100,
                axis: ScrollAxis::Vertical,
                reached_limit: false,
            })
        );
        assert_eq!(
            s.push(&frame_with_hover(100, Some(120))),
            Ok(StitchStatus::NoMove)
        );
        assert_eq!(
            s.push(&frame_with_hover(180, Some(125))),
            Ok(StitchStatus::Adopted {
                total_px: view_h + 180,
                axis: ScrollAxis::Vertical,
                reached_limit: false,
            })
        );
    }

    #[test]
    fn caret_blink_is_treated_as_stationary_and_allows_scrolling() {
        let w = 200;
        let view_h = 240;
        let doc = make_vertical_doc(w, 600);

        let frame_with_caret = |y0: u32, caret_on: bool| -> CroppedImage {
            let mut raw = crop_vertical_doc(&doc, w, y0, view_h).rgba().to_vec();
            if caret_on {
                overlay_caret(&mut raw, w, 90, 100, 20);
            }
            CroppedImage::new(w, view_h, raw).expect("caret帧")
        };

        let mut s = Stitcher::new();
        assert_eq!(
            s.push(&frame_with_caret(0, false)),
            Ok(StitchStatus::FirstFrame)
        );
        assert_eq!(s.push(&frame_with_caret(0, true)), Ok(StitchStatus::NoMove));
        assert_eq!(
            s.push(&frame_with_caret(0, false)),
            Ok(StitchStatus::NoMove)
        );
        assert_eq!(
            s.push(&frame_with_caret(40, true)),
            Ok(StitchStatus::Adopted {
                total_px: view_h + 40,
                axis: ScrollAxis::Vertical,
                reached_limit: false,
            })
        );
        assert_eq!(
            s.push(&frame_with_caret(40, false)),
            Ok(StitchStatus::NoMove)
        );
        assert_eq!(
            s.push(&frame_with_caret(100, true)),
            Ok(StitchStatus::Adopted {
                total_px: view_h + 100,
                axis: ScrollAxis::Vertical,
                reached_limit: false,
            })
        );
    }

    #[test]
    fn scrollbar_movement_is_tolerated_during_scroll() {
        let w = 260;
        let view_h = 260;
        let doc_h = 900;
        let doc = make_vertical_doc(w, doc_h);

        let frame_with_sb = |y0: u32| -> CroppedImage {
            let mut raw = crop_vertical_doc(&doc, w, y0, view_h).rgba().to_vec();
            overlay_scrollbar(&mut raw, w, view_h, doc_h, y0);
            CroppedImage::new(w, view_h, raw).expect("scrollbar帧")
        };

        let mut s = Stitcher::new();
        assert_eq!(s.push(&frame_with_sb(0)), Ok(StitchStatus::FirstFrame));
        for &(y0, expected_total) in &[
            (20u32, view_h + 20),
            (60, view_h + 60),
            (120, view_h + 120),
            (240, view_h + 240),
        ] {
            assert_eq!(
                s.push(&frame_with_sb(y0)),
                Ok(StitchStatus::Adopted {
                    total_px: expected_total,
                    axis: ScrollAxis::Vertical,
                    reached_limit: false,
                })
            );
        }
    }

    #[test]
    fn small_vertical_scroll_steps_succeed() {
        let w = 120;
        let view_h = 220;
        let doc_h = 400;
        let doc = make_vertical_doc(w, doc_h);

        let mut s = Stitcher::new();
        assert_eq!(
            s.push(&crop_vertical_doc(&doc, w, 0, view_h)),
            Ok(StitchStatus::FirstFrame)
        );
        for &(y0, total) in &[
            (8u32, view_h + 8),
            (20, view_h + 20),
            (40, view_h + 40),
            (75, view_h + 75),
            (135, view_h + 135),
        ] {
            assert_eq!(
                s.push(&crop_vertical_doc(&doc, w, y0, view_h)),
                Ok(StitchStatus::Adopted {
                    total_px: total,
                    axis: ScrollAxis::Vertical,
                    reached_limit: false,
                })
            );
        }
        let composed = s.compose().expect("合成");
        assert_eq!(composed.height(), view_h + 135);
        assert_eq!(
            composed.rgba(),
            crop_vertical_doc(&doc, w, 0, view_h + 135).rgba()
        );
    }
}
