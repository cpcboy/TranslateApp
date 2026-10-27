//! 截屏翻译：选区 → 截图 → 离线 OCR → 翻译 → 原位覆盖。
//!
//! 分工：
//! - 选区与截图都交给系统自带的 `screencapture -i`（原生选框，拖动最可靠）；
//!   鼠标按下与松开的位置只用来推算「截图落在屏幕的哪里」，尺寸以截出来的 PNG 为准
//! - OCR 用 `tools/ocr.swift` 编译出的 sidecar（系统 Vision，离线免费）
//! - 翻译走和查词同一个 provider / Key，不新增任何服务端

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

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

/// 结果窗口的 label。
pub const OVERLAY_WINDOW: &str = "overlay";

/// 覆盖层带失败提示时的最小尺寸（逻辑像素）：12.5px 的提示两三行刚好放下。
const ALERT_MIN_WIDTH: f64 = 300.0;
const ALERT_MIN_HEIGHT: f64 = 84.0;

/// 推算出的选区终点与实际采样到的松手位置，允许差这么多逻辑点。
/// 采样每 12ms 一次，快速拖动时最后一次采样会落后十几个点。
const ALIGN_TOLERANCE: f64 = 40.0;
/// 拖动距离小于这个（逻辑点）就不算「拖出来的选区」
const MIN_DRAG: f64 = 8.0;

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
    /// 提示：翻译成功了，但有话要告诉用户（比如译文没能对准原文的位置）
    notice: Mutex<Option<String>>,
    /// 选框正开着。这段时间再按快捷键不能开第二个 screencapture：两个会抢同一个鼠标轨迹。
    /// 只管选框这一段——选完之后的 OCR / 翻译不挡快捷键（挡了就是几十秒里一个哑巴快捷键）
    selecting: AtomicBool,
    /// 第几次截屏。后来者胜：新的一次开始后，还在识别 / 翻译的旧一次做完也不再弹译文窗口，
    /// 否则旧译文会盖在新的上面、两个覆盖层窗口的 label 也会撞车
    generation: AtomicU64,
}

/// 离开作用域就把「选框开着」标志放掉，早退和 panic 都不会卡死之后的截屏。
struct BusyGuard<'a>(&'a AtomicBool);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// 覆盖层拿到的全部内容：识别块 + 翻译失败的原因（成功时为 None）。
#[derive(Debug, Clone, Serialize)]
pub struct CaptureView {
    pub blocks: Vec<Block>,
    pub error: Option<String>,
    pub notice: Option<String>,
}

/// 覆盖层最终放在屏幕哪里（全局逻辑坐标），以及要不要附带一句提示。
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub notice: Option<String>,
}

/// 选区所在那块屏幕：截图的缩放（OCR 像素 → 逻辑点）与逻辑范围（覆盖层不能伸出去）。
///
/// 在**选完框的那一刻**取，随选区一起带着走。后面的识别 / 翻译要花几秒，期间用户可能把鼠标挪到别的屏，
/// 到时候再按「光标现在在哪块屏」去量，量到的就是错的屏。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    pub scale: f64,
    pub bounds: Option<(f64, f64, f64, f64)>,
}

impl Screen {
    /// 光标此刻所在的那块屏。
    fn under_cursor(app: &AppHandle) -> Self {
        match monitor_under_cursor(app) {
            Some((monitor, rect)) => Self {
                scale: monitor.scale_factor(),
                bounds: Some(rect),
            },
            None => Self {
                scale: primary_scale(app),
                bounds: None,
            },
        }
    }
}

/// 临时 PNG 的路径：进程号 + 递增序号，同一进程里也不会重名互相覆盖。
fn temp_png(kind: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "sense-{kind}-{}-{}.png",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ))
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
pub fn close_overlay(app: AppHandle) {
    close(&app, OVERLAY_WINDOW);
}

#[tauri::command]
pub fn capture_blocks(app: AppHandle, state: State<'_, CaptureState>) -> CaptureView {
    let view = CaptureView {
        blocks: state.blocks.lock().map(|items| items.clone()).unwrap_or_default(),
        error: state.error.lock().ok().and_then(|slot| slot.clone()),
        notice: state.notice.lock().ok().and_then(|slot| slot.clone()),
    };
    // 覆盖层真的取到了什么，留一行日志：从访达启动时只能靠它复盘
    trace(
        &app,
        format!(
            "overlay fetched blocks={} error={:?} notice={:?}",
            view.blocks.len(),
            view.error,
            view.notice
        ),
    );
    view
}

/// 左键按下 / 松开的轨迹：只认最后一次按下。
///
/// 从选区的轮询循环里抽出来，是为了能脱离屏幕单测：拖过头再拉回来、松手后的那一次采样、
/// 窗口模式单击这些行为，以前只能靠真人操作验证。
#[derive(Debug, Default, PartialEq)]
struct DragTracker {
    start: Option<(f64, f64)>,
    end: Option<(f64, f64)>,
    dragging: bool,
}

impl DragTracker {
    /// 每轮询一次调用一次：`down` 是左键此刻是否按住，`cursor` 是此刻的光标（逻辑点）。
    fn observe(&mut self, down: bool, cursor: Option<(f64, f64)>) {
        if down && !self.dragging {
            // 新的一次按下：前面的丢掉
            self.dragging = true;
            self.start = cursor;
            self.end = cursor;
        }
        // 按住期间持续采样；松手后的第一次轮询也采一次，拿到最接近松手的位置
        if self.dragging {
            if let Some(position) = cursor {
                self.end = Some(position);
            }
        }
        if self.dragging && !down {
            self.dragging = false;
        }
    }
}

/// screencapture 退出后，临时文件里有内容才算选了区域；没有就是用户按了 Esc。
/// 不再看鼠标轨迹：窗口模式单击时轨迹退化成一个点，以前会被误当成取消。
fn has_selection(png_len: Option<u64>) -> bool {
    png_len.is_some_and(|len| len > 0)
}

/// 由「拖动方向 + 截图真实尺寸 + 松手位置」推算选区落在屏幕哪里。全部是逻辑点。
///
/// 以前用鼠标轨迹的包围盒当选区，两处会错：拖过头再拉回来，包围盒比选区大；
/// 拖动时按空格移动选区、按 ⇧/⌥ 约束边、或者切到窗口模式，轨迹和截图根本对不上
/// （窗口模式单击时包围盒退化成一个点，还会被当成「用户取消」）。
/// 现在尺寸取自 PNG；位置以**松手点**为锚（光标那时已经停住，比按下点采样得准），
/// 并用「按下点 + 拖动方向 + 尺寸」推出的终点核对一遍，对不上就如实说「位置不准」，而不是悄悄盖歪。
fn place_selection(start: Option<(f64, f64)>, end: Option<(f64, f64)>, size: (f64, f64)) -> Placement {
    let (width, height) = size;
    if let (Some((sx, sy)), Some((ex, ey))) = (start, end) {
        let sign = |delta: f64| if delta >= 0.0 { 1.0 } else { -1.0 };
        let (dir_x, dir_y) = (sign(ex - sx), sign(ey - sy));
        // 确实拖动过：按下点和松手点在两个方向上都至少隔开一点。
        // 窗口模式下单击一个很小的窗口，两点重合，不能算「拖出来的」
        let dragged = (ex - sx).abs() >= MIN_DRAG && (ey - sy).abs() >= MIN_DRAG;
        // 沿拖动方向走一个截图那么大，终点应当落在松手的位置附近
        let predicted = (sx + dir_x * width, sy + dir_y * height);
        if dragged && (ex - predicted.0).abs() <= ALIGN_TOLERANCE && (ey - predicted.1).abs() <= ALIGN_TOLERANCE {
            // 位置以松手点为准：按下点是轮询到的，比真正按下晚了一个采样间隔 + 一次系统调用，
            // 快速拖动时会偏十几个点；松手时光标已经停住，采样更准
            return Placement {
                x: if dir_x > 0.0 { ex - width } else { ex },
                y: if dir_y > 0.0 { ey - height } else { ey },
                w: width,
                h: height,
                notice: None,
            };
        }
    }

    // 对不上：把覆盖层放在鼠标附近（至少用户看得到），并说明不一定对准
    let (cx, cy) = end.or(start).unwrap_or((100.0 + width / 2.0, 100.0 + height / 2.0));
    Placement {
        x: cx - width / 2.0,
        y: cy - height / 2.0,
        w: width,
        h: height,
        notice: Some(
            "没法确定选区的准确位置（拖动时按了空格或 ⇧ ⌥，或是窗口模式），译文显示在鼠标附近，\
             不一定对准原文。直接拖框可以原位覆盖。"
                .to_string(),
        ),
    }
}

/// 原生选区：调 `screencapture -i` 让系统画选框（拖动永远可靠），
/// 同时轮询鼠标：记下左键按下与松开时的位置，供 `place_selection` 推算落点。
///
/// 为什么不用自绘遮罩：透明全屏 WebView 在 macOS 上拿第一次点击、重绘、
/// 焦点都不可靠，实测拖不动、松手才刷新。
fn select_region(app: &AppHandle) -> Result<Option<(Placement, Screen, PathBuf)>, String> {
    let png = temp_png("select");
    let _ = std::fs::remove_file(&png);

    let mut child = Command::new("/usr/sbin/screencapture")
        .args(["-i", "-x", &png.to_string_lossy()])
        .spawn()
        .map_err(|err| format!("启动选区失败：{err}"))?;

    let mut tracker = DragTracker::default();
    let mut exit_status = None;
    // 主屏缩放在这次选区里不会变：取一次就够，别在每 12ms 一轮的轮询里再多跑一趟主线程
    let primary = primary_scale(app);

    loop {
        let down = left_button_down();
        tracker.observe(down, cursor_logical_with(app, primary));

        match child.try_wait() {
            Ok(Some(status)) => {
                exit_status = Some(status);
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(12)),
            Err(_) => break,
        }
    }

    let png_len = std::fs::metadata(&png).map(|meta| meta.len()).ok();
    if !has_selection(png_len) {
        // 没有截图：多半是 Esc；退出码一并记下，万一是 screencapture 自己出错也能从日志看出来
        trace(app, format!("native select: 没有截图（用户取消？）exit={exit_status:?}"));
        let _ = std::fs::remove_file(&png);
        return Ok(None);
    }

    // 尺寸以截图本身为准，不用鼠标轨迹
    let (pixel_w, pixel_h) = match png_size(&png) {
        Ok(size) => size,
        Err(err) => {
            let _ = std::fs::remove_file(&png);
            return Err(format!("截图文件读不出来：{err}"));
        }
    };
    // 截图是按选区所在那块屏的缩放截的；光标坐标已经换成逻辑点了（两者用的不是同一个缩放）。
    // 这块屏的缩放与范围现在就量好，随选区带着走
    let screen = Screen::under_cursor(app);
    let scale = screen.scale;
    let size = (f64::from(pixel_w) / scale, f64::from(pixel_h) / scale);
    if size.0 < 8.0 || size.1 < 8.0 {
        let _ = std::fs::remove_file(&png);
        return Ok(None);
    }

    let placement = place_selection(tracker.start, tracker.end, size);
    trace(
        app,
        format!(
            "native select: start={:?} end={:?} png={pixel_w}x{pixel_h} scale={scale} aligned={}",
            tracker.start,
            tracker.end,
            placement.notice.is_none()
        ),
    );

    Ok(Some((placement, screen, png)))
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

    // 选框已经开着：这次按键忽略（选框就在屏幕上，用户看得见）。
    // 只挡选框这一段；选完之后的识别 / 翻译不挡，新的一次照常开始，旧的一次做完发现自己过期就作废
    if state.selecting.swap(true, Ordering::SeqCst) {
        trace(&app, "capture ignored: 选框还开着");
        return Ok(());
    }
    let selecting = BusyGuard(&state.selecting);
    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;

    // 上一次的译文窗口还在：先收掉并等它真的消失。
    // 否则它会被拍进这一次的截图，译文被当成原文再翻一遍；新窗口也会因为 label 重复建不出来。
    if close_overlay_and_wait(&app).await {
        tokio::time::sleep(Duration::from_millis(80)).await; // 让合成器把旧窗口擦掉
    }

    // 再收起主窗口，别让它挡住要截的内容
    if let Some(main) = app.get_webview_window(crate::window::MAIN_WINDOW) {
        let _ = main.hide();
    }

    let picked = {
        let handle = app.clone();
        tauri::async_runtime::spawn_blocking(move || select_region(&handle))
            .await
            .map_err(|err| format!("选区线程失败：{err}"))??
    };

    drop(selecting); // 选框阶段结束；之后的识别 / 翻译不再挡快捷键

    let Some((placement, screen, png)) = picked else {
        return Ok(());
    };

    trace(
        &app,
        format!(
            "capture rect={},{},{},{} notice={:?}",
            placement.x, placement.y, placement.w, placement.h, placement.notice
        ),
    );

    let result = run_with_image(&app, &state, placement, screen, &png, Some(generation)).await;
    let _ = std::fs::remove_file(&png);
    match result {
        // 已经被新的一次取代：它的错误（没识别到文字、识别失败……）也不能再往外报。
        // 报了的话热键处理会把 960×900 的主窗口弹到最前，压在用户正在拖选的新一次上
        Err(err) if !is_current(&state.generation, generation) => {
            trace(&app, format!("capture #{generation} 已被取代，丢弃它的错误：{err}"));
            Ok(())
        }
        other => other.map(|_| ()),
    }
}

/// 关掉已有的译文窗口，并等它真的从窗口表里消失（最多 1 秒）。返回之前有没有这个窗口。
async fn close_overlay_and_wait(app: &AppHandle) -> bool {
    if app.get_webview_window(OVERLAY_WINDOW).is_none() {
        return false;
    }
    close(app, OVERLAY_WINDOW);
    for _ in 0..40 {
        if app.get_webview_window(OVERLAY_WINDOW).is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    true
}

/// 选区确定：截图 → OCR → 翻译 → 原位覆盖。
///
/// `placement` 是**全局逻辑坐标**（屏幕点）：覆盖层窗口放哪、多大，以及要不要附一句提示。
/// 命令的实体：抽出来是为了自动化验证（跳过选区直接给一个矩形）。
pub async fn run_with_image(
    app: &AppHandle,
    state: &CaptureState,
    placement: Placement,
    screen: Screen,
    png: &Path,
    generation: Option<u64>,
) -> Result<usize, String> {
    let Placement { x, y, w, h, notice } = placement;
    if w < 12.0 || h < 12.0 {
        return Err("选区太小了".to_string());
    }


    // OCR 最多会占 30 秒，放到阻塞线程里跑，别霸占异步运行时的工作线程
    let ocr_png = png.to_path_buf();
    let lines = tauri::async_runtime::spawn_blocking(move || run_ocr(&ocr_png))
        .await
        .map_err(|err| format!("识别线程失败：{err}"))?
        .map_err(|err| format!("识别失败：{err}"))?;
    let _ = std::fs::remove_file(png);

    trace(app, format!("ocr lines={}", lines.len()));
    if lines.is_empty() {
        return Err("选中的区域里没识别到文字。也可能是屏幕录制权限没给：系统设置 → 隐私与安全性 → 屏幕录制。".to_string());
    }

    let mut blocks = merge_lines(&lines, screen.scale);

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

    // 有失败或位置提示时，覆盖层至少要放得下这条提示：只框了一行字时选区太矮，提示会被截掉
    let (w, h) = if failure.is_some() || notice.is_some() {
        (w.max(ALERT_MIN_WIDTH), h.max(ALERT_MIN_HEIGHT))
    } else {
        (w, h)
    };
    // 放大后不能伸出屏幕（靠右下角框选时）：窗口往回挪，块的位置同步补偿，仍对准原文
    let (shift_x, shift_y) = screen
        .bounds
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
            "blocks={} translated={} error={:?} notice={:?}",
            blocks.len(),
            blocks.iter().filter(|block| block.translated).count(),
            failure,
            notice
        ),
    );
    // 做的这段时间里用户又发起了新的一次：这一次的结果作废，不再弹窗。
    // 要先检查再关旧窗口：新的一次可能已经把它的译文窗口弹出来了，
    // 过期的这一次不能把它关掉（否则最新的结果凭空消失，也没有东西来替代）
    let superseded = |mine: u64| {
        let stale = !is_current(&state.generation, mine);
        if stale {
            trace(app, format!("capture #{mine} 已被新的一次取代，结果作废"));
        }
        stale
    };
    if generation.is_some_and(superseded) {
        return Ok(0);
    }
    // 旧的译文窗口还没关干净时 label 会撞车（开发钩子和并发都可能走到这里）；关窗要 await，
    // 所以回来之后再检查一次，检查之后到建窗之间没有 await
    close_overlay_and_wait(app).await;
    if generation.is_some_and(superseded) {
        return Ok(0);
    }

    if let Ok(mut slot) = state.blocks.lock() {
        *slot = blocks.clone();
    }
    if let Ok(mut slot) = state.error.lock() {
        *slot = failure;
    }
    if let Ok(mut slot) = state.notice.lock() {
        *slot = notice;
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
    let png = temp_png("dev-capture");
    std::fs::copy(source, &png).map_err(|err| format!("复制测试图片失败：{err}"))?;
    let (width, height) = png_size(&png)?;
    let screen = Screen::under_cursor(app);
    let placement = Placement {
        x: 120.0,
        y: 120.0,
        w: f64::from(width) / screen.scale,
        h: f64::from(height) / screen.scale,
        notice: None,
    };
    run_with_image(app, state, placement, screen, &png, None).await
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

/// 这一次截屏是不是还是最新的一次。
fn is_current(generation: &AtomicU64, mine: u64) -> bool {
    generation.load(Ordering::SeqCst) == mine
}

/// 主屏的缩放。tao 在 macOS 上的 `cursor_position()` 返回的是「逻辑点 × 主屏缩放」，
/// 不管光标在哪块屏，换回逻辑点都要除以**主屏**的缩放。
fn primary_scale(app: &AppHandle) -> f64 {
    app.primary_monitor()
        .ok()
        .flatten()
        .map(|monitor| monitor.scale_factor())
        .unwrap_or(1.0)
}

/// 光标的逻辑坐标（屏幕点，原点在主屏左上）。`primary` 是主屏缩放，轮询时由调用方取一次传进来。
fn cursor_logical_with(app: &AppHandle, primary: f64) -> Option<(f64, f64)> {
    let position = app.cursor_position().ok()?;
    Some((position.x / primary, position.y / primary))
}

fn cursor_logical(app: &AppHandle) -> Option<(f64, f64)> {
    cursor_logical_with(app, primary_scale(app))
}

/// 点 (x, y) 在不在矩形 (左, 上, 右, 下) 里。
fn rect_contains(rect: (f64, f64, f64, f64), x: f64, y: f64) -> bool {
    let (left, top, right, bottom) = rect;
    x >= left && x < right && y >= top && y < bottom
}

/// 光标所在的那块屏幕，以及它的逻辑坐标范围：(左, 上, 右, 下)。
///
/// 不能用 `monitor_from_point(cursor_position())`：它拿 CoreGraphics 的逻辑范围去比较，
/// 而 `cursor_position()` 已经乘过主屏缩放。Retina 主屏上光标稍微往右下走一点就「不在任何屏幕里」，
/// 缩放退回 1.0，覆盖层的位置和大小都翻倍。这里先把光标换回逻辑点，再按各屏的逻辑范围找。
fn monitor_under_cursor(app: &AppHandle) -> Option<(tauri::Monitor, (f64, f64, f64, f64))> {
    let (x, y) = cursor_logical(app)?;
    for monitor in app.available_monitors().ok()? {
        let scale = monitor.scale_factor();
        let origin = monitor.position().to_logical::<f64>(scale);
        let size = monitor.size().to_logical::<f64>(scale);
        let rect = (origin.x, origin.y, origin.x + size.width, origin.y + size.height);
        if rect_contains(rect, x, y) {
            return Some((monitor, rect));
        }
    }
    None
}

/// 窗口 (x, y, w, h) 要挪多少才能完全落在屏幕里：正数往左/往上挪，负数往右/往下挪。
/// 比屏幕还大放不下时，优先贴住左上边。
fn shift_into(x: f64, y: f64, w: f64, h: f64, bounds: (f64, f64, f64, f64)) -> (f64, f64) {
    let (left, top, right, bottom) = bounds;
    let fitted_x = x.min(right - w).max(left);
    let fitted_y = y.min(bottom - h).max(top);
    (x - fitted_x, y - fitted_y)
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

/// OCR 子进程最多跑这么久，超时就杀掉并如实报错：一直卡着的话，这次截屏会悄无声息地没有下文。
const OCR_TIMEOUT: Duration = Duration::from_secs(30);

/// 跑一个子进程，带超时；返回 (是否成功退出, stdout, stderr)。
///
/// 管道放到线程里读：子进程输出多了会把管道写满，不读它就永远退不出来；
/// 超时就杀掉并回收，不留僵尸进程。
fn run_with_timeout(mut command: Command, timeout: Duration) -> Result<(bool, Vec<u8>, Vec<u8>), String> {
    use std::io::Read;
    use std::process::Stdio;

    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("启动失败：{err}"))?;

    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_end(&mut buffer);
        }
        buffer
    });
    let err_reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = pipe.read_to_end(&mut buffer);
        }
        buffer
    });

    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("超过 {} 秒没有结果，已中止", timeout.as_secs_f32()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("等待失败：{err}"));
            }
        }
    };

    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    Ok((status.success(), stdout, stderr))
}

fn run_ocr(png: &Path) -> Result<Vec<OcrLine>, String> {
    let binary = ocr_binary()?;
    let mut command = Command::new(&binary);
    command.arg(png);
    let (ok, stdout, stderr) = run_with_timeout(command, OCR_TIMEOUT).map_err(|err| format!("OCR {err}"))?;
    if !ok {
        let message = String::from_utf8_lossy(&stderr).trim().to_string();
        // 崩溃时往往一个字都没有：别让界面显示一个空的「识别失败：」
        return Err(if message.is_empty() { "OCR 组件异常退出，没有留下错误信息".to_string() } else { message });
    }
    serde_json::from_slice::<Vec<OcrLine>>(&stdout).map_err(|err| format!("OCR 输出解析失败：{err}"))
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
    let outcome = openai_compat::stream_chat_adaptive(
        crate::client_for(&provider.base_url),
        &provider,
        key.as_deref(),
        &body,
        reasoning.as_ref(),
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
        assert_eq!(shift_into(-50.0, 20.0, 300.0, 84.0, screen), (-50.0, 0.0), "伸出左边要往右挪");
        assert_eq!(shift_into(100.0, -30.0, 300.0, 84.0, screen), (0.0, -30.0), "伸出上边要往下挪");
    }

    fn placed(start: Option<(f64, f64)>, end: Option<(f64, f64)>, size: (f64, f64)) -> Placement {
        place_selection(start, end, size)
    }

    #[test]
    fn normal_drag_is_placed_by_its_release_corner_with_the_real_size() {
        // 向右下拖：松手点是右下角
        let down_right = placed(Some((100.0, 100.0)), Some((400.0, 340.0)), (300.0, 240.0));
        assert_eq!(down_right.notice, None);
        assert_eq!((down_right.x, down_right.y, down_right.w, down_right.h), (100.0, 100.0, 300.0, 240.0));

        // 向左上拖：松手点是左上角
        let up_left = placed(Some((500.0, 400.0)), Some((200.0, 160.0)), (300.0, 240.0));
        assert_eq!(up_left.notice, None);
        assert_eq!((up_left.x, up_left.y), (200.0, 160.0));

        // 斜向的另外一个方向
        let up_right = placed(Some((100.0, 400.0)), Some((400.0, 160.0)), (300.0, 240.0));
        assert_eq!((up_right.x, up_right.y, up_right.notice), (100.0, 160.0, None));

        // 松手点落后一点（采样滞后）也算对得上，位置跟着松手点走
        let lagging = placed(Some((100.0, 100.0)), Some((395.0, 330.0)), (300.0, 240.0));
        assert_eq!(lagging.notice, None);
        assert_eq!((lagging.x, lagging.y), (95.0, 90.0));
    }

    #[test]
    fn overshooting_the_drag_does_not_enlarge_the_selection() {
        // 旧做法取全程轨迹的包围盒：拖到 (1000,800) 再拉回来，选区会被算成 900×700。
        // 现在只看按下点和松手点，尺寸取自截图
        let result = placed(Some((100.0, 100.0)), Some((398.0, 341.0)), (300.0, 240.0));
        assert_eq!(result.notice, None);
        assert_eq!((result.w, result.h), (300.0, 240.0));
    }

    #[test]
    fn moved_or_constrained_selection_is_reported_not_silently_misplaced() {
        // 拖动时按空格把选区整体挪走了：松手位置离「按下点 + 截图尺寸」很远
        let moved = placed(Some((100.0, 100.0)), Some((900.0, 700.0)), (300.0, 240.0));
        let notice = moved.notice.as_deref().expect("对不上必须带提示");
        assert!(notice.contains("不一定对准"), "{notice}");
        // 退化成显示在鼠标附近，尺寸仍是截图真实尺寸
        assert_eq!((moved.x, moved.y, moved.w, moved.h), (750.0, 580.0, 300.0, 240.0));
    }

    #[test]
    fn window_mode_click_is_not_mistaken_for_a_cancel() {
        // 窗口模式单击：按下点 = 松手点，但截到了整个窗口。以前包围盒退化成点，被当成取消
        let click = placed(Some((400.0, 300.0)), Some((400.0, 300.0)), (800.0, 600.0));
        assert!(click.notice.is_some());
        assert_eq!((click.w, click.h), (800.0, 600.0), "尺寸取自截图");
    }

    #[test]
    fn missing_mouse_samples_still_give_a_usable_placement() {
        let none = placed(None, None, (300.0, 240.0));
        assert!(none.notice.is_some());
        assert_eq!((none.w, none.h), (300.0, 240.0));
        let only_end = placed(None, Some((500.0, 500.0)), (300.0, 240.0));
        assert_eq!((only_end.x, only_end.y), (350.0, 380.0));
    }

    #[test]
    fn busy_guard_releases_on_every_exit() {
        let busy = AtomicBool::new(false);
        assert!(!busy.swap(true, Ordering::SeqCst));
        {
            let _guard = BusyGuard(&busy);
            assert!(busy.load(Ordering::SeqCst), "选框开着");
        }
        assert!(!busy.load(Ordering::SeqCst), "离开作用域必须放掉，否则之后再也截不了屏");
    }

    #[test]
    fn busy_guard_releases_when_dropped_early() {
        // start_capture 选完框就提前 drop 守卫：之后的识别 / 翻译不能再挡快捷键
        let busy = AtomicBool::new(true);
        let guard = BusyGuard(&busy);
        drop(guard);
        assert!(!busy.load(Ordering::SeqCst));
    }

    #[test]
    fn a_newer_capture_supersedes_the_older_one() {
        let generation = AtomicU64::new(0);
        let first = generation.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(is_current(&generation, first));
        let second = generation.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(!is_current(&generation, first), "第二次开始后，第一次的结果必须作废");
        assert!(is_current(&generation, second));
    }

    fn walk(steps: &[(bool, Option<(f64, f64)>)]) -> DragTracker {
        let mut tracker = DragTracker::default();
        for (down, cursor) in steps {
            tracker.observe(*down, *cursor);
        }
        tracker
    }

    #[test]
    fn drag_tracker_uses_press_and_release_not_the_whole_path() {
        // 按下 → 拖到很远 → 拉回来 → 松手：终点是松手那一刻，不是走过的最远处
        let tracker = walk(&[
            (false, Some((50.0, 50.0))), // 按下之前光标停在哪都不算
            (true, Some((100.0, 100.0))),
            (true, Some((1000.0, 800.0))),
            (true, Some((395.0, 338.0))),
            (false, Some((398.0, 341.0))), // 松手后的第一次轮询，位置比按住时最后一次又挪了一点
            (false, Some((700.0, 700.0))), // 松手之后鼠标乱动，不能再影响
        ]);
        assert_eq!(tracker.start, Some((100.0, 100.0)));
        assert_eq!(tracker.end, Some((398.0, 341.0)));
        assert!(!tracker.dragging);
    }

    #[test]
    fn drag_tracker_keeps_only_the_last_press() {
        let tracker = walk(&[
            (true, Some((10.0, 10.0))),
            (true, Some((60.0, 60.0))),
            (false, Some((60.0, 60.0))),
            (true, Some((300.0, 300.0))), // 第二次按下：前一次丢掉
            (true, Some((500.0, 450.0))),
            (false, Some((500.0, 450.0))),
        ]);
        assert_eq!(tracker.start, Some((300.0, 300.0)));
        assert_eq!(tracker.end, Some((500.0, 450.0)));
    }

    #[test]
    fn window_mode_click_leaves_start_equal_to_end_and_is_not_a_cancel() {
        let tracker = walk(&[(true, Some((400.0, 300.0))), (false, Some((400.0, 300.0)))]);
        assert_eq!(tracker.start, tracker.end);
        // 是不是取消只看有没有截图，不看轨迹
        assert!(has_selection(Some(120_000)));
        assert!(!has_selection(Some(0)));
        assert!(!has_selection(None));
    }

    #[test]
    fn no_press_means_no_samples() {
        let tracker = walk(&[(false, Some((1.0, 1.0))), (false, Some((2.0, 2.0)))]);
        assert_eq!(tracker, DragTracker::default());
    }

    #[test]
    fn down_left_drag_is_anchored_at_the_release_point() {
        // 向左下拖：按下点是右上角，松手点是左下角
        let result = placed(Some((500.0, 100.0)), Some((202.0, 338.0)), (300.0, 240.0));
        assert_eq!(result.notice, None);
        assert_eq!((result.x, result.y), (202.0, 98.0), "x 取松手点（左边），y 取松手点往上推一个高度");
    }

    #[test]
    fn a_tiny_window_clicked_in_window_mode_is_not_taken_for_a_drag() {
        // 窗口只有 30×30 点，单击后按下点 = 松手点：差得比容差小，但根本没有拖动
        let click = placed(Some((400.0, 300.0)), Some((400.0, 300.0)), (30.0, 30.0));
        assert!(click.notice.is_some(), "没拖动就不能说「已对准」");
    }

    #[test]
    fn fast_drags_anchor_at_the_release_point_not_the_lagging_press_sample() {
        // 按下点采样晚了十几个点（114 而不是 100）；松手点是准的。位置以松手点为准
        let result = placed(Some((114.0, 100.0)), Some((400.0, 340.0)), (300.0, 240.0));
        assert_eq!(result.notice, None);
        assert_eq!((result.x, result.y), (100.0, 100.0));
    }

    #[test]
    fn a_hung_child_is_killed_at_the_timeout() {
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let started = std::time::Instant::now();
        let err = run_with_timeout(command, Duration::from_millis(300)).expect_err("卡住的子进程必须被中止");
        assert!(err.contains("没有结果"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5), "要在超时附近就返回，不能等满 30 秒");
    }

    #[test]
    fn failing_child_reports_its_stderr_like_the_ocr_stub_does() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "echo 'OCR 组件没有编译成功' >&2; exit 1"]);
        let (ok, _, stderr) = run_with_timeout(command, Duration::from_secs(5)).expect("正常退出");
        assert!(!ok);
        assert!(String::from_utf8_lossy(&stderr).contains("OCR 组件没有编译成功"));
    }

    #[test]
    fn large_output_does_not_deadlock_the_pipe() {
        // 输出远超管道缓冲（64KB）：不边跑边读的话，子进程写满管道就永远退不出来
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "head -c 400000 /dev/zero | tr '\\0' 'a'"]);
        let (ok, stdout, _) = run_with_timeout(command, Duration::from_secs(10)).expect("不能死锁");
        assert!(ok);
        assert_eq!(stdout.len(), 400_000);
    }

    #[test]
    fn rect_contains_is_half_open() {
        let screen = (0.0, 0.0, 1440.0, 900.0);
        assert!(rect_contains(screen, 0.0, 0.0));
        assert!(rect_contains(screen, 1439.9, 899.9));
        assert!(!rect_contains(screen, 1440.0, 100.0), "右边界属于隔壁那块屏");
        assert!(!rect_contains(screen, -1.0, 100.0));
    }

    #[test]
    fn temp_files_never_collide_within_one_process() {
        let first = temp_png("select");
        let second = temp_png("select");
        assert_ne!(first, second, "同一进程里连续两次也要不同名");
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
