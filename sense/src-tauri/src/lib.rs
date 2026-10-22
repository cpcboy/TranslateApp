//! SENSE 主进程。
//!
//! D1 范围只有窗口：无边框、置顶、失焦隐藏、⌥Space 显隐。
//! 模型请求、密钥、缓存都在后面的日子进来，且一律留在 Rust 侧。

mod window;

use tauri::menu::Menu;
use tauri::Manager;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(window::WindowState::default())
        .invoke_handler(tauri::generate_handler![window::hide_window])
        .setup(|app| {
            // 默认菜单：给出 ⌘Q 退出与 Edit 菜单（输入框的 ⌘C/⌘V 依赖它）。
            app.set_menu(Menu::default(app.handle())?)?;

            let shortcut = Shortcut::new(Some(Modifiers::ALT), Code::Space);
            match app
                .global_shortcut()
                .on_shortcut(shortcut, |app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        window::toggle(app);
                    }
                }) {
                Ok(()) => println!("[sense] hotkey registered: {}", window::HOTKEY_LABEL),
                Err(err) => {
                    // 热键是唯一入口：注册失败还躲着不露脸，用户会以为应用坏了。
                    eprintln!("[sense] hotkey register FAILED: {err}");
                    window::show(app.handle());
                }
            }

            // 仅供本机验证：不设这个变量时，启动后窗口保持隐藏（产品行为）。
            if window::dev_flag("SENSE_SHOW_ON_START") {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(600));
                    window::show(&handle);
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != window::MAIN_WINDOW {
                return;
            }
            if let tauri::WindowEvent::Focused(focused) = event {
                if *focused {
                    println!("[sense] window focused");
                } else {
                    window::on_blur(window.app_handle());
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // 点 Dock 图标时把窗口叫回来（窗口默认是隐藏的）。
            if let tauri::RunEvent::Reopen { .. } = event {
                window::show(app);
            }
        });
}
