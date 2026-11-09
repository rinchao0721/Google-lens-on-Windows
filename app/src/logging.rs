//! 日志初始化：GUI 程序没有控制台，统一写文件。

use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::Path;

use anyhow::Context;
use simplelog::{ConfigBuilder, LevelFilter, WriteLogger};

const LOG_FILE_NAME: &str = "lenswin.log";
const OLD_LOG_FILE_NAME: &str = "lenswin.log.old";
/// 超过该大小则在启动时轮转。
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

/// 初始化文件日志并安装 panic 钩子，使崩溃原因落盘。
pub fn init(log_dir: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(log_dir).context("创建日志目录失败")?;
    let log_path = log_dir.join(LOG_FILE_NAME);
    // 轮转失败时继续追加写入，不影响启动；此时日志器尚未就绪，无处记录
    match rotate_if_large(&log_path, &log_dir.join(OLD_LOG_FILE_NAME), MAX_LOG_BYTES) {
        Ok(_) | Err(_) => {}
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .context("打开日志文件失败")?;

    let mut builder = ConfigBuilder::new();
    builder
        .set_thread_level(LevelFilter::Off)
        .set_location_level(LevelFilter::Off);
    match builder.set_time_offset_to_local() {
        Ok(_) | Err(_) => {} // 取不到本地时区时保持 UTC
    }
    let level = if cfg!(debug_assertions) {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    WriteLogger::init(level, builder.build(), file).context("安装日志器失败")?;

    std::panic::set_hook(Box::new(|info| {
        log::error!("程序 panic：{info}");
    }));
    Ok(())
}

/// 日志超过 `max_bytes` 时改名为 `old`，覆盖已有旧文件；返回是否发生了轮转。
fn rotate_if_large(current: &Path, old: &Path, max_bytes: u64) -> io::Result<bool> {
    let len = match fs::metadata(current) {
        Ok(meta) => meta.len(),
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if len <= max_bytes {
        return Ok(false);
    }
    // 首次轮转时旧文件不存在，属于正常情况
    match fs::remove_file(old) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    fs::rename(current, old)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 测试用临时目录，离开作用域时清理。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("lenswin-log-test-{tag}-{}", std::process::id()));
            fs::create_dir_all(&dir).expect("创建临时目录");
            Self(dir)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            // 清理失败不影响测试结论
            match fs::remove_dir_all(&self.0) {
                Ok(()) | Err(_) => {}
            }
        }
    }

    #[test]
    fn rotates_when_old_file_missing() {
        let dir = TempDir::new("missing-old");
        let (cur, old) = (dir.file("a.log"), dir.file("a.log.old"));
        fs::write(&cur, vec![b'x'; 100]).expect("写入");
        assert!(rotate_if_large(&cur, &old, 10).expect("轮转"));
        assert!(!cur.exists());
        assert_eq!(fs::read(&old).expect("读取").len(), 100);
    }

    #[test]
    fn rotation_replaces_existing_old_file() {
        let dir = TempDir::new("existing-old");
        let (cur, old) = (dir.file("a.log"), dir.file("a.log.old"));
        fs::write(&old, b"stale").expect("写入");
        fs::write(&cur, vec![b'y'; 50]).expect("写入");
        assert!(rotate_if_large(&cur, &old, 10).expect("轮转"));
        assert!(!cur.exists());
        assert_eq!(fs::read(&old).expect("读取"), vec![b'y'; 50]);
    }

    #[test]
    fn does_not_rotate_below_limit() {
        let dir = TempDir::new("small");
        let (cur, old) = (dir.file("a.log"), dir.file("a.log.old"));
        fs::write(&cur, vec![b'z'; 10]).expect("写入");
        assert!(!rotate_if_large(&cur, &old, 10).expect("检查"));
        assert!(cur.exists());
        assert!(!old.exists());
    }

    #[test]
    fn missing_current_file_is_not_an_error() {
        let dir = TempDir::new("no-current");
        let result = rotate_if_large(&dir.file("a.log"), &dir.file("a.log.old"), 10);
        assert!(matches!(result, Ok(false)));
    }
}
