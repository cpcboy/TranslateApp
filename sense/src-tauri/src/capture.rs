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

use crate::commands::language::load_pref;
use crate::lang::{self, Lang};
use crate::providers::openai_compat;
use crate::providers as provider_registry;
use crate::secrets;

// 中文折行段落的判定（纯函数，不碰 OCR / 窗口）：一句话被折成多行时合回一块再翻译
mod zh_wrap;
// 英文硬折行段落的判定（同上）：PDF 论文、等宽字体的 README、网页里的多行段落按行折断时合回一块再翻译
mod en_wrap;
// 与模型之间的协议（纯函数 + 一个把「问模型」当参数的 `translate_all`）：分块、带编号的消息、按编号对齐、重试、对齐粗检
mod protocol;

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
///
/// `words` / `bg` / `fg` / `ink` / `room` 是辅助程序后加的，全部可缺省：
/// 缺了（旧版辅助程序、像素分析失败）就退回「只有文字和位置」的旧做法。
#[derive(Debug, Clone, Default, Deserialize)]
struct OcrLine {
    text: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    /// 逐词框。行首的图标被读成的单个字符自己成一个词，反推字号时靠它把图标的宽度排除掉
    #[serde(default)]
    words: Vec<OcrWord>,
    /// 这一行底下的底色与文字颜色（sRGB，三个 0–255）
    #[serde(default)]
    bg: Option<Vec<f64>>,
    #[serde(default)]
    fg: Option<Vec<f64>>,
    /// 原文笔画的紧包围盒（含越出 Vision 框的部分）
    #[serde(default)]
    ink: Option<OcrInk>,
    /// 笔画右边到下一个非底色像素的空白宽度
    #[serde(default)]
    room: Option<f64>,
    /// 允许遮盖的范围：笔画外扩一点，每一侧只扩到下一个非底色像素为止（不会盖到隔壁的色块、分隔线上）
    #[serde(default)]
    cover: Option<OcrInk>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct OcrWord {
    t: String,
    #[serde(default)]
    w: f64,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
struct OcrInk {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// 矩形（逻辑像素，相对选区左上）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn union(self, other: Rect) -> Rect {
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        let right = (self.x + self.w).max(other.x + other.w);
        let bottom = (self.y + self.h).max(other.y + other.h);
        Rect { x: left, y: top, w: right - left, h: bottom - top }
    }
}

/// 原文里的一个词：只要宽度，前端用 `measureText` 量出「这个词在多大的字号下有这么宽」来反推原文字号。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Word {
    pub t: String,
    /// 逻辑像素
    pub w: f64,
}

/// 合并成段之后、经翻译的块（逻辑像素坐标，相对选区左上）。
#[derive(Debug, Clone, Default, Serialize)]
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
    /// 译成功了，但译文和原文一样（命令、版本号、专有名词原样保留）：覆盖层不画这一块，留着原来的像素
    pub unchanged: bool,
    /// 合并了几行 OCR 文字（≥ 1）
    pub lines: u32,
    /// 原文底下的底色 / 文字颜色，`#rrggbb`。读不出来是 None，覆盖层退回固定的深色样式
    pub bg: Option<String>,
    pub fg: Option<String>,
    /// 原文笔画的紧包围盒：译文从它的左边起笔，竖直方向按它的中心对齐
    pub ink: Option<Rect>,
    /// 允许遮盖的范围：覆盖层只往这个范围里填底色。没有它就退回「笔画包围盒外扩一圈」
    pub cover: Option<Rect>,
    /// 笔画右边的空白宽度；None = 没量到（0 是「量到了，紧贴着别的东西」）。
    /// 译文比原文长时可以往这片空白里延伸，不必缩小字号
    pub room: Option<f64>,
    /// 原文的逐词宽度，前端靠它反推原文的字号
    pub words: Vec<Word>,
}

impl Block {
    /// 整块平移（覆盖层窗口为了不伸出屏幕往回挪时，块要同步补偿，仍对准原文）。
    fn shift(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        for rect in [self.ink.as_mut(), self.cover.as_mut()].into_iter().flatten() {
            rect.x += dx;
            rect.y += dy;
        }
    }
}

/// 辅助程序给的三个通道值 → `#rrggbb`。长度不对、有非有限数就当没有。
fn rgb_hex(color: Option<&Vec<f64>>) -> Option<String> {
    let channels = color?;
    if channels.len() != 3 || channels.iter().any(|value| !value.is_finite()) {
        return None;
    }
    let byte = |value: f64| value.round().clamp(0.0, 255.0) as u8;
    Some(format!("#{:02x}{:02x}{:02x}", byte(channels[0]), byte(channels[1]), byte(channels[2])))
}

/// 辅助程序给的三个通道值 → 三个数。长度不对、有非有限数就当没有（和 `rgb_hex` 同一个口径，只是不转成字符串）。
fn rgb_triple(color: Option<&Vec<f64>>) -> Option<[f64; 3]> {
    let channels = color?;
    if channels.len() != 3 || channels.iter().any(|value| !value.is_finite()) {
        return None;
    }
    Some([channels[0], channels[1], channels[2]])
}

/// 辅助程序给的矩形（图片像素）→ 逻辑像素。有非有限数、宽或高不大于 0 就当没有。
fn scaled_rect(rect: Option<OcrInk>, scale: f64) -> Option<Rect> {
    let rect = rect?;
    if ![rect.x, rect.y, rect.w, rect.h].iter().all(|value| value.is_finite()) || rect.w <= 0.0 || rect.h <= 0.0 {
        return None;
    }
    Some(Rect { x: rect.x / scale, y: rect.y / scale, w: rect.w / scale, h: rect.h / scale })
}

/// 两个可能缺失的矩形取并集：**任何一个缺了结果就缺**。
/// 合并多行时用：只剩部分行的框比没有更糟——前端会按它定位，错一行、漏盖另一行。
fn union_all(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.union(b)),
        _ => None,
    }
}

/// 两个可能没量到的数取较小的：没量到的不参与（0 是量到了的值，会被保留）。
fn min_known(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
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

/// 覆盖层要不要放大到能放下一条提示：翻译失败、位置不准，或所有块的译文都和原文一样
/// （前端这时不画任何块，只给一条说明——选区再小也得让人读完）。
fn needs_alert_room(failure: &Option<String>, notice: &Option<String>, blocks: &[Block]) -> bool {
    failure.is_some() || notice.is_some() || (!blocks.is_empty() && blocks.iter().all(|block| block.unchanged))
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


    // OCR 最多会占 60 秒，放到阻塞线程里跑，别霸占异步运行时的工作线程
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
    let (failure, notice) = match capture_plan(&blocks, load_pref(app)) {
        Some((target, direction)) => {
            let translated = translate_blocks(app, &mut blocks, target.name, direction).await;
            if let Err(err) = &translated {
                eprintln!("[sense] 截图翻译失败：{err}");
            }
            settle(translated, notice)
        }
        // 识别出来的全是数字、符号：没什么可译的，也要说一声
        None => (Some("选区里没有可翻译的文字（只识别出数字或符号）".to_string()), notice),
    };

    // 有失败、位置提示、或「译文全和原文一样」的提示时，覆盖层至少要放得下这条提示：
    // 只框了一行字（比如一个版本号、一条命令）时选区太矮，提示会被截掉
    let (w, h) = if needs_alert_room(&failure, &notice, &blocks) {
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
        block.shift(shift_x, shift_y);
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
///
/// 辅助程序对每张图做两遍识别，中文的大图更慢：5120×2880 的密集中文页在 M4 上要 12 秒左右，慢两倍多的机器就逼近 30 秒了，
/// 所以放宽到 60 秒——这个上限防的是卡死，不是慢。
const OCR_TIMEOUT: Duration = Duration::from_secs(60);

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

/// 一行已经把话说完了的末尾字符：英文那套「明显被折断」的判据靠它认出独立的两句话（终端输出、代码、列表），
/// 中文折行（`zh_wrap`）判「上一行说完了」时也先看它。
const SENTENCE_END: &str = ".!?。！？:：;；$>|/\\）)】]」』";

/// 同一段的多行合回一块。三条路，前面两条沿用以前的判据，第三条是英文硬折行：
/// - 英文那套：行高接近、竖直间隙小、同列，并且上一行以逗号 / 连字符结尾或这一行以小写字母开头（明显被折断的一句话）；
///   只和**上一块**比（顺序里挨着的）
/// - 中文折行：按版面找「同列里紧挨在上面的那一行」，上一行写满、话没说完、这一对像正文才合（见 `zh_wrap`）；
///   并进的可能是更早的一块（夹在中间的是别栏的内容）
/// - 英文硬折行：同样按版面找「同列里紧挨在上面的那一行」（见 `en_wrap`），一整段连着合；Vision 把一行拆成两段的，右半截跟着左半截走。
///   前两条没认领的行才轮到它；不含 CJK 字符（汉字、假名、谚文、全角形式）的行只会被第一条和第三条认领，含 CJK 字符的行只会被前两条认领。
///   第一条只看「上一块」的几何：上一块被第三条并成了更高的一块以后，它认不出这一行了（HEAD 合并过的行对里有 1.7% 因此不再同块，见 CHANGELOG）
fn merge_lines(lines: &[OcrLine], scale: f64) -> Vec<Block> {
    let mut sorted: Vec<&OcrLine> = lines.iter().filter(|line| !line.text.trim().is_empty()).collect();
    // `total_cmp`：y 是 NaN 时 `partial_cmp(..).unwrap_or(Equal)` 不是全序，标准库的排序会 panic；有限数上两者排出来一样
    sorted.sort_by(|a, b| a.y.total_cmp(&b.y));

    // 中文折行：每一行应当并进哪一行（下标对的是 `sorted`）
    let zh_links = {
        let geometry: Vec<zh_wrap::LineGeo> = sorted
            .iter()
            .map(|line| zh_wrap::LineGeo { x: line.x / scale, y: line.y / scale, w: line.w / scale, h: line.h / scale, text: line.text.trim(), fg: rgb_triple(line.fg.as_ref()) })
            .collect();
        zh_wrap::wrap_links(&geometry)
    };

    // 英文硬折行：每一行应当并进哪一行、哪些行只是同一视觉行的右半截（下标同样对的是 `sorted`）
    let en = {
        let geometry: Vec<en_wrap::EnGeo> = sorted
            .iter()
            .map(|line| {
                let words = line.words.iter().filter(|word| word.w.is_finite() && word.w > 0.0);
                en_wrap::EnGeo {
                    x: line.x / scale,
                    y: line.y / scale,
                    w: line.w / scale,
                    h: line.h / scale,
                    text: line.text.trim(),
                    fg: rgb_triple(line.fg.as_ref()),
                    first_word: line.words.first().filter(|word| word.w.is_finite() && word.w > 0.0).map(|word| word.w / scale),
                    word_chars: words.clone().map(|word| word.t.chars().count()).sum(),
                    word_width: words.map(|word| word.w / scale).sum(),
                }
            })
            .collect();
        en_wrap::wrap_links(&geometry)
    };

    // 处理顺序：按 y 从小到大；同一视觉行的右半截紧跟在左半截后面（它的 y 可能比左半截还小，不能先处理）
    let order = {
        let mut rest: Vec<Vec<usize>> = vec![Vec::new(); sorted.len()];
        for (k, head) in en.fragment_of.iter().enumerate() {
            if let Some(head) = head {
                rest[*head].push(k);
            }
        }
        let mut order = Vec::with_capacity(sorted.len());
        for k in 0..sorted.len() {
            if en.fragment_of[k].is_some() {
                continue;
            }
            order.push(k);
            let mut tail = std::mem::take(&mut rest[k]);
            tail.sort_by(|a, b| sorted[*a].x.total_cmp(&sorted[*b].x));
            order.extend(tail);
        }
        order
    };

    let mut blocks: Vec<Block> = Vec::new();
    // `sorted` 里每一行落进了哪一块：折行并进的是「同列上一行」所在的块，不一定是最后一块
    let mut block_of: Vec<usize> = vec![usize::MAX; sorted.len()];
    for index in order {
        let line = sorted[index];
        let (x, y, w, h) = (line.x / scale, line.y / scale, line.w / scale, line.h / scale);
        let text = line.text.trim().to_string();
        let ink = scaled_rect(line.ink, scale);
        let cover = scaled_rect(line.cover, scale);
        let room = line.room.filter(|value| value.is_finite() && *value >= 0.0).map(|value| value / scale);
        let words = || {
            line.words
                .iter()
                .filter(|word| word.w.is_finite() && word.w > 0.0)
                .map(|word| Word { t: word.t.clone(), w: word.w / scale })
                .collect::<Vec<_>>()
        };

        // 英文那套：和上一块比（同一视觉行的右半截不走这一条：它跟着左半截）
        let joins_last = en.fragment_of[index].is_none()
            && blocks.last().is_some_and(|last| {
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
            let ends_sentence = previous.chars().last().map(|ch| SENTENCE_END.contains(ch)).unwrap_or(true);
            let starts_lower = text
                .chars()
                .next()
                .map(|ch| ch.is_lowercase())
                .unwrap_or(false);
            let wraps = !ends_sentence && (ends_open || starts_lower);

            same_column && close_enough && similar_height && wraps
        });
        // 并进的块：右半截跟着左半截；其次英文老路（上一块）；其次中文折行、英文硬折行（「同列上一行」所在的块）。
        // 「同列上一行」通常排在前面、已经有块了；但一条视觉行被拆成高度逐段变大的几段、紧接着又有一行比它们都高时，
        // 「上一行」的行头可能排在这一行后面——还没处理就当没连上（这一行自己起一块），不能越界（评审 A 构造出过）
        let assigned = |line: usize| Some(block_of[line]).filter(|block| *block != usize::MAX);
        let target = if let Some(head) = en.fragment_of[index] {
            assigned(head)
        } else if joins_last {
            Some(blocks.len() - 1)
        } else {
            zh_links[index].or(en.wraps[index]).and_then(assigned)
        };

        if let Some(position) = target {
            let last = &mut blocks[position];
            let right = (last.x + last.w).max(x + w);
            let bottom = (last.y + last.h).max(y + h);
            last.x = last.x.min(x);
            last.y = last.y.min(y);
            last.w = right - last.x;
            last.h = bottom - last.y;
            last.line_h = last.line_h.max(h);
            last.text = zh_wrap::join_lines(&last.text, &text);
            last.lines += 1;
            last.words.extend(words());
            // 一段里各行的底色、字色是同一套：沿用第一行的，第一行没采到就用后面行的。
            // 笔画包围盒 / 遮盖范围取并集——但任何一行缺了就整块不给：只剩部分行的框，前端会按它错位一行、漏盖另一行；
            // 不给的话前端退回按 Vision 的框盖。右边的空白取各行里最窄的（没量到的行不参与）
            if last.bg.is_none() {
                last.bg = rgb_hex(line.bg.as_ref());
            }
            if last.fg.is_none() {
                last.fg = rgb_hex(line.fg.as_ref());
            }
            last.ink = union_all(last.ink, ink);
            last.cover = union_all(last.cover, cover);
            last.room = min_known(last.room, room);
            block_of[index] = position;
            continue;
        }

        block_of[index] = blocks.len();
        blocks.push(Block {
            text,
            x,
            y,
            w,
            h,
            line_h: h,
            translated: false,
            unchanged: false,
            lines: 1,
            bg: rgb_hex(line.bg.as_ref()),
            fg: rgb_hex(line.fg.as_ref()),
            ink,
            cover,
            room,
            words: words(),
        });
    }

    blocks
}

/// 目标语言：整页识别出的文字合起来判原文语言，再按偏好取舍（规则见 `lang.rs`）。
///
/// 偏好是 `auto` 时，中文、英文页面的方向与以前完全一致：中文为主译成英文，否则译成中文
/// （以前直接拒绝的日文、韩文、俄文页面，现在也默认译成中文）；
/// 手选了别的语言就译成它（原文本身就是那种语言时退回默认）。
/// 一个字母都没有（只有数字、符号）返回 None，没什么可译的。
///
/// 识别（OCR）本身目前只读中英文，其它语言的截图读不准——这是 OCR 的限制，不是翻译这边的。
#[cfg(test)]
fn capture_target(blocks: &[Block], pref: &str) -> Option<&'static Lang> {
    capture_plan(blocks, pref).map(|(target, _)| target)
}

/// 目标语言，加上对齐粗检能不能用：粗检的长度比只量过「中文页 → 英文」和「英文页 → 中文」，
/// 别的源 → 目标组合（法语 → 英文、日文 → 中文……）每个字符的权重不同，量不准，会把对齐的译文当错位，所以不检。
/// 英文页只要识别为英文就算（拉丁字母没把握时默认就是英文，和以前一样）。
fn capture_plan(blocks: &[Block], pref: &str) -> Option<(&'static Lang, protocol::Direction)> {
    let sample: String = blocks.iter().map(|block| block.text.as_str()).collect::<Vec<_>>().join(" ");
    let plan = lang::plan(&sample, pref);
    if !plan.source.letters {
        return None;
    }
    let source = plan.source.lang.map(|lang| lang.code);
    let direction = match (source, plan.target.name) {
        (Some("zh"), "英文") => protocol::Direction::ToEnglish,
        (Some("en"), "中文") => protocol::Direction::ToChinese,
        _ => protocol::Direction::Other,
    };
    Some((plan.target, direction))
}

/// 截屏翻译的系统提示词。
///
/// 这段措辞 2026-10-31 用真实模型核对过（`deepseek-flash`，样本外页面，按页口径；预设里的 `deepseek-chat` 是它的别名，2026-11-01 又补测了 `deepseek-v4-pro`），数据在 CHANGELOG
/// 「截屏翻译：整页『译文条数对不上』改成带编号的协议」：每条带编号、模型照抄编号译回，应用按编号对齐，取代了「等长字符串数组、条数是唯一校验」的老协议。
/// 第 5 条（半句话、单个符号 / 单词也用自己的编号单独译）是在 PDF 两栏页上量出来的：没有它，模型会顺着句意把译文重新分配到
/// 各个断行上（编号对、位置错一格）；加上之后第一轮的部分失败与对齐粗检触发都明显少了，别的页面上没有变差。
/// 第 4 条「每条尽量简短」对整段验证过，没有让模型删正文（CHANGELOG「验证 · 2026-10-31 — 截屏翻译提示词」）。
/// 逐字钉在 `tests/fixtures/capture-prompt.to-{en,zh}.txt`（评测用的就是这份原文）；改措辞之前先看 CHANGELOG：
/// 完整优先、写明条数、「必须恰好 N 条」这几类改法在老协议下试过，都有代价，没采用；这里也别加「不要删减信息」之类的完整性措辞。
/// 开头那句是 tools/mock-provider.mjs 认「截屏翻译」请求的标记（MARKERS.capture），必须原样保留。
fn capture_system_prompt(target: &str) -> String {
    format!(
        "你是一位翻译，正在翻译屏幕上一段连续的文字。input 是按阅读顺序排好的 JSON 数组，\
         每个元素形如 {{\"i\":编号,\"t\":原文}}，相邻两条通常属于同一段内容。\n\n要求：\n\
         1. 先通读全部条目，理解整体在讲什么，再逐条翻译成{target}；指代、术语、语气前后保持一致。\n\
         2. 输出 JSON 数组，每个元素形如 {{\"i\":编号,\"t\":译文}}，编号照抄输入，一个编号一个元素。\
         只输出数组本身，不要解释、不要代码块。\n\
         3. 命令行、代码、变量名、路径、URL、版本号、错误码、日志级别原样保留，不要翻译。\n\
         4. 每条尽量简短，以适配原来的版面宽度。\n\
         5. 有的条目只是被折行或分栏拆开的半句话，或者只是一个符号、一个词：照样用它自己的编号单独翻译，\
         哪怕读起来不通顺，不要并进相邻的条目。"
    )
}

/// 向模型问一次（流式；截屏翻译没有「停止」按钮，所以不带取消）：返回完整的回复文本，和回复有没有被 max_tokens 截断。
async fn ask_model(
    client: &reqwest::Client,
    provider: &provider_registry::Provider,
    key: Option<&str>,
    system: &str,
    reasoning: Option<&serde_json::Value>,
    user: String,
) -> Result<protocol::Reply, String> {
    let body = openai_compat::build_body(&provider.model, system, &user, 0.2, reasoning, openai_compat::MAX_TOKENS);
    let outcome = openai_compat::stream_chat_adaptive(client, provider, key, &body, reasoning, &mut |_: &str| {}, None)
        .await
        .map_err(|err| err.message())?;
    Ok(reply_from(outcome.text, outcome.finish_reason.as_deref()))
}

/// 模型的一次回复：文本，和有没有被 max_tokens 截断（`finish_reason` 是 `length`）。
fn reply_from(text: String, finish_reason: Option<&str>) -> protocol::Reply {
    protocol::Reply { truncated: finish_reason == Some("length"), text }
}

/// 问模型的方式。
struct Delivery {
    /// 各块同时发（云端 provider）。本地模型（不要 Key 的）一次只处理一个请求：同时发只会排队超时，一块一块来
    parallel: bool,
    /// 译成什么方向：对齐粗检按它取长度比；方向传反了，会把对齐的译文当成错位；中英以外的目标不检
    to_english: protocol::Direction,
}

fn delivery(direction: protocol::Direction, requires_key: bool) -> Delivery {
    Delivery { parallel: requires_key, to_english: direction }
}

/// 翻译这一步的结果怎么落到覆盖层的两个槽：整次失败的原因，和要附的提示。
/// 一部分没译成时，那句话排在位置提示（`notice`，如果有）前面，一起说。
fn settle(translated: Result<Option<String>, String>, notice: Option<String>) -> (Option<String>, Option<String>) {
    match translated {
        Ok(None) => (None, notice),
        Ok(Some(partial)) => (
            None,
            Some(match notice {
                Some(existing) => format!("{partial}{existing}"),
                None => partial,
            }),
        ),
        Err(err) => (Some(err), notice),
    }
}

/// 一块都没译成（至少有一块、而且全失败了）才是整次失败；没有块时没东西可译，不算失败。
fn all_failed(outcome: &protocol::Outcome) -> bool {
    let total = outcome.translations.len();
    total > 0 && outcome.failed() == total
}

/// 翻译整屏的块。分块并行发、按编号对齐，没译成的块保留原文：
/// - `Ok(None)`：全部译成；
/// - `Ok(Some(话))`：有的块没译成（原文原样留着，`translated` 仍为 false），这句话要交给覆盖层，不能让原文冒充译文；
/// - `Err`：一块都没译成。
async fn translate_blocks(app: &AppHandle, blocks: &mut [Block], target: &str, direction: protocol::Direction) -> Result<Option<String>, String> {
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

    let system = capture_system_prompt(target);
    // 上下文由「整屏一起送 + 先通读再翻」的提示词保证，不靠思维链。
    // 实测开思考要多等 2–5 秒，换来的是翻译任务里几乎看不出的提升。
    let reasoning = provider_registry::reasoning_fields(&provider.id);
    let client = crate::client_for(&provider.base_url);
    let (provider_ref, key_ref, reasoning_ref) = (&provider, key.as_deref(), reasoning.as_ref());
    let ask = |user: String| ask_model(client, provider_ref, key_ref, &system, reasoning_ref, user);

    let Delivery { parallel, to_english } = delivery(direction, provider.requires_key);
    let outcome = {
        let texts: Vec<&str> = blocks.iter().map(|block| block.text.as_str()).collect();
        protocol::translate_all(&texts, parallel, to_english, ask).await
    };

    let (total, failed) = (outcome.translations.len(), outcome.failed());
    trace(app, format!("translate blocks={total} requests={} retries={} failed={failed}", outcome.requests, outcome.retries));
    let notice = failure_notice(&outcome);
    if all_failed(&outcome) {
        return Err(outcome.main_reason().map(protocol::Why::describe).unwrap_or_default());
    }
    apply_translations(blocks, outcome.translations);
    Ok(notice)
}

/// 有的块没译成时，给覆盖层的一句话：几块、为什么、这几块是什么样（保留原文，用虚线标出）。
fn failure_notice(outcome: &protocol::Outcome) -> Option<String> {
    let failed = outcome.failed();
    if failed == 0 {
        return None;
    }
    let reason = outcome.main_reason().map(protocol::Why::describe).unwrap_or_default();
    Some(format!("有 {failed} 块没能译出：{}。那几块仍是原文，用虚线下划线标出。", reason.trim_end_matches(['。', '.'])))
}

/// 把译文套回各块。没译成的块（`Err`）和空译文保留原文（`translated` 仍为 false）；
/// 译文和原文一样（模型按提示词把命令、版本号、专有名词原样保留了）的标 `unchanged`：
/// 覆盖层不去动这样的块，原来的像素一点不改，比用另一种字体照着再画一遍更像原文。
fn apply_translations(blocks: &mut [Block], translations: Vec<Result<String, protocol::Why>>) {
    for (block, item) in blocks.iter_mut().zip(translations) {
        let Ok(text) = item else { continue };
        let text = text.trim().to_string();
        if !text.is_empty() {
            block.unchanged = same_text(&block.text, &text);
            block.text = text;
            block.translated = true;
        }
    }
}

/// 两段文字只差空白（OCR 的空格与模型的空格不一定一致）也算一样。
fn same_text(a: &str, b: &str) -> bool {
    a.split_whitespace().eq(b.split_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 偏好是 auto 时的翻译方向（以前 `translate_target` 的老口径，下面这些断言原样保留）。
    fn translate_target(blocks: &[Block]) -> Option<&'static str> {
        capture_target(blocks, lang::AUTO).map(|lang| lang.name)
    }

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
    fn the_text_color_the_helper_measured_reaches_the_chinese_wrap_check() {
        // 辅助程序量到的字色（sRGB 三个通道）要原样交给折行判据：任何一个通道差得远——这里每次只有一个通道——就不并
        let merged = |first: [f64; 3], second: [f64; 3]| {
            let mut upper = line("所有的文字识别都在本机完成，图片不会离开你的电脑", 22.0, 100.0, 300.0, 20.0);
            let mut lower = line("只有识别出来的文字会发送给你选择的翻译服务。", 22.0, 126.0, 120.0, 20.0);
            upper.fg = Some(first.to_vec());
            lower.fg = Some(second.to_vec());
            merge_lines(&[upper, lower], 1.0).len()
        };
        assert_eq!(merged([30.0, 30.0, 30.0], [30.0, 30.0, 30.0]), 1, "同色：并");
        assert_eq!(merged([30.0, 30.0, 30.0], [90.0, 30.0, 30.0]), 2, "只有红色通道差得远：不并");
        assert_eq!(merged([30.0, 30.0, 30.0], [30.0, 90.0, 30.0]), 2, "只有绿色通道差得远：不并");
        assert_eq!(merged([30.0, 30.0, 30.0], [30.0, 30.0, 90.0]), 2, "只有蓝色通道差得远：不并");
    }

    #[test]
    fn a_text_color_that_is_not_three_finite_numbers_counts_as_not_measured() {
        assert_eq!(rgb_triple(None), None);
        assert_eq!(rgb_triple(Some(&vec![1.0, 2.0, 3.0])), Some([1.0, 2.0, 3.0]));
        for broken in [vec![], vec![1.0, 2.0], vec![1.0, 2.0, 3.0, 4.0], vec![f64::NAN, 2.0, 3.0], vec![1.0, f64::INFINITY, 3.0], vec![1.0, 2.0, f64::NEG_INFINITY]] {
            assert_eq!(rgb_triple(Some(&broken)), None, "{broken:?}");
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

    /// 带样式信息的一行（辅助程序新版的输出）。
    fn styled(text: &str, x: f64, y: f64, w: f64, h: f64, ink: OcrInk, room: f64) -> OcrLine {
        OcrLine {
            words: text
                .split_whitespace()
                .map(|word| OcrWord { t: word.to_string(), w: word.chars().count() as f64 * 6.0 })
                .collect(),
            bg: Some(vec![30.0, 30.0, 30.0]),
            fg: Some(vec![200.0, 201.0, 202.0]),
            ink: Some(ink),
            cover: Some(OcrInk { x: ink.x - 2.0, y: ink.y - 2.0, w: ink.w + 4.0, h: ink.h + 4.0 }),
            room: Some(room),
            ..line(text, x, y, w, h)
        }
    }

    #[test]
    fn old_sidecar_output_without_style_fields_still_parses_and_degrades_to_plain_blocks() {
        // 旧版辅助程序只有文字、位置和置信度：必须照样能用，样式字段退回「没有」
        let raw = r#"[{"text":"hello world","x":10,"y":20,"w":100,"h":16,"conf":0.9}]"#;
        let lines: Vec<OcrLine> = serde_json::from_str(raw).expect("旧格式要能解析");
        let blocks = merge_lines(&lines, 1.0);
        assert_eq!(blocks.len(), 1);
        let block = &blocks[0];
        assert_eq!((block.lines, block.bg.as_deref(), block.fg.as_deref()), (1, None, None));
        assert_eq!((block.ink, block.cover, block.room, block.words.len()), (None, None, None, 0));
    }

    #[test]
    fn new_sidecar_output_is_carried_into_the_block_in_logical_pixels() {
        let raw = r##"[{"text":"© New task","x":28,"y":168,"w":168,"h":28,"conf":1,
            "words":[{"t":"©","x":28,"w":46},{"t":"New","x":70,"w":50},{"t":"task","x":124,"w":72}],
            "bg":[30,30,30],"fg":[200,201,202],"ink":{"x":34,"y":166,"w":156,"h":26},"room":410,
            "cover":{"x":30,"y":162,"w":164,"h":34}}]"##;
        let lines: Vec<OcrLine> = serde_json::from_str(raw).expect("新格式要能解析");
        let blocks = merge_lines(&lines, 2.0); // Retina：图片像素 → 逻辑点要除以 2
        let block = &blocks[0];
        assert_eq!((block.x, block.y, block.w, block.h), (14.0, 84.0, 84.0, 14.0));
        assert_eq!(block.bg.as_deref(), Some("#1e1e1e"));
        assert_eq!(block.fg.as_deref(), Some("#c8c9ca"));
        assert_eq!(block.ink, Some(Rect { x: 17.0, y: 83.0, w: 78.0, h: 13.0 }));
        assert_eq!(block.cover, Some(Rect { x: 15.0, y: 81.0, w: 82.0, h: 17.0 }));
        assert_eq!(block.room, Some(205.0));
        assert_eq!(
            block.words,
            vec![
                Word { t: "©".into(), w: 23.0 },
                Word { t: "New".into(), w: 25.0 },
                Word { t: "task".into(), w: 36.0 },
            ]
        );
    }

    #[test]
    fn merged_lines_union_the_ink_keep_the_first_style_and_use_the_tightest_room() {
        let lines = vec![
            styled("The quick brown,", 10.0, 10.0, 200.0, 20.0, OcrInk { x: 12.0, y: 12.0, w: 190.0, h: 16.0 }, 120.0),
            OcrLine {
                bg: Some(vec![250.0, 250.0, 250.0]),
                ..styled("fox jumps over", 10.0, 32.0, 180.0, 20.0, OcrInk { x: 11.0, y: 34.0, w: 170.0, h: 16.0 }, 300.0)
            },
        ];
        let blocks = merge_lines(&lines, 1.0);
        assert_eq!(blocks.len(), 1, "以逗号结尾的行与下一行合成一段");
        let block = &blocks[0];
        assert_eq!(block.lines, 2);
        assert_eq!(block.bg.as_deref(), Some("#1e1e1e"), "沿用第一行的底色");
        assert_eq!(block.ink, Some(Rect { x: 11.0, y: 12.0, w: 191.0, h: 38.0 }), "笔画包围盒取并集");
        assert_eq!(block.cover, Some(Rect { x: 9.0, y: 10.0, w: 195.0, h: 42.0 }), "遮盖范围同样取并集");
        assert_eq!(block.room, Some(120.0), "右边的空白取各行里最窄的（最窄的在第一行：不是「取最后一行」）");
        let words: Vec<&str> = block.words.iter().map(|word| word.t.as_str()).collect();
        assert_eq!(words, ["The", "quick", "brown,", "fox", "jumps", "over"]);

        // 最窄的在最后一行：也要取到（不是「取第一行」）
        let reversed = vec![
            styled("The quick brown,", 10.0, 10.0, 200.0, 20.0, OcrInk { x: 12.0, y: 12.0, w: 190.0, h: 16.0 }, 300.0),
            styled("fox jumps over", 10.0, 32.0, 180.0, 20.0, OcrInk { x: 11.0, y: 34.0, w: 170.0, h: 16.0 }, 120.0),
        ];
        assert_eq!(merge_lines(&reversed, 1.0)[0].room, Some(120.0));
    }

    #[test]
    fn a_measured_zero_room_is_kept_not_swallowed_by_later_lines() {
        // 第一行「量到了，右边紧贴着别的东西」（0）和「没量到」（None）不是一回事
        let lines = vec![
            styled("The quick brown,", 10.0, 10.0, 200.0, 20.0, OcrInk { x: 12.0, y: 12.0, w: 190.0, h: 16.0 }, 0.0),
            styled("fox jumps over", 10.0, 32.0, 180.0, 20.0, OcrInk { x: 11.0, y: 34.0, w: 170.0, h: 16.0 }, 100.0),
        ];
        assert_eq!(merge_lines(&lines, 1.0)[0].room, Some(0.0));
    }

    #[test]
    fn a_line_without_ink_makes_the_whole_block_drop_its_boxes() {
        // 只剩部分行的笔画框，前端会按它定位：错一行、漏盖另一行。不给，前端就退回按 Vision 的框盖
        let first_missing = vec![
            OcrLine { ink: None, cover: None, room: None, ..line("The quick brown,", 10.0, 10.0, 200.0, 20.0) },
            styled("fox jumps over", 10.0, 32.0, 180.0, 20.0, OcrInk { x: 11.0, y: 34.0, w: 170.0, h: 16.0 }, 300.0),
        ];
        let block = &merge_lines(&first_missing, 1.0)[0];
        assert_eq!(block.lines, 2);
        assert_eq!((block.ink, block.cover), (None, None), "首行没有：不能只留后一行的框");
        assert_eq!(block.room, Some(300.0), "没量到的行不参与「取最窄」，不把别的行的值丢掉");

        let last_missing = vec![
            styled("The quick brown,", 10.0, 10.0, 200.0, 20.0, OcrInk { x: 12.0, y: 12.0, w: 190.0, h: 16.0 }, 300.0),
            OcrLine { ink: None, cover: None, room: None, ..line("fox jumps over", 10.0, 32.0, 180.0, 20.0) },
        ];
        let block = &merge_lines(&last_missing, 1.0)[0];
        assert_eq!((block.ink, block.cover), (None, None), "后一行没有：同样整块不给");
    }

    #[test]
    fn style_colors_missing_on_the_first_line_are_taken_from_a_later_one() {
        let lines = vec![
            OcrLine { bg: None, fg: None, ..line("The quick brown,", 10.0, 10.0, 200.0, 20.0) },
            styled("fox jumps over", 10.0, 32.0, 180.0, 20.0, OcrInk { x: 11.0, y: 34.0, w: 170.0, h: 16.0 }, 300.0),
        ];
        let block = &merge_lines(&lines, 1.0)[0];
        assert_eq!((block.bg.as_deref(), block.fg.as_deref()), (Some("#1e1e1e"), Some("#c8c9ca")));
    }

    /// 块里每一块的 (行数, 文字)，看分块结果用。
    fn shape(blocks: &[Block]) -> Vec<(u32, &str)> {
        blocks.iter().map(|block| (block.lines, block.text.as_str())).collect()
    }

    /// 真 Vision 在 1 倍图上量到的一页文章（标题、三段、脚注）。一段两行、一段三行、一段首行缩进两字（末行 `口和复制粘贴。`）。
    fn article_lines() -> Vec<OcrLine> {
        vec![
            line("SENSE 截屏翻译", 22.0, 20.0, 150.0, 22.0),
            line("SENSE 是一款运行在 macOS上的 AI 翻译器，可以翻译", 22.0, 56.0, 370.0, 20.0),
            line("你在屏幕上选中的任何内容。", 22.0, 82.0, 190.0, 20.0),
            line("为了保护你的隐私，所有的文字识别都在本机完成，图", 22.0, 120.0, 358.0, 20.0),
            line("片不会离开你的电脑。只有识别出来的文字会发送给你", 24.0, 144.0, 356.0, 22.0),
            line("选择的翻译服务，而密钥只保存在系统钥匙串里。", 22.0, 170.0, 324.0, 20.0),
            line("按下快捷键，拖出一个框，译文会直接盖在原文的", 52.0, 208.0, 328.0, 22.0),
            line("位置上，字号和颜色都和原文一致，不用再来回切换窗", 22.0, 234.0, 358.0, 20.0),
            line("口和复制粘贴。", 24.0, 260.0, 98.0, 19.0),
            line("提示：首次使用需要授权屏幕录制。", 24.0, 296.0, 186.0, 16.0),
        ]
    }

    #[test]
    fn a_wrapped_chinese_paragraph_becomes_one_block_with_no_spaces_between_characters() {
        let blocks = merge_lines(&article_lines(), 1.0);
        assert_eq!(
            shape(&blocks),
            [
                (1, "SENSE 截屏翻译"),
                (2, "SENSE 是一款运行在 macOS上的 AI 翻译器，可以翻译你在屏幕上选中的任何内容。"),
                (3, "为了保护你的隐私，所有的文字识别都在本机完成，图片不会离开你的电脑。只有识别出来的文字会发送给你选择的翻译服务，而密钥只保存在系统钥匙串里。"),
                (3, "按下快捷键，拖出一个框，译文会直接盖在原文的位置上，字号和颜色都和原文一致，不用再来回切换窗口和复制粘贴。"),
                (1, "提示：首次使用需要授权屏幕录制。"),
            ],
            "标题、三段正文、脚注：段落各合成一块，标题和脚注自己一块"
        );
        // 合并后的框是各行的并集，单行行高仍是一行的高度（译文字号靠它，不能变成两倍三倍）
        let two = &blocks[1];
        assert_eq!((two.x, two.y, two.w, two.h, two.line_h), (22.0, 56.0, 370.0, 46.0, 20.0));
        // 首行缩进（52 → 22）：块的左缘取最靠左的
        let indented = &blocks[3];
        assert_eq!((indented.x, indented.w, indented.line_h), (22.0, 358.0, 22.0));
        // 翻译方向仍按整页的汉字 / 字母判（中文为主：译成英文）
        assert_eq!(translate_target(&blocks), Some("英文"));
    }

    #[test]
    fn chinese_menus_sidebars_settings_rows_and_plain_lists_stay_one_block_per_line() {
        // 真 Vision 量到的一页（语料里的另一页，文字与夹具略有不同）：左边是侧栏菜单，中间是设置页的「标题 + 描述」，下面是一列带逗号的功能列表（宽度只差 14px）。
        // 它们每行都是独立的一条，一行不许并进别的行
        let lines = vec![
            line("新建任务", 14.0, 6.0, 58.0, 20.0),
            line("界面语言", 182.0, 20.0, 62.0, 18.0),
            line("设置与偏好", 14.0, 36.0, 72.0, 20.0),
            line("选择应用的界面语言和显示区域", 182.0, 38.0, 184.0, 18.0),
            line("使用说明与帮助中", 14.0, 66.0, 114.0, 20.0),
            line("自动更新", 184.0, 69.0, 62.0, 21.0),
            line("有新版本时自动下载并在下次启动时安装", 182.0, 90.0, 234.0, 18.0),
            line("聪手", 14.0, 98.0, 30.0, 18.0),
            line("隐私模式", 182.0, 122.0, 64.0, 20.0),
            line("退出", 14.0, 128.0, 30.0, 18.0),
            line("开启后不会保存任何查询和翻译记录", 182.0, 142.0, 208.0, 18.0),
            line("通知中心", 14.0, 158.0, 58.0, 19.0),
            line("隐私与安全设置", 14.0, 186.0, 100.0, 20.0),
            line("支持中英互译，识别准确率高", 170.0, 202.0, 182.0, 22.0),
            line("离线识别，不上传任何图片", 170.0, 224.0, 168.0, 20.0),
            line("译文原位替换到截图上", 170.0, 244.0, 142.0, 20.0),
            line("查词卡片按常用度排序义项", 170.0, 266.0, 170.0, 18.0),
        ];
        let blocks = merge_lines(&lines, 1.0);
        assert_eq!(blocks.len(), lines.len(), "一行一块：{:?}", shape(&blocks));
        assert!(blocks.iter().all(|block| block.lines == 1));
    }

    #[test]
    fn a_menu_whose_lines_have_equal_widths_is_still_not_merged_when_nothing_in_it_reads_like_prose() {
        // 宽度完全一样、紧挨着、左对齐、都不以句号结尾——只差一个逗号句号：没有它，几何再像折行也不合
        let labels = ["管理已授权的应用和设备", "查看所有已保存的搜索记录", "使用说明与帮助中心入口", "屏幕录制与辅助功能权限"];
        let lines: Vec<OcrLine> = labels.iter().enumerate().map(|(index, label)| line(label, 14.0, 6.0 + 22.0 * index as f64, 168.0, 18.0)).collect();
        assert_eq!(merge_lines(&lines, 1.0).len(), labels.len());
    }

    #[test]
    fn a_sidebar_beside_an_article_does_not_split_the_paragraph_and_blocks_stay_in_reading_order() {
        // 左边侧栏、右边正文：按 y 排序后两栏的行交错（正文的第二行和第一行之间夹着侧栏的一项）
        let lines = vec![
            line("设置与偏好", 14.0, 104.0, 76.0, 20.0),
            line("所有的文字识别都在本机完成，图片不会离开你的电脑", 200.0, 100.0, 300.0, 20.0),
            line("使用说明与帮助中心", 14.0, 134.0, 136.0, 20.0),
            line("只有识别出来的文字会发送给你选择的翻译服务，而", 200.0, 126.0, 296.0, 20.0),
            line("密钥只保存在系统钥匙串里。", 200.0, 152.0, 150.0, 20.0),
        ];
        let blocks = merge_lines(&lines, 1.0);
        assert_eq!(
            shape(&blocks),
            [
                (3, "所有的文字识别都在本机完成，图片不会离开你的电脑只有识别出来的文字会发送给你选择的翻译服务，而密钥只保存在系统钥匙串里。"),
                (1, "设置与偏好"),
                (1, "使用说明与帮助中心"),
            ],
            "块按第一行的位置排：正文先，侧栏两项随后"
        );
    }

    #[test]
    fn merged_chinese_lines_follow_the_same_union_rules_as_any_other_merge() {
        // words / ink / cover / room / 底色 的合并语义不因走了中文这条路而变：沿用第一行的底色，框取并集，空白取最窄
        let lines = vec![
            styled("所有的文字识别都在本机完成，图片不会离开你的电脑", 22.0, 100.0, 300.0, 20.0, OcrInk { x: 24.0, y: 102.0, w: 296.0, h: 16.0 }, 120.0),
            OcrLine {
                bg: Some(vec![250.0, 250.0, 250.0]),
                ..styled("只有识别出来的文字会发送给你选择的翻译服务。", 22.0, 126.0, 120.0, 20.0, OcrInk { x: 23.0, y: 128.0, w: 116.0, h: 16.0 }, 300.0)
            },
        ];
        let blocks = merge_lines(&lines, 1.0);
        assert_eq!(blocks.len(), 1);
        let block = &blocks[0];
        assert_eq!(block.lines, 2);
        assert_eq!(block.text, "所有的文字识别都在本机完成，图片不会离开你的电脑只有识别出来的文字会发送给你选择的翻译服务。");
        assert_eq!(block.bg.as_deref(), Some("#1e1e1e"), "沿用第一行的底色");
        assert_eq!(block.ink, Some(Rect { x: 23.0, y: 102.0, w: 297.0, h: 42.0 }), "笔画包围盒取并集");
        assert_eq!(block.cover, Some(Rect { x: 21.0, y: 100.0, w: 301.0, h: 46.0 }), "遮盖范围取并集");
        assert_eq!(block.room, Some(120.0), "右边的空白取各行里最窄的");
        let words: Vec<&str> = block.words.iter().map(|word| word.t.as_str()).collect();
        assert_eq!(words, ["所有的文字识别都在本机完成，图片不会离开你的电脑", "只有识别出来的文字会发送给你选择的翻译服务。"], "中文行没有空格，一行就是一个词，逐行接上");

        // 有一行没量到笔画框：整块不给（和英文行一样）
        let mut missing = lines.clone();
        missing[1].ink = None;
        missing[1].cover = None;
        let block = &merge_lines(&missing, 1.0)[0];
        assert_eq!((block.lines, block.ink, block.cover), (2, None, None));
    }

    #[test]
    fn two_chinese_lines_joined_by_the_old_comma_rule_get_no_space_between_the_characters() {
        // 以 `，` 结尾的两行，英文那条老路照合（诗句、短句都会），只是汉字之间不再塞一个半角空格
        let lines = vec![line("欲穷千里目，", 10.0, 10.0, 80.0, 20.0), line("更上一层楼。", 10.0, 32.0, 80.0, 20.0)];
        assert_eq!(shape(&merge_lines(&lines, 1.0)), [(2, "欲穷千里目，更上一层楼。")]);
    }

    #[test]
    fn english_old_rule_still_merges_what_it_merged_and_the_hard_wrap_rule_goes_on_from_there() {
        // 英文那条老路没动（它的每个子条件在下面一个用例里逐条钉着），但它不再是英文合并的全部：
        // 以前连逗号结尾的多行段落只会两两合并（合并后用整块的高度比下一行，第三行就对不上），现在一整段合成一块——
        // 老路先合上前两行，英文硬折行（`en_wrap`）把后面的行接到「同列上一行」所在的块上
        let paragraph = vec![
            line("The quick brown,", 10.0, 10.0, 200.0, 20.0),
            line("fox jumps over the,", 10.0, 32.0, 190.0, 20.0),
            line("lazy dog and then,", 10.0, 54.0, 180.0, 20.0),
            line("runs away quickly", 10.0, 76.0, 170.0, 20.0),
        ];
        assert_eq!(shape(&merge_lines(&paragraph, 1.0)), [(4, "The quick brown, fox jumps over the, lazy dog and then, runs away quickly")]);
        // 下一行小写开头
        let lowercase = vec![line("Open the settings", 10.0, 10.0, 160.0, 20.0), line("to configure the app.", 10.0, 32.0, 190.0, 20.0)];
        assert_eq!(shape(&merge_lines(&lowercase, 1.0)), [(2, "Open the settings to configure the app.")]);
        // 独立的英文行
        let menu = vec![line("Settings", 10.0, 10.0, 60.0, 20.0), line("Keyboard shortcuts", 10.0, 32.0, 130.0, 20.0), line("About", 10.0, 54.0, 40.0, 20.0)];
        assert_eq!(merge_lines(&menu, 1.0).len(), 3);
        // 连字符结尾：合并仍由老路认领，只是英文单词被连字符断开的，拼接时不再隔空格（`inter-national`；以前是 `inter- national`）
        let hyphen = vec![line("an inter-", 10.0, 10.0, 70.0, 20.0), line("national event", 10.0, 32.0, 110.0, 20.0)];
        assert_eq!(shape(&merge_lines(&hyphen, 1.0)), [(2, "an inter-national event")]);
        // 中文行后面跟小写拉丁开头的行：英文那条老路合，边界有拉丁字符，隔一个空格（与以前一样）
        let mixed = vec![line("使用前请先打开", 10.0, 10.0, 150.0, 20.0), line("macOS 的系统设置", 10.0, 32.0, 130.0, 20.0)];
        assert_eq!(shape(&merge_lines(&mixed, 1.0)), [(2, "使用前请先打开 macOS 的系统设置")]);
        // 上一行已经说完（现有那套句末字符里的任何一个）：下一行即便小写开头也是新的一句，不合
        for end in ".!?。！？:：;；$>|/\\）)】]」』".chars() {
            let lines = vec![line(&format!("done{end}"), 10.0, 10.0, 80.0, 20.0), line("next one", 10.0, 32.0, 80.0, 20.0)];
            assert_eq!(merge_lines(&lines, 1.0).len(), 2, "上一行以「{end}」结尾：不合");
        }
        // 英文段落和中文段落同在一页：各走各的
        let page = vec![
            line("Open the settings", 10.0, 10.0, 160.0, 20.0),
            line("to configure the app.", 10.0, 32.0, 190.0, 20.0),
            line("SENSE 是一款运行在 macOS上的 AI 翻译器，可以翻译", 10.0, 80.0, 370.0, 20.0),
            line("你在屏幕上选中的任何内容。", 10.0, 106.0, 190.0, 20.0),
        ];
        assert_eq!(
            shape(&merge_lines(&page, 1.0)),
            [(2, "Open the settings to configure the app."), (2, "SENSE 是一款运行在 macOS上的 AI 翻译器，可以翻译你在屏幕上选中的任何内容。")]
        );
    }

    #[test]
    fn each_condition_of_the_english_rule_is_pinned_one_by_one() {
        // 英文那条老路的子条件（同列、间隙、行高、`ends_open`、`starts_lower`）搬进了闭包：每一条单独钉住。
        // 第一行只用两个词（`Quick brown,`）：英文硬折行（`en_wrap`）要求上一行至少四个词，不会插手，这里测的是老路自己。
        // 基准：上一行以逗号结尾、下一行大写开头（只靠 `ends_open` 合并），几何合格。期望值按老判据逐行移植算出
        let merged = |first: &str, second: OcrLine| merge_lines(&[line(first, 10.0, 10.0, 200.0, 20.0), second], 1.0).len() == 1;
        assert!(merged("Quick brown,", line("Fox jumps over", 10.0, 32.0, 190.0, 20.0)), "基准：逗号结尾 + 同列 + 紧挨着 + 行高一样 → 合");
        // 同列：下一行的左缘要落在上一块左缘 ± 0.35 倍块宽之内
        assert!(merged("Quick brown,", line("Fox jumps over", 60.0, 32.0, 190.0, 20.0)));
        assert!(!merged("Quick brown,", line("Fox jumps over", 95.0, 32.0, 190.0, 20.0)), "左缘偏出去 85（> 70）");
        assert!(merged("Quick brown,", line("Fox jumps over", -40.0, 32.0, 190.0, 20.0)), "往左偏 50（< 70）也算同列");
        assert!(!merged("Quick brown,", line("Fox jumps over", -70.0, 32.0, 190.0, 20.0)), "往左偏 80（> 70）");
        // 竖直间隙：≥ −2 且 < 0.55 倍块高（块高 20 → 11）
        assert!(merged("Quick brown,", line("Fox jumps over", 10.0, 40.0, 190.0, 20.0)), "间隙 10 < 11");
        assert!(!merged("Quick brown,", line("Fox jumps over", 10.0, 42.0, 190.0, 20.0)), "间隙 12 ≥ 11");
        assert!(merged("Quick brown,", line("Fox jumps over", 10.0, 28.0, 190.0, 20.0)), "重叠 2 还算");
        assert!(!merged("Quick brown,", line("Fox jumps over", 10.0, 27.0, 190.0, 20.0)), "重叠 3 不算");
        // 行高：差 < 较大者的 40%
        assert!(merged("Quick brown,", line("Fox jumps over", 10.0, 32.0, 190.0, 13.0)), "20 vs 13：差 7 < 8");
        assert!(!merged("Quick brown,", line("Fox jumps over", 10.0, 32.0, 190.0, 12.0)), "20 vs 12：差 8 不 < 8");
        // 上一行怎么结尾：逗号、全角逗号、连字符、破折号都算「没说完」，别的（句号、分号、冒号、右括号）不算
        for open in ["Quick brown,", "Quick brown，", "Quick inter-", "Quick brown—"] {
            assert!(merged(open, line("Fox jumps over", 10.0, 32.0, 190.0, 20.0)), "「{open}」没说完，下一行大写开头也合");
        }
        for closed in ["Quick brown.", "Quick brown;", "Quick brown:", "Quick (brown)", "Quick brown"] {
            assert!(!merged(closed, line("Fox jumps over", 10.0, 32.0, 190.0, 20.0)), "「{closed}」：既没有逗号 / 连字符，下一行又不是小写开头，不合");
        }
        // 下一行小写开头（上一行没有逗号）才合；上一行已说完则不合（上面那个循环已经钉过字符表）
        assert!(merged("Quick brown", line("fox jumps over", 10.0, 32.0, 190.0, 20.0)));
        assert!(!merged("Quick brown", line("Fox jumps over", 10.0, 32.0, 190.0, 20.0)));
        assert!(!merged("Quick brown.", line("fox jumps over", 10.0, 32.0, 190.0, 20.0)), "说完了：小写开头也不合");
        let blank = merge_lines(&[line("Quick brown,", 10.0, 10.0, 200.0, 20.0), line("   ", 10.0, 32.0, 190.0, 20.0)], 1.0);
        assert_eq!(shape(&blank), [(1, "Quick brown,")], "空白行直接被丢掉，没有下一行可并");
    }

    #[test]
    fn a_merged_block_keeps_the_first_lines_color_and_starts_at_the_leftmost_edge() {
        // 首行缩进：块的左缘取最靠左的行，宽度从那里量到最右的右缘；各行里字色不同时沿用第一行的（色差在容差内才会合）
        let mut lines = vec![
            styled("按下快捷键，拖出一个框，译文会直接盖在原文的", 52.0, 208.0, 328.0, 22.0, OcrInk { x: 54.0, y: 210.0, w: 324.0, h: 17.0 }, 100.0),
            styled("位置上，字号和颜色都和原文一致，不用再来回切换窗", 22.0, 234.0, 358.0, 20.0, OcrInk { x: 24.0, y: 236.0, w: 354.0, h: 16.0 }, 90.0),
            styled("口和复制粘贴。", 24.0, 260.0, 98.0, 19.0, OcrInk { x: 26.0, y: 262.0, w: 94.0, h: 15.0 }, 250.0),
        ];
        lines[1].fg = Some(vec![210.0, 205.0, 200.0]);
        lines[2].fg = Some(vec![190.0, 190.0, 190.0]);
        let block = &merge_lines(&lines, 1.0)[0];
        assert_eq!(block.lines, 3);
        assert_eq!((block.x, block.w, block.line_h), (22.0, 358.0, 22.0));
        assert_eq!(block.fg.as_deref(), Some("#c8c9ca"), "沿用第一行的字色");
        assert_eq!(block.room, Some(90.0), "空白取最窄的");
        // 字色差得远（标题 + 灰色的摘要）：不合，各自一块、各用各的字色
        let mut apart = vec![
            styled("电科金仓发布 KES AI 数据库，面向多模态场景", 430.0, 462.0, 226.0, 22.0, OcrInk { x: 432.0, y: 464.0, w: 222.0, h: 17.0 }, 100.0),
            styled("让大模型看懂私有数据。", 430.0, 484.0, 140.0, 20.0, OcrInk { x: 432.0, y: 486.0, w: 136.0, h: 15.0 }, 200.0),
        ];
        apart[0].fg = Some(vec![38.0, 38.0, 38.0]);
        apart[1].fg = Some(vec![127.0, 127.0, 127.0]);
        assert_eq!(merge_lines(&apart, 1.0).len(), 2, "字色差 89：不是同一段文字");
        // 字色缺一边（旧版辅助程序、像素分析失败）：不能因为缺数据就不合并
        apart[1].fg = None;
        assert_eq!(merge_lines(&apart, 1.0).len(), 1);
        // 字色不是三个通道 / 有非有限数：当作没有，不影响合并
        apart[0].fg = Some(vec![38.0, 38.0]);
        apart[1].fg = Some(vec![127.0, f64::NAN, 127.0]);
        assert_eq!(merge_lines(&apart, 1.0).len(), 1);
    }

    #[test]
    fn the_translation_direction_counts_every_block_not_only_the_first() {
        let chinese_first = vec![line("你好", 10.0, 10.0, 40.0, 20.0), line("The quick brown fox jumps over the lazy dog and keeps running", 10.0, 200.0, 400.0, 20.0)];
        assert_eq!(translate_target(&merge_lines(&chinese_first, 1.0)), Some("中文"), "整页英文多：译成中文");
        let english_first = vec![line("Hi", 10.0, 10.0, 20.0, 20.0), line("这是一整段很长的中文说明文字，用来把汉字的数量堆上去", 10.0, 200.0, 400.0, 20.0)];
        assert_eq!(translate_target(&merge_lines(&english_first, 1.0)), Some("英文"), "整页中文多：译成英文");
        assert_eq!(translate_target(&merge_lines(&[line("12345 ——", 10.0, 10.0, 80.0, 20.0)], 1.0)), None, "没有汉字也没有拉丁字母");
    }

    #[test]
    fn a_retina_image_groups_lines_exactly_like_the_same_page_at_one_x() {
        let one_x = article_lines();
        let two_x: Vec<OcrLine> = one_x.iter().map(|l| line(&l.text, l.x * 2.0, l.y * 2.0, l.w * 2.0, l.h * 2.0)).collect();
        let (a, b) = (merge_lines(&one_x, 1.0), merge_lines(&two_x, 2.0));
        assert_eq!(shape(&a), shape(&b));
        for (left, right) in a.iter().zip(&b) {
            assert_eq!((left.x, left.y, left.w, left.h, left.line_h), (right.x, right.y, right.w, right.h, right.line_h));
        }
    }

    #[test]
    fn the_input_order_does_not_matter_and_empty_or_blank_lines_are_ignored() {
        let mut shuffled = article_lines();
        shuffled.reverse();
        shuffled.push(line("   ", 22.0, 100.0, 50.0, 20.0));
        shuffled.push(line("", 22.0, 101.0, 50.0, 20.0));
        assert_eq!(shape(&merge_lines(&shuffled, 1.0)), shape(&merge_lines(&article_lines(), 1.0)));
        assert!(merge_lines(&[], 1.0).is_empty());
    }

    #[test]
    fn non_finite_numbers_from_the_sidecar_never_reach_the_blocks() {
        // JSON 里写不出 NaN，但结构体里能有；一个都不能漏进块里（前端拿到 NaN 会算出整屏的 NaN 坐标）
        assert_eq!(scaled_rect(Some(OcrInk { x: f64::NAN, y: 0.0, w: 5.0, h: 5.0 }), 1.0), None);
        assert_eq!(scaled_rect(Some(OcrInk { x: 0.0, y: 0.0, w: f64::INFINITY, h: 5.0 }), 1.0), None);
        assert_eq!(scaled_rect(Some(OcrInk { x: 0.0, y: 0.0, w: 5.0, h: -1.0 }), 1.0), None);
        assert_eq!(scaled_rect(Some(OcrInk { x: 0.0, y: 0.0, w: 5.0, h: 5.0 }), 2.0), Some(Rect { x: 0.0, y: 0.0, w: 2.5, h: 2.5 }));
        assert_eq!(scaled_rect(None, 1.0), None);

        let nan_everywhere = OcrLine {
            room: Some(f64::NAN),
            words: vec![OcrWord { t: "ok".into(), w: 12.0 }, OcrWord { t: "bad".into(), w: f64::NAN }, OcrWord { t: "inf".into(), w: f64::INFINITY }],
            ink: Some(OcrInk { x: 0.0, y: f64::NAN, w: 5.0, h: 5.0 }),
            ..line("ok bad inf", 0.0, 0.0, 50.0, 10.0)
        };
        let block = &merge_lines(&[nan_everywhere], 1.0)[0];
        assert_eq!((block.room, block.ink), (None, None));
        assert_eq!(block.words, vec![Word { t: "ok".into(), w: 12.0 }]);
        assert_eq!(rgb_hex(Some(&vec![f64::NAN, 1.0, 2.0])), None);
        assert_eq!(rgb_hex(Some(&vec![f64::INFINITY, 1.0, 2.0])), None);
    }

    #[test]
    fn the_overlay_is_enlarged_whenever_there_is_a_banner_to_show() {
        let unchanged = Block { unchanged: true, ..Default::default() };
        let changed = Block::default();
        let none: Option<String> = None;
        let some = Some("x".to_string());
        assert!(needs_alert_room(&some, &none, &[changed.clone()]), "翻译失败");
        assert!(needs_alert_room(&none, &some, &[changed.clone()]), "位置不准");
        assert!(needs_alert_room(&none, &none, &[unchanged.clone(), unchanged.clone()]), "全部原样保留：覆盖层里只有一条说明，选区再小也要放得下");
        assert!(!needs_alert_room(&none, &none, &[unchanged.clone(), changed.clone()]), "只要有一块要画，就不放大");
        assert!(!needs_alert_room(&none, &none, &[changed.clone()]));
        assert!(!needs_alert_room(&none, &none, &[]), "没有任何块（识别失败走别的分支）不算「全部原样」");
    }

    #[test]
    fn nonsense_style_values_are_dropped_instead_of_breaking_the_capture() {
        let broken = OcrLine {
            bg: Some(vec![1.0, 2.0]), // 少一个通道
            fg: Some(vec![300.0, -5.0, 12.4]), // 越界：夹进 0–255
            ink: Some(OcrInk { x: 1.0, y: 1.0, w: 0.0, h: 9.0 }), // 零宽
            cover: Some(OcrInk { x: 1.0, y: 1.0, w: 5.0, h: -3.0 }), // 负高
            room: Some(-4.0),
            words: vec![OcrWord { t: "ok".into(), w: 12.0 }, OcrWord { t: "zero".into(), w: 0.0 }],
            ..line("ok zero", 0.0, 0.0, 50.0, 10.0)
        };
        let block = &merge_lines(&[broken], 1.0)[0];
        assert_eq!(block.bg, None);
        assert_eq!(block.fg.as_deref(), Some("#ff000c"));
        assert_eq!((block.ink, block.cover), (None, None));
        assert_eq!(block.room, None, "负数的空白没有意义，当没量到");
        assert_eq!(block.words, vec![Word { t: "ok".into(), w: 12.0 }], "宽度为 0 的词没有信息量，丢掉");
    }

    #[test]
    fn shifting_a_block_moves_its_ink_box_too() {
        let mut block = Block {
            x: 10.0,
            y: 20.0,
            ink: Some(Rect { x: 12.0, y: 22.0, w: 50.0, h: 10.0 }),
            cover: Some(Rect { x: 10.0, y: 20.0, w: 54.0, h: 14.0 }),
            ..Default::default()
        };
        block.shift(5.0, -3.0);
        assert_eq!((block.x, block.y), (15.0, 17.0));
        assert_eq!(block.ink, Some(Rect { x: 17.0, y: 19.0, w: 50.0, h: 10.0 }), "笔画框不跟着挪，覆盖层就盖歪了");
        assert_eq!(block.cover, Some(Rect { x: 15.0, y: 17.0, w: 54.0, h: 14.0 }), "遮盖范围也要跟着挪");
        block.ink = None;
        block.cover = None;
        block.shift(1.0, 1.0); // 没有笔画框也不能出错
        assert_eq!((block.x, block.y), (16.0, 18.0));
    }

    /// 真正的辅助程序：不是占位脚本，也不比源码旧。没有就返回 None，调用方跳过并说明。
    /// 没有 swiftc 时 build.rs 放的是占位脚本，这时没东西可测。
    fn real_sidecar() -> Option<PathBuf> {
        let Ok(binary) = ocr_binary() else {
            eprintln!("跳过：找不到 sense-ocr");
            return None;
        };
        if std::fs::read(&binary).map(|bytes| bytes.starts_with(b"#!")).unwrap_or(true) {
            eprintln!("跳过：sense-ocr 是占位脚本，不是真产物");
            return None;
        }
        // build.rs 编译失败时只发 cargo:warning 并保留旧产物——这时跑旧产物等于没测新源码，必须报出来
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        if binary.starts_with(manifest.join("binaries")) {
            let modified = |path: &Path| std::fs::metadata(path).and_then(|meta| meta.modified()).ok();
            if let (Some(source), Some(product)) = (modified(&manifest.join("tools/ocr.swift")), modified(&binary)) {
                assert!(
                    product >= source,
                    "OCR 辅助程序比 tools/ocr.swift 旧，这里跑的不是当前源码：swiftc 编译失败了（build.rs 只发警告、保留旧产物，先 cargo build 看警告），\
                     或者编译被 SENSE_SKIP_OCR 跳过了（没有警告：去掉它再 cargo build）"
                );
            }
        }
        Some(binary)
    }

    #[test]
    fn real_ocr_sidecar_passes_its_own_pixel_selftest() {
        // 像素分析（底色 / 字色 / 笔画包围盒 / 右边的空白）、语言挑选、切词写在 Swift 里，它们的逻辑靠辅助程序自带的自检守住
        let Some(binary) = real_sidecar() else {
            return;
        };
        let mut command = Command::new(&binary);
        command.arg("--selftest");
        let (ok, stdout, stderr) = run_with_timeout(command, Duration::from_secs(30)).expect("自检要能跑起来");
        assert!(ok, "辅助程序自检失败：{}", String::from_utf8_lossy(&stderr));
        assert!(String::from_utf8_lossy(&stdout).contains("selftest ok"));
    }

    /// 跑真的 OCR。图在 tests/fixtures/（来源和重做办法见那里的 README），是真实渲染出来的截图。
    fn recognized_lines(fixture: &str) -> Vec<OcrLine> {
        let png = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(fixture);
        assert!(png.exists(), "缺测试图片 {}", png.display());
        run_ocr(&png).expect("识别要能跑完")
    }

    fn joined(lines: &[OcrLine]) -> String {
        lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join("\n")
    }

    fn han_chars(text: &str) -> Vec<char> {
        text.chars().filter(|ch| ('\u{4e00}'..='\u{9fff}').contains(ch)).collect()
    }

    #[test]
    fn real_ocr_reads_chinese_text_in_a_screenshot() {
        // Vision 以识别语言列表里的第一个为主：英文排前面时，中文整行读不出来——读成乱码（`X#5127`）或者直接丢掉，
        // 中文截图就等于没法翻译。图里四行：两行纯中文、一行英文、一行中英混排。
        // 不断言 `API Key`：中文优先的识别偶尔会把 I 读成 l（`APl Key`），那是已知的小毛病，不该让测试跟着抖。
        if real_sidecar().is_none() {
            return;
        }
        let lines = recognized_lines("ocr-zh-en.png");
        let text = joined(&lines);
        for expected in ["新建任务", "设置与偏好", "Tasks", "Keychain"] {
            assert!(text.contains(expected), "识别结果里没有「{expected}」——中文截图要能读出来：\n{text}");
        }
        // 方向：中文为主就译成英文。这正是「冒出汉字会把方向带偏」要守住的那个判断
        assert_eq!(translate_target(&merge_lines(&lines, 1.0)), Some("英文"), "中文截图要译成英文：\n{text}");
        // 逐词框：译文字号靠它反推，中文行现在也会走这条路。每个词都要有正的宽度、出现在它那一行里，加起来是整行的一部分
        let mut multi_word_lines = 0;
        for line in &lines {
            assert!(!line.words.is_empty(), "「{}」这一行没有逐词框", line.text);
            for word in &line.words {
                assert!(word.w.is_finite() && word.w > 0.0, "「{}」里的词「{}」宽度不对：{}", line.text, word.t, word.w);
                assert!(line.text.contains(&word.t), "词「{}」不在它那一行「{}」里", word.t, line.text);
            }
            // 词框是这一行框的一部分：加起来不会只占一小半（图标那一个词也算在里面），也不会比整行宽出一大截——
            // 每个词的框自带一点边距，词多了会累积，所以上界按词数放宽
            let total: f64 = line.words.iter().map(|word| word.w).sum();
            assert!(
                total <= line.w * 1.15 + 3.0 * line.words.len() as f64 && total >= line.w * 0.5,
                "「{}」的词框宽度加起来是 {total}，整行是 {}，对不上",
                line.text,
                line.w
            );
            if line.words.len() > 1 {
                multi_word_lines += 1;
            }
        }
        assert!(multi_word_lines >= 1, "没有一行被切成多个词：\n{text}");
    }

    #[test]
    fn real_ocr_reads_short_chinese_selections() {
        // 选一两行是最常见的用法：带图标的菜单项（`B 首页`，每行置信度只有 0.3）、中英混排的一行（`Search 搜索`，
        // 英文优先把汉字丢了、剩下的英文两遍一样）——量凑不够，要靠系统的中文分词认出「真词」（ocr.swift 的 `pickRecognition`）
        if real_sidecar().is_none() {
            return;
        }
        let icon_label = joined(&recognized_lines("ocr-zh-icon-label.png"));
        assert!(icon_label.contains("首页"), "带图标的单行菜单项要读得出来：\n{icon_label}");
        let lines = recognized_lines("ocr-zh-icon-label.png");
        assert_eq!(translate_target(&merge_lines(&lines, 1.0)), Some("英文"), "中文菜单项要译成英文：\n{icon_label}");
        // 中英混排的一行，方向由汉字和字母的多少定（`Search 搜索`：2 个汉字、6 个字母，会判成译成中文），这里只看汉字读没读出来
        let mixed = joined(&recognized_lines("ocr-zh-mixed-line.png"));
        assert!(mixed.contains("搜索"), "中英混排的一行，汉字要读得出来：\n{mixed}");
    }

    #[test]
    fn real_ocr_reads_a_chinese_menu_decorated_with_emoji() {
        // 每行都夹着表情符号的中文菜单：Vision 给每一行的置信度都是 0.3（表情把识别拖低了），
        // 单看置信度会把它当成图标读歪的杂音。要靠「弱证据行多、汉字多」才认得出来（ocr.swift 的 `pickRecognition`）
        if real_sidecar().is_none() {
            return;
        }
        let lines = recognized_lines("ocr-zh-emoji.png");
        let text = joined(&lines);
        let found = ["帮助", "设置与偏好", "集成", "共享", "发现", "团队", "视频", "照片", "提醒", "联系人", "下载"]
            .iter()
            .filter(|word| text.contains(**word))
            .count();
        assert!(found >= 4, "只读出了 {found} 个菜单项，中文菜单要读得出来：\n{text}");
        assert_eq!(translate_target(&merge_lines(&lines, 1.0)), Some("英文"), "中文菜单要译成英文：\n{text}");
    }

    /// 两个汉字之间夹着空格（行与行接起来时不该有）。
    fn has_space_between_han(text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        chars.windows(3).any(|w| w[1] == ' ' && ('\u{4e00}'..='\u{9fff}').contains(&w[0]) && ('\u{4e00}'..='\u{9fff}').contains(&w[2]))
    }

    /// 夹具里读出来的某一行：按一小段关键字找（4–6 个字）。读歪的字只要落在关键字之外就不影响；落在关键字里，这一行就找不到，用例会红。
    fn ocr_line<'a>(lines: &'a [OcrLine], anchor: &str) -> &'a OcrLine {
        lines.iter().find(|line| line.text.contains(anchor)).unwrap_or_else(|| panic!("没有读出含「{anchor}」的一行：\n{}", joined(lines)))
    }

    #[test]
    fn real_ocr_lines_of_a_wrapped_chinese_paragraph_are_merged_into_one_block_per_paragraph() {
        // 一页文章：标题、三段正文（一段两行、一段三行、一段首行缩进两字）、脚注。没有合并之前，一句话会在词中间被切成两条各译一遍
        //（`…可以翻译` / `你在屏幕上选中的任何内容。`）；现在每段合成一块，标题和脚注自己一块，汉字之间没有空格。
        // 断言只认短锚点（每段开头一个、末尾一个，同一块里都有 = 整段合上了）和块的结构，不比整行文字：Vision 偶尔读歪一两个字不该让它变红
        if real_sidecar().is_none() {
            return;
        }
        let lines = recognized_lines("ocr-zh-paragraph.png");
        let text = joined(&lines);
        let blocks = merge_lines(&lines, 1.0);
        assert!(lines.len() >= 9, "这一页至少读出 9 行（标题、三段共 8 行、脚注），只有 {} 行：\n{text}", lines.len());
        let summary = || blocks.iter().map(|b| (b.lines, b.text.as_str())).collect::<Vec<_>>();
        let block_with = |needle: &str| blocks.iter().find(|block| block.text.contains(needle)).unwrap_or_else(|| panic!("没有哪一块里有「{needle}」：{:?}", summary()));

        // 前提：这一页真的有「会折行」的几何——每段的第一行读成一条、末行读成另一条
        for (first, last) in [("可以翻译", "任何内容"), ("保护你的", "钥匙串"), ("按下快捷", "复制粘")] {
            let (a, b) = (ocr_line(&lines, first), ocr_line(&lines, last));
            assert!(a.y < b.y, "「{first}」与「{last}」要在不同的两行里：\n{text}");
        }
        // 每段：开头的锚点和末尾的锚点落在同一块里，行数不少于它折成的行数
        for (first, last, at_least) in [("可以翻译", "任何内容", 2), ("保护你的", "钥匙串", 3), ("按下快捷", "复制粘", 3)] {
            let block = block_with(first);
            assert!(block.text.contains(last) && block.lines >= at_least, "「{first}」到「{last}」应当合成一块、至少 {at_least} 行：{:?}", summary());
        }
        // 三段各是一块，标题和脚注各自一块；块与块之间不串
        assert_eq!(blocks.iter().filter(|block| block.lines > 1).count(), 3, "只有三段正文是多行块：{:?}", summary());
        assert_eq!(block_with("截屏翻").lines, 1, "标题自己一块");
        assert_eq!(block_with("首次使用").lines, 1, "脚注自己一块");
        for block in &blocks {
            assert!(!has_space_between_han(&block.text), "汉字之间不该有空格：{:?}", block.text);
        }
        // 合并后的块是各行的并集，单行行高仍是一行的高度（译文字号靠它）；方向仍是中文 → 英文
        let intro = block_with("可以翻译");
        assert!(intro.h > intro.line_h * 1.8 && intro.line_h < 30.0, "框高 {}、行高 {}", intro.h, intro.line_h);
        assert_eq!(translate_target(&blocks), Some("英文"));
    }

    #[test]
    fn real_ocr_lines_of_a_chinese_sidebar_settings_page_and_list_stay_one_block_per_line() {
        // 一页界面：侧栏菜单、设置页的「标题 + 描述」、一列带逗号的功能列表——每行都是独立的一条。
        // 其中侧栏下半是四行宽度几乎一样、紧挨着、左对齐的标签（几何上最像折行），没有逗号句号；
        // 功能列表里有逗号，但最宽的一行（第四条）把列的最右缘撑开，前面几条都不算写满。一行都不许并进别的行
        if real_sidecar().is_none() {
            return;
        }
        let lines = recognized_lines("ocr-zh-sidebar.png");
        let text = joined(&lines);
        assert!(lines.len() >= 15, "这一页有 19 行，只读出 {} 行：\n{text}", lines.len());

        // 前提一：四行紧凑的等宽标签各是独立的一条 OCR 行，右缘相差不到半个行高、间隙不到 0.55 个行高、左缘对齐——
        // 也就是说，除了没有逗号句号，它们在几何上和折行一模一样；Vision 要是把它们读成一行，这条用例就什么也没守住
        let tight = ["屏幕录制", "已授权的", "已保存的", "帮助中心入口"];
        let rows: Vec<&OcrLine> = tight.iter().map(|anchor| ocr_line(&lines, anchor)).collect();
        for pair in rows.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let h = a.h.max(b.h);
            assert!(b.y > a.y, "四行标签要按这个顺序各占一行：\n{text}");
            assert!(((a.x + a.w) - (b.x + b.w)).abs() <= 0.5 * h, "「{}」与「{}」的右缘要几乎齐（这是这条用例的前提）", a.text, b.text);
            assert!((b.y - (a.y + a.h)) <= 0.55 * h && (b.x - a.x).abs() <= 0.6 * h, "「{}」与「{}」要紧挨着、左对齐", a.text, b.text);
        }
        // 前提二：功能列表里有一行比其它宽出一大截（把列的最右缘撑开），并且每条都带逗号或本身很短
        let widest = ocr_line(&lines, "一目了然");
        let first = ocr_line(&lines, "中英互译");
        assert!(widest.w - first.w > 0.5 * widest.h.max(first.h), "功能列表的最后一条要明显比第一条宽：\n{text}");
        assert!(text.contains('，'), "功能列表里的逗号要读出来：\n{text}");

        let blocks = merge_lines(&lines, 1.0);
        assert_eq!(blocks.len(), lines.len(), "一行一块：\n{:?}", blocks.iter().map(|b| (b.lines, b.text.as_str())).collect::<Vec<_>>());
        assert!(blocks.iter().all(|block| block.lines == 1));
        assert_eq!(translate_target(&blocks), Some("英文"));
    }

    /// 英文截图里的图标、星级会被中文优先的那一遍读成汉字（见 ocr.swift 的 `pickRecognition`）：
    /// 不含中文的图必须用英文优先的那一遍，否则英文截图里会冒出汉字，翻译方向还可能被带偏。
    fn assert_english_screenshot_stays_english(fixture: &str, expected: &[&str]) {
        let lines = recognized_lines(fixture);
        let text = joined(&lines);
        let han = han_chars(&text);
        assert!(han.is_empty(), "英文截图 {fixture} 里冒出了汉字 {han:?}——多半是误用了中文优先的那一遍：\n{text}");
        for expected in expected {
            assert!(text.contains(expected), "{fixture} 的识别结果里没有「{expected}」：\n{text}");
        }
        assert_eq!(translate_target(&merge_lines(&lines, 1.0)), Some("中文"), "英文截图要译成中文：\n{text}");
    }

    #[test]
    fn real_ocr_does_not_invent_chinese_from_icons_in_an_english_screenshot() {
        // 中文优先时 Vision 会把菜单图标读成「三」、复选框读成「口」、日历图标读成「曲」、文档图标读成「目」
        if real_sidecar().is_none() {
            return;
        }
        assert_english_screenshot_stays_english(
            "ocr-en-icons.png",
            &["Menu", "Remember me", "Calendar", "Documents", "Release notes", "Favorites"],
        );
        // 数量步进器 `[−] 2 [+]`：中文优先把 `+ 2 −` 读成 `+乙一`（置信度 0.3），英文优先读成 `+ Z -`——对不上，但只有一行 0.3、
        // 不是整行方框字、也没有真词。要靠「置信度 0.3 是弱证据」才挡得住；调用处要是把置信度传丢了（当成 1.0），这行就成了强证据
        let stepper = joined(&recognized_lines("ocr-en-stepper.png"));
        assert!(han_chars(&stepper).is_empty() && stepper.contains("Quantity"), "数量步进器那一行不该冒汉字：\n{stepper}");
        // 只有成排的方框图标、一个字也没有：中文优先把它们读成「回回回」「回回回口回回」，每行置信度只有 0.3。
        // 英文优先在那里读出的是乱码字母，「解释得了」解释不了；要靠「置信度 0.3 的行要 ≥ 6 行、≥ 20 个汉字才算数」挡住
        let boxed = joined(&recognized_lines("ocr-en-icons-boxed.png"));
        let han = han_chars(&boxed);
        assert!(han.is_empty(), "纯图标的图里冒出了汉字 {han:?}——弱证据（置信度 0.3）不该被采信：\n{boxed}");
    }

    #[test]
    fn real_ocr_does_not_invent_chinese_from_star_ratings_in_an_english_screenshot() {
        // 星级、成排的图标会被中文优先读成连着的好几个汉字（`大實女實*`、`實責責次次`），光数「连续的汉字」分不开；
        // 要靠英文优先那一遍：它在同一位置读出符号，旁边的英文字母和中文优先那一遍一样
        if real_sidecar().is_none() {
            return;
        }
        assert_english_screenshot_stays_english(
            "ocr-en-stars.png",
            &["Excellent quality", "Good value", "battery life", "Reviewed in"],
        );
        // 选区只有一行星级 + 两个英文词时更要紧：中文优先读成 5 个汉字、字母只有 6 个，汉字 ×2 ≥ 字母，方向会被判成「英文」
        assert_english_screenshot_stays_english("ocr-en-stars-one-line.png", &["Not bad"]);
        // SVG 描边星：中文优先那一遍里有置信度 0.5 的星级行（`真真責众众 Okay, but …`）。没有「英文优先解释得了」这一层，
        // 一行 0.5 就够让整张图改用中文优先。上面两张的星级行都只有 0.3，被「弱证据要 6 行」挡住了，守不住这一层
        assert_english_screenshot_stays_english("ocr-en-stars-svg.png", &["Excellent quality", "Good value", "battery life"]);
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

    fn plain_block(text: &str) -> Block {
        Block { text: text.into(), ..Default::default() }
    }

    #[test]
    fn translations_are_applied_in_order_and_identical_ones_are_flagged_unchanged() {
        let mut blocks = vec![plain_block("New task"), plain_block("V5.9.3"), plain_block("git commit -m fix")];
        apply_translations(&mut blocks, vec![Ok("新建任务".into()), Ok("  V5.9.3 ".into()), Ok("git  commit -m fix".into())]);
        assert_eq!(blocks[0].text, "新建任务");
        assert!(blocks[0].translated && !blocks[0].unchanged);
        assert_eq!(blocks[1].text, "V5.9.3", "两头的空白去掉");
        assert!(blocks[1].translated && blocks[1].unchanged, "原样保留的版本号：不用重画");
        assert!(blocks[2].unchanged, "只差空白也算一样");
    }

    #[test]
    fn an_empty_translation_keeps_the_original_and_is_not_marked_translated() {
        let mut blocks = vec![plain_block("Library")];
        apply_translations(&mut blocks, vec![Ok("   ".into())]);
        assert_eq!(blocks[0].text, "Library");
        assert!(!blocks[0].translated, "没有译文就是没译，覆盖层要按「原文」画并带提示");
        assert!(!blocks[0].unchanged, "没译成不能当成「译文和原文一样」");
    }

    #[test]
    fn a_block_that_failed_keeps_its_original_while_its_neighbours_are_translated() {
        let mut blocks = vec![plain_block("Home"), plain_block("Docs"), plain_block("Blog")];
        apply_translations(&mut blocks, vec![Ok("首页".into()), Err(protocol::Why::Missing), Ok("博客".into())]);
        assert_eq!(blocks.iter().map(|block| block.text.as_str()).collect::<Vec<_>>(), ["首页", "Docs", "博客"]);
        assert_eq!(blocks.iter().map(|block| block.translated).collect::<Vec<_>>(), [true, false, true]);
        assert!(!blocks[1].unchanged, "没译成不能当成「译文和原文一样」：覆盖层会照原样盖住它");
    }

    fn outcome_of(translations: Vec<Result<String, protocol::Why>>) -> protocol::Outcome {
        protocol::Outcome { translations, requests: 1, retries: 0 }
    }

    #[test]
    fn a_partial_failure_tells_the_user_how_many_blocks_and_why() {
        assert_eq!(failure_notice(&outcome_of(vec![Ok("a".into()), Ok("b".into())])), None);
        let notice = failure_notice(&outcome_of(vec![Ok("a".into()), Err(protocol::Why::Missing), Err(protocol::Why::Missing)])).unwrap();
        assert!(notice.starts_with("有 2 块没能译出："), "{notice}");
        assert!(notice.contains("模型没有给出这几条的译文"), "{notice}");
        assert!(notice.contains("仍是原文"), "{notice}");
        // 原因是一句话时不重复句号
        let notice = failure_notice(&outcome_of(vec![Ok("a".into()), Err(protocol::Why::Request("请求太频繁了。".into()))])).unwrap();
        assert!(notice.contains("请求太频繁了。那几块") && !notice.contains("。。"), "{notice}");
    }

    #[test]
    fn the_translation_step_hands_the_overlay_its_failure_and_its_notice_in_the_right_slots() {
        // 全译成：什么都不加，位置提示（如果有）原样留着
        assert_eq!(settle(Ok(None), None), (None, None));
        assert_eq!(settle(Ok(None), Some("位置提示".into())), (None, Some("位置提示".into())));
        // 一部分没译成：那句话要交给覆盖层，排在位置提示前面
        assert_eq!(settle(Ok(Some("有 2 块没能译出。".into())), None), (None, Some("有 2 块没能译出。".into())));
        assert_eq!(
            settle(Ok(Some("有 2 块没能译出。".into())), Some("位置提示".into())),
            (None, Some("有 2 块没能译出。位置提示".into()))
        );
        // 整次失败：原因进失败槽，位置提示照旧
        assert_eq!(settle(Err("连不上".into()), Some("位置提示".into())), (Some("连不上".into()), Some("位置提示".into())));
    }

    #[test]
    fn local_providers_go_one_chunk_at_a_time_and_the_direction_follows_the_source_and_target() {
        use protocol::Direction::*;
        let (cloud_en, cloud_zh) = (delivery(ToEnglish, true), delivery(ToChinese, true));
        assert!(cloud_en.parallel && cloud_en.to_english == ToEnglish);
        assert!(cloud_zh.parallel && cloud_zh.to_english == ToChinese);
        let (local_en, local_zh) = (delivery(ToEnglish, false), delivery(ToChinese, false));
        assert!(!local_en.parallel && local_en.to_english == ToEnglish);
        assert!(!local_zh.parallel && local_zh.to_english == ToChinese);
    }

    #[test]
    fn the_alignment_check_is_only_on_for_the_two_directions_whose_length_ratio_was_measured() {
        use protocol::Direction::*;
        let direction = |text: &str, pref: &str| capture_plan(&one_block(text), pref).map(|(_, direction)| direction);
        let chinese = "这是一段中文，用来确认译成英文时的方向没有变。";
        let english = "This is a plain English sentence that has to be translated into Chinese by default.";
        // 默认方向：和以前一样开
        assert_eq!(direction(chinese, lang::AUTO), Some(ToEnglish));
        assert_eq!(direction(english, lang::AUTO), Some(ToChinese));
        // 中英以外的目标、或中英以外的原文：长度比没量过，不检
        assert_eq!(direction(chinese, "ja"), Some(Other));
        assert_eq!(direction(chinese, "zh-Hant"), Some(Other));
        assert_eq!(direction(english, "fr"), Some(Other));
        assert_eq!(direction("Où est la gare, s'il vous plaît? Je voudrais acheter un billet pour demain.", "en"), Some(Other));
        assert_eq!(direction("ありがとうございます、お願いします", lang::AUTO), Some(Other));
        assert_eq!(direction("Привет, как дела? Спасибо большое", lang::AUTO), Some(Other));
        assert_eq!(direction("12345 ——", lang::AUTO), None);
    }

    #[test]
    fn a_reply_cut_off_by_the_token_limit_is_marked_truncated() {
        assert!(reply_from("[".into(), Some("length")).truncated);
        assert!(!reply_from("[]".into(), Some("stop")).truncated);
        assert!(!reply_from("[]".into(), None).truncated);
        assert_eq!(reply_from("文本".into(), None).text, "文本");
    }

    #[test]
    fn only_a_screen_where_every_block_failed_is_a_total_failure() {
        use protocol::Why;
        assert!(!all_failed(&outcome_of(vec![])), "没有块就没东西可译");
        assert!(!all_failed(&outcome_of(vec![Ok("a".into()), Err(Why::Missing)])));
        assert!(all_failed(&outcome_of(vec![Err(Why::Missing), Err(Why::Empty)])));
    }

    #[test]
    fn the_prompt_is_the_wording_that_was_evaluated_byte_for_byte() {
        // 评测用的就是这两份原文；改措辞要连同 CHANGELOG 里的数据一起重新评测，再有意识地更新金标准文件
        assert_eq!(capture_system_prompt("英文"), include_str!("../tests/fixtures/capture-prompt.to-en.txt"));
        assert_eq!(capture_system_prompt("中文"), include_str!("../tests/fixtures/capture-prompt.to-zh.txt"));
    }

    /// tools/mock-provider.mjs 里 `MARKERS.capture` 的值：从那个文件里读，别在这里再写一遍字面量
    /// （写了，mockProvider.test.ts 里「标记出现在 capture.rs 里」的检查就会被这个单测自己满足而失效）。
    fn mock_capture_marker() -> String {
        let mock = include_str!("../../tools/mock-provider.mjs");
        let after = &mock[mock.find("capture: \"").expect("mock-provider.mjs 里有 MARKERS.capture") + "capture: \"".len()..];
        after[..after.find('"').unwrap()].to_string()
    }

    #[test]
    fn the_prompt_starts_with_the_marker_the_mock_provider_recognises() {
        let marker = mock_capture_marker();
        assert!(marker.chars().count() > 10, "标记读空了：{marker:?}");
        for target in ["英文", "中文"] {
            assert!(capture_system_prompt(target).starts_with(&marker), "{target}");
        }
    }

    #[test]
    fn the_prompt_asks_for_ids_and_does_not_pressure_the_model_to_hit_a_count_or_to_keep_everything() {
        let prompt = capture_system_prompt("中文");
        assert!(prompt.contains("\"i\":编号") && prompt.contains("编号照抄输入"));
        // 老协议下试过的「必须恰好 N 条」「不要删减信息」会诱导模型凑条数 / 并回邻条，见 CHANGELOG；别加回来。
        // 反过来「可以省略 / 概括」会让模型删正文（上一轮核对过现有措辞不删）：同样不要加
        for banned in ["恰好", "不要删减", "译全", "共 ", "N 条", "删减", "省略", "概括"] {
            assert!(!prompt.contains(banned), "提示词里不该有「{banned}」");
        }
        assert!(prompt.contains("翻译成中文") && capture_system_prompt("英文").contains("翻译成英文"));
        // 第 5 条：半句话也用自己的编号单独译（PDF 两栏页上量出来的，见 CHANGELOG）；它说的是每个编号各译各的，不是凑条数
        assert!(prompt.contains("5. 有的条目只是被折行或分栏拆开的半句话") && prompt.contains("不要并进相邻的条目"));
    }

    #[test]
    fn different_text_is_never_flagged_unchanged() {
        assert!(same_text("Scheduled", "  Scheduled\n"));
        assert!(!same_text("Scheduled", "scheduled"), "大小写不同算不同");
        assert!(!same_text("© Scheduled", "已计划"));
        assert!(!same_text("a b", "ab"), "空格不能被吃掉");
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
            ..Default::default()
        }];
        assert_eq!(translate_target(&english), Some("中文"));
    }

    fn one_block(text: &str) -> Vec<Block> {
        vec![Block {
            text: text.into(),
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            line_h: 10.0,
            translated: false,
            ..Default::default()
        }]
    }

    #[test]
    fn other_scripts_are_translated_instead_of_rejected() {
        // 以前：日文假名、俄文、韩文直接报「目前只支持中英互译」。现在默认译成中文
        for text in ["今日は天気がいいですね", "Привет, как дела?", "안녕하세요", "Bonjour tout le monde"] {
            assert_eq!(capture_target(&one_block(text), lang::AUTO).map(|lang| lang.code), Some("zh"), "{text}");
        }
        // 识别出来的全是数字、符号：没什么可译的
        assert_eq!(capture_target(&one_block("12345 ——"), lang::AUTO).map(|lang| lang.code), None);
        assert_eq!(capture_target(&one_block("12345 ——"), "ja").map(|lang| lang.code), None);
    }

    #[test]
    fn a_chosen_language_applies_to_screen_translation_too() {
        let english = one_block("The weather is nice and we can go out today.");
        let chinese = one_block("今天天气很好，我们可以出去玩。");
        assert_eq!(capture_target(&english, "ja").map(|lang| lang.name), Some("日语"));
        assert_eq!(capture_target(&chinese, "fr").map(|lang| lang.name), Some("法语"));
        assert_eq!(capture_target(&chinese, "zh-Hant").map(|lang| lang.name), Some("繁体中文"));
        // 选的就是页面本身的语言：译成自己没有意义，退回默认方向
        assert_eq!(capture_target(&english, "en").map(|lang| lang.name), Some("中文"));
        assert_eq!(capture_target(&chinese, "zh").map(|lang| lang.name), Some("英文"));
        // 认不得的偏好（配置被改坏）：当 auto
        assert_eq!(capture_target(&chinese, "klingon").map(|lang| lang.name), Some("英文"));
    }

    #[test]
    fn the_prompt_names_the_target_the_blocks_are_translated_into() {
        // 提示词里写的是语言的中文名；默认方向仍是以前的「中文」「英文」，不多不少
        let names: Vec<&str> = ["zh", "en", "ja", "zh-Hant"]
            .iter()
            .filter_map(|code| lang::lang_by_code(code))
            .map(|lang| lang.name)
            .collect();
        assert_eq!(names, ["中文", "英文", "日语", "繁体中文"]);
    }

    // ───────────────────────── 英文硬折行（`en_wrap`）：`merge_lines` 这一层 ─────────────────────────

    /// 一栏两端对齐的论文正文（几何照真 Vision 在 BERT 论文第一页上量到的形状取整）：x = 134、右缘 420、行高 16、行距 18，
    /// 一段九行（末行短），隔一行空隙后是下一段。
    fn paper_column() -> Vec<OcrLine> {
        let rows: [(&str, f64); 9] = [
            ("We introduce a new language representa-", 286.0),
            ("tion model called BERT, which stands for", 286.0),
            ("Bidirectional Encoder Representations from", 286.0),
            ("Transformers. Unlike recent language repre-", 286.0),
            ("sentation models (Peters et al., 2018a; Rad-", 284.0),
            ("ford et al., 2018), BERT is designed to pre-", 284.0),
            ("train deep bidirectional representations from", 286.0),
            ("unlabeled text by jointly conditioning on both", 286.0),
            ("left and right context in all layers.", 226.0),
        ];
        let mut lines: Vec<OcrLine> = rows.iter().enumerate().map(|(k, (text, w))| line(text, 134.0, 376.0 + 18.0 * k as f64, *w, 16.0)).collect();
        // 下一段：空一行
        lines.push(line("BERT is conceptually simple and empirically", 136.0, 376.0 + 18.0 * 9.0 + 8.0, 282.0, 16.0));
        lines.push(line("powerful. It obtains new state-of-the-art re-", 134.0, 376.0 + 18.0 * 10.0 + 8.0, 286.0, 16.0));
        lines.push(line("sults on eleven natural language processing", 134.0, 376.0 + 18.0 * 11.0 + 8.0, 284.0, 16.0));
        lines.push(line("tasks, including pushing the GLUE score.", 134.0, 376.0 + 18.0 * 12.0 + 8.0, 250.0, 16.0));
        lines
    }

    #[test]
    fn a_hard_wrapped_english_paragraph_becomes_one_block_and_the_next_paragraph_another() {
        let blocks = merge_lines(&paper_column(), 1.0);
        let summary: Vec<(u32, String)> = blocks.iter().map(|b| (b.lines, b.text.chars().take(24).collect())).collect();
        assert_eq!(summary.len(), 2, "两段：{summary:?}");
        assert_eq!(blocks[0].lines, 9);
        assert_eq!(blocks[1].lines, 4);
        // 连字符断开的词紧挨着接起来；其余隔一个空格；合并后是各行的并集，单行行高仍是一行
        assert!(blocks[0].text.starts_with("We introduce a new language representa-tion model called BERT, which stands for Bidirectional"), "{}", blocks[0].text);
        assert!(blocks[0].text.contains("repre-sentation models (Peters et al., 2018a; Rad-ford et al., 2018)"), "{}", blocks[0].text);
        assert!(blocks[0].text.ends_with("conditioning on both left and right context in all layers."), "{}", blocks[0].text);
        assert!(blocks[1].text.starts_with("BERT is conceptually simple and empirically powerful."), "{}", blocks[1].text);
        assert!((blocks[0].y - 376.0).abs() < 1e-9 && (blocks[0].h - (8.0 * 18.0 + 16.0)).abs() < 1e-9 && (blocks[0].line_h - 16.0).abs() < 1e-9, "{:?}", (blocks[0].y, blocks[0].h, blocks[0].line_h));
        assert_eq!(translate_target(&blocks), Some("中文"));
    }

    #[test]
    fn the_same_paragraph_in_double_pixels_and_in_any_input_order_gives_the_same_blocks() {
        let one_x = paper_column();
        let two_x: Vec<OcrLine> = one_x.iter().map(|l| OcrLine { x: l.x * 2.0, y: l.y * 2.0, w: l.w * 2.0, h: l.h * 2.0, ..l.clone() }).collect();
        assert_eq!(shape(&merge_lines(&one_x, 1.0)), shape(&merge_lines(&two_x, 2.0)));
        let mut shuffled = one_x.clone();
        shuffled.reverse();
        shuffled.swap(2, 7);
        assert_eq!(shape(&merge_lines(&shuffled, 1.0)), shape(&merge_lines(&one_x, 1.0)));
    }

    #[test]
    fn two_columns_and_a_vertical_stamp_do_not_get_in_each_others_way() {
        // 两栏的行按 y 交替，加上页边一条竖排的 arXiv 戳（框高 270）：每一栏各自成段，戳自己一块
        let left = ["Translating text that appears on a screen is harder than", "translating a document, because the text has already been", "laid out by a typesetting engine, not by the author."];
        let right = ["We recover whole paragraphs from the geometry of the", "recognized lines instead of the sentence that was cut in", "half by the typesetting engine of the screen."];
        let mut lines = vec![line("arXiv:1810.04805v2 [cs.CL] 24 May 2019", 26.0, 100.0, 28.0, 270.0)];
        for k in 0..3 {
            let w = if k < 2 { 420.0 } else { 330.0 };
            lines.push(line(left[k], 100.0, 100.0 + 18.0 * k as f64, w, 16.0));
            lines.push(line(right[k], 560.0, 101.0 + 18.0 * k as f64, w, 16.0));
        }
        let blocks = merge_lines(&lines, 1.0);
        assert_eq!(blocks.len(), 3, "{:?}", shape(&blocks));
        assert_eq!(blocks.iter().filter(|b| b.lines == 3).count(), 2);
        assert!(blocks.iter().any(|b| b.text.starts_with("Translating text") && b.text.ends_with("by the author.")));
        assert!(blocks.iter().any(|b| b.text.starts_with("We recover whole") && b.text.ends_with("of the screen.")));
        assert!(blocks.iter().any(|b| b.lines == 1 && b.text.starts_with("arXiv")));
    }

    #[test]
    fn a_line_vision_split_in_two_is_glued_back_in_reading_order_even_when_the_right_half_sorts_first() {
        // 第二行被拆成左右两段；右半截的 y 比左半截还小一点，按 y 排会排在前面——不能先处理，更不能把顺序弄反
        let lines = vec![
            line("Translating text that appears on a screen is harder than", 100.0, 100.0, 420.0, 16.0),
            line("has already been", 400.0, 117.5, 118.0, 16.0),
            line("translating a document, because the text", 100.0, 118.0, 296.0, 16.0),
            line("engine and not by the author, so a sentence is often.", 100.0, 136.0, 385.0, 16.0),
        ];
        let blocks = merge_lines(&lines, 1.0);
        assert_eq!(shape(&blocks), [(4, "Translating text that appears on a screen is harder than translating a document, because the text has already been engine and not by the author, so a sentence is often.")]);
        // 孤零零的相邻两项（没有任何折行的上下文）不拼：导航栏
        let nav = vec![line("Home", 100.0, 40.0, 50.0, 16.0), line("Products", 156.0, 40.0, 70.0, 16.0)];
        assert_eq!(merge_lines(&nav, 1.0).len(), 2);
    }

    #[test]
    fn headings_list_items_code_tables_and_menus_are_not_pulled_into_the_paragraph_around_them() {
        let lines = vec![
            line("1 Introduction", 100.0, 20.0, 130.0, 18.0),
            line("Translating text that appears on a screen is harder than", 100.0, 46.0, 420.0, 16.0),
            line("translating a document, because the text has been", 100.0, 64.0, 404.0, 16.0),
            line("laid out by a typesetting engine and not the author.", 100.0, 82.0, 380.0, 16.0),
            line("• Headings are never merged with the text below them.", 100.0, 116.0, 400.0, 16.0),
            line("• Every list item starts a new unit of its own, as shown.", 100.0, 134.0, 420.0, 16.0),
            line("$ ./tools/install-app.sh --release", 100.0, 168.0, 250.0, 16.0),
            line("$ open /Applications/SENSE.app", 100.0, 186.0, 220.0, 16.0),
            line("Timeout", 100.0, 220.0, 60.0, 16.0),
            line("Retries", 100.0, 238.0, 56.0, 16.0),
            line("Settings", 100.0, 270.0, 60.0, 16.0),
            line("Keyboard shortcuts", 100.0, 288.0, 130.0, 16.0),
            line("About", 100.0, 306.0, 40.0, 16.0),
        ];
        let blocks = merge_lines(&lines, 1.0);
        let summary = shape(&blocks);
        assert_eq!(blocks.len(), 11, "{summary:?}");
        assert_eq!(blocks.iter().filter(|b| b.lines == 3).count(), 1, "只有那一段正文是三行：{summary:?}");
        assert!(blocks.iter().any(|b| b.lines == 3 && b.text.starts_with("Translating text") && b.text.ends_with("not the author.")));
        assert!(blocks.iter().any(|b| b.lines == 1 && b.text == "1 Introduction"));
    }

    #[test]
    fn lines_with_cjk_punctuation_or_a_chinese_majority_stay_with_the_chinese_rules() {
        // 前两行由英文老路合上（下一行小写开头）；第三行只有靠英文硬折行才接得上——它含全角逗号，英文这条路就不认领
        let third = |text: &str| {
            vec![
                line("Translating text that appears on a screen is harder than", 100.0, 100.0, 420.0, 16.0),
                line("translating a document, because the text has already", 100.0, 118.0, 405.0, 16.0),
                line(text, 100.0, 136.0, 380.0, 16.0),
            ]
        };
        assert_eq!(merge_lines(&third("been laid out by a typesetting engine, not the author."), 1.0).len(), 1, "对照：纯 ASCII 的第三行接上");
        assert_eq!(merge_lines(&third("been laid out by a typesetting engine， not the author."), 1.0).len(), 2, "含全角逗号：不接");
        assert_eq!(merge_lines(&third("been laid out by a typesetting engine（not the author）"), 1.0).len(), 2, "含全角括号：不接");
        // 英文段落和中文段落同在一页：各走各的，汉字之间不加空格
        let page = vec![
            line("Translating text that appears on a screen is harder than", 10.0, 10.0, 420.0, 16.0),
            line("translating a document, because the text has already", 10.0, 28.0, 405.0, 16.0),
            line("been laid out by a typesetting engine and not the author.", 10.0, 46.0, 380.0, 16.0),
            line("为了保护你的隐私，所有的文字识别都在本机完成，图", 500.0, 10.0, 358.0, 20.0),
            line("片不会离开你的电脑。只有识别出来的文字会发送给你", 500.0, 34.0, 356.0, 22.0),
            line("选择的翻译服务，而密钥只保存在系统钥匙串里。", 500.0, 60.0, 324.0, 20.0),
        ];
        let blocks = merge_lines(&page, 1.0);
        assert_eq!(blocks.iter().map(|b| b.lines).collect::<Vec<_>>(), [3, 3], "{:?}", shape(&blocks));
        assert!(blocks[1].text.starts_with("为了保护你的隐私，所有的文字识别都在本机完成，图片不会离开你的电脑。"), "汉字之间不加空格：{}", blocks[1].text);
    }

    #[test]
    fn a_merged_english_block_unions_the_geometry_and_keeps_the_first_lines_style() {
        let ink = |x: f64, y: f64, w: f64| OcrInk { x, y, w, h: 14.0 };
        let mut lines = vec![
            styled("Translating text that appears on a screen is harder than", 100.0, 100.0, 420.0, 16.0, ink(101.0, 101.0, 418.0), 80.0),
            styled("translating a document, because the text has been", 100.0, 118.0, 405.0, 16.0, ink(101.0, 119.0, 403.0), 30.0),
            styled("laid out by a typesetting engine and not the author.", 100.0, 136.0, 380.0, 16.0, ink(101.0, 137.0, 378.0), 55.0),
        ];
        for l in lines.iter_mut() {
            l.words = l.text.split_whitespace().map(|word| OcrWord { t: word.to_string(), w: word.chars().count() as f64 * 7.0 }).collect();
        }
        let blocks = merge_lines(&lines, 1.0);
        assert_eq!(blocks.len(), 1);
        let block = &blocks[0];
        assert_eq!(block.lines, 3);
        assert_eq!((block.x, block.y, block.w, block.h, block.line_h), (100.0, 100.0, 420.0, 52.0, 16.0));
        assert_eq!(block.ink, Some(Rect { x: 101.0, y: 101.0, w: 418.0, h: 50.0 }));
        assert_eq!(block.room, Some(30.0), "右边的空白取各行里最窄的");
        assert_eq!(block.bg.as_deref(), Some("#1e1e1e"));
        assert_eq!(block.fg.as_deref(), Some("#c8c9ca"));
        assert_eq!(block.words.len(), lines.iter().map(|l| l.words.len()).sum::<usize>(), "逐词框接上");
    }

    fn assert_block_with<'a>(blocks: &'a [Block], needle: &str) -> &'a Block {
        blocks.iter().find(|block| block.text.contains(needle)).unwrap_or_else(|| panic!("没有哪一块里有「{needle}」：{:?}", shape(blocks)))
    }

    #[test]
    fn real_ocr_lines_of_a_two_column_english_paper_become_one_block_per_paragraph() {
        // 真 Vision 读两栏、两端对齐、带连字符断词和引用链接的英文页：每段合成一块，标题、列表项、图注各自成块，
        // 左右两栏的行不串。断言只认短锚点与块的结构，不比整行文字（Vision 偶尔读歪一两个字不该让它变红）
        if real_sidecar().is_none() {
            return;
        }
        let lines = recognized_lines("ocr-en-paper.png");
        let text = joined(&lines);
        assert!(lines.len() >= 18, "这一页至少读出 18 行，只有 {} 行：\n{text}", lines.len());
        let blocks = merge_lines(&lines, 1.0);
        let summary = || shape(&blocks);
        // 每段的开头和末尾的锚点落在同一块里
        for (first, last, at_least) in [("Translating text", "phrase", 4), ("A translator", "instead", 5), ("Two lines belong", "at all", 4), ("Hyphenated", "wrote", 2), ("Figure 1", "translation", 2)] {
            let block = assert_block_with(&blocks, first);
            assert!(block.text.contains(last) && block.lines >= at_least, "「{first}」到「{last}」应当合成一块、至少 {at_least} 行：{:?}", summary());
        }
        // 前提与护栏：标题、列表项各自一块；没有哪一块把两栏的内容拼在一起
        for single in ["Reading Paragraphs", "1 Introduction", "2 Method", "Every list item", "Headings are never"] {
            assert_eq!(assert_block_with(&blocks, single).lines, 1, "「{single}」自己一块：{:?}", summary());
        }
        assert!(blocks.iter().all(|b| !(b.text.contains("Translating text") && b.text.contains("Two lines belong"))), "左右两栏不串：{:?}", summary());
        // 连字符断开的词紧挨着（`recog-` + `nized`、`sen-` + `tence`）
        let first = assert_block_with(&blocks, "Translating text");
        assert!(first.text.contains("sen-tence"), "连字符断词紧挨着：{}", first.text);
        assert_eq!(translate_target(&blocks), Some("中文"));
    }

    #[test]
    fn real_ocr_lines_of_a_hard_wrapped_monospace_readme_merge_paragraphs_but_not_code_or_table_cells() {
        // 等宽字体、按字符数硬折的 README：段落合并，缩进的列表项连着它的续行，代码行与表格单元格各自一块
        if real_sidecar().is_none() {
            return;
        }
        let lines = recognized_lines("ocr-en-readme.png");
        let text = joined(&lines);
        assert!(lines.len() >= 17, "这一页至少读出 17 行，只有 {} 行：\n{text}", lines.len());
        let blocks = merge_lines(&lines, 1.0);
        let summary = || shape(&blocks);
        for (first, last, at_least) in [("Screenshot translation", "pasted", 3), ("Download the latest", "your Mac", 2), ("The key never", "copy of it", 2)] {
            let block = assert_block_with(&blocks, first);
            assert!(block.text.contains(last) && block.lines >= at_least, "「{first}」到「{last}」应当合成一块、至少 {at_least} 行：{:?}", summary());
        }
        for single in ["Installation", "Grant the screen", "install-app", "open /Applications", "retries"] {
            assert_eq!(assert_block_with(&blocks, single).lines, 1, "「{single}」自己一块：{:?}", summary());
        }
        // 表格里同一列的两个单元格（3 个词和 4 个词，几何上像折行）不并到一起。不要求它们各自单独成块：表头 `Notes` 与 `Name` / `Value`
        // 的 y 只差 1 px，排序一变，老路（不是新判据）就会把 `Notes` 并上 `seconds, per request`——读数抖动 1 px 时有一成的概率
        let position = |anchor: &str| blocks.iter().position(|block| block.text.contains(anchor)).unwrap_or_else(|| panic!("没有包含「{anchor}」的块：{:?}", summary()));
        assert_ne!(position("seconds, per"), position("before giving"), "两个单元格不能合成一块：{:?}", summary());
        assert_eq!(translate_target(&blocks), Some("中文"));
    }

    /// 一个小的线性同余随机数发生器（测试里要可复现，不引依赖）。
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n.max(1)
        }

        fn unit(&mut self) -> f64 {
            (self.next() % 10_000) as f64 / 10_000.0
        }
    }

    #[test]
    fn random_pages_never_panic_and_every_non_empty_line_lands_in_exactly_one_block() {
        // 三千页随机版面：英文 / 中文 / 混排 / 列表符号 / 空白 / 非有限数 / 零宽零高 / 巨大坐标。要求：不 panic、行数守恒、每块至少一行、
        // 合并后的块框里没有 NaN（输入的框都是有限数时）、结果与输入顺序无关（同一页打乱后块数一样）
        let words = [
            "the", "of", "and", "to", "a", "in", "is", "that", "Translating", "text", "screen", "layout", "inter-", "national", "e.g.", "et", "al.", "(Smith", "2018)", "1.", "•", "-", "7.2", "//", "#", "x_y", "a=b;", "->", "你好", "，", "翻译。", "，。", "SENSE", "https://x.org/a", "$", "~", "[12]", "(", ")", "“quoted”",
        ];
        let mut rng = Lcg(20261101);
        for page in 0..3000 {
            let count = rng.below(40) as usize;
            let finite = page % 7 != 0;
            let lines: Vec<OcrLine> = (0..count)
                .map(|_| {
                    let n = rng.below(12) + 1;
                    let text = if rng.below(25) == 0 { "   ".to_string() } else { (0..n).map(|_| words[rng.below(words.len() as u64) as usize]).collect::<Vec<_>>().join(" ") };
                    let column = rng.below(3) as f64 * 520.0;
                    let mut l = line(&text, column + rng.below(40) as f64, rng.below(900) as f64 + rng.unit(), 40.0 + rng.below(420) as f64, 6.0 + rng.below(30) as f64);
                    if !finite {
                        match rng.below(9) {
                            0 => l.x = f64::NAN,
                            8 => l.y = f64::NAN,
                            1 => l.y = f64::INFINITY,
                            2 => l.w = f64::NEG_INFINITY,
                            3 => l.h = f64::NAN,
                            4 => l.w = 0.0,
                            5 => l.h = 0.0,
                            6 => l.x = 1e300,
                            _ => l.y = -1e300,
                        }
                    }
                    if rng.below(3) == 0 {
                        l.fg = Some(vec![rng.below(256) as f64, rng.below(256) as f64, rng.below(256) as f64]);
                    }
                    if rng.below(3) == 0 {
                        l.words = text.split_whitespace().map(|word| OcrWord { t: word.to_string(), w: word.chars().count() as f64 * (3.0 + rng.below(8) as f64) }).collect();
                    }
                    l
                })
                .collect();
            let scale = if page % 5 == 0 { 2.0 } else { 1.0 };
            let blocks = merge_lines(&lines, scale);
            let non_empty = lines.iter().filter(|l| !l.text.trim().is_empty()).count();
            assert_eq!(blocks.iter().map(|b| b.lines as usize).sum::<usize>(), non_empty, "第 {page} 页：行数守恒");
            assert!(blocks.iter().all(|b| b.lines >= 1 && !b.text.trim().is_empty()), "第 {page} 页：每块至少一行、有字");
            if finite {
                assert!(blocks.iter().all(|b| [b.x, b.y, b.w, b.h, b.line_h].iter().all(|v| v.is_finite())), "第 {page} 页：框是有限数");
                let mut shuffled = lines.clone();
                shuffled.reverse();
                let again = merge_lines(&shuffled, scale);
                // y 相同的行顺序可以不同（排序是稳定的）：只要页上没有 y 完全相同的行，结果就该一样
                let mut ys: Vec<u64> = lines.iter().map(|l| l.y.to_bits()).collect();
                ys.sort();
                ys.dedup();
                if ys.len() == lines.len() {
                    assert_eq!(shape(&blocks), shape(&again), "第 {page} 页：与输入顺序无关");
                }
            }
        }
    }

    // ───────────────────────── 评审补的用例（接线层：崩溃、碎片顺序、字色、倍数、已知取舍） ─────────────────────────

    #[test]
    fn a_visual_row_split_into_growing_fragments_does_not_panic_when_a_taller_line_follows() {
        // 评审 A 构造的输入：同一视觉行被拆成四段，高度 8 / 12 / 18 / 27（相邻一对的比值 ≤ 1.5，所以都算同一行的碎片），底边对齐；
        // 下一行高 48、顶比最左碎片还高。最左碎片（行头）按 y 排在下一行后面，处理下一行时它的块还没有——以前这里越界 panic（release 下整个应用退出）
        let bottom = 200.0;
        let page = vec![
            line("Translating text", 100.0, bottom - 8.0, 70.0, 8.0),
            line("that appears on a", 172.0, bottom - 12.0, 80.0, 12.0),
            line("screen is harder than", 254.0, bottom - 18.0, 100.0, 18.0),
            line("translating", 356.0, bottom - 27.0, 110.0, 27.0),
            line("a document because the text has already been laid out by a typesetting", 100.0, 190.0, 366.0, 48.0),
        ];
        let blocks = merge_lines(&page, 1.0);
        assert_eq!(blocks.iter().map(|block| block.lines as usize).sum::<usize>(), 5, "行数守恒：{:?}", shape(&blocks));
    }

    #[test]
    fn random_fragment_chains_never_panic_and_every_line_lands_in_exactly_one_block() {
        // 每一行由 1–3 段碎片组成（碎片的高度、竖直错位、间隙都随机，跨过各条阈值的两侧），行与行的高度、间距也随机；四分之一的页再加一条
        // 「高度逐段变大」的碎片串，后面跟一行更高的。专门撞「处理顺序」「碎片并入」「同列上一行」这几处——上一个随机用例里各行的框互相独立，几乎不出碎片
        let snippets = [
            "translating a document, because the text has already been",
            "laid out by a typesetting engine and not by the author, so a",
            "sentence is often cut in the middle of a phrase and",
            "which is why the",
            "of the model that we",
            "Alice and Bob wrote the",
            "the second moment estimate is therefore a poor approximation to the",
        ];
        let mut rng = Lcg(20261102);
        for page in 0..4000 {
            let mut lines: Vec<OcrLine> = Vec::new();
            let right_edge = 400.0 + rng.below(200) as f64;
            let mut y = 100.0;
            for _ in 0..2 + rng.below(8) {
                let h = 8.0 + rng.below(36) as f64;
                let segments = 1 + rng.below(3);
                let mut x = 100.0 + (rng.below(5) as f64 - 2.0) * h * 0.3;
                let justified = rng.below(2) == 0;
                for segment in 0..segments {
                    let text = snippets[rng.below(snippets.len() as u64) as usize];
                    let segment_h = h * (0.6 + rng.unit());
                    let segment_y = y + (rng.unit() - 0.5) * h * 0.9;
                    let w = if segment + 1 == segments && justified { (right_edge - x).max(30.0) } else { 40.0 + rng.below(160) as f64 };
                    lines.push(line(text, x, segment_y, w, segment_h));
                    x += w + (rng.unit() * 1.1 - 0.35) * h;
                }
                y += h * (0.9 + rng.unit() * 0.7);
            }
            if page % 4 == 0 {
                // 评审 A 构造的那种：四段碎片，高度按 1.35–1.5 倍逐段变大、底边对齐，后面一行更高、右缘与碎片串的右缘对齐、顶压在最左碎片上面
                let bottom = y + 40.0;
                let growth = 1.35 + rng.unit() * 0.15;
                let mut h = 6.0 + rng.below(7) as f64;
                let mut x = 100.0;
                for (segment, (text, width)) in [("Translating text", 70.0), ("that appears on a", 80.0), ("screen is harder than", 100.0), ("translating", 110.0)].iter().enumerate() {
                    let w = width * (0.9 + rng.unit() * 0.2);
                    lines.push(line(text, x, bottom - h, w, h));
                    x += w + 2.0 + rng.unit() * 4.0;
                    if segment < 3 {
                        h *= growth;
                    }
                }
                let tall = h * (1.5 + rng.unit() * 0.5);
                lines.push(line("a document because the text has already been laid out by a typesetting", 100.0, bottom - 8.0 - rng.below(6) as f64, x - 100.0, tall));
            }
            let blocks = merge_lines(&lines, if page % 3 == 0 { 2.0 } else { 1.0 });
            assert_eq!(blocks.iter().map(|block| block.lines as usize).sum::<usize>(), lines.len(), "第 {page} 页：行数守恒");
        }
    }

    #[test]
    fn independent_bracket_prefixed_log_lines_stay_separate_blocks() {
        // 四条独立的 Maven 日志（几何取自真 Vision 对渲染页的识别）。HEAD 是 4 块；`[` 开头曾被当成「小写开头」的证据，合成了 2 块
        let lines = vec![
            line("[INFO] Downloading from central the artifact for the plugin", 24.0, 134.0, 462.0, 16.0),
            line("[INFO] Compiling 14 source files to the target directory", 26.0, 152.0, 438.0, 14.0),
            line("[INFO] Building demo-app 1.0-SNAPSHOT", 24.0, 170.0, 292.0, 16.0),
            line("[INFO] Tests run 12 Failures 0 Errors 0 Skipped 0 in the module", 24.0, 188.0, 494.0, 14.0),
        ];
        assert_eq!(merge_lines(&lines, 1.0).len(), 4, "{:?}", shape(&merge_lines(&lines, 1.0)));
    }

    #[test]
    fn a_line_vision_split_in_three_is_glued_left_to_right_whatever_the_input_order() {
        // 一条视觉行被拆成左 / 中 / 右三段，输入里右段排在中段前面：文字要按 左 中 右 拼，再接下一行
        let mut lines = vec![
            line("Translating text", 100.0, 100.0, 112.0, 16.0),
            line("that appears on a", 216.0, 100.0, 119.0, 16.0),
            line("screen is harder than", 339.0, 100.0, 147.0, 16.0),
            line("translating a document, because the text has already been", 100.0, 118.0, 399.0, 16.0),
        ];
        lines.swap(1, 2);
        let blocks = merge_lines(&lines, 1.0);
        assert_eq!(shape(&blocks), vec![(4, "Translating text that appears on a screen is harder than translating a document, because the text has already been")]);
    }

    #[test]
    fn the_text_color_the_helper_measured_reaches_the_english_wrap_check() {
        // 下一行大写开头，老路不会管它；上一行断在连接词上：只有英文硬折行这一条在判，字色差得远（页脚小字、灰色摘要）就不并
        let merged = |first: f64, second: f64| {
            let mut upper = line("Translating text that appears on a screen is harder than", 100.0, 100.0, 420.0, 16.0);
            let mut lower = line("Documents laid out by an engine, because the text has been", 100.0, 118.0, 380.0, 16.0);
            upper.fg = Some(vec![first; 3]);
            lower.fg = Some(vec![second; 3]);
            merge_lines(&[upper, lower], 1.0).len()
        };
        assert_eq!(merged(20.0, 20.0), 1, "同色：并");
        assert_eq!(merged(20.0, 70.0), 1, "亮度差 50：并");
        assert_eq!(merged(20.0, 150.0), 2, "灰字：不并");
    }

    #[test]
    fn english_wrap_geometry_and_word_boxes_are_divided_by_the_scale() {
        // 词框 `OcrWord.w` 和行框一样是物理像素，2 倍图要除以倍数。第三行的第一个词（`laid`）在 1 倍图上宽 28，放得下上一行剩下的 40：不并；
        // 词框没除以倍数（56）就成了「放不下」，把第三行并进来
        let words = |text: &str, scale: f64| -> Vec<OcrWord> { text.split_whitespace().map(|word| OcrWord { t: word.to_string(), w: word.chars().count() as f64 * 7.0 * scale }).collect() };
        let page = |scale: f64| -> Vec<OcrLine> {
            [
                ("Translating text that appears on a screen is harder than", 100.0, 100.0, 420.0),
                ("translating a document, because the text has already been", 100.0, 118.0, 380.0),
                ("laid out by a typesetting engine and not by the author.", 100.0, 136.0, 300.0),
            ]
            .iter()
            .map(|(text, x, y, w)| {
                let mut l = line(text, x * scale, y * scale, w * scale, 16.0 * scale);
                l.words = words(text, scale);
                l
            })
            .collect()
        };
        let one = merge_lines(&page(1.0), 1.0);
        let two = merge_lines(&page(2.0), 2.0);
        assert_eq!(shape(&one), shape(&two));
        assert_eq!(one.iter().map(|block| block.lines).collect::<Vec<_>>(), vec![2, 1], "{:?}", shape(&one));
    }

    #[test]
    fn known_limit_a_pair_the_old_rule_merged_is_split_when_the_line_above_joined_a_paragraph() {
        // 已知的取舍（CHANGELOG「相对 HEAD 丢掉的合并」）。HEAD 是 [1 2][3 4]；现在 1–3 先并成一段，块变高，老路认不出第 4 行，
        // 而第 4 行字母不到一半、新判据又不认领它。补回来要让新判据对「老路本来会合」的行对兜底——会带回老路的错合，没做。改了这里要同时改文档
        let lines = vec![
            line("Translating text that appears on a screen is harder than", 100.0, 100.0, 420.0, 16.0),
            line("translating a document, because the text has already been", 100.0, 118.0, 420.0, 16.0),
            line("laid out by a typesetting engine and not by the author, so", 100.0, 136.0, 420.0, 16.0),
            line("a size is in (224, 256, 384, 480, 640}).", 100.0, 154.0, 250.0, 16.0),
        ];
        let blocks = merge_lines(&lines, 1.0);
        assert_eq!(blocks.iter().map(|block| block.lines).collect::<Vec<_>>(), vec![3, 1], "{:?}", shape(&blocks));
    }

    #[test]
    fn known_limit_a_heading_the_old_rule_joins_to_a_lowercase_first_line_pulls_the_whole_paragraph_in() {
        // 已知的取舍（评审 A 的 P1-3）。HEAD 的老路只能两两合并，「标题 + 小写开头的首行」这个错合的伤害限在 2 行；
        // 现在后面每一行都是「同列上一行」，整段跟着进了标题那一块（几何取自真 Vision）。改了这里要同时改文档
        let lines = vec![
            line("Introduction to the conll-2003 shared task: Language-independent", 24.0, 234.0, 484.0, 16.0),
            line("iPhone we plan to extend the Transformer to problems involving input and output", 24.0, 256.0, 556.0, 20.0),
            line("modalities other than text and to investigate local, restricted attention", 22.0, 278.0, 480.0, 23.0),
            line("mechanisms to efficiently handle large inputs and outputs such as images, audio", 22.0, 300.0, 554.0, 22.0),
            line("and video. We adopt Faster R-CNN [32] as the detection method. For NER, we", 24.0, 320.0, 538.0, 22.0),
            line("report both fine-tuning and feature-based approaches, as we expect the", 22.0, 346.0, 500.0, 18.0),
        ];
        let blocks = merge_lines(&lines, 1.0);
        assert_eq!(blocks.iter().map(|block| block.lines).collect::<Vec<_>>(), vec![6], "{:?}", shape(&blocks));
    }
}
