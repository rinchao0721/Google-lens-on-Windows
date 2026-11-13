//! 检查更新：结论交给 `app` 决定如何提示。

use lenswin_core::error::ActionError;
use lenswin_core::update::UpdateCheck;

/// 在工作线程里调用；会发起网络请求。
pub fn check_update(include_prerelease: bool) -> Result<UpdateCheck, ActionError> {
    lenswin_update::check(include_prerelease).map_err(|e| ActionError::Failed(e.to_string()))
}
