//! 屏幕贴图窗口管理：置于顶层且不进任务栏，支持拖动、滚轮缩放、透明度、旋转、穿透与右键菜单。
//!
//! 只负责贴图呈现与用户交互，复制、保存、搜索等动作通过 [`lenswin_core::pin::PinEvent`] 回投给主循环。

#![deny(unsafe_op_in_unsafe_fn)]

mod error;
mod menu;
mod render;
mod state;
mod window;

pub use error::PinError;
pub use window::{PinId, PinManager};

/// 显式丢弃 `Drop` 或清理路径中无法处理的返回值。
pub(crate) fn discard<T>(_value: T) {}
