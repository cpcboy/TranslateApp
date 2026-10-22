//! SENSE 主进程。
//!
//! D1 范围只有窗口：无边框、置顶、失焦隐藏、⌥Space 显隐。
//! 模型请求、密钥、缓存都在后面的日子进来，且一律留在 Rust 侧。

mod window;

use tauri::Manager;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(window::WindowState::default())
        .invoke_handler(tauri::generate_handler![window::hide_window])
        .setup(|app| {
            // 常驻后台的小工具：不要 Dock 图标，避免切 App 时被它干扰。
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let shortcut = Shortcut::new(Some(Modifiers::ALT), Code::Space);
            match app.global_shortcut().on_shortcut(shortcut, |app, _shortcut, event| {
                if event.state() == ShortcutState::Pressed {
                    window::toggle(app);
                }
            }) {
                Ok(()) => println!("[sense] hotkey registered: {}", window::HOTKEY_LABEL),
                Err(err) => eprintln!("[sense] hotkey register FAILED: {err}"),
            }

            // 仅供本机验证：不设这个变量时，启动后窗口保持隐藏（产品行为）。
            if std::env::var("SENSE_SHOW_ON_START").is_ok() {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(600));
                    window::show(&handle);
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(focused) = event {
                if !focused {
                    window::on_blur(window.app_handle());
                } else {
                    println!("[sense] window focused");
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
