//! 便携版 / 安装版的模式判定，以及配置与日志目录。

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
use windows::core::GUID;

use crate::error::{PlatformError, win32};

/// exe 同目录存在该文件即为便携版。
pub const PORTABLE_MARKER: &str = "portable";
/// 应用数据目录名。
const APP_DIR_NAME: &str = "lenswin";
/// 配置文件名。
const CONFIG_FILE_NAME: &str = "config.toml";
/// 便携版数据目录名。
const PORTABLE_DATA_DIR: &str = "data";
/// 写入探测用的临时文件名。
const WRITE_PROBE_NAME: &str = ".lenswin-write-test";

/// 运行模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 便携版：数据在 exe 旁的 `data` 目录。
    Portable,
    /// 安装版：数据在用户的 `%APPDATA%` 与 `%LOCALAPPDATA%`。
    Installed,
    /// 存在便携标记但目录不可写，本次改用安装版路径。
    PortableFallback,
}

impl Mode {
    /// 面向用户的名称。
    pub const fn label(self) -> &'static str {
        match self {
            Mode::Portable => "便携版",
            Mode::Installed => "安装版",
            Mode::PortableFallback => "便携版（目录不可写，已改用安装版路径）",
        }
    }
}

/// 运行期使用的目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    /// 运行模式。
    pub mode: Mode,
    /// 配置目录。
    pub config_dir: PathBuf,
    /// 日志目录。
    pub log_dir: PathBuf,
}

impl AppPaths {
    /// 配置文件完整路径。
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join(CONFIG_FILE_NAME)
    }

    /// 探测当前进程的运行模式并得到目录；便携目录不可写时降级为 `PortableFallback`。
    pub fn detect() -> Result<Self, PlatformError> {
        let exe = std::env::current_exe().map_err(|source| PlatformError::Io {
            context: "获取 exe 路径",
            source,
        })?;
        let exe_dir = exe
            .parent()
            .ok_or_else(|| PlatformError::Invalid("exe 路径没有父目录".to_string()))?;
        let has_marker = exe_dir.join(PORTABLE_MARKER).is_file();
        let writable = has_marker && probe_writable(&exe_dir.join(PORTABLE_DATA_DIR));
        let roaming = known_folder(&FOLDERID_RoamingAppData, "SHGetKnownFolderPath(Roaming)")?;
        let local = known_folder(&FOLDERID_LocalAppData, "SHGetKnownFolderPath(Local)")?;
        Ok(resolve(exe_dir, has_marker, writable, &roaming, &local))
    }
}

/// 由探测结果推出目录，不做任何 IO，便于测试两种模式。
pub fn resolve(
    exe_dir: &Path,
    has_marker: bool,
    portable_writable: bool,
    roaming: &Path,
    local: &Path,
) -> AppPaths {
    let installed = |mode| AppPaths {
        mode,
        config_dir: roaming.join(APP_DIR_NAME),
        log_dir: local.join(APP_DIR_NAME).join("logs"),
    };
    match (has_marker, portable_writable) {
        (false, _) => installed(Mode::Installed),
        (true, false) => installed(Mode::PortableFallback),
        (true, true) => {
            let data = exe_dir.join(PORTABLE_DATA_DIR);
            AppPaths {
                mode: Mode::Portable,
                log_dir: data.join("logs"),
                config_dir: data,
            }
        }
    }
}

/// 创建目录并写入再删除一个探测文件，确认可写。
fn probe_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(WRITE_PROBE_NAME);
    if std::fs::write(&probe, b"").is_err() {
        return false;
    }
    std::fs::remove_file(&probe).is_ok()
}

fn known_folder(id: &GUID, api: &'static str) -> Result<PathBuf, PlatformError> {
    // SAFETY: id 指向有效 GUID；成功时返回的缓冲区在下面复制后用 CoTaskMemFree 释放。
    let pwstr = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.map_err(win32(api))?;
    // SAFETY: SHGetKnownFolderPath 成功时返回以 NUL 结尾的有效宽字符串。
    let wide = unsafe { pwstr.as_wide() };
    let path = PathBuf::from(OsString::from_wide(wide));
    // SAFETY: 缓冲区由 SHGetKnownFolderPath 分配，此处只释放一次，之后不再使用 pwstr。
    unsafe { CoTaskMemFree(Some(pwstr.0.cast_const().cast())) };
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roaming() -> PathBuf {
        PathBuf::from(r"C:\Users\u\AppData\Roaming")
    }

    fn local() -> PathBuf {
        PathBuf::from(r"C:\Users\u\AppData\Local")
    }

    #[test]
    fn installed_mode_uses_appdata() {
        let p = resolve(
            Path::new(r"C:\Program Files\lenswin"),
            false,
            false,
            &roaming(),
            &local(),
        );
        assert_eq!(p.mode, Mode::Installed);
        assert_eq!(
            p.config_dir,
            PathBuf::from(r"C:\Users\u\AppData\Roaming\lenswin")
        );
        assert_eq!(
            p.log_dir,
            PathBuf::from(r"C:\Users\u\AppData\Local\lenswin\logs")
        );
        assert_eq!(
            p.config_file(),
            PathBuf::from(r"C:\Users\u\AppData\Roaming\lenswin\config.toml")
        );
    }

    #[test]
    fn marker_without_write_access_is_ignored_for_installed() {
        let p = resolve(
            Path::new(r"D:\tools\lenswin"),
            false,
            true,
            &roaming(),
            &local(),
        );
        assert_eq!(p.mode, Mode::Installed);
    }

    #[test]
    fn portable_mode_uses_exe_data_dir() {
        let p = resolve(
            Path::new(r"D:\tools\lenswin"),
            true,
            true,
            &roaming(),
            &local(),
        );
        assert_eq!(p.mode, Mode::Portable);
        assert_eq!(p.config_dir, PathBuf::from(r"D:\tools\lenswin\data"));
        assert_eq!(p.log_dir, PathBuf::from(r"D:\tools\lenswin\data\logs"));
    }

    #[test]
    fn unwritable_portable_falls_back_to_installed_paths() {
        let p = resolve(
            Path::new(r"C:\Program Files\lenswin"),
            true,
            false,
            &roaming(),
            &local(),
        );
        assert_eq!(p.mode, Mode::PortableFallback);
        assert_eq!(
            p.config_dir,
            PathBuf::from(r"C:\Users\u\AppData\Roaming\lenswin")
        );
        assert_eq!(
            p.log_dir,
            PathBuf::from(r"C:\Users\u\AppData\Local\lenswin\logs")
        );
    }

    #[test]
    fn detect_returns_paths_for_current_process() {
        let p = AppPaths::detect().expect("探测运行目录");
        assert!(!p.config_dir.as_os_str().is_empty());
        assert!(!p.log_dir.as_os_str().is_empty());
    }
}
