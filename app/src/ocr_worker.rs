//! OCR 工作线程：常驻，首次任务时加载引擎，加载失败下次任务再试。

use std::io;
use std::sync::mpsc::{Receiver, SendError, Sender, channel};

use lenswin_actions::{OcrEngine, OcrError, load_ocr_engine, recognize_image};
use lenswin_core::frame::CroppedImage;
use lenswin_platform::paths;
use lenswin_platform::window::TaskNotifier;

/// ONNX Runtime 动态库文件名，与 exe 同目录。
const DLL_NAME: &str = "onnxruntime.dll";

/// 一次识别的结果。
pub type OcrOutcome = Result<Option<String>, OcrError>;

/// 工作线程的任务入口。
pub struct OcrWorker {
    jobs: Sender<CroppedImage>,
}

impl OcrWorker {
    /// 启动工作线程；识别结果经 `results` 回传，并用 `notifier` 唤醒主线程。
    pub fn spawn(results: Sender<OcrOutcome>, notifier: TaskNotifier) -> io::Result<Self> {
        let (jobs, inbox) = channel();
        std::thread::Builder::new()
            .name("lenswin-ocr".to_string())
            .spawn(move || run(&inbox, &results, notifier))?;
        Ok(Self { jobs })
    }

    /// 提交一张待识别的图像；工作线程已退出时返回错误。
    pub fn submit(&self, image: CroppedImage) -> Result<(), SendError<CroppedImage>> {
        self.jobs.send(image)
    }
}

fn run(inbox: &Receiver<CroppedImage>, results: &Sender<OcrOutcome>, notifier: TaskNotifier) {
    let mut engine: Option<OcrEngine> = None;
    while let Ok(image) = inbox.recv() {
        let outcome = recognize(&mut engine, &image);
        if results.send(outcome).is_err() {
            return;
        }
        if let Err(e) = notifier.notify() {
            log::error!("投递文字识别结果失败：{e}");
        }
    }
}

fn recognize(engine: &mut Option<OcrEngine>, image: &CroppedImage) -> OcrOutcome {
    let loaded = ensure_loaded(engine, || {
        let dir = paths::exe_dir().map_err(|e| OcrError::LoadFailed(e.to_string()))?;
        load_ocr_engine(&dir.join(DLL_NAME))
    })?;
    recognize_image(loaded, image)
}

/// 槽里没有引擎时才调用 `load`；加载失败不占槽，下次仍会重试。
fn ensure_loaded<E>(
    slot: &mut Option<E>,
    load: impl FnOnce() -> Result<E, OcrError>,
) -> Result<&E, OcrError> {
    if slot.is_none() {
        *slot = Some(load()?);
    }
    slot.as_ref()
        .ok_or_else(|| OcrError::Failed("识别引擎未就绪".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_loads_once_and_failures_are_retried() {
        let mut slot: Option<u32> = None;
        let mut calls = 0;

        let failed = ensure_loaded(&mut slot, || {
            calls += 1;
            Err(OcrError::DllMissing("x".to_string()))
        });
        assert!(matches!(failed, Err(OcrError::DllMissing(_))));
        assert!(slot.is_none());

        let loaded = ensure_loaded(&mut slot, || {
            calls += 1;
            Ok(7)
        });
        assert_eq!(loaded.copied(), Ok(7));

        let again = ensure_loaded(&mut slot, || {
            calls += 1;
            Ok(8)
        });
        assert_eq!(again.copied(), Ok(7));
        assert_eq!(calls, 2);
    }
}
