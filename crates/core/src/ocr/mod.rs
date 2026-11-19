//! 离线 OCR 的纯逻辑：检测与识别的前后处理、CTC 解码、行聚类与文本拼接。
//!
//! 推理由 `ocr` crate 完成，这里只处理张量数据，不依赖任何运行时。

mod det;
mod layout;
mod num;
mod rec;
mod resample;
mod text;

pub use det::{DET_MAX_SIDE, DetInput, PixelBox, det_postprocess, det_preprocess, det_target_size};
pub use layout::{MIN_CONFIDENCE, Segment, assemble_lines};
pub use rec::{
    DecodedChar, DecodedLine, REC_HEIGHT, REC_MAX_BATCH, REC_MAX_WIDTH, REC_MIN_WIDTH, RecBatch,
    RecCrop, column_ink, decode_ctc, pack_rec_batch, parse_charset, plan_rec_batches,
    rec_bucket_width, rec_prepare, rec_target_width, valid_steps,
};
pub use text::{
    crop_ocr_chunk, has_recognized_text, insert_script_spaces, join_lines, merge_ocr_chunks,
    plan_ocr_chunks,
};
