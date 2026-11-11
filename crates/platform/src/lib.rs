//! Win32 / WinRT 封装层，不含业务逻辑。

#![deny(unsafe_op_in_unsafe_fn)]

pub mod autostart;
pub mod capture;
pub mod clipboard;
pub mod clock;
pub mod dialog;
pub mod error;
pub mod focus;
pub mod fonts;
pub mod handle;
pub mod hotkey;
pub mod ipc;
pub mod message_loop;
pub mod monitor;
pub mod notify;
pub mod paths;
pub mod shell;
pub mod single_instance;
pub mod tray;
pub mod window;
pub mod window_snapshot;

mod com;
mod text;
