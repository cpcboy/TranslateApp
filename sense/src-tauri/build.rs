use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    build_ocr();
    tauri_build::build();
}

/// 用 swiftc 把 tools/ocr.swift 编成 sidecar（截屏翻译的离线 OCR）。
///
/// 产物不入库（`src-tauri/binaries/` 在 .gitignore 里）：swiftc 每次编出来的字节都不同，
/// 入库只会带来没有意义的 diff。克隆下来第一次构建时由这里现编。
///
/// 只在源文件比产物新时才重编，省掉每次 cargo build 的两三秒。
/// 编不出来（没装 Xcode 命令行工具）不算致命：Tauri 的 `externalBin` 要求文件必须存在，
/// 所以放一个占位脚本，应用照常构建和启动，只是截屏翻译会如实报
/// 「OCR 组件没有编译成功」，而不是 Exec format error 之类看不懂的错。
/// 设 `SENSE_SKIP_OCR=1` 可以主动跳过编译（没有 Xcode 的 CI 用；已有编好的产物时保持不动）。
fn build_ocr() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("apple-darwin") {
        return;
    }

    let source = manifest.join("tools/ocr.swift");
    let output = manifest.join("binaries").join(format!("sense-ocr-{target}"));
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-env-changed=SENSE_SKIP_OCR");

    if let Some(dir) = output.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    // 空值、0、false / no / off 不算设置
    let skip = std::env::var("SENSE_SKIP_OCR")
        .is_ok_and(|value| !matches!(value.trim().to_ascii_lowercase().as_str(), "" | "0" | "false" | "no" | "off"));
    if skip {
        // 跳过的是「编译」：已经编好的真产物不动，没有产物时才放占位
        if !output.exists() {
            write_stub(&output, "已用 SENSE_SKIP_OCR 跳过编译");
            println!("cargo:warning=SENSE_SKIP_OCR 已设置，OCR 用占位脚本，截屏翻译将不可用");
        }
        return;
    }

    let source_time = std::fs::metadata(&source).and_then(|meta| meta.modified()).ok();
    let output_time = std::fs::metadata(&output).and_then(|meta| meta.modified()).ok();
    if let (Some(source_time), Some(output_time)) = (source_time, output_time) {
        if output_time >= source_time && !is_stub(&output) {
            return;
        }
    }

    let status = Command::new("swiftc")
        .arg("-O")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .status();

    match status {
        Ok(status) if status.success() => println!("cargo:warning=OCR sidecar 已更新"),
        _ => {
            // 不覆盖已经编好的真产物；只有压根没有文件时才放占位
            if !output.exists() {
                write_stub(&output, "swiftc 编译失败（需要 Xcode 命令行工具：xcode-select --install）");
            }
            println!("cargo:warning=swiftc 编译 OCR sidecar 失败，截屏翻译将不可用");
        }
    }
}

/// 占位脚本的第一行标记，用来区分「真产物」和「占位」。
const STUB_MARK: &str = "# sense-ocr-stub";

fn is_stub(path: &Path) -> bool {
    std::fs::read(path)
        .map(|bytes| bytes.starts_with(b"#!") && String::from_utf8_lossy(&bytes).contains(STUB_MARK))
        .unwrap_or(false)
}

fn write_stub(path: &Path, reason: &str) {
    use std::os::unix::fs::PermissionsExt;
    let script = format!(
        "#!/bin/sh\n{STUB_MARK}\necho \"OCR 组件没有编译成功：{reason}。装好后重新构建一次应用。\" >&2\nexit 1\n"
    );
    if std::fs::write(path, script).is_ok() {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
    }
}
