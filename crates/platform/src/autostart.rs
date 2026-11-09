//! 开机自启：读写 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`。

use std::path::Path;

use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, REG_VALUE_TYPE, RegDeleteValueW, RegQueryValueExW,
    RegSetValueExW,
};
use windows::core::PCWSTR;

use crate::error::{PlatformError, win32};
use crate::handle::RegKey;
use crate::text::to_wide;

/// 自启项在注册表里的值名。
pub const VALUE_NAME: &str = "lenswin";

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// 自启项当前状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutostartState {
    /// 未开启。
    Disabled,
    /// 已开启且指向当前 exe。
    Enabled,
    /// 已开启，但指向别的路径（如便携版被移动过），附带注册表里的命令行。
    StalePath(String),
}

/// 写入注册表的命令行：路径加引号，避免空格被拆开。
fn command_line(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// 取命令行里的可执行文件路径（去掉首尾引号）。
fn exe_of_command(command: &str) -> &str {
    let trimmed = command.trim();
    match trimmed.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or_default(),
        None => trimmed,
    }
}

/// 由注册表里的值判断状态；路径比较不区分大小写。
fn classify(value: Option<&str>, exe: &Path) -> AutostartState {
    let Some(value) = value else {
        return AutostartState::Disabled;
    };
    let registered = exe_of_command(value);
    if registered.eq_ignore_ascii_case(&exe.display().to_string()) {
        AutostartState::Enabled
    } else {
        AutostartState::StalePath(value.to_string())
    }
}

fn read_value(subkey: &str, name: &str) -> Result<Option<String>, PlatformError> {
    let Some(key) = RegKey::open(subkey, KEY_QUERY_VALUE)? else {
        return Ok(None);
    };
    let wide_name = to_wide(name);
    let mut kind = REG_VALUE_TYPE::default();
    let mut size = 0u32;
    // SAFETY: 数据指针为空时只返回所需字节数，其余指针指向有效的局部变量。
    let probe = unsafe {
        RegQueryValueExW(
            key.raw(),
            PCWSTR(wide_name.as_ptr()),
            None,
            Some(&mut kind),
            None,
            Some(&mut size),
        )
    };
    if probe == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    probe.ok().map_err(win32("RegQueryValueExW"))?;
    if kind != REG_SZ {
        return Ok(Some(String::new()));
    }
    let units = usize::try_from(size).unwrap_or(0) / 2 + 1;
    let mut buffer = vec![0u16; units];
    let mut capacity = u32::try_from(buffer.len().saturating_mul(2)).unwrap_or(u32::MAX);
    // SAFETY: buffer 至少有 capacity 字节可写，capacity 是其真实字节数。
    unsafe {
        RegQueryValueExW(
            key.raw(),
            PCWSTR(wide_name.as_ptr()),
            None,
            Some(&mut kind),
            Some(buffer.as_mut_ptr().cast::<u8>()),
            Some(&mut capacity),
        )
    }
    .ok()
    .map_err(win32("RegQueryValueExW"))?;
    let written = usize::try_from(capacity).unwrap_or(0) / 2;
    let text: Vec<u16> = buffer
        .into_iter()
        .take(written)
        .take_while(|unit| *unit != 0)
        .collect();
    Ok(Some(String::from_utf16_lossy(&text)))
}

fn write_value(subkey: &str, name: &str, command: &str) -> Result<(), PlatformError> {
    let key = RegKey::create(subkey, KEY_SET_VALUE)?;
    let wide_name = to_wide(name);
    let data: Vec<u8> = to_wide(command)
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect();
    // SAFETY: wide_name 以 NUL 结尾，data 是以 NUL 结尾的 UTF-16 字节，均在调用期间有效。
    unsafe {
        RegSetValueExW(
            key.raw(),
            PCWSTR(wide_name.as_ptr()),
            None,
            REG_SZ,
            Some(&data),
        )
    }
    .ok()
    .map_err(win32("RegSetValueExW"))
}

fn delete_value(subkey: &str, name: &str) -> Result<(), PlatformError> {
    let Some(key) = RegKey::open(subkey, KEY_SET_VALUE)? else {
        return Ok(());
    };
    let wide_name = to_wide(name);
    // SAFETY: wide_name 以 NUL 结尾且在调用期间有效。
    let status = unsafe { RegDeleteValueW(key.raw(), PCWSTR(wide_name.as_ptr())) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    status.ok().map_err(win32("RegDeleteValueW"))
}

/// 查询自启项相对 `exe` 的状态。
pub fn state(name: &str, exe: &Path) -> Result<AutostartState, PlatformError> {
    Ok(classify(read_value(RUN_KEY, name)?.as_deref(), exe))
}

/// 开启自启，指向 `exe`；已有旧路径时直接覆盖。
pub fn enable(name: &str, exe: &Path) -> Result<(), PlatformError> {
    write_value(RUN_KEY, name, &command_line(exe))
}

/// 关闭自启；本来就没有时视为成功。
pub fn disable(name: &str) -> Result<(), PlatformError> {
    delete_value(RUN_KEY, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyW};

    #[test]
    fn command_line_quotes_path_with_spaces() {
        let exe = PathBuf::from(r"C:\Program Files\lenswin\lenswin.exe");
        assert_eq!(
            command_line(&exe),
            r#""C:\Program Files\lenswin\lenswin.exe""#
        );
    }

    #[test]
    fn exe_of_command_handles_quotes_and_arguments() {
        assert_eq!(exe_of_command(r#""C:\a b\x.exe" --flag"#), r"C:\a b\x.exe");
        assert_eq!(exe_of_command(r"C:\x.exe"), r"C:\x.exe");
        assert_eq!(exe_of_command(""), "");
    }

    #[test]
    fn classify_covers_all_states() {
        let exe = PathBuf::from(r"C:\Tools\lenswin.exe");
        assert_eq!(classify(None, &exe), AutostartState::Disabled);
        assert_eq!(
            classify(Some(r#""c:\tools\LENSWIN.exe""#), &exe),
            AutostartState::Enabled
        );
        assert_eq!(
            classify(Some(r#""D:\old\lenswin.exe""#), &exe),
            AutostartState::StalePath(r#""D:\old\lenswin.exe""#.to_string())
        );
    }

    /// 测试专用子项；销毁时删除整个子项，断言失败触发 panic 也会清理。
    struct PrivateKey(String);

    impl PrivateKey {
        fn new(tag: &str) -> Self {
            let key = Self(format!(
                r"Software\lenswin-test-{}-{tag}",
                std::process::id()
            ));
            // 清掉可能残留的同名子项，保证从空白开始
            key.delete();
            key
        }

        fn delete(&self) {
            let wide = to_wide(&self.0);
            // 子项不存在时会返回错误，属于预期情况
            // SAFETY: wide 以 NUL 结尾且在调用期间有效。
            match unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr())) }.ok() {
                Ok(()) | Err(_) => {}
            }
        }
    }

    impl Drop for PrivateKey {
        fn drop(&mut self) {
            self.delete();
        }
    }

    #[test]
    fn registry_roundtrip_in_private_key() {
        let key = PrivateKey::new("roundtrip");
        let subkey = key.0.as_str();
        assert_eq!(read_value(subkey, "v").expect("读取"), None);
        write_value(subkey, "v", r#""C:\x y\a.exe""#).expect("写入");
        assert_eq!(
            read_value(subkey, "v").expect("读取").as_deref(),
            Some(r#""C:\x y\a.exe""#)
        );
        delete_value(subkey, "v").expect("删除");
        assert_eq!(read_value(subkey, "v").expect("读取"), None);
        delete_value(subkey, "v").expect("重复删除视为成功");
    }

    #[test]
    fn private_key_is_removed_after_test() {
        let subkey;
        {
            let key = PrivateKey::new("cleanup");
            subkey = key.0.clone();
            write_value(&subkey, "v", "x").expect("写入");
            assert!(
                RegKey::open(&subkey, KEY_QUERY_VALUE)
                    .expect("打开")
                    .is_some()
            );
        }
        assert!(
            RegKey::open(&subkey, KEY_QUERY_VALUE)
                .expect("打开")
                .is_none()
        );
    }
}
