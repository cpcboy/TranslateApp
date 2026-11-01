use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    build_ocr();
    tauri_build::build();
}

/// OCR 辅助程序要求的最低 macOS。Vision 的中文识别（`VNRecognizeTextRequest` 第 3 版）从 13 起才有。
///
/// 一定要显式给 `-target`：不给时 swiftc 拿**本机**系统版本当下限，在 macOS 26 上编出来就写成「要 26」
/// （之前装出去的就是这样），拷到别的电脑上，系统比它旧就跑不起来；也编不出 Intel 的版本。
const OCR_MIN_MACOS: &str = "13.0";

/// cargo 的 `TARGET` → swiftc 的 `-target`。只认两种芯片的 macOS 目标。
fn swift_target(cargo_target: &str) -> Option<String> {
    let arch = match cargo_target.split('-').next()? {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        _ => return None,
    };
    Some(format!("{arch}-apple-macos{OCR_MIN_MACOS}"))
}

/// 用 swiftc 把 tools/ocr.swift 编成 sidecar（截屏翻译的离线 OCR）。
///
/// 产物不入库（`src-tauri/binaries/` 在 .gitignore 里）：swiftc 每次编出来的字节都不同，
/// 入库只会带来没有意义的 diff。克隆下来第一次构建时由这里现编。
///
/// 只在源文件比产物新、且产物是按当前「配方」（swiftc 的参数）编的时才复用，省掉每次 cargo build 的两三秒。
/// 配方记在产物旁边的隐藏文件 `.sense-ocr-<target>.recipe` 里：改了 `-target` 之后，各台机器上留着的旧产物（下限 26）会被重编，
/// 不会悄悄混进发布包。
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

    let Some(triple) = swift_target(&target) else {
        if !output.exists() {
            write_stub(&output, &format!("不认识的目标 {target}，只支持 aarch64 / x86_64 的 macOS"));
        }
        println!("cargo:warning=OCR sidecar 不支持目标 {target}，截屏翻译将不可用");
        return;
    };
    let recipe = format!("swiftc -O -target {triple}\n");
    // 点开头的隐藏文件：capture.rs 在开发目录里按「以 sense-ocr 开头」找辅助程序，戳文件不能叫这个名字
    let recipe_path = output.with_file_name(format!(".sense-ocr-{target}.recipe"));

    let source_time = std::fs::metadata(&source).and_then(|meta| meta.modified()).ok();
    let output_time = std::fs::metadata(&output).and_then(|meta| meta.modified()).ok();
    if let (Some(source_time), Some(output_time)) = (source_time, output_time) {
        let same_recipe = std::fs::read_to_string(&recipe_path).is_ok_and(|saved| saved == recipe);
        if output_time >= source_time && same_recipe && !is_stub(&output) {
            return;
        }
    }

    let status = Command::new("swiftc")
        .arg("-O")
        .arg("-target")
        .arg(&triple)
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .status();

    match status {
        Ok(status) if status.success() => {
            let _ = std::fs::write(&recipe_path, &recipe);
            println!("cargo:warning=OCR sidecar 已更新（{triple}）");
        }
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
