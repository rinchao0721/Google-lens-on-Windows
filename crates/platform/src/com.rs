//! 当前线程的 COM 初始化。

use std::marker::PhantomData;
use std::rc::Rc;

use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize,
};

use crate::error::{PlatformError, win32};

/// 当前线程的 COM 初始化，离开作用域时按需反初始化。
pub(crate) struct ComScope {
    initialized: bool,
}

impl ComScope {
    /// 初始化为单线程套间；线程已按其它模式初始化时沿用，不算失败。
    pub(crate) fn enter() -> Self {
        // SAFETY: 无前置条件；成功（含 S_FALSE）时必须与 CoUninitialize 配对。
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Self {
            initialized: hr.is_ok(),
        }
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: 与 enter 中成功的 CoInitializeEx 配对，仅调用一次。
            unsafe { CoUninitialize() };
        }
    }
}

/// 当前线程的多线程套间（MTA）COM 初始化，离开作用域时反初始化。
pub struct MtaScope {
    _not_send: PhantomData<Rc<()>>,
}

impl MtaScope {
    /// 初始化为多线程套间（WinRT 异步等待必需）；线程已按其它模式初始化时返回错误。
    pub fn enter() -> Result<Self, PlatformError> {
        // SAFETY: 无前置条件；ok() 将 S_OK 与 S_FALSE 视为成功，Drop 时配对 CoUninitialize。
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(win32("CoInitializeEx"))?;
        Ok(Self {
            _not_send: PhantomData,
        })
    }
}

impl Drop for MtaScope {
    fn drop(&mut self) {
        // SAFETY: 与 enter 中成功的 CoInitializeEx 配对，仅调用一次。
        unsafe { CoUninitialize() };
    }
}
