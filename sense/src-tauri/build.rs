use std::path::PathBuf;
use std::process::Command;
use std::time::SystemTime;

fn main() {
    build_ocr();
    tauri_build::build();
}

/// 用 swiftc 把 tools/ocr.swift 编成 sidecar（截屏翻译的离线 OCR）。
///
/// 只在源文件比产物新时才重编，省掉每次 cargo build 的两三秒。
/// 编不出来不算致命：应用照常启动，只是截屏翻译会报「找不到 OCR 组件」。
fn build_ocr() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("apple-darwin") {
        return;
    }

    let source = manifest.join("tools/ocr.swift");
    let output = manifest.join("binaries").join(format!("sense-ocr-{target}"));
    println!("cargo:rerun-if-changed={}", source.display());

    let source_time = std::fs::metadata(&source).and_then(|meta| meta.modified()).ok();
    let output_time = std::fs::metadata(&output).and_then(|meta| meta.modified()).ok();
    if let (Some(source_time), Some(output_time)) = (source_time, output_time) {
        if output_time >= source_time && newer_than_script(&source_time, SystemTime::now()) {
            return;
        }
    }

    if let Some(dir) = output.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    let status = Command::new("swiftc")
        .arg("-O")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .status();

    match status {
        Ok(status) if status.success() => println!("cargo:warning=OCR sidecar 已更新"),
        _ => println!("cargo:warning=swiftc 编译 OCR sidecar 失败，截屏翻译将不可用"),
    }
}

fn newer_than_script(_source: &SystemTime, _now: SystemTime) -> bool {
    true
}
