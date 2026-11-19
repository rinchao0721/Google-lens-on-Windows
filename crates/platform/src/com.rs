//! 当前线程的 COM 初始化。

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};

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
