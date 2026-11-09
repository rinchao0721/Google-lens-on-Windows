//! 配置文件的读取与原子写入。
//!
//! 这是 M4 的最小实现，只为持久化隐私确认；M5 的 `ConfigStore` 会接管读取、热更新和统一写入。

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use lenswin_core::config::Config;

/// 配置文件的来源状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigState {
    /// 读取成功。
    Loaded,
    /// 文件不存在，已写入默认值。
    Created,
    /// 文件无法使用，本次运行用默认值，且不得覆盖原文件。
    Invalid,
}

/// 读取结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedConfig {
    /// 实际使用的配置。
    pub config: Config,
    /// 来源状态。
    pub state: ConfigState,
    /// 需要告知用户的问题。
    pub notice: Option<String>,
}

/// 由文件内容（`None` 表示文件不存在）得到配置，不做 IO。
fn interpret(text: Option<&str>) -> LoadedConfig {
    let Some(text) = text else {
        return LoadedConfig {
            config: Config::default(),
            state: ConfigState::Created,
            notice: None,
        };
    };
    match Config::from_toml_str(text) {
        Ok(config) => LoadedConfig {
            config,
            state: ConfigState::Loaded,
            notice: None,
        },
        Err(e) => LoadedConfig {
            config: Config::default(),
            state: ConfigState::Invalid,
            notice: Some(format!("配置文件有误，本次使用默认设置：{e}")),
        },
    }
}

/// 读取配置；不存在则写入默认值，解析或校验失败时保持原文件不动。
pub fn load(path: &Path) -> LoadedConfig {
    match fs::read_to_string(path) {
        Ok(text) => interpret(Some(&text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut loaded = interpret(None);
            if let Err(e) = save(path, &loaded.config) {
                log::warn!("写入默认配置失败：{e}");
                loaded.notice = Some(format!("无法创建配置文件：{e}"));
            }
            loaded
        }
        Err(e) => LoadedConfig {
            config: Config::default(),
            state: ConfigState::Invalid,
            notice: Some(format!("无法读取配置文件，本次使用默认设置：{e}")),
        },
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(ToOwned::to_owned).unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

/// 原子写入：先写临时文件并落盘，再替换目标。
pub fn save(path: &Path, config: &Config) -> io::Result<()> {
    let text = config
        .to_toml_string()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temp = temp_path(path);
    let written = (|| {
        let mut file = fs::File::create(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })();
    if let Err(e) = written {
        // 临时文件残留无害，清理失败不影响错误上报
        match fs::remove_file(&temp) {
            Ok(()) | Err(_) => {}
        }
        return Err(e);
    }
    fs::rename(&temp, path)
}

/// 配置来源允许写回时才写；`Invalid` 状态下写回会覆盖用户手写的文件。
pub fn persist(path: &Path, state: ConfigState, config: &Config) -> Result<(), String> {
    if state == ConfigState::Invalid {
        return Err("配置文件有误，未写入".to_string());
    }
    save(path, config).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lenswin-cfg-{tag}-{}", std::process::id()));
        match fs::remove_dir_all(&dir) {
            Ok(()) | Err(_) => {}
        }
        dir
    }

    #[test]
    fn missing_file_means_defaults_created() {
        let loaded = interpret(None);
        assert_eq!(loaded.state, ConfigState::Created);
        assert_eq!(loaded.config, Config::default());
    }

    #[test]
    fn valid_text_is_loaded() {
        let loaded = interpret(Some("[privacy]\nacknowledged_host = \"a.b\"\n"));
        assert_eq!(loaded.state, ConfigState::Loaded);
        assert_eq!(loaded.config.privacy.acknowledged_host, "a.b");
        assert_eq!(loaded.notice, None);
    }

    #[test]
    fn broken_text_falls_back_without_permission_to_write() {
        for text in [
            "not toml ===",
            "[hotkeys]\nsearch = \"A\"\n",
            "[unknown]\nx = 1\n",
        ] {
            let loaded = interpret(Some(text));
            assert_eq!(loaded.state, ConfigState::Invalid, "{text}");
            assert_eq!(loaded.config, Config::default());
            assert!(loaded.notice.is_some());
        }
    }

    #[test]
    fn persist_refuses_invalid_state() {
        let dir = temp_dir("refuse");
        let path = dir.join("config.toml");
        assert!(persist(&path, ConfigState::Invalid, &Config::default()).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn load_creates_default_file_then_roundtrips() {
        let dir = temp_dir("create");
        let path = dir.join("config.toml");
        let first = load(&path);
        assert_eq!(first.state, ConfigState::Created);
        assert!(path.is_file());

        let mut config = first.config;
        config.privacy.acknowledged_host = "litterbox.catbox.moe".to_string();
        persist(&path, ConfigState::Created, &config).expect("写回");
        let second = load(&path);
        assert_eq!(second.state, ConfigState::Loaded);
        assert_eq!(second.config, config);
        assert!(!temp_path(&path).exists());
        fs::remove_dir_all(&dir).expect("清理");
    }

    #[test]
    fn broken_file_is_left_untouched() {
        let dir = temp_dir("untouched");
        fs::create_dir_all(&dir).expect("创建目录");
        let path = dir.join("config.toml");
        fs::write(&path, "garbage ===").expect("写入");
        let loaded = load(&path);
        assert_eq!(loaded.state, ConfigState::Invalid);
        assert_eq!(fs::read_to_string(&path).expect("读取"), "garbage ===");
        fs::remove_dir_all(&dir).expect("清理");
    }
}
