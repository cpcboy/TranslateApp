//! 窗口生命周期：显隐切换、失焦隐藏、全局快捷键。
//!
//! 所有窗口操作都在 Rust 侧完成；前端只调 `hide_window`，不碰窗口权限。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

/// 主窗口 label，与 tauri.conf.json 保持一致。
pub const MAIN_WINDOW: &str = "main";

/// 给人看的快捷键文案。
pub const HOTKEY_LABEL: &str = "Alt+Space (⌥Space)";

/// 刚显示就被判为失焦的宽限期。
///
/// macOS 上 `show()` 与焦点事件存在竞态：窗口刚出现时可能先收到一次
/// `Focused(false)`，不设宽限期会出现「呼出即自动隐藏」。
const BLUR_GRACE: Duration = Duration::from_millis(260);

/// 记录窗口最近一次被显示的时刻，用于失焦宽限。
#[derive(Default)]
pub struct WindowState {
    shown_at: Mutex<Option<Instant>>,
}

impl WindowState {
    fn mark_shown(&self) {
        if let Ok(mut guard) = self.shown_at.lock() {
            *guard = Some(Instant::now());
        }
    }

    fn last_shown(&self) -> Option<Instant> {
        self.shown_at.lock().ok().and_then(|guard| *guard)
    }
}

/// 失焦是否应当被忽略（处于刚呼出的宽限期内）。
pub fn should_ignore_blur(last_shown: Option<Instant>, now: Instant, grace: Duration) -> bool {
    match last_shown {
        Some(shown) => now.saturating_duration_since(shown) < grace,
        None => false,
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
    match window.inner_size() {
        Ok(size) => println!("[sense] window shown ({}x{})", size.width, size.height),
        Err(_) => println!("[sense] window shown"),
    }
}

/// 隐藏窗口（不退出应用）。
pub fn hide(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    if let Err(err) = window.hide() {
        eprintln!("[sense] hide failed: {err}");
        return;
    }
    println!("[sense] window hidden");
}

/// ⌥Space 的行为：可见就藏，不可见就显。
pub fn toggle(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    match window.is_visible() {
        Ok(true) => hide(app),
        _ => show(app),
    }
}

/// 失焦回调：宽限期内忽略，其余情况隐藏。
pub fn on_blur(app: &AppHandle) {
    // 仅本机验证用：截图/录屏时进程会被抢焦点，窗口会立刻藏起来。
    if std::env::var("SENSE_NO_AUTOHIDE").is_ok() {
        return;
    }
    let last_shown = app
        .try_state::<WindowState>()
        .and_then(|state| state.last_shown());
    if should_ignore_blur(last_shown, Instant::now(), BLUR_GRACE) {
        return;
    }
    hide(app);
}

/// 前端 Esc 触发：隐藏窗口。
#[tauri::command]
pub fn hide_window(app: AppHandle) {
    hide(&app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_within_grace_is_ignored() {
        let now = Instant::now();
        let shown = now - Duration::from_millis(100);
        assert!(should_ignore_blur(Some(shown), now, BLUR_GRACE));
    }

    #[test]
    fn blur_after_grace_hides() {
        let now = Instant::now();
        let shown = now - Duration::from_millis(900);
        assert!(!should_ignore_blur(Some(shown), now, BLUR_GRACE));
    }

    #[test]
    fn blur_without_show_record_hides() {
        assert!(!should_ignore_blur(None, Instant::now(), BLUR_GRACE));
    }
}
