//! 日志初始化：GUI 程序没有控制台，统一写文件。

use std::fs::{self, OpenOptions};
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
    rotate_if_large(&log_path, &log_dir.join(OLD_LOG_FILE_NAME));
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

fn rotate_if_large(current: &Path, old: &Path) {
    let too_large = fs::metadata(current).is_ok_and(|m| m.len() > MAX_LOG_BYTES);
    if !too_large {
        return;
    }
    // 轮转失败时继续追加写入，不影响启动；此时日志器尚未就绪，无处记录
    match fs::remove_file(old).and_then(|()| fs::rename(current, old)) {
        Ok(()) | Err(_) => {}
    }
}
