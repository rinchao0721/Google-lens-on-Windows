//! 纯逻辑层：几何、选区、裁剪、配置，不依赖任何平台 API。

#![forbid(unsafe_code)]

pub mod action;
pub mod config;
pub mod error;
pub mod frame;
pub mod geometry;
pub mod hotkey;
pub mod ipc;
pub mod monitor;
pub mod selection;
