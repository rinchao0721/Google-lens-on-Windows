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

/// 固定栏逐行匹配的最大平均亮度差（千分位，5000 = 5.0 灰度级）。
const FIXED_LINE_MAD_MILLI: u64 = 2_000;

/// 固定栏逐行允许的最大强变化像素比例（千分位，20 = 2%）。
const FIXED_LINE_OUTLIER_MILLI: u64 = 20;

/// 位移匹配可接受的最大平均亮度差（千分位，4500 = 4.5 灰度级）。
const MATCH_MAD_MILLI: u64 = 4_500;

/// 判定内容静止的最大平均亮度差（千分位，2500 = 2.5 灰度级）。
const NO_MOVE_MAD_MILLI: u64 = 2_500;

/// 纯色/无纹理区域判定的最小跨行亮度极差。
const MIN_TEXTURE_SPAN: u8 = 3;

/// 粗筛保留的候选位移个数。
const TOP_CANDIDATES: usize = 8;

/// 判定近似并列时优先选较大重叠的容差（千分位，250 = 0.25 灰度级）。
const TIE_TOLERANCE_MILLI: u64 = 250;

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
    /// 每条带的 16 段均值签名，供位移粗筛与纹理判定。
    sigs: Vec<[u8; SIG_BUCKETS]>,
    /// 每条带自身是否近似纯色，以及纯色均值（用于避免把大片纯白空白误当成固定栏）。
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
    /// 重叠区的平均亮度误差（千分位）。
    mad_milli: u64,
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
            self.prev_vert = Some(OrientedPlane::from_rgba(w, h, rgba, ScrollAxis::Vertical));
            self.prev_horiz = Some(OrientedPlane::from_rgba(w, h, rgba, ScrollAxis::Horizontal));
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
        // 快速判定整帧未动
        if whole_mad_milli(
            prev_v,
            &OrientedPlane::from_rgba(w, h, rgba, ScrollAxis::Vertical),
        ) <= 500
        {
            return Ok(StitchStatus::NoMove);
        }
        let curr_v = OrientedPlane::from_rgba(w, h, rgba, ScrollAxis::Vertical);
        let curr_h = OrientedPlane::from_rgba(w, h, rgba, ScrollAxis::Horizontal);
        let search_v = search_axis(prev_v, &curr_v);
        let search_h = search_axis(prev_h, &curr_h);

        let chosen = match (&search_v, &search_h) {
            (AxisSearch::Matched(cv), AxisSearch::Matched(ch)) if cv.shift > 0 && ch.shift > 0 => {
                if ch.mad_milli < cv.mad_milli {
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
                    self.prev_horiz = Some(curr_h);
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

fn whole_mad_milli(a: &OrientedPlane, b: &OrientedPlane) -> u64 {
    range_mad_milli(a, 0, b, 0, a.lines)
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

/// 从 `start` 起向后累积，找出首次出现足够纵向纹理所需的最小窗口长度。
/// 若整个 `[start .. start + len)` 都缺乏纵向纹理，则返回 `None`。
fn min_textured_len(plane: &OrientedPlane, start: usize, len: usize) -> Option<usize> {
    let end = start.saturating_add(len).min(plane.sigs.len());
    let slice = plane.sigs.get(start..end)?;
    let mut mins = [255u8; SIG_BUCKETS];
    let mut maxs = [0u8; SIG_BUCKETS];
    for (idx, sig) in slice.iter().enumerate() {
        for ((min_slot, max_slot), &v) in mins.iter_mut().zip(&mut maxs).zip(sig) {
            *min_slot = (*min_slot).min(v);
            *max_slot = (*max_slot).max(v);
        }
        if mins
            .iter()
            .zip(&maxs)
            .any(|(&lo, &hi)| hi.saturating_sub(lo) >= MIN_TEXTURE_SPAN)
        {
            return Some(idx.saturating_add(1));
        }
    }
    None
}

fn sig_mad_milli(
    a: &OrientedPlane,
    a_start: usize,
    b: &OrientedPlane,
    b_start: usize,
    line_count: usize,
) -> u64 {
    if line_count == 0 {
        return u64::MAX;
    }
    // 粗匹配最多等距抽样 128 行，避免高分辨率选区出现 O(H^2) 开销
    let step = (line_count / 128).max(1);
    let mut sum: u64 = 0;
    let mut sampled: usize = 0;
    let mut offset = 0usize;
    while offset < line_count {
        let Some(sa) = a.sigs.get(a_start.saturating_add(offset)) else {
            return u64::MAX;
        };
        let Some(sb) = b.sigs.get(b_start.saturating_add(offset)) else {
            return u64::MAX;
        };
        for (&va, &vb) in sa.iter().zip(sb) {
            sum = sum.saturating_add(u64::from(va.abs_diff(vb)));
        }
        sampled = sampled.saturating_add(1);
        offset = offset.saturating_add(step);
    }
    let denom = u64::try_from(sampled.saturating_mul(SIG_BUCKETS)).unwrap_or(1);
    sum.saturating_mul(1000) / denom
}

fn search_axis(prev: &OrientedPlane, curr: &OrientedPlane) -> AxisSearch {
    let total = prev.lines;
    let (head, tail) = detect_fixed_bars(prev, curr);
    let mid_end = total.saturating_sub(tail);
    let mid_len = mid_end.saturating_sub(head);
    let min_overlap = usize::try_from(MIN_OVERLAP_PX).unwrap_or(32);

    if mid_len < min_overlap {
        return if whole_mad_milli(prev, curr) <= NO_MOVE_MAD_MILLI {
            AxisSearch::NoMove
        } else {
            AxisSearch::TooFast
        };
    }

    let zero_mad = range_mad_milli(prev, head, curr, head, mid_len);
    if zero_mad <= 500 {
        return AxisSearch::NoMove;
    }

    let min_tex_curr = min_textured_len(curr, head, mid_len);
    let min_tex_prev = min_textured_len(prev, head, mid_len);
    let max_shift = mid_len.saturating_sub(min_overlap);
    // (签名误差, 有符号位移)
    let mut rough: Vec<(u64, isize)> = Vec::new();
    for d in 1..=max_shift {
        let overlap = mid_len.saturating_sub(d);
        // 正向：curr[head..head+overlap] 对齐 prev[head+d..head+d+overlap]
        if min_tex_curr.is_some_and(|need| overlap >= need) {
            let mad = sig_mad_milli(prev, head.saturating_add(d), curr, head, overlap);
            if let Ok(shift) = isize::try_from(d) {
                rough.push((mad, shift));
            }
        }
        // 反向：prev[head..head+overlap] 对齐 curr[head+d..head+d+overlap]
        if min_tex_prev.is_some_and(|need| overlap >= need) {
            let mad = sig_mad_milli(prev, head, curr, head.saturating_add(d), overlap);
            if let Ok(shift) = isize::try_from(d) {
                rough.push((mad, -shift));
            }
        }
    }

    if rough.is_empty() {
        return if zero_mad <= NO_MOVE_MAD_MILLI {
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
    for &(_, shift) in rough.iter().take(TOP_CANDIDATES) {
        let abs_d = shift.unsigned_abs();
        let overlap = mid_len.saturating_sub(abs_d);
        let (p_start, c_start) = if shift > 0 {
            (head.saturating_add(abs_d), head)
        } else {
            (head, head.saturating_add(abs_d))
        };
        let mad_milli = range_mad_milli(prev, p_start, curr, c_start, overlap);
        if mad_milli <= MATCH_MAD_MILLI {
            verified.push(MatchCandidate {
                shift,
                mad_milli,
                overlap,
                head,
                tail,
            });
        }
    }

    let min_mad = verified.iter().map(|c| c.mad_milli).min();
    let best = min_mad.and_then(|min_m| {
        verified
            .into_iter()
            .filter(|c| c.mad_milli <= min_m.saturating_add(TIE_TOLERANCE_MILLI))
            .max_by(|a, b| {
                a.overlap
                    .cmp(&b.overlap)
                    .then_with(|| b.mad_milli.cmp(&a.mad_milli))
            })
    });

    match best {
        Some(cand)
            if zero_mad <= cand.mad_milli.saturating_add(500) && zero_mad <= NO_MOVE_MAD_MILLI =>
        {
            AxisSearch::NoMove
        }
        Some(cand) => AxisSearch::Matched(cand),
        None if zero_mad <= NO_MOVE_MAD_MILLI => AxisSearch::NoMove,
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
}
