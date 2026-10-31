//! 窗口生命周期：显隐切换、失焦隐藏、全局快捷键，以及红黄绿灯 / 边缘缩放要用的窗口命令。
//!
//! 所有窗口操作都在 Rust 侧完成；前端只调这里的命令（`hide_window`、`quit_app`、`toggle_zoom`、
//! `get_window_frame`、`set_window_frame`），不直接调窗口 API，所以也不需要窗口写权限。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewWindow};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};

use crate::settings::{self, DEFAULT_HOTKEY_SPEC};

/// 主窗口 label，与 tauri.conf.json 保持一致。
pub const MAIN_WINDOW: &str = "main";

/// 默认键帽，与 `DEFAULT_HOTKEY_SPEC` 对应；有断言守护。
pub const DEFAULT_HOTKEY_DISPLAY: &str = "⌃⌥⌘Space";

/// 截屏翻译的默认快捷键：⌃⌥⌘X。
/// 选它的理由：不在 macOS 符号快捷键表里，也不与微信/QQ/系统截屏（⇧⌘4）冲突。
pub const DEFAULT_CAPTURE_SPEC: &str = "ctrl+alt+cmd+x";

/// 一个**已经注册成功**的快捷键：键本身 + 给人看的键帽。
#[derive(Clone)]
struct ActiveKey {
    shortcut: Shortcut,
    display: String,
}

/// 已注册的两个快捷键，也是「当前实际生效的是什么」唯一的来源。
/// 改键时要先 unregister 旧的、注册失败还要能回滚；设置页、标题栏键帽都读这里，
/// 不读配置文件——配置里写的键可能根本没注册上。
#[derive(Default)]
pub struct HotkeyState {
    lookup: Mutex<Option<ActiveKey>>,
    capture: Mutex<Option<ActiveKey>>,
}

impl HotkeyState {
    fn slot(&self, kind: &str) -> &Mutex<Option<ActiveKey>> {
        if kind == "capture" {
            &self.capture
        } else {
            &self.lookup
        }
    }

    pub fn get(&self, kind: &str) -> Option<Shortcut> {
        self.slot(kind)
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|active| active.shortcut))
    }

    /// 实际生效的键帽；没注册上就是 None。
    pub fn display(&self, kind: &str) -> Option<String> {
        self.slot(kind)
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|active| active.display.clone()))
    }

    pub fn remember(&self, kind: &str, shortcut: Shortcut, display: &str) {
        if let Ok(mut slot) = self.slot(kind).lock() {
            *slot = Some(ActiveKey {
                shortcut,
                display: display.to_string(),
            });
        }
    }

    /// 旧键装不回去时用：别再宣称它生效。
    pub fn forget(&self, kind: &str) {
        if let Ok(mut slot) = self.slot(kind).lock() {
            *slot = None;
        }
    }

    fn other(&self, kind: &str) -> Option<Shortcut> {
        self.get(if kind == "capture" { "lookup" } else { "capture" })
    }
}

/// 启动时注册一个快捷键的结果。
pub struct Startup {
    /// 实际生效的键帽；两个都注册不上时为 None
    pub active: Option<String>,
    /// 配置的键没注册上，生效的是回退键
    pub fell_back: bool,
    /// 按先后顺序的失败原因，给日志用
    pub errors: Vec<String>,
}

/// 启动时注册：配置的键注册不上就退回 `fallback`（可以没有）。
///
/// 成功注册的那个——无论哪一个——都记进 `HotkeyState`。以前回退成功后没记，
/// 之后在设置里改键，`set_hotkey` 取不到旧键，回退键就一直占着，两个组合同时能呼出窗口。
pub fn register_with_fallback(
    state: &HotkeyState,
    kind: &str,
    configured: Hotkey,
    fallback: Option<Hotkey>,
    mut register: impl FnMut(Shortcut) -> Result<(), String>,
) -> Startup {
    let mut errors = Vec::new();
    match register(configured.shortcut) {
        Ok(()) => {
            state.remember(kind, configured.shortcut, &configured.display);
            return Startup {
                active: Some(configured.display),
                fell_back: false,
                errors,
            };
        }
        Err(err) => errors.push(err),
    }
    if let Some(fallback) = fallback {
        match register(fallback.shortcut) {
            Ok(()) => {
                state.remember(kind, fallback.shortcut, &fallback.display);
                return Startup {
                    active: Some(fallback.display),
                    fell_back: true,
                    errors,
                };
            }
            Err(err) => errors.push(err),
        }
    }
    Startup {
        active: None,
        fell_back: false,
        errors,
    }
}

/// 改一个快捷键：解析 → 查重 → 换注册 → 落盘。
///
/// 注册失败会把旧键装回去，并且返回可读的原因（最常见是被别的 App 占了）。
#[tauri::command]
pub fn set_hotkey(
    app: AppHandle,
    state: tauri::State<'_, HotkeyState>,
    kind: String,
    spec: String,
) -> Result<HotkeyView, String> {
    if kind != "lookup" && kind != "capture" {
        return Err("未知的快捷键类型".to_string());
    }

    let parsed = parse_hotkey(&spec)?;

    if let Some(other) = state.other(&kind) {
        if other.to_string() == parsed.shortcut.to_string() {
            return Err("这个组合已经分配给另一个功能了，换一个".to_string());
        }
    }

    let plugin = app.global_shortcut();
    let previous = state.get(&kind);
    if let Some(shortcut) = previous {
        // 注销失败很少见，但失败了旧键就还占着：不能装作没事，至少留下日志
        if let Err(err) = plugin.unregister(shortcut) {
            eprintln!("[sense] 注销旧的 {kind} 快捷键失败：{err}");
        }
    }

    match crate::register_hotkey(&app, &kind, parsed.shortcut) {
        Ok(()) => {
            state.remember(&kind, parsed.shortcut, &parsed.display);
            // 标题栏的键帽只在启动时取过一次：告诉界面重新取
            let _ = app.emit("sense://hotkey-changed", &kind);
            let key = if kind == "capture" {
                "capture_hotkey"
            } else {
                "hotkey"
            };
            let value = spec.trim().to_string();
            settings::update(&app, |map| {
                map.insert(key.to_string(), serde_json::json!(value));
            })
            .map_err(|err| format!("键已生效，但写配置失败：{err}"))?;

            println!("[sense] hotkey updated: {kind} → {}", parsed.display);
            Ok(HotkeyView {
                kind,
                display: parsed.display,
                active: true,
            })
        }
        Err(err) => {
            if let Some(shortcut) = previous {
                // 旧键装不回去就别再说它生效
                if crate::register_hotkey(&app, &kind, shortcut).is_err() {
                    state.forget(&kind);
                    let _ = app.emit("sense://hotkey-changed", &kind);
                }
            }
            Err(format!("这个组合注册不上（可能被别的 App 占了）：{err}"))
        }
    }
}

/// 刚显示就被判为失焦的宽限期。
///
/// macOS 上 `show()` 与焦点事件存在竞态：窗口刚出现时可能先收到一次
/// `Focused(false)`。宽限期内不隐藏，但要**延迟复查**——直接丢弃这次失焦，
/// 窗口会永远钉在所有 App 之上。
const BLUR_GRACE: Duration = Duration::from_millis(260);

const LETTERS: [Code; 26] = [
    Code::KeyA,
    Code::KeyB,
    Code::KeyC,
    Code::KeyD,
    Code::KeyE,
    Code::KeyF,
    Code::KeyG,
    Code::KeyH,
    Code::KeyI,
    Code::KeyJ,
    Code::KeyK,
    Code::KeyL,
    Code::KeyM,
    Code::KeyN,
    Code::KeyO,
    Code::KeyP,
    Code::KeyQ,
    Code::KeyR,
    Code::KeyS,
    Code::KeyT,
    Code::KeyU,
    Code::KeyV,
    Code::KeyW,
    Code::KeyX,
    Code::KeyY,
    Code::KeyZ,
];

const DIGITS: [Code; 10] = [
    Code::Digit0,
    Code::Digit1,
    Code::Digit2,
    Code::Digit3,
    Code::Digit4,
    Code::Digit5,
    Code::Digit6,
    Code::Digit7,
    Code::Digit8,
    Code::Digit9,
];

/// 解析结果：可注册的快捷键 + 给人看的键帽。
pub struct Hotkey {
    pub shortcut: Shortcut,
    pub display: String,
}

/// 把 `ctrl+alt+cmd+space` 这样的字符串解析成快捷键。
///
/// 支持的修饰键：`ctrl` / `alt`(option) / `shift` / `cmd`(command)。
/// 支持的主键：`space`、单字母、数字。
/// 强制要求至少一个 `ctrl` 或 `cmd`：只有 shift/alt 的组合会抢走全局输入
/// （例如 `shift+space` 会让所有输入框打不出不换行空格）。
pub fn parse_hotkey(spec: &str) -> Result<Hotkey, String> {
    let mut mods = Modifiers::empty();
    let mut key: Option<(Code, String)> = None;

    for raw in spec.split('+') {
        let token = raw.trim().to_ascii_lowercase();
        match token.as_str() {
            "" => return Err(format!("快捷键 `{spec}` 里有空的按键片段")),
            "ctrl" | "control" => mods |= Modifiers::CONTROL,
            "alt" | "opt" | "option" => mods |= Modifiers::ALT,
            "shift" => mods |= Modifiers::SHIFT,
            "cmd" | "command" | "super" | "meta" => mods |= Modifiers::SUPER,
            other => {
                if key.is_some() {
                    return Err(format!("快捷键 `{spec}` 里有多个主键"));
                }
                key = Some(parse_key(other)?);
            }
        }
    }

    let (code, label) = key.ok_or_else(|| format!("快捷键 `{spec}` 缺少主键，例如 space 或 k"))?;
    if !mods.contains(Modifiers::CONTROL) && !mods.contains(Modifiers::SUPER) {
        return Err(format!("快捷键 `{spec}` 至少要有 ⌃ 或 ⌘，否则会抢走全局输入"));
    }

    Ok(Hotkey {
        shortcut: Shortcut::new(Some(mods), code),
        display: render(mods, &label),
    })
}

fn parse_key(token: &str) -> Result<(Code, String), String> {
    if token == "space" {
        return Ok((Code::Space, "Space".to_string()));
    }
    let mut chars = token.chars();
    let ch = chars.next().ok_or_else(|| "空主键".to_string())?;
    if chars.next().is_some() {
        return Err(format!("不支持的按键 `{token}`"));
    }
    let upper = ch.to_ascii_uppercase();
    match upper {
        'A'..='Z' => Ok((LETTERS[(upper as u8 - b'A') as usize], upper.to_string())),
        '0'..='9' => Ok((DIGITS[(upper as u8 - b'0') as usize], upper.to_string())),
        _ => Err(format!("不支持的按键 `{token}`")),
    }
}

fn render(mods: Modifiers, label: &str) -> String {
    let mut out = String::new();
    if mods.contains(Modifiers::CONTROL) {
        out.push('⌃');
    }
    if mods.contains(Modifiers::ALT) {
        out.push('⌥');
    }
    if mods.contains(Modifiers::SHIFT) {
        out.push('⇧');
    }
    if mods.contains(Modifiers::SUPER) {
        out.push('⌘');
    }
    out.push_str(label);
    out
}

/// 内置默认值，永远可解析；解析不了说明代码坏了。
pub fn default_hotkey() -> Hotkey {
    let hotkey = parse_hotkey(DEFAULT_HOTKEY_SPEC).expect("默认快捷键常量必须能被解析");
    assert_eq!(
        hotkey.display, DEFAULT_HOTKEY_DISPLAY,
        "默认键帽常量与解析结果不一致"
    );
    hotkey
}

/// 加载快捷键：配置优先，任何失败都退回默认值并说清原因。
///
/// 返回值第二项是给人看的来源说明，会打进日志，方便排查「为什么键不生效」。
pub fn load_hotkey(app: &AppHandle) -> (Hotkey, String) {
    load_named(app, "hotkey", DEFAULT_HOTKEY_SPEC)
}

/// 截屏翻译的快捷键。配置键名是 `capture_hotkey`，缺省 ⌃⌥⌘X。
pub fn load_capture_hotkey(app: &AppHandle) -> (Hotkey, String) {
    load_named(app, "capture_hotkey", DEFAULT_CAPTURE_SPEC)
}

/// 读某个键位的配置；任何失败都退回默认值并说清原因，不静默。
fn load_named(app: &AppHandle, key: &str, default_spec: &str) -> (Hotkey, String) {
    let fallback = || parse_hotkey(default_spec).expect("默认快捷键常量必须能被解析");
    let path_label = settings::path(app)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "settings.json".to_string());

    let value = match settings::read_value(app) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("[sense] {err}，改用默认 {default_spec}");
            return (fallback(), format!("{path_label}（读取失败，已回退默认）"));
        }
    };

    let spec = value
        .get(key)
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|spec| !spec.is_empty());

    let Some(spec) = spec else {
        // 没配过就用默认，不算错误（首次启动就是这种情况）
        return (fallback(), format!("{path_label}（未配置，用默认）"));
    };

    match parse_hotkey(spec) {
        Ok(hotkey) => (hotkey, path_label),
        Err(err) => {
            eprintln!("[sense] {err}（来自 {path_label}），改用默认 {default_spec}");
            (
                fallback(),
                format!("{path_label}（配置无效，已回退默认）"),
            )
        }
    }
}

/// 一个快捷键给界面看的样子。
#[derive(serde::Serialize, Debug, PartialEq, Eq)]
pub struct HotkeyView {
    pub kind: String,
    pub display: String,
    /// 是否真的注册成功。false 时 `display` 是配置里写的键，它并不生效。
    pub active: bool,
}

fn view_of(state: &HotkeyState, kind: &str, configured_display: String) -> HotkeyView {
    match state.display(kind) {
        Some(display) => HotkeyView {
            kind: kind.to_string(),
            display,
            active: true,
        },
        None => HotkeyView {
            kind: kind.to_string(),
            display: configured_display,
            active: false,
        },
    }
}

/// 当前两个快捷键，供设置页展示。以注册状态为准，不是配置文件：
/// 配置里写了、却被别的 App 占着没注册上的键，要标成「未生效」。
#[tauri::command]
pub fn hotkey_list(app: AppHandle, state: tauri::State<'_, HotkeyState>) -> Vec<HotkeyView> {
    let (lookup, _) = load_named(&app, "hotkey", DEFAULT_HOTKEY_SPEC);
    let (capture, _) = load_named(&app, "capture_hotkey", DEFAULT_CAPTURE_SPEC);
    vec![
        view_of(&state, "lookup", lookup.display),
        view_of(&state, "capture", capture.display),
    ]
}

/// 前端取查词键帽文案。没有可用快捷键时返回空串，界面会退化成只显示 Esc。
#[tauri::command]
pub fn hotkey_hint(state: tauri::State<'_, HotkeyState>) -> String {
    state.display("lookup").unwrap_or_default()
}

/// 记录窗口最近一次被显示的时刻，用于失焦宽限。
#[derive(Default)]
pub struct WindowState {
    shown_at: Mutex<Option<Instant>>,
}

impl WindowState {
    fn mark_shown(&self) {
        let mut guard = self
            .shown_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some(Instant::now());
    }

    fn last_shown(&self) -> Option<Instant> {
        self.shown_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .to_owned()
    }
}

/// 失焦时应该怎么处理。
#[derive(Debug, PartialEq, Eq)]
pub enum BlurAction {
    /// 立刻隐藏。
    HideNow,
    /// 还在宽限期内，等这么久之后复查。
    HideAfter(Duration),
}

/// 纯函数：根据「上次显示时刻」决定失焦动作，便于单测。
pub fn decide_blur(last_shown: Option<Instant>, now: Instant, grace: Duration) -> BlurAction {
    match last_shown {
        Some(shown) => {
            let elapsed = now.saturating_duration_since(shown);
            if elapsed < grace {
                BlurAction::HideAfter(grace - elapsed)
            } else {
                BlurAction::HideNow
            }
        }
        None => BlurAction::HideNow,
    }
}

/// 显示并抢焦点，同时刷新宽限期起点。
pub fn show(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        eprintln!("[sense] show: 找不到窗口 {MAIN_WINDOW}");
        return;
    };
    if let Err(err) = window.show() {
        eprintln!("[sense] show failed: {err}");
        return;
    }
    if let Err(err) = window.set_focus() {
        eprintln!("[sense] set_focus failed: {err}");
    }
    if let Some(state) = app.try_state::<WindowState>() {
        state.mark_shown();
    }
    // show/set_focus 在非主线程只是入队，Err 只表示通道断了；
    // 真正失败（窗口没成为 key）不会返回错误，所以这里把可见性打出来。
    match (window.is_visible(), window.inner_size().ok()) {
        (Ok(true), Some(size)) => {
            println!("[sense] window shown ({}x{})", size.width, size.height)
        }
        (Ok(true), None) => println!("[sense] window shown"),
        (Ok(false), _) => eprintln!("[sense] show: 调用成功但窗口仍不可见"),
        (Err(err), _) => eprintln!("[sense] is_visible failed: {err}"),
    }
}

/// 隐藏窗口（不退出应用）。
pub fn hide(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    if !window.is_visible().unwrap_or(false) {
        return;
    }
    if let Err(err) = window.hide() {
        eprintln!("[sense] hide failed: {err}");
        return;
    }
    println!("[sense] window hidden");
}

/// 快捷键的行为：可见就藏，不可见就显。
pub fn toggle(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    match window.is_visible() {
        Ok(true) => hide(app),
        _ => show(app),
    }
}

/// 失焦回调：宽限期内延迟复查，其余情况立即隐藏。
pub fn on_blur(app: &AppHandle) {
    // 仅本机验证用：截图/录屏时进程会被抢焦点，窗口会立刻藏起来。
    if dev_flag("SENSE_NO_AUTOHIDE") {
        return;
    }

    let app = app.clone();
    let last_shown = app
        .try_state::<WindowState>()
        .and_then(|state| state.last_shown());

    match decide_blur(last_shown, Instant::now(), BLUR_GRACE) {
        BlurAction::HideNow => hide(&app),
        BlurAction::HideAfter(delay) => {
            std::thread::spawn(move || {
                std::thread::sleep(delay + Duration::from_millis(20));
                let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
                    return;
                };
                // 复查：期间又被呼出过，就当这次失焦没发生。
                let current = app
                    .try_state::<WindowState>()
                    .and_then(|state| state.last_shown());
                if current != last_shown {
                    return;
                }
                if window.is_focused().unwrap_or(false) {
                    return;
                }
                hide(&app);
            });
        }
    }
}

/// 前端 Esc 触发：隐藏窗口。
#[tauri::command]
pub fn hide_window(app: AppHandle) {
    hide(&app);
}

/// 主窗口的最小尺寸：与 tauri.conf.json 的 minWidth / minHeight、前端 `windowFrame.ts` 保持一致
/// （有测试对拍，改一处忘了另一处会红）。
pub const MIN_WIDTH: f64 = 760.0;
pub const MIN_HEIGHT: f64 = 720.0;
/// 窗口一边的上限：macOS 的窗口服务器自己限在 10000，这里留余量，也挡掉前端传来的离谱数字。
const MAX_SIDE: f64 = 8192.0;
/// 坐标的上限：AppKit 在 |坐标| 逼近 2^31 时会抛未捕获的异常，整个进程 abort（异常出在主队列里，Rust 抓不到）。
/// 真实的屏幕坐标远在这个范围里；前端传来离谱的值，夹住比崩溃强。
const MAX_COORD: f64 = 100_000.0;

/// 窗口外框，逻辑像素，原点在屏幕左上角（与前端 `screenX` / `screenY` 同一个坐标系）。
/// 窗口无边框，外框就是内容区。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// 校验前端给的窗口框：数必须是有限的；宽高夹进 [最小尺寸, 上限]，坐标夹进 ±MAX_COORD，都取整。
pub fn sanitize_frame(frame: Frame) -> Result<Frame, String> {
    let numbers = [frame.x, frame.y, frame.width, frame.height];
    if !numbers.iter().all(|value| value.is_finite()) {
        return Err("窗口尺寸不是有限的数".to_string());
    }
    Ok(Frame {
        x: frame.x.clamp(-MAX_COORD, MAX_COORD).round(),
        y: frame.y.clamp(-MAX_COORD, MAX_COORD).round(),
        width: frame.width.clamp(MIN_WIDTH, MAX_SIDE).round(),
        height: frame.height.clamp(MIN_HEIGHT, MAX_SIDE).round(),
    })
}

/// 顶边不能高过屏幕可用区域的顶（菜单栏下沿）。
///
/// 实测（1920×1080，菜单栏 30px）：往上拖顶边拖进菜单栏，请求 y=10，系统把窗口按回 y=30，
/// 高度却照给的那样，于是底边被顶着往下跑 20px。所以自己先夹：顶边到顶就别再长高，底边不动。
pub fn keep_below_menu_bar(frame: Frame, top: f64) -> Frame {
    if frame.y >= top {
        return frame;
    }
    let lifted = top - frame.y;
    Frame {
        y: top,
        height: (frame.height - lifted).max(MIN_HEIGHT),
        ..frame
    }
}

/// 窗口所在屏幕可用区域的顶边（逻辑像素）；取不到就不夹。
fn work_area_top(window: &WebviewWindow) -> Option<f64> {
    let monitor = window.current_monitor().ok()??;
    Some(f64::from(monitor.work_area().position.y) / monitor.scale_factor())
}

/// 窗口操作只对主窗口开放；截屏译文窗口（overlay）不该被这些命令挪动。
fn ensure_main(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == MAIN_WINDOW {
        Ok(())
    } else {
        Err("只有主窗口可以这样操作".to_string())
    }
}

fn read_frame(window: &WebviewWindow) -> Result<Frame, String> {
    let scale = window.scale_factor().map_err(|err| err.to_string())?;
    let position = window
        .outer_position()
        .map_err(|err| err.to_string())?
        .to_logical::<f64>(scale);
    let size = window
        .outer_size()
        .map_err(|err| err.to_string())?
        .to_logical::<f64>(scale);
    Ok(Frame {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    })
}

fn apply_frame(window: &WebviewWindow, frame: Frame) -> Result<(), String> {
    // 先改大小、再定位：改大小时 AppKit 保哪个角不动，不由我们说了算；
    // 最后一步定位，保证左上角落在要的位置（拖左边 / 上边缩放时窗口要跟着挪）。
    window
        .set_size(LogicalSize::new(frame.width, frame.height))
        .map_err(|err| err.to_string())?;
    window
        .set_position(LogicalPosition::new(frame.x, frame.y))
        .map_err(|err| err.to_string())
}

/// 主窗口现在的外框。前端从窗口边缘 / 角上拖着缩放时，拿它当起点。
#[tauri::command]
pub fn get_window_frame(window: WebviewWindow) -> Result<Frame, String> {
    ensure_main(&window)?;
    read_frame(&window)
}

/// 一次设好位置和大小。
///
/// 系统对无边框窗口自带的缩放区只有窗口最外缘约 3px（落在里面的按下由 AppKit 先处理，网页收不到），
/// 可见的面板比窗口缩进 12px，抓不到；tao 0.37 的 `drag_resize_window` 在 macOS 上又是 NotSupported，
/// 没法从前端发起系统缩放。所以前端自己跟踪指针，逐帧调这个命令；
/// 拖左边 / 上边时位置和大小必须一起变，所以是一个命令，不是两个。
#[tauri::command]
pub fn set_window_frame(
    window: WebviewWindow,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    ensure_main(&window)?;
    let mut frame = sanitize_frame(Frame { x, y, width, height })?;
    if let Some(top) = work_area_top(&window) {
        frame = keep_below_menu_bar(frame, top);
    }
    apply_frame(&window, frame)
}

/// 绿灯「缩放」：放大到屏幕可用区域（不含菜单栏和 Dock），再点一次还原。返回操作之后是不是放大状态。
///
/// 用 tao 的最大化：无边框窗口上它把窗口铺满屏幕可用区域，并记下放大前的外框，还原时恢复
/// （2026-10-30 在 1920×1080 上实测：放大后外框恰为可用区域，还原后与放大前逐像素一致）。
/// 标题栏双击是 Tauri 内置的，走同一套状态：已放大时双击能还原；未放大时 Tauri 不会放大无边框窗口
/// （`is_maximizable` 对无边框窗口恒为 false），所以放大只能点绿灯。
///
/// 已知边界（评审读 tao 源码并在真实 AppKit 里复刻出的，都是 P3，与系统自带的 zoom 同一种语义）：
/// - tao 判断「已放大」只比较大小（容差 1pt）：手动把窗口拖到恰好等于可用区域大小再点绿灯，它当作已放大，
///   还原到自己的默认位置（50,50）800×600；
/// - 放大后可用区域变了（Dock 自动隐藏、换分辨率、插拔显示器），下次点绿灯是再放大一次，
///   并把过期的放大外框记成「标准外框」，之后回不到原来的小窗；
/// - 放大带动画，主线程会被占住约 0.35 秒。
#[tauri::command]
pub fn toggle_zoom(window: WebviewWindow) -> Result<bool, String> {
    ensure_main(&window)?;
    let zoomed = window.is_maximized().map_err(|err| err.to_string())?;
    if zoomed {
        window.unmaximize().map_err(|err| err.to_string())?;
    } else {
        window.maximize().map_err(|err| err.to_string())?;
    }
    Ok(!zoomed)
}

/// 红灯「关闭」：退出应用（等同 ⌘Q）。负责人 2026-10-30 明确选了「红 = 退出、黄 = 隐藏」。
/// 退出会丢掉只存在内存里的东西（正在翻译的长文、输入框里的字），快捷键也不再响应，要手动重开；
/// 只想收起窗口、保留快捷键呼出的，用黄灯 `hide_window`。和其它窗口命令一样只对主窗口开放。
#[tauri::command]
pub fn quit_app(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    ensure_main(&window)?;
    println!("[sense] quit requested from window controls");
    app.exit(0);
    Ok(())
}

/// 开发期开关：只在 debug 构建里生效，避免发布版被环境变量改行为。
pub fn dev_flag(name: &str) -> bool {
    cfg!(debug_assertions) && std::env::var(name).is_ok()
}

/// 开发期取值：同 `dev_flag`，只在 debug 构建里读，空值当没设。
pub fn dev_value(name: &str) -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_within_grace_is_delayed_not_dropped() {
        let now = Instant::now();
        let shown = now - Duration::from_millis(100);
        match decide_blur(Some(shown), now, BLUR_GRACE) {
            BlurAction::HideAfter(wait) => {
                assert_eq!(wait, BLUR_GRACE - Duration::from_millis(100));
            }
            BlurAction::HideNow => panic!("宽限期内不应立刻隐藏"),
        }
    }

    #[test]
    fn blur_after_grace_hides_now() {
        let now = Instant::now();
        let shown = now - Duration::from_millis(900);
        assert_eq!(decide_blur(Some(shown), now, BLUR_GRACE), BlurAction::HideNow);
    }

    #[test]
    fn blur_exactly_at_grace_boundary_hides_now() {
        let now = Instant::now();
        let shown = now - BLUR_GRACE;
        assert_eq!(decide_blur(Some(shown), now, BLUR_GRACE), BlurAction::HideNow);
    }

    #[test]
    fn blur_without_show_record_hides_now() {
        assert_eq!(
            decide_blur(None, Instant::now(), BLUR_GRACE),
            BlurAction::HideNow
        );
    }

    fn frame(x: f64, y: f64, width: f64, height: f64) -> Frame {
        Frame { x, y, width, height }
    }

    #[test]
    fn sanitize_clamps_size_into_limits_and_rounds() {
        // 太小：夹到最小尺寸，位置照旧
        assert_eq!(
            sanitize_frame(frame(10.4, 20.6, 100.0, 50.0)),
            Ok(frame(10.0, 21.0, MIN_WIDTH, MIN_HEIGHT))
        );
        // 太大：夹到上限
        let huge = sanitize_frame(frame(0.0, 0.0, 1.0e9, 1.0e9)).expect("应夹住");
        assert_eq!((huge.width, huge.height), (MAX_SIDE, MAX_SIDE));
        // 正常值只取整
        assert_eq!(
            sanitize_frame(frame(-30.0, 5.0, 900.4, 800.6)),
            Ok(frame(-30.0, 5.0, 900.0, 801.0)),
            "多屏时坐标可以是负的，不能被夹掉"
        );
    }

    #[test]
    fn top_edge_stops_at_the_menu_bar_and_bottom_edge_stays_put() {
        // 屏幕可用区域顶在 y=30。窗口原来 y=60、高 960，底边在 1020
        let dragged = frame(400.0, 10.0, 1140.0, 1010.0); // 顶边拖进了菜单栏，底边在 1020
        let kept = keep_below_menu_bar(dragged, 30.0);
        assert_eq!(kept, frame(400.0, 30.0, 1140.0, 990.0));
        assert_eq!(kept.y + kept.height, dragged.y + dragged.height, "底边不能动");
    }

    #[test]
    fn frame_below_the_menu_bar_is_left_alone() {
        let ok = frame(100.0, 30.0, 900.0, 800.0);
        assert_eq!(keep_below_menu_bar(ok, 30.0), ok);
        let lower = frame(100.0, 200.0, 900.0, 800.0);
        assert_eq!(keep_below_menu_bar(lower, 30.0), lower);
    }

    #[test]
    fn menu_bar_clamp_never_goes_below_min_height() {
        // 离谱的输入：整个窗口要被推到菜单栏上方很远
        let kept = keep_below_menu_bar(frame(0.0, -900.0, 800.0, 800.0), 30.0);
        assert_eq!(kept.y, 30.0);
        assert_eq!(kept.height, MIN_HEIGHT);
    }

    #[test]
    fn sanitize_clamps_coordinates_so_appkit_never_sees_absurd_values() {
        // |坐标| 逼近 2^31 时 AppKit 会抛未捕获的异常、整个进程退出：评审在真实 AppKit 上复现过
        let wild = sanitize_frame(frame(2.0e9, -2.5e9, 900.0, 800.0)).expect("应夹住");
        assert_eq!((wild.x, wild.y), (MAX_COORD, -MAX_COORD));
        // 多屏时的正常负坐标不受影响
        assert_eq!(
            sanitize_frame(frame(-1920.0, -300.0, 900.0, 800.0)),
            Ok(frame(-1920.0, -300.0, 900.0, 800.0))
        );
    }

    #[test]
    fn sanitize_rejects_non_finite_numbers() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(sanitize_frame(frame(bad, 0.0, 800.0, 800.0)).is_err());
            assert!(sanitize_frame(frame(0.0, bad, 800.0, 800.0)).is_err());
            assert!(sanitize_frame(frame(0.0, 0.0, bad, 800.0)).is_err());
            assert!(sanitize_frame(frame(0.0, 0.0, 800.0, bad)).is_err());
        }
    }

    #[test]
    fn min_size_matches_tauri_conf() {
        // 最小尺寸写在三处：tauri.conf.json、这里、前端 windowFrame.ts；这里和配置文件对拍
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("配置必须是合法 JSON");
        let main = conf["app"]["windows"]
            .as_array()
            .and_then(|windows| windows.iter().find(|w| w["label"] == MAIN_WINDOW))
            .expect("配置里必须有主窗口");
        assert_eq!(main["minWidth"].as_f64(), Some(MIN_WIDTH));
        assert_eq!(main["minHeight"].as_f64(), Some(MIN_HEIGHT));
        assert_eq!(main["resizable"], true, "窗口得是可缩放的，否则边缘拖拽无效");
    }

    #[test]
    fn default_hotkey_is_ctrl_alt_cmd_space() {
        let hotkey = parse_hotkey(DEFAULT_HOTKEY_SPEC).expect("默认值必须可解析");
        assert_eq!(hotkey.shortcut.key, Code::Space);
        assert!(hotkey.shortcut.mods.contains(Modifiers::CONTROL), "缺少 ⌃");
        assert!(hotkey.shortcut.mods.contains(Modifiers::ALT), "缺少 ⌥");
        assert!(hotkey.shortcut.mods.contains(Modifiers::SUPER), "缺少 ⌘");
        assert_eq!(hotkey.display, DEFAULT_HOTKEY_DISPLAY);
    }

    #[test]
    fn parse_normalizes_case_spacing_and_order() {
        let hotkey = parse_hotkey(" CTRL + Alt + Cmd + SPACE ").expect("应能解析");
        assert_eq!(hotkey.display, DEFAULT_HOTKEY_DISPLAY);

        let reordered = parse_hotkey("cmd+shift+k").expect("应能解析");
        assert_eq!(reordered.display, "⇧⌘K", "修饰键顺序应归一为 ⌃⌥⇧⌘");
    }

    #[test]
    fn parse_rejects_bad_input() {
        for bad in [
            "",
            "+",
            "ctrl++k",
            "ctrl+",
            "ctrl+alt",
            "k+k",
            "ctrl+alt+ff",
            "ctrl+alt+中",
            // 只有 shift/alt 会抢走全局输入，必须拒绝
            "shift+space",
            "alt+a",
        ] {
            assert!(parse_hotkey(bad).is_err(), "`{bad}` 应当被拒绝");
        }
    }

    fn hotkey(spec: &str) -> Hotkey {
        parse_hotkey(spec).expect("测试用快捷键必须可解析")
    }

    #[test]
    fn fallback_registration_is_remembered_so_it_can_be_replaced_later() {
        let state = HotkeyState::default();
        let configured = hotkey("ctrl+alt+k");
        let bad = configured.shortcut;
        let default = default_hotkey();
        let default_shortcut = default.shortcut;

        // 配置的键被别的 App 占了，默认键能注册
        let startup = register_with_fallback(&state, "lookup", configured, Some(default), |shortcut| {
            if shortcut == bad {
                Err("被占用".to_string())
            } else {
                Ok(())
            }
        });

        assert!(startup.fell_back);
        assert_eq!(startup.active.as_deref(), Some(DEFAULT_HOTKEY_DISPLAY));
        assert_eq!(startup.errors, vec!["被占用".to_string()]);
        // 关键：回退键必须记下来，否则 set_hotkey 的 previous 是 None，回退键永远不会被注销
        assert_eq!(state.get("lookup"), Some(default_shortcut));
        assert_eq!(state.display("lookup").as_deref(), Some(DEFAULT_HOTKEY_DISPLAY));
    }

    #[test]
    fn configured_key_wins_when_it_registers() {
        let state = HotkeyState::default();
        let configured = hotkey("ctrl+alt+k");
        let shortcut = configured.shortcut;
        let mut attempts = 0;
        let startup = register_with_fallback(&state, "lookup", configured, Some(default_hotkey()), |_| {
            attempts += 1;
            Ok(())
        });
        assert!(!startup.fell_back);
        assert_eq!(attempts, 1, "配置的键能用就不去碰默认键");
        assert_eq!(startup.active.as_deref(), Some("⌃⌥K"));
        assert_eq!(state.get("lookup"), Some(shortcut));
    }

    #[test]
    fn nothing_is_remembered_when_every_attempt_fails() {
        let state = HotkeyState::default();
        let startup = register_with_fallback(
            &state,
            "lookup",
            hotkey("ctrl+alt+k"),
            Some(default_hotkey()),
            |_| Err("都被占了".to_string()),
        );
        assert!(startup.active.is_none());
        assert_eq!(startup.errors.len(), 2);
        assert!(state.get("lookup").is_none());
        assert!(state.display("lookup").is_none());
    }

    #[test]
    fn capture_key_has_no_fallback() {
        let state = HotkeyState::default();
        let startup = register_with_fallback(&state, "capture", hotkey("ctrl+alt+cmd+x"), None, |_| {
            Err("被占用".to_string())
        });
        assert!(startup.active.is_none());
        assert_eq!(startup.errors.len(), 1);
        assert!(state.get("capture").is_none());
        assert!(state.get("lookup").is_none(), "两个槽位互不影响");
    }

    #[test]
    fn hotkey_view_follows_registration_not_config() {
        let state = HotkeyState::default();
        // 配置写了 ⌃⌥⌘X，但没注册上：必须标成未生效，而不是照着配置显示
        let view = view_of(&state, "capture", "⌃⌥⌘X".to_string());
        assert_eq!(
            view,
            HotkeyView { kind: "capture".into(), display: "⌃⌥⌘X".into(), active: false }
        );

        // 回退到默认键之后，显示的是实际生效的默认键
        state.remember("lookup", default_hotkey().shortcut, DEFAULT_HOTKEY_DISPLAY);
        let view = view_of(&state, "lookup", "⌃⌥K".to_string());
        assert_eq!(
            view,
            HotkeyView { kind: "lookup".into(), display: DEFAULT_HOTKEY_DISPLAY.into(), active: true }
        );

        // 旧键装不回去：忘掉它，界面不再宣称生效
        state.forget("lookup");
        assert!(!view_of(&state, "lookup", "⌃⌥K".to_string()).active);
    }
}
