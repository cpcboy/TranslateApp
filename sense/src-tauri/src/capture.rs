//! 截屏翻译：选区 → 截图 → 离线 OCR → 翻译 → 原位覆盖。
//!
//! 分工：
//! - 选区由一个全屏透明窗口完成（前端画框，Rust 负责建窗与收尾）
//! - 截图用系统自带的 `screencapture -R`（准确、无需自己合成多屏坐标）
//! - OCR 用 `tools/ocr.swift` 编译出的 sidecar（系统 Vision，离线免费）
//! - 翻译走和查词同一个 provider / Key，不新增任何服务端

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

// CoreGraphics 的两个 TCC 接口：
// - Preflight 只查询，不弹框；返回当前进程有没有屏幕录制权限
// - Request 会弹一次系统授权框
//
// 用它们比「截一张图看是不是黑的」可靠得多。
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// 当前进程有没有屏幕录制权限。
pub fn has_screen_permission() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// 请求权限：会弹系统框，并返回**当前**是否已经拿到（刚点允许时通常还是 false）。
pub fn request_screen_permission() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}

use crate::providers::openai_compat;
use crate::providers as provider_registry;
use crate::secrets;

/// 选区窗口与结果窗口的 label。
pub const CAPTURE_WINDOW: &str = "capture";
pub const OVERLAY_WINDOW: &str = "overlay";

/// OCR 出来的一行（图片像素坐标，原点左上）。
#[derive(Debug, Clone, Default, Deserialize)]
struct OcrLine {
    text: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// 合并成段之后、经翻译的块（逻辑像素坐标，相对选区左上）。
#[derive(Debug, Clone, Serialize)]
pub struct Block {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// 这一块里**单行**的高度。合并了多行时 h 会是两倍三倍，
    /// 字号必须按 line_h 算，否则一段会突然变成巨字。
    pub line_h: f64,
    /// 没有译文时为 false，前端只画原文
    pub translated: bool,
}

#[derive(Default)]
pub struct CaptureState {
    blocks: Mutex<Vec<Block>>,
    /// 最近一次选区（逻辑像素，全局坐标），重试或调试用
    last_rect: Mutex<Option<(f64, f64, f64, f64)>>,
}

/// 记一行到 `~/Library/Application Support/com.sense.app/capture.log`。
///
/// 从访达启动时 stdout 是看不到的，出问题只能靠这个文件复盘。
fn trace(app: &AppHandle, line: impl AsRef<str>) {
    use std::io::Write;
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("capture.log"))
    {
        let _ = writeln!(file, "{}", line.as_ref());
    }
}

fn close(app: &AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.close();
    }
}

/// 在鼠标所在屏幕铺一层透明选区窗口。
#[tauri::command]
pub async fn start_capture(app: AppHandle) -> Result<(), String> {
    trace(
        &app,
        format!("start_capture: screen_permission={}", has_screen_permission()),
    );
    if !has_screen_permission() {
        let granted = request_screen_permission();
        return Err(if granted {
            "权限已拿到，再按一次快捷键试试".to_string()
        } else {
            "SENSE 还没有「屏幕录制」权限。系统已经弹出授权框（没看到就去 系统设置 → 隐私与安全性 → 屏幕录制 里勾选 SENSE），\
             勾选后**退出并重新打开 SENSE** 才会生效——这是 macOS 的规定，不是应用的问题。"
                .to_string()
        });
    }
    close(&app, CAPTURE_WINDOW);

    let cursor = app.cursor_position().map_err(|err| format!("取不到鼠标位置：{err}"))?;
    // 优先取鼠标所在屏幕；拿不到就退回主屏。
    // （拿不到不是罕见情况：刚启动、鼠标在屏幕边缘、或系统还没把显示器信息给到进程）
    let monitor = app
        .monitor_from_point(cursor.x, cursor.y)
        .map_err(|err| format!("取不到屏幕信息：{err}"))?
        .or_else(|| app.primary_monitor().ok().flatten())
        .ok_or_else(|| format!("找不到可用屏幕（cursor={},{}）", cursor.x, cursor.y))?;
    trace(
        &app,
        format!("select monitor: cursor=({}, {}) size={:?}", cursor.x, cursor.y, monitor.size()),
    );

    let scale = monitor.scale_factor();
    let position = monitor.position();
    let size = monitor.size();
    let x = position.x as f64 / scale;
    let y = position.y as f64 / scale;
    let width = size.width as f64 / scale;
    let height = size.height as f64 / scale;

    let window = WebviewWindowBuilder::new(&app, CAPTURE_WINDOW, WebviewUrl::App("index.html".into()))
        .title("SENSE 截图")
        .position(x, y)
        .inner_size(width, height)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .visible_on_all_workspaces(true)
        .focused(true)
        .build()
        .map_err(|err| format!("打不开选区窗口：{err}"))?;

    // 关键：把应用和这个窗口都顶到最前。
    // 触发截屏时 SENSE 通常不是前台应用，macOS 会把「激活应用」的那一次按下
    // 自己吃掉——用户看到遮罩却拖不动，就是这个原因。
    let _ = window.set_focus();
    trace(&app, "select window focused");

    Ok(())
}

#[tauri::command]
pub fn cancel_capture(app: AppHandle) {
    close(&app, CAPTURE_WINDOW);
}

#[tauri::command]
pub fn close_overlay(app: AppHandle) {
    close(&app, OVERLAY_WINDOW);
}

#[tauri::command]
pub fn capture_blocks(state: State<'_, CaptureState>) -> Vec<Block> {
    state.blocks.lock().map(|items| items.clone()).unwrap_or_default()
}

/// 选区确定：截图 → OCR → 翻译 → 原位覆盖。
///
/// `x/y/w/h` 是**全局逻辑坐标**（屏幕点），由选区窗口用 outerPosition + 框选尺寸算出来。
#[tauri::command]
pub async fn finish_capture(
    app: AppHandle,
    state: State<'_, CaptureState>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Result<usize, String> {
    run(app, &state, x, y, w, h).await
}

/// 命令的实体：抽出来是为了自动化验证（跳过选区直接给一个矩形）。
pub async fn run(
    app: AppHandle,
    state: &CaptureState,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Result<usize, String> {
    close(&app, CAPTURE_WINDOW);
    // 给窗口系统一点时间把遮罩撤掉，否则截图里会带上一层暗色
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;

    if w < 12.0 || h < 12.0 {
        return Err("选区太小了".to_string());
    }
    if let Ok(mut slot) = state.last_rect.lock() {
        *slot = Some((x, y, w, h));
    }

    let png = std::env::temp_dir().join(format!("sense-shot-{}.png", std::process::id()));
    let rect = format!(
        "{},{},{},{}",
        x.round() as i64,
        y.round() as i64,
        w.round() as i64,
        h.round() as i64
    );

    trace(&app, format!("capture rect={rect}"));
    let output = Command::new("/usr/sbin/screencapture")
        .args(["-x", "-R", &rect, &png.to_string_lossy()])
        .output()
        .map_err(|err| format!("调用截图失败了：{err}"))?;

    trace(
        &app,
        format!(
            "screencapture status={:?} exists={} stderr={}",
            output.status.code(),
            png.exists(),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    );
    if !output.status.success() || !png.exists() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if detail.is_empty() {
            "截图失败。第一次使用需要在「系统设置 → 隐私与安全性 → 屏幕录制」里勾选 SENSE，然后重启应用。".to_string()
        } else {
            format!("截图失败：{detail}")
        });
    }

    let lines = run_ocr(&png).map_err(|err| format!("识别失败：{err}"))?;
    let _ = std::fs::remove_file(&png);

    trace(&app, format!("ocr lines={}", lines.len()));
    if lines.is_empty() {
        return Err("选中的区域里没识别到文字。也可能是屏幕录制权限没给：系统设置 → 隐私与安全性 → 屏幕录制。".to_string());
    }

    let scale = monitor_scale(&app);
    let mut blocks = merge_lines(&lines, scale);

    if let Some(target) = translate_target(&blocks) {
        match translate_blocks(&app, &mut blocks, target).await {
            Ok(()) => {}
            Err(err) => {
                // 翻译失败也把原文盖上去：至少让用户看到识别结果，而不是一片空白
                eprintln!("[sense] 截图翻译失败：{err}");
            }
        }
    }

    trace(
        &app,
        format!(
            "blocks={} translated={}",
            blocks.len(),
            blocks.iter().filter(|block| block.translated).count()
        ),
    );
    if let Ok(mut slot) = state.blocks.lock() {
        *slot = blocks.clone();
    }

    let overlay = WebviewWindowBuilder::new(&app, OVERLAY_WINDOW, WebviewUrl::App("index.html".into()))
        .title("SENSE 译文")
        .position(x, y)
        .inner_size(w, h)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .visible_on_all_workspaces(true)
        .focused(true)
        .build()
        .map_err(|err| format!("打不开译文窗口：{err}"))?;
    let _ = overlay.set_focus();

    Ok(blocks.len())
}

/// 取鼠标所在屏幕的缩放，用来把图片像素换算成逻辑像素。
fn monitor_scale(app: &AppHandle) -> f64 {
    app.cursor_position()
        .ok()
        .and_then(|point| app.monitor_from_point(point.x, point.y).ok().flatten())
        .map(|monitor| monitor.scale_factor())
        .unwrap_or(1.0)
}

/// 找到 OCR sidecar：打包后在主程序旁边，dev 时在 src-tauri/binaries/。
fn ocr_binary() -> Result<PathBuf, String> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("sense-ocr");
            if sibling.exists() {
                return Ok(sibling);
            }
        }
    }

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries");
    if let Ok(entries) = std::fs::read_dir(&manifest) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|item| item.to_str()).unwrap_or("");
            if name.starts_with("sense-ocr") && path.exists() {
                return Ok(path);
            }
        }
    }

    Err("找不到 OCR 组件（sense-ocr），重新构建一次应用".to_string())
}

fn run_ocr(png: &Path) -> Result<Vec<OcrLine>, String> {
    let binary = ocr_binary()?;
    let output = Command::new(&binary)
        .arg(png)
        .output()
        .map_err(|err| format!("启动 OCR 失败：{err}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    serde_json::from_slice::<Vec<OcrLine>>(&output.stdout).map_err(|err| format!("OCR 输出解析失败：{err}"))
}

/// 同一段的多行合回一块：行高接近、竖直间隙小、水平有重叠。
fn merge_lines(lines: &[OcrLine], scale: f64) -> Vec<Block> {
    let mut sorted: Vec<&OcrLine> = lines.iter().filter(|line| !line.text.trim().is_empty()).collect();
    sorted.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal));

    let mut blocks: Vec<Block> = Vec::new();
    for line in sorted {
        let (x, y, w, h) = (line.x / scale, line.y / scale, line.w / scale, line.h / scale);
        let text = line.text.trim().to_string();

        if let Some(last) = blocks.last_mut() {
            let same_column = x >= last.x - last.w * 0.35 && x <= last.x + last.w * 0.35;
            let gap = y - (last.y + last.h);
            let close_enough = gap >= -2.0 && gap < last.h * 0.55;
            let similar_height = (h - last.h).abs() < last.h.max(h) * 0.4;
            // 关键：只合并「明显是被折断的一句话」。
            // 终端输出、代码、列表里相邻两行是两句独立的话，合起来反而翻错。
            let previous = last.text.trim_end();
            let ends_open = previous.ends_with(',')
                || previous.ends_with('，')
                || previous.ends_with('-')
                || previous.ends_with('—');
            let ends_sentence = previous
                .chars()
                .last()
                .map(|ch| ".!?。！？:：;；$>|/\\）)】]」』".contains(ch))
                .unwrap_or(true);
            let starts_lower = text
                .chars()
                .next()
                .map(|ch| ch.is_lowercase())
                .unwrap_or(false);
            let wraps = !ends_sentence && (ends_open || starts_lower);

            if same_column && close_enough && similar_height && wraps {
                let right = (last.x + last.w).max(x + w);
                let bottom = (last.y + last.h).max(y + h);
                last.x = last.x.min(x);
                last.y = last.y.min(y);
                last.w = right - last.x;
                last.h = bottom - last.y;
                last.line_h = last.line_h.max(h);
                last.text.push(' ');
                last.text.push_str(&text);
                continue;
            }
        }

        blocks.push(Block {
            text,
            x,
            y,
            w,
            h,
            line_h: h,
            translated: false,
        });
    }

    blocks
}

/// 目标语言：源文以英文为主就译成中文，否则译成英文。
fn translate_target(blocks: &[Block]) -> Option<&'static str> {
    let sample: String = blocks.iter().map(|block| block.text.as_str()).collect::<Vec<_>>().join(" ");
    let cjk = sample.chars().filter(|ch| ('\u{4e00}'..='\u{9fff}').contains(ch)).count();
    let latin = sample.chars().filter(|ch| ch.is_ascii_alphabetic()).count();
    if cjk == 0 && latin == 0 {
        return None;
    }
    Some(if cjk * 2 >= latin { "英文" } else { "中文" })
}

/// 一次请求翻译全部块，顺序对齐；失败就保留原文。
async fn translate_blocks(app: &AppHandle, blocks: &mut [Block], target: &str) -> Result<(), String> {
    let provider = provider_registry::active(app)?;
    let key = if provider.requires_key {
        secrets::get_key(&provider.id)?
    } else {
        None
    };

    let payload: Vec<&str> = blocks.iter().map(|block| block.text.as_str()).collect();
    let user = serde_json::to_string(&payload).map_err(|err| err.to_string())?;
    let system = format!(
        "你是一位翻译，正在翻译屏幕上一段连续的文字。input 是按阅读顺序排好的 JSON 字符串数组，\
         相邻两条通常属于同一段内容。\n\n要求：\n\
         1. 先通读全部条目，理解整体在讲什么，再逐条翻译成{target}；指代、术语、语气前后保持一致。\n\
         2. 输出同样长度、同样顺序的 JSON 字符串数组，只输出数组本身，不要解释、不要代码块。\n\
         3. 命令行、代码、变量名、路径、URL、版本号、错误码、日志级别原样保留，不要翻译。\n\
         4. 每条尽量简短，以适配原来的版面宽度。"
    );

    // 上下文由「整屏一次送 + 先通读再翻」的提示词保证，不靠思维链。
    // 实测开思考要多等 2–5 秒，换来的是翻译任务里几乎看不出的提升。
    let reasoning = provider_registry::reasoning_fields(&provider.id);
    let body = openai_compat::build_body(
        &provider.model,
        &system,
        &user,
        0.2,
        reasoning.as_ref(),
        openai_compat::MAX_TOKENS,
    );

    let mut raw = String::new();
    let outcome = openai_compat::stream_chat(
        crate::client_for(&provider.base_url),
        &provider,
        key.as_deref(),
        &body,
        &mut |delta: &str| raw.push_str(delta),
        None,
    )
    .await
    .map_err(|err| err.message())?;

    let translations = parse_translation_array(&outcome.text)?;
    if translations.len() != blocks.len() {
        return Err(format!(
            "译文条数对不上：{} vs {}",
            translations.len(),
            blocks.len()
        ));
    }

    for (block, text) in blocks.iter_mut().zip(translations) {
        let text = text.trim().to_string();
        if !text.is_empty() {
            block.text = text;
            block.translated = true;
        }
    }
    Ok(())
}

/// 模型偶尔会包一层 ```json，这里统一剥掉再解析。
fn parse_translation_array(raw: &str) -> Result<Vec<String>, String> {
    let mut text = raw.trim();
    if let Some(rest) = text.strip_prefix("```") {
        let rest = rest.trim_start_matches("json").trim_start();
        text = rest.trim_end_matches("```").trim();
    }
    let start = text.find('[').ok_or("译文里没有 JSON 数组")?;
    let end = text.rfind(']').ok_or("译文 JSON 不完整")?;
    serde_json::from_str::<Vec<String>>(&text[start..=end]).map_err(|err| format!("译文解析失败：{err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x: f64, y: f64, w: f64, h: f64) -> OcrLine {
        OcrLine {
            text: text.into(),
            x,
            y,
            w,
            h,
            ..Default::default()
        }
    }

    #[test]
    fn adjacent_lines_merge_into_one_block() {
        let lines = vec![
            line("The quick brown", 10.0, 10.0, 200.0, 20.0),
            line("fox jumps over", 10.0, 32.0, 180.0, 20.0),
            line("另一个段落", 10.0, 200.0, 120.0, 20.0),
        ];
        let blocks = merge_lines(&lines, 1.0);
        assert_eq!(blocks.len(), 2, "相邻两行应合成一段");
        assert_eq!(blocks[0].text, "The quick brown fox jumps over");
        assert!(blocks[0].h > 30.0, "高度要覆盖两行");
    }

    #[test]
    fn translation_array_survives_code_fence() {
        let raw = "```json\n[\"甲\",\"乙\"]\n```";
        assert_eq!(parse_translation_array(raw).unwrap(), vec!["甲", "乙"]);
    }

    #[test]
    fn target_language_follows_source() {
        let english = vec![Block {
            text: "hello world".into(),
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            line_h: 10.0,
            translated: false,
        }];
        assert_eq!(translate_target(&english), Some("中文"));
    }
}
