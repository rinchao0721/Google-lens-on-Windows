//! 检测与识别两个 ONNX 会话的加载与推理编排。

use std::path::Path;
use std::sync::Mutex;

use lenswin_core::error::OcrDataError;
use lenswin_core::frame::CroppedImage;
use lenswin_core::ocr::{
    DecodedLine, PixelBox, REC_HEIGHT, Segment, assemble_lines, column_ink, decode_ctc,
    det_postprocess, det_preprocess, pack_rec_batch, parse_charset, plan_rec_batches, rec_prepare,
    valid_steps,
};
use ort::environment::Environment;
use ort::session::Session;
use ort::value::Tensor;

use crate::error::OcrError;

/// 嵌入的检测模型。
const DET_MODEL: &[u8] = include_bytes!("../../../assets/ocr/det.onnx");
/// 嵌入的识别模型。
const REC_MODEL: &[u8] = include_bytes!("../../../assets/ocr/rec.onnx");
/// 嵌入的字典，一行一个字符。
const DICT: &str = include_str!("../../../assets/ocr/dict.txt");

/// 已加载的识别引擎，加载后可反复调用。
pub struct OcrEngine {
    det: Mutex<Session>,
    rec: Mutex<Session>,
    charset: Vec<String>,
    // 会话之后释放
    _env: Environment,
}

fn inference(err: impl std::fmt::Display) -> OcrError {
    OcrError::Inference(err.to_string())
}

fn model_error(model: &'static str) -> impl FnOnce(ort::Error) -> OcrError {
    move |e| OcrError::Model {
        model,
        reason: e.to_string(),
    }
}

impl OcrEngine {
    /// 从 `dll_path`（绝对路径）加载 ONNX Runtime，并创建检测与识别会话。
    ///
    /// DLL 不存在时返回 [`OcrError::DllMissing`]，调用方可在补回文件后重试。
    pub fn load(dll_path: &Path) -> Result<Self, OcrError> {
        if !dll_path.is_file() {
            return Err(OcrError::DllMissing(dll_path.to_path_buf()));
        }
        let env = ort::init_from(dll_path)
            .map_err(|e| OcrError::Runtime(e.to_string()))?
            .build()
            .map_err(|e| OcrError::Runtime(e.to_string()))?;
        let det = Session::builder(&env)
            .and_then(|mut b| b.commit_from_memory(DET_MODEL))
            .map_err(model_error("检测"))?;
        let rec = Session::builder(&env)
            .and_then(|mut b| b.commit_from_memory(REC_MODEL))
            .map_err(model_error("识别"))?;
        let charset = parse_charset(DICT);
        check_charset(&rec, charset.len())?;
        Ok(Self {
            det: Mutex::new(det),
            rec: Mutex::new(rec),
            charset,
            _env: env,
        })
    }

    /// 识别图像中的文字，返回逐行文本；没有文字时返回空列表。
    pub fn recognize(&self, image: &CroppedImage) -> Result<Vec<String>, OcrError> {
        let boxes = self.detect(image)?;
        if boxes.is_empty() {
            return Ok(Vec::new());
        }
        let segments = self.recognize_boxes(image, &boxes)?;
        Ok(assemble_lines(&segments))
    }

    fn detect(&self, image: &CroppedImage) -> Result<Vec<PixelBox>, OcrError> {
        let input = det_preprocess(image)?;
        let shape = [1, 3, dim(input.height)?, dim(input.width)?];
        let (_, prob) = infer(&self.det, shape, input.data)?;
        Ok(det_postprocess(
            &prob,
            input.width,
            input.height,
            image.width(),
            image.height(),
        )?)
    }

    fn recognize_boxes(
        &self,
        image: &CroppedImage,
        boxes: &[PixelBox],
    ) -> Result<Vec<Segment>, OcrError> {
        let crops = boxes
            .iter()
            .map(|b| rec_prepare(image, b))
            .collect::<Result<Vec<_>, _>>()?;
        let widths: Vec<u32> = crops.iter().map(|c| c.width).collect();
        let mut lines: Vec<Option<DecodedLine>> = vec![None; crops.len()];
        for batch in plan_rec_batches(&widths) {
            let members: Vec<_> = batch.members.iter().filter_map(|&i| crops.get(i)).collect();
            let data = pack_rec_batch(&members, batch.width)?;
            let shape = [members.len(), 3, dim(REC_HEIGHT)?, dim(batch.width)?];
            let (out_shape, logits) = infer(&self.rec, shape, data)?;
            let decoded = self.decode_batch(&out_shape, &logits, &members, batch.width)?;
            for (&index, line) in batch.members.iter().zip(decoded) {
                if let Some(slot) = lines.get_mut(index) {
                    *slot = Some(line);
                }
            }
        }
        Ok(boxes
            .iter()
            .zip(lines)
            .zip(&crops)
            .filter_map(|((bounds, line), crop)| {
                line.map(|line| Segment {
                    bounds: *bounds,
                    line,
                    ink: column_ink(crop),
                })
            })
            .collect())
    }

    /// 解码一批识别输出，输出形状须为 `[批大小, 时间步, 类别数]`。
    fn decode_batch(
        &self,
        out_shape: &[i64],
        logits: &[f32],
        members: &[&lenswin_core::ocr::RecCrop],
        bucket_width: u32,
    ) -> Result<Vec<DecodedLine>, OcrError> {
        let &[n, steps, classes] = out_shape else {
            return Err(OcrError::BadOutput("识别输出不是三维张量"));
        };
        let (n, steps, classes) = (to_len(n)?, to_len(steps)?, to_len(classes)?);
        if n != members.len() {
            return Err(OcrError::BadOutput("识别输出的批大小与输入不符"));
        }
        let per_sample = steps
            .checked_mul(classes)
            .ok_or(OcrDataError::SizeOverflow)?;
        if per_sample == 0 {
            return Err(OcrError::BadOutput("识别输出为空"));
        }
        logits
            .chunks_exact(per_sample)
            .zip(members)
            .map(|(sample, crop)| {
                let valid = valid_steps(steps, crop.width, bucket_width);
                Ok(decode_ctc(sample, steps, classes, valid, &self.charset)?)
            })
            .collect()
    }
}

/// 张量维度转 `usize`。
fn dim(value: u32) -> Result<usize, OcrError> {
    usize::try_from(value).map_err(|_| OcrDataError::SizeOverflow.into())
}

fn to_len(value: i64) -> Result<usize, OcrError> {
    usize::try_from(value).map_err(|_| OcrError::BadOutput("张量维度为负"))
}

/// 加载时核对字典与识别模型的类别数；模型声明为动态维度时留到推理时校验。
fn check_charset(rec: &Session, charset_len: usize) -> Result<(), OcrError> {
    let declared = rec
        .outputs()
        .first()
        .and_then(|o| o.dtype().tensor_shape())
        .and_then(|s| s.last().copied())
        .and_then(|d| usize::try_from(d).ok())
        .filter(|&d| d > 0);
    match declared {
        Some(classes) if classes != charset_len => Err(OcrDataError::CharsetMismatch {
            charset: charset_len,
            classes,
        }
        .into()),
        _ => Ok(()),
    }
}

/// 运行会话一次，返回第一个输出的形状与数据。
fn infer(
    session: &Mutex<Session>,
    shape: [usize; 4],
    data: Vec<f32>,
) -> Result<(Vec<i64>, Vec<f32>), OcrError> {
    let tensor = Tensor::from_array((shape, data)).map_err(inference)?;
    let mut session = session.lock().map_err(|_| OcrError::Poisoned)?;
    let name = session
        .inputs()
        .first()
        .map(|i| i.name().to_string())
        .ok_or(OcrError::BadOutput("模型没有输入"))?;
    let outputs = session
        .run(ort::inputs![name.as_str() => tensor])
        .map_err(inference)?;
    let first = outputs
        .values()
        .next()
        .ok_or(OcrError::BadOutput("模型没有输出"))?;
    let (out_shape, out_data) = first.try_extract_tensor::<f32>().map_err(inference)?;
    Ok((out_shape.to_vec(), out_data.to_vec()))
}
