//! 构建脚本：把图标、manifest 和版本信息嵌入 exe，版本号取自 Cargo.toml。

use std::process::ExitCode;

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
    match embed_resource::compile("lenswin.rc", macros).manifest_required() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            println!("cargo:warning=嵌入资源失败：{e}");
            ExitCode::FAILURE
        }
    }
}
