//! 纯逻辑层：几何、选区、裁剪、配置，不依赖任何平台 API。

#![forbid(unsafe_code)]

pub mod action;
pub mod annotation;
pub mod config;
pub mod error;
pub mod frame;
pub mod geometry;
pub mod hotkey;
pub mod ipc;
pub mod lens;
pub mod monitor;
pub mod selection;
pub mod snip;
pub mod text_box;
pub mod text_edit;
pub mod update;
pub mod url;
