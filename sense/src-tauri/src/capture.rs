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
    /// 组合会话里鼠标左键是否按下（0 = kCGEventSourceStateCombinedSessionState，
    /// 0 = kCGMouseButtonLeft）。只读状态，不需要辅助功能权限。
    fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
}

/// 左键是否按住。
fn left_button_down() -> bool {
    unsafe { CGEventSourceButtonState(0, 0) }
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

/// 覆盖层带失败提示时的最小尺寸（逻辑像素）：12.5px 的提示两三行刚好放下。
const ALERT_MIN_WIDTH: f64 = 300.0;
const ALERT_MIN_HEIGHT: f64 = 84.0;

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
    /// 翻译这一步失败的原因。覆盖层必须把它说出来：只画原文，用户会以为「翻译过了」
    error: Mutex<Option<String>>,
    /// 最近一次选区（逻辑像素，全局坐标），重试或调试用
    last_rect: Mutex<Option<(f64, f64, f64, f64)>>,
}

/// 覆盖层拿到的全部内容：识别块 + 翻译失败的原因（成功时为 None）。
#[derive(Debug, Clone, Serialize)]
pub struct CaptureView {
    pub blocks: Vec<Block>,
    pub error: Option<String>,
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

#[tauri::command]
pub fn cancel_capture(app: AppHandle) {
    close(&app, CAPTURE_WINDOW);
}

#[tauri::command]
pub fn close_overlay(app: AppHandle) {
    close(&app, OVERLAY_WINDOW);
}

#[tauri::command]
pub fn capture_blocks(app: AppHandle, state: State<'_, CaptureState>) -> CaptureView {
    let view = CaptureView {
        blocks: state.blocks.lock().map(|items| items.clone()).unwrap_or_default(),
        error: state.error.lock().ok().and_then(|slot| slot.clone()),
    };
    // 覆盖层真的取到了什么，留一行日志：从访达启动时只能靠它复盘
    trace(
        &app,
        format!("overlay fetched blocks={} error={:?}", view.blocks.len(), view.error),
    );
    view
}

/// 原生选区：调 `screencapture -i` 让系统画选框（拖动永远可靠），
/// 同时以 15ms 频率采样鼠标位置——用户拖出来的矩形就是这些采样点的包围盒。
///
/// 为什么不用自绘遮罩：透明全屏 WebView 在 macOS 上拿第一次点击、重绘、
/// 焦点都不可靠，实测拖不动、松手才刷新。
fn select_region(app: &AppHandle) -> Result<Option<(f64, f64, f64, f64, std::path::PathBuf)>, String> {
    let png = std::env::temp_dir().join(format!("sense-select-{}.png", std::process::id()));
    let _ = std::fs::remove_file(&png);

    let mut child = Command::new("/usr/sbin/screencapture")
        .args(["-i", "-x", &png.to_string_lossy()])
        .spawn()
        .map_err(|err| format!("启动选区失败：{err}"))?;

    let mut min = (f64::MAX, f64::MAX);
    let mut max = (f64::MIN, f64::MIN);
    let mut dragging = false;

    loop {
        // 只在按住左键期间采样：否则开始拖之前光标停在哪，都会被算进包围盒
        let down = left_button_down();
        if down && !dragging {
            dragging = true;
            min = (f64::MAX, f64::MAX);
            max = (f64::MIN, f64::MIN);
        }
        if dragging && !down {
            dragging = false;
        }

        if dragging {
            if let Ok(pos) = app.cursor_position() {
                min.0 = min.0.min(pos.x);
                min.1 = min.1.min(pos.y);
                max.0 = max.0.max(pos.x);
                max.1 = max.1.max(pos.y);
            }
        }

        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(12)),
            Err(_) => break,
        }
    }

    trace(app, format!("native select: min={min:?} max={max:?} exists={}", png.exists()));

    if !png.exists() || max.0 <= min.0 || max.1 <= min.1 {
        let _ = std::fs::remove_file(&png);
        return Ok(None); // 用户按 Esc 取消
    }

    let scale = monitor_scale(app);
    let x = min.0 / scale;
    let y = min.1 / scale;
    let w = (max.0 - min.0) / scale;
    let h = (max.1 - min.1) / scale;
    if w < 8.0 || h < 8.0 {
        let _ = std::fs::remove_file(&png);
        return Ok(None);
    }

    Ok(Some((x, y, w, h, png)))
}

/// 开始一次截屏翻译：原生选区 → OCR → 翻译 → 原位覆盖。
#[tauri::command]
pub async fn start_capture(app: AppHandle, state: State<'_, CaptureState>) -> Result<(), String> {
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

    // 先收起主窗口，别让它挡住要截的内容
    if let Some(main) = app.get_webview_window(crate::window::MAIN_WINDOW) {
        let _ = main.hide();
    }

    let picked = {
        let handle = app.clone();
        tauri::async_runtime::spawn_blocking(move || select_region(&handle))
            .await
            .map_err(|err| format!("选区线程失败：{err}"))??
    };

    let Some((x, y, w, h, png)) = picked else {
        return Ok(());
    };

    trace(&app, format!("capture rect={x},{y},{w},{h}"));

    let result = run_with_image(&app, &state, x, y, w, h, &png).await;
    let _ = std::fs::remove_file(&png);
    result.map(|_| ())
}

/// 选区确定：截图 → OCR → 翻译 → 原位覆盖。
///
/// `x/y/w/h` 是**全局逻辑坐标**（屏幕点），由选区窗口用 outerPosition + 框选尺寸算出来。
/// 命令的实体：抽出来是为了自动化验证（跳过选区直接给一个矩形）。
pub async fn run_with_image(
    app: &AppHandle,
    state: &CaptureState,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    png: &Path,
) -> Result<usize, String> {
    if w < 12.0 || h < 12.0 {
        return Err("选区太小了".to_string());
    }


    let lines = run_ocr(png).map_err(|err| format!("识别失败：{err}"))?;
    let _ = std::fs::remove_file(&png);

    trace(app, format!("ocr lines={}", lines.len()));
    if lines.is_empty() {
        return Err("选中的区域里没识别到文字。也可能是屏幕录制权限没给：系统设置 → 隐私与安全性 → 屏幕录制。".to_string());
    }

    let scale = monitor_scale(app);
    let mut blocks = merge_lines(&lines, scale);

    // 翻译失败也把原文盖上去，至少让用户看到识别结果；
    // 但失败原因必须一起交给覆盖层，不能让原文冒充译文。
    let mut failure = None;
    match translate_target(&blocks) {
        Some(target) => {
            if let Err(err) = translate_blocks(app, &mut blocks, target).await {
                eprintln!("[sense] 截图翻译失败：{err}");
                failure = Some(err);
            }
        }
        // 只有日文假名、韩文、俄文或纯数字符号：没翻也要说一声
        None => failure = Some("选区里没有中文或英文，目前只支持中英互译".to_string()),
    }

    // 有失败提示时，覆盖层至少要放得下这条提示：只框了一行字时选区太矮，提示会被截掉
    let (w, h) = if failure.is_some() {
        (w.max(ALERT_MIN_WIDTH), h.max(ALERT_MIN_HEIGHT))
    } else {
        (w, h)
    };
    // 放大后不能伸出屏幕（靠右下角框选时）：窗口往回挪，块的位置同步补偿，仍对准原文
    let (shift_x, shift_y) = screen_bounds(app)
        .map(|bounds| shift_into(x, y, w, h, bounds))
        .unwrap_or((0.0, 0.0));
    let (x, y) = (x - shift_x, y - shift_y);
    for block in &mut blocks {
        block.x += shift_x;
        block.y += shift_y;
    }

    trace(
        app,
        format!(
            "blocks={} translated={} error={:?}",
            blocks.len(),
            blocks.iter().filter(|block| block.translated).count(),
            failure
        ),
    );
    if let Ok(mut slot) = state.blocks.lock() {
        *slot = blocks.clone();
    }
    if let Ok(mut slot) = state.error.lock() {
        *slot = failure;
    }

    let overlay = WebviewWindowBuilder::new(app, OVERLAY_WINDOW, WebviewUrl::App("index.html".into()))
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

/// 仅供自动化验证（`SENSE_DEV_CAPTURE_PNG`）：把一张现成的 PNG 当作选区截图，
/// 跳过屏幕录制权限与手工拖框，走同一条 OCR → 翻译 → 覆盖层链路。
pub async fn dev_capture_png(app: &AppHandle, state: &CaptureState, source: &str) -> Result<usize, String> {
    // run_with_image 用完会删掉图片，先复制一份，别动调用方的文件
    let png = std::env::temp_dir().join(format!("sense-dev-capture-{}.png", std::process::id()));
    std::fs::copy(source, &png).map_err(|err| format!("复制测试图片失败：{err}"))?;
    let (width, height) = png_size(&png)?;
    let scale = monitor_scale(app);
    run_with_image(
        app,
        state,
        120.0,
        120.0,
        f64::from(width) / scale,
        f64::from(height) / scale,
        &png,
    )
    .await
}

/// 读 PNG 的宽高（IHDR 固定在第 16–24 字节，大端）。
fn png_size(path: &Path) -> Result<(u32, u32), String> {
    let bytes = std::fs::read(path).map_err(|err| format!("读取图片失败：{err}"))?;
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("不是 PNG 图片".to_string());
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Ok((width, height))
}

/// 鼠标所在屏幕的逻辑坐标范围：(左, 上, 右, 下)。
fn screen_bounds(app: &AppHandle) -> Option<(f64, f64, f64, f64)> {
    let point = app.cursor_position().ok()?;
    let monitor = app.monitor_from_point(point.x, point.y).ok().flatten()?;
    let scale = monitor.scale_factor();
    let origin = monitor.position().to_logical::<f64>(scale);
    let size = monitor.size().to_logical::<f64>(scale);
    Some((origin.x, origin.y, origin.x + size.width, origin.y + size.height))
}

/// 窗口 (x, y, w, h) 伸出屏幕右边或下边时，需要往左、往上挪多少；挪到贴住左上边为止。
fn shift_into(x: f64, y: f64, w: f64, h: f64, bounds: (f64, f64, f64, f64)) -> (f64, f64) {
    let (left, top, right, bottom) = bounds;
    let shift_x = (x + w - right).max(0.0).min((x - left).max(0.0));
    let shift_y = (y + h - bottom).max(0.0).min((y - top).max(0.0));
    (shift_x, shift_y)
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
        match secrets::get_key(&provider.id)? {
            Some(key) => Some(key),
            // 没 Key 就别发请求：发出去只会拿到 401，提示会变成误导人的「Key 无效」
            None => return Err("还没填 API Key，去「设置」里填一个".to_string()),
        }
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
    fn enlarged_overlay_is_pulled_back_on_screen() {
        let screen = (0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(shift_into(100.0, 100.0, 300.0, 84.0, screen), (0.0, 0.0), "放得下就不挪");
        assert_eq!(shift_into(1800.0, 1050.0, 300.0, 84.0, screen), (180.0, 54.0), "右下角往回挪");
        assert_eq!(shift_into(10.0, 10.0, 3000.0, 84.0, screen), (10.0, 0.0), "最多挪到贴住左边");
    }

    #[test]
    fn png_size_reads_ihdr_and_rejects_other_files() {
        let path = std::env::temp_dir().join(format!("sense-png-size-{}.png", std::process::id()));
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend_from_slice(&277u32.to_be_bytes());
        bytes.extend_from_slice(&104u32.to_be_bytes());
        std::fs::write(&path, &bytes).expect("写测试文件");
        assert_eq!(png_size(&path).expect("应能读出尺寸"), (277, 104));

        std::fs::write(&path, b"not a png at all, just text").expect("写测试文件");
        assert!(png_size(&path).is_err(), "非 PNG 必须报错");
        let _ = std::fs::remove_file(&path);
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
