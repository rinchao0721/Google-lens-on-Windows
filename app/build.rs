//! 构建脚本：把图标、manifest 和版本信息嵌入 exe，版本号取自 Cargo.toml；并把 OCR 运行库拷到 exe 旁。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// 随包分发的 ONNX Runtime 动态库，相对于仓库根目录。
const OCR_DLL: &str = "assets/ocr/onnxruntime.dll";

/// 构建输出目录（`target/<profile>`）：由 `OUT_DIR` 向上三级得到。
fn profile_dir() -> Result<PathBuf, String> {
    let out_dir = std::env::var_os("OUT_DIR").ok_or("缺少 OUT_DIR")?;
    Path::new(&out_dir)
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
        .ok_or_else(|| "无法由 OUT_DIR 推出构建输出目录".to_string())
}

fn copy_ocr_runtime() -> Result<(), String> {
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").ok_or("缺少 CARGO_MANIFEST_DIR")?;
    let root = Path::new(&manifest_dir)
        .parent()
        .ok_or("无法定位仓库根目录")?;
    let source = root.join(OCR_DLL);
    println!("cargo:rerun-if-changed={}", source.display());
    let name = source.file_name().ok_or("DLL 路径没有文件名")?;
    let target = profile_dir()?.join(name);
    std::fs::copy(&source, &target)
        .map(|_| ())
        .map_err(|e| format!("拷贝 {} 失败：{e}", source.display()))
}

fn version_part(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| "0".to_string())
}

fn main() -> ExitCode {
    println!("cargo:rerun-if-changed=lenswin.rc");
    println!("cargo:rerun-if-changed=lenswin.manifest");
    println!("cargo:rerun-if-changed=assets/lenswin.ico");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    let macros = [
        format!("VERSION_MAJOR={}", version_part("CARGO_PKG_VERSION_MAJOR")),
        format!("VERSION_MINOR={}", version_part("CARGO_PKG_VERSION_MINOR")),
        format!("VERSION_PATCH={}", version_part("CARGO_PKG_VERSION_PATCH")),
        format!("VERSION_STRING=\"{version}\""),
    ];
    if let Err(e) = copy_ocr_runtime() {
        println!("cargo:warning=拷贝 OCR 运行库失败：{e}");
        return ExitCode::FAILURE;
    }
    match embed_resource::compile("lenswin.rc", macros).manifest_required() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            println!("cargo:warning=嵌入资源失败：{e}");
            ExitCode::FAILURE
        }
    }
}
