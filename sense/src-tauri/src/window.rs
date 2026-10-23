//! 窗口生命周期：显隐切换、失焦隐藏、全局快捷键。
//!
//! 所有窗口操作都在 Rust 侧完成；前端只调 `hide_window`，不碰窗口权限。

use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};

use crate::settings::{self, DEFAULT_HOTKEY_SPEC};

/// 主窗口 label，与 tauri.conf.json 保持一致。
pub const MAIN_WINDOW: &str = "main";

/// 默认键帽，与 `DEFAULT_HOTKEY_SPEC` 对应；有断言守护。
pub const DEFAULT_HOTKEY_DISPLAY: &str = "⌃⌥⌘Space";

/// 截屏翻译的默认快捷键：⌃⌥⌘X。
/// 选它的理由：不在 macOS 符号快捷键表里，也不与微信/QQ/系统截屏（⇧⌘4）冲突。
pub const DEFAULT_CAPTURE_SPEC: &str = "ctrl+alt+cmd+x";

/// 当前**已注册成功**的键帽。注册失败时保持 None，界面就不显示键帽。
static ACTIVE_DISPLAY: RwLock<Option<String>> = RwLock::new(None);

/// 已注册的两个快捷键。改键时要先 unregister 旧的、注册失败还要能回滚。
#[derive(Default)]
pub struct HotkeyState {
    lookup: Mutex<Option<Shortcut>>,
    capture: Mutex<Option<Shortcut>>,
}

impl HotkeyState {
    fn slot(&self, kind: &str) -> &Mutex<Option<Shortcut>> {
        if kind == "capture" {
            &self.capture
        } else {
            &self.lookup
        }
    }

    pub fn get(&self, kind: &str) -> Option<Shortcut> {
        self.slot(kind).lock().ok().and_then(|slot| *slot)
    }

    pub fn remember(&self, kind: &str, shortcut: Shortcut) {
        if let Ok(mut slot) = self.slot(kind).lock() {
            *slot = Some(shortcut);
        }
    }

    fn other(&self, kind: &str) -> Option<Shortcut> {
        self.get(if kind == "capture" { "lookup" } else { "capture" })
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
        let _ = plugin.unregister(shortcut);
    }

    match crate::register_hotkey(&app, &kind, parsed.shortcut) {
        Ok(()) => {
            state.remember(&kind, parsed.shortcut);
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

            if kind == "lookup" {
                set_active_display(Some(parsed.display.clone()));
            }
            println!("[sense] hotkey updated: {kind} → {}", parsed.display);
            Ok(HotkeyView {
                kind,
                display: parsed.display,
            })
        }
        Err(err) => {
            if let Some(shortcut) = previous {
                let _ = crate::register_hotkey(&app, &kind, shortcut);
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

/// 改键结果：前端拿到新的键帽显示。
#[derive(serde::Serialize)]
pub struct HotkeyView {
    pub kind: String,
    pub display: String,
}

/// 当前两个快捷键的键帽，供设置页展示。
#[tauri::command]
pub fn hotkey_list(app: AppHandle) -> Vec<HotkeyView> {
    let (lookup, _) = load_named(&app, "hotkey", DEFAULT_HOTKEY_SPEC);
    let (capture, _) = load_named(&app, "capture_hotkey", DEFAULT_CAPTURE_SPEC);
    vec![
        HotkeyView {
            kind: "lookup".into(),
            display: lookup.display,
        },
        HotkeyView {
            kind: "capture".into(),
            display: capture.display,
        },
    ]
}

/// 记录「当前实际生效的键帽」。注册成功才调用；传 None 表示没有可用快捷键。
pub fn set_active_display(display: Option<String>) {
    let mut guard = ACTIVE_DISPLAY
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = display;
}

/// 前端取键帽文案。没有可用快捷键时返回空串，界面会退化成只显示 Esc。
#[tauri::command]
pub fn hotkey_hint() -> String {
    ACTIVE_DISPLAY
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .unwrap_or_default()
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

/// 开发期开关：只在 debug 构建里生效，避免发布版被环境变量改行为。
pub fn dev_flag(name: &str) -> bool {
    cfg!(debug_assertions) && std::env::var(name).is_ok()
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
}
