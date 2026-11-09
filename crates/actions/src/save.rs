//! 把图像保存为 PNG 文件。

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use lenswin_core::action::{
    Action, ActionContext, ActionInput, ActionKind, ActionOutcome, InputKind,
};
use lenswin_core::error::ActionError;
use lenswin_core::selection::SelectionMode;
use lenswin_core::url::screenshot_file_name;
use lenswin_platform::clock::local_now;

use crate::encode::encode_png;

/// 同一秒内重名时最多尝试的序号。
const MAX_NAME_ATTEMPTS: u32 = 100;

/// 保存动作，目录取自 `screenshot.save_dir`。
#[derive(Debug, Default, Clone, Copy)]
pub struct SaveAction;

/// 给文件名加序号：`a.png` → `a_1.png`；序号为 0 时原样返回。
fn numbered_name(name: &str, n: u32) -> String {
    if n == 0 {
        return name.to_string();
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}_{n}.{ext}"),
        None => format!("{name}_{n}"),
    }
}

/// 写入失败时删除刚创建的文件，避免留下半截 PNG。
fn write_or_cleanup<W: Write>(mut writer: W, path: &Path, bytes: &[u8]) -> Result<(), ActionError> {
    let Err(e) = writer.write_all(bytes) else {
        return Ok(());
    };
    // 先释放句柄再删，Windows 下占用中的文件无法删除
    drop(writer);
    if let Err(remove_err) = fs::remove_file(path) {
        log::warn!("清理未写完的文件 {} 失败：{remove_err}", path.display());
    }
    Err(ActionError::Failed(format!("写入文件失败：{e}")))
}

/// 以「不覆盖已有文件」的方式写入，重名时递增序号。
fn write_unique(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, ActionError> {
    for n in 0..MAX_NAME_ATTEMPTS {
        let path = dir.join(numbered_name(name, n));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                write_or_cleanup(file, &path, bytes)?;
                return Ok(path);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(ActionError::Failed(format!("创建文件失败：{e}"))),
        }
    }
    Err(ActionError::Failed(
        "同名截图过多，无法生成文件名".to_string(),
    ))
}

impl Action for SaveAction {
    fn kind(&self) -> ActionKind {
        ActionKind::Save
    }

    fn input_kind(&self) -> InputKind {
        InputKind::Image
    }

    fn preferred_mode(&self) -> SelectionMode {
        SelectionMode::Rect
    }

    fn run(&self, ctx: &ActionContext, input: ActionInput) -> Result<ActionOutcome, ActionError> {
        let ActionInput::Image(image) = input else {
            return Err(ActionError::InvalidInput);
        };
        let dir = ctx.config.screenshot.save_dir.trim();
        if dir.is_empty() {
            return Err(ActionError::Failed("未设置截图保存目录".to_string()));
        }
        let dir = Path::new(dir);
        fs::create_dir_all(dir)
            .map_err(|e| ActionError::Failed(format!("创建保存目录失败：{e}")))?;
        let bytes = encode_png(&image)?;
        let now = local_now();
        let name = screenshot_file_name(
            now.year, now.month, now.day, now.hour, now.minute, now.second,
        );
        let path = write_unique(dir, &name, &bytes)?;
        log::info!("截图已保存：{}", path.display());
        Ok(ActionOutcome::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lenswin_core::config::Config;
    use lenswin_core::frame::CroppedImage;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lenswin-test-{tag}-{}", std::process::id()));
        // 目录可能不存在，清理失败无需处理
        match fs::remove_dir_all(&dir) {
            Ok(()) | Err(_) => {}
        }
        dir
    }

    #[test]
    fn numbered_names() {
        assert_eq!(numbered_name("a.png", 0), "a.png");
        assert_eq!(numbered_name("a.png", 2), "a_2.png");
        assert_eq!(numbered_name("a", 1), "a_1");
        assert_eq!(numbered_name("a.b.png", 1), "a.b_1.png");
    }

    #[test]
    fn write_unique_never_overwrites() {
        let dir = temp_dir("unique");
        fs::create_dir_all(&dir).expect("创建目录");
        let first = write_unique(&dir, "x.png", b"one").expect("写入");
        let second = write_unique(&dir, "x.png", b"two").expect("写入");
        assert_ne!(first, second);
        assert_eq!(fs::read(&first).expect("读取"), b"one");
        assert_eq!(fs::read(&second).expect("读取"), b"two");
        fs::remove_dir_all(&dir).expect("清理");
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("磁盘已满"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn failed_write_removes_the_created_file() {
        let dir = temp_dir("cleanup");
        fs::create_dir_all(&dir).expect("创建目录");
        let path = dir.join("half.png");
        fs::write(&path, b"").expect("预置文件");
        let err = write_or_cleanup(FailingWriter, &path, b"data");
        assert!(matches!(err, Err(ActionError::Failed(_))));
        assert!(!path.exists());
        fs::remove_dir_all(&dir).expect("清理");
    }

    #[test]
    fn run_saves_png_into_configured_dir() {
        let dir = temp_dir("run");
        let mut config = Config::default();
        config.screenshot.save_dir = dir.to_string_lossy().into_owned();
        let ctx = ActionContext { config };
        let image = CroppedImage::new(1, 1, vec![1, 2, 3, 255]).expect("图像");
        SaveAction
            .run(&ctx, ActionInput::Image(image))
            .expect("保存");
        let entries: Vec<_> = fs::read_dir(&dir).expect("读取目录").collect();
        assert_eq!(entries.len(), 1);
        fs::remove_dir_all(&dir).expect("清理");
    }

    #[test]
    fn empty_dir_is_an_error() {
        let ctx = ActionContext {
            config: Config::default(),
        };
        let image = CroppedImage::new(1, 1, vec![0, 0, 0, 255]).expect("图像");
        assert!(SaveAction.run(&ctx, ActionInput::Image(image)).is_err());
    }
}
