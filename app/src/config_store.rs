//! 配置中心：所有配置修改的唯一入口，读写盘都在专用工作线程。

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use lenswin_core::config::{Config, validate};

use crate::config_file::{self, ConfigState, LoadedConfig};

/// 等待工作线程读完配置的上限。
const LOAD_TIMEOUT: Duration = Duration::from_secs(10);

/// 发给工作线程的任务。
enum Job {
    Write {
        config: Box<Config>,
        state: ConfigState,
    },
    Load {
        reply: Sender<LoadedConfig>,
    },
}

/// 重新加载成功后的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reloaded {
    /// 重新加载前生效的配置。
    pub previous: Config,
    /// 文件里读到并已生效的新配置。
    pub current: Config,
}

/// 配置的持有者，只在主线程使用。
pub struct ConfigStore {
    path: PathBuf,
    current: RefCell<Config>,
    state: Cell<ConfigState>,
    jobs: Option<Sender<Job>>,
    failures: Receiver<String>,
    worker: Option<JoinHandle<()>>,
}

/// 把校验问题拼成一段给用户看的文字。
fn describe_issues(issues: &[lenswin_core::config::ValidationIssue]) -> String {
    issues
        .iter()
        .map(|i| format!("{}：{}", i.field, i.message))
        .collect::<Vec<_>>()
        .join("；")
}

fn run_worker(
    path: PathBuf,
    jobs: Receiver<Job>,
    failures: Sender<String>,
    notify: impl Fn() + Send + 'static,
) {
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Load { reply } => {
                let loaded = config_file::load(&path);
                if reply.send(loaded).is_err() {
                    // 调用方已超时放弃，结果无人接收
                    log::warn!("配置读取结果无人接收");
                }
            }
            Job::Write { config, state } => {
                if let Err(message) = config_file::persist(&path, state, &config) {
                    log::warn!("保存配置失败：{message}");
                    if failures.send(message).is_ok() {
                        notify();
                    }
                }
            }
        }
    }
}

impl ConfigStore {
    /// 启动工作线程并读取配置；返回需要告知用户的问题（如文件有误）。
    ///
    /// `notify` 在工作线程里写盘失败时调用，用于唤醒主线程。
    pub fn open(
        path: PathBuf,
        notify: impl Fn() + Send + 'static,
    ) -> Result<(Self, Option<String>), String> {
        let (jobs_tx, jobs_rx) = channel();
        let (failures_tx, failures_rx) = channel();
        let worker_path = path.clone();
        let worker = std::thread::Builder::new()
            .name("lenswin-config".to_string())
            .spawn(move || run_worker(worker_path, jobs_rx, failures_tx, notify))
            .map_err(|e| format!("创建配置线程失败：{e}"))?;
        let store = Self {
            path,
            current: RefCell::new(Config::default()),
            state: Cell::new(ConfigState::Created),
            jobs: Some(jobs_tx),
            failures: failures_rx,
            worker: Some(worker),
        };
        let loaded = store.request_load()?;
        store.current.replace(loaded.config);
        store.state.set(loaded.state);
        Ok((store, loaded.notice))
    }

    fn send(&self, job: Job) -> Result<(), String> {
        let jobs = self.jobs.as_ref().ok_or("配置线程已停止")?;
        jobs.send(job).map_err(|_| "配置线程已停止".to_string())
    }

    fn request_load(&self) -> Result<LoadedConfig, String> {
        let (reply, answer) = channel();
        self.send(Job::Load { reply })?;
        answer
            .recv_timeout(LOAD_TIMEOUT)
            .map_err(|_| "读取配置超时".to_string())
    }

    /// 配置文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 当前生效配置的副本。
    pub fn snapshot(&self) -> Config {
        self.current.borrow().clone()
    }

    /// 校验并应用新配置，写盘交给工作线程；校验失败时保持旧配置。
    pub fn apply(&self, new: Config) -> Result<(), String> {
        validate(&new).map_err(|issues| describe_issues(&issues))?;
        self.current.replace(new.clone());
        self.send(Job::Write {
            config: Box::new(new),
            state: self.state.get(),
        })
    }

    /// 在当前配置上做一处修改并应用。
    pub fn update(&self, change: impl FnOnce(&mut Config)) -> Result<(), String> {
        let mut next = self.snapshot();
        change(&mut next);
        self.apply(next)
    }

    /// 重新读取配置文件；文件有误时保持旧配置并返回原因。
    pub fn reload(&self) -> Result<Reloaded, String> {
        let loaded = self.request_load()?;
        if loaded.state == ConfigState::Invalid {
            return Err(loaded
                .notice
                .unwrap_or_else(|| "配置文件无法使用".to_string()));
        }
        let previous = self.current.replace(loaded.config.clone());
        self.state.set(loaded.state);
        Ok(Reloaded {
            previous,
            current: loaded.config,
        })
    }

    /// 取出工作线程累积的写盘失败原因。
    pub fn drain_failures(&self) -> Vec<String> {
        self.failures.try_iter().collect()
    }
}

impl Drop for ConfigStore {
    fn drop(&mut self) {
        // 先关闭任务通道让线程退出，并等它写完排队中的配置
        self.jobs.take();
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                log::error!("配置线程异常退出");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lenswin_core::selection::SelectionMode;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lenswin-store-{tag}-{}", std::process::id()));
        match fs::remove_dir_all(&dir) {
            Ok(()) | Err(_) => {}
        }
        dir
    }

    fn open(path: PathBuf) -> (ConfigStore, Option<String>) {
        ConfigStore::open(path, || {}).expect("打开")
    }

    #[test]
    fn open_creates_default_file_and_apply_persists_after_drop() {
        let dir = temp_dir("persist");
        let path = dir.join("config.toml");
        let (store, notice) = open(path.clone());
        assert_eq!(notice, None);
        assert!(path.is_file());
        store
            .update(|c| c.search.selection_mode = SelectionMode::Rect)
            .expect("应用");
        assert_eq!(store.snapshot().search.selection_mode, SelectionMode::Rect);
        drop(store);
        let reopened = open(path).0;
        assert_eq!(
            reopened.snapshot().search.selection_mode,
            SelectionMode::Rect
        );
        drop(reopened);
        fs::remove_dir_all(&dir).expect("清理");
    }

    #[test]
    fn invalid_config_is_rejected_and_old_one_kept() {
        let dir = temp_dir("invalid");
        let (store, _) = open(dir.join("config.toml"));
        let err = store
            .update(|c| c.upload.timeout_secs = 0)
            .expect_err("应被拒绝");
        assert!(err.contains("upload.timeout_secs"), "{err}");
        assert_eq!(store.snapshot(), Config::default());
        drop(store);
        fs::remove_dir_all(&dir).expect("清理");
    }

    #[test]
    fn broken_file_is_never_overwritten_and_failure_is_reported() {
        let dir = temp_dir("broken");
        fs::create_dir_all(&dir).expect("目录");
        let path = dir.join("config.toml");
        fs::write(&path, "garbage ===").expect("写入");
        let (store, notice) = open(path.clone());
        assert!(notice.is_some());
        store
            .update(|c| c.search.selection_mode = SelectionMode::Rect)
            .expect("内存中仍可应用");
        // 等工作线程处理完写入任务
        let failures = {
            let mut got = Vec::new();
            for _ in 0..100 {
                got = store.drain_failures();
                if !got.is_empty() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            got
        };
        assert_eq!(failures.len(), 1);
        drop(store);
        assert_eq!(fs::read_to_string(&path).expect("读取"), "garbage ===");
        fs::remove_dir_all(&dir).expect("清理");
    }

    #[test]
    fn reload_picks_up_manual_edit_and_recovers_from_broken_file() {
        let dir = temp_dir("reload");
        let path = dir.join("config.toml");
        let (store, _) = open(path.clone());
        fs::write(&path, "[search]\nselection_mode = \"rect\"\n").expect("手动编辑");
        let reloaded = store.reload().expect("重新加载");
        assert_eq!(reloaded.previous, Config::default());
        assert_eq!(reloaded.current.search.selection_mode, SelectionMode::Rect);
        assert_eq!(store.snapshot(), reloaded.current);

        fs::write(&path, "oops ===").expect("写坏");
        assert!(store.reload().is_err());
        assert_eq!(store.snapshot(), reloaded.current);

        fs::write(&path, "").expect("修好");
        assert!(store.reload().is_ok());
        assert_eq!(store.snapshot(), Config::default());
        drop(store);
        fs::remove_dir_all(&dir).expect("清理");
    }
}
