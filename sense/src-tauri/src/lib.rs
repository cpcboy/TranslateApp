//! SENSE 主进程。
//!
//! D1 范围只有窗口：无边框、置顶、失焦隐藏、⌃⌥⌘Space（可配置）显隐。
//! 模型请求、密钥、缓存都在后面的日子进来，且一律留在 Rust 侧。

mod cache;
mod capture;
mod commands;
mod prompts;
mod providers;
mod secrets;
mod settings;
mod stream;
mod window;

use std::sync::OnceLock;
use std::time::Duration;

use tauri::menu::Menu;
use tauri::Emitter;
use tauri::Manager;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// 全局复用一个 HTTP 客户端：连接池、TLS 会话都能复用。
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// 本地模型专用：绕开系统代理。
///
/// reqwest 默认读 HTTP_PROXY / HTTPS_PROXY，而 NO_PROXY 里通常没有 127.0.0.1。
/// 一旦用户（或开发机）设了全局代理，发给 Ollama / LM Studio 的请求会被
/// 转给代理，本地模型直接不可用，报错还很难懂。
static LOOPBACK_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn is_loopback(base_url: &str) -> bool {
    let rest = base_url.split("://").nth(1).unwrap_or_default();
    let authority = rest.split('/').next().unwrap_or_default();
    // 去掉 user:pass@：不然 split(':') 会先把用户名当成 host
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        // IPv6 字面量：[::1]:11434
        stripped.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// 按目标地址挑客户端：本地直连，远端才走系统代理。
pub fn client_for(base_url: &str) -> &'static reqwest::Client {
    if !is_loopback(base_url) {
        return http_client();
    }
    LOOPBACK_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(180))
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .expect("构建本地 HTTP 客户端失败")
    })
}

pub fn http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            // 连接要快，整条流给足时间；卡住由 openai_compat 里的分片超时兜底。
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(180))
            // 查词是一次一次来的，连接必须留着复用：每次重新握手要多 100–300ms。
            // 很多网关/LB 在 60–120s 就掐掉空闲连接；留太久会拿到半关闭的连接，
            // 表现为「连不上模型服务」，比多握一次手更糟。
            .pool_idle_timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(4)
            .tcp_keepalive(Duration::from_secs(60))
            .build()
            .expect("构建 HTTP 客户端失败")
    })
}

/// 注册一个全局快捷键。抽出来是为了注册失败时能换默认值再试一次。
fn register_shortcut(app: &tauri::AppHandle, shortcut: Shortcut) -> Result<(), String> {
    register_hotkey(app, "lookup", shortcut)
}

/// 按用途注册：查词是显隐窗口，截屏是拉选区。
pub fn register_hotkey(
    app: &tauri::AppHandle,
    kind: &str,
    shortcut: Shortcut,
) -> Result<(), String> {
    let plugin = app.global_shortcut();
    let result = match kind {
        "capture" => plugin.on_shortcut(shortcut, |app, _shortcut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                let state = handle.state::<capture::CaptureState>();
                if let Err(err) = capture::start_capture(handle.clone(), state).await {
                    eprintln!("[sense] 截屏启动失败：{err}");
                    // 热键触发时用户看不到终端，必须把原因放到界面上
                    window::show(&handle);
                    let _ = handle.emit("sense://capture-error", err);
                }
            });
        }),
        _ => plugin.on_shortcut(shortcut, |app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                window::toggle(app);
            }
        }),
    };
    result.map_err(|err| err.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(window::WindowState::default())
        .manage(window::HotkeyState::default())
        .manage(capture::CaptureState::default())
        .invoke_handler(tauri::generate_handler![
            window::hide_window,
            window::hotkey_hint,
            commands::settings::list_providers,
            commands::settings::save_provider,
            commands::settings::set_active_provider,
            commands::settings::set_api_key,
            commands::settings::clear_api_key,
            commands::settings::test_connection,
            commands::settings::dev_flags,
            commands::lookup::lookup,
            commands::lookup::invalidate_lookup,
            commands::longtext::translate_text,
            commands::longtext::cancel_translate,
            commands::reading::check_grammar,
            commands::reading::explain_word,
            window::hotkey_list,
            window::set_hotkey,
            capture::start_capture,
            capture::cancel_capture,
            capture::close_overlay,
            capture::capture_blocks,
        ])
        .setup(|app| {
            // 默认菜单：给出 ⌘Q 退出与 Edit 菜单（输入框的 ⌘C/⌘V 依赖它）。
            app.set_menu(Menu::default(app.handle())?)?;

            // 预热到 provider 的连接：DNS + TCP + TLS 在启动时就做完，
            // 第一次查词不用再付这 100–300ms。
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    // 配置读不出来就别猜默认 provider：那会朝 deepseek 发一次
                    // 用户根本没配过的请求。
                    if settings::read_value(&handle).is_err() {
                        return;
                    }
                    if let Ok(provider) = providers::active(&handle) {
                        providers::openai_compat::warmup(client_for(&provider.base_url), &provider)
                            .await;
                    }
                });
            }

            // 缓存：打不开就退到内存缓存，别为此让应用起不来。
            let cache = match app.path().app_data_dir() {
                Ok(dir) => match cache::Cache::open(&dir.join("sense.db")) {
                    Ok(cache) => cache,
                    Err(err) => {
                        eprintln!("[sense] {err}，改用内存缓存");
                        cache::Cache::in_memory().expect("内存缓存必须可用")
                    }
                },
                Err(err) => {
                    eprintln!("[sense] 取不到数据目录: {err}，改用内存缓存");
                    cache::Cache::in_memory().expect("内存缓存必须可用")
                }
            };
            app.manage(cache);

            // 快捷键：注册成功才对外宣称它生效，避免界面显示一个按不出来的键。
            let (hotkey, source) = window::load_hotkey(app.handle());
            match register_shortcut(app.handle(), hotkey.shortcut) {
                Ok(()) => {
                    println!(
                        "[sense] hotkey registered: {}；来源：{}",
                        hotkey.display, source
                    );
                    window::set_active_display(Some(hotkey.display));
                    app.state::<window::HotkeyState>()
                        .remember("lookup", hotkey.shortcut);
                }
                Err(err) => {
                    eprintln!("[sense] hotkey register FAILED ({}): {err}", hotkey.display);
                    let fallback = window::default_hotkey();
                    match register_shortcut(app.handle(), fallback.shortcut) {
                        Ok(()) => {
                            eprintln!("[sense] 已回退到默认 {}", fallback.display);
                            window::set_active_display(Some(fallback.display));
                        }
                        Err(second) => {
                            eprintln!("[sense] 默认键也无法注册: {second}");
                            window::set_active_display(None);
                        }
                    }
                    // 热键是唯一入口：注册不上还躲着不露脸，用户会以为应用坏了。
                    window::show(app.handle());
                }
            }

            // 截屏翻译的快捷键。注册不上不影响查词，只打日志并让用户在设置里改。
            {
                let (hotkey, source) = window::load_capture_hotkey(app.handle());
                match register_hotkey(app.handle(), "capture", hotkey.shortcut) {
                    Ok(()) => {
                        println!(
                            "[sense] capture hotkey registered: {}；来源：{}",
                            hotkey.display, source
                        );
                        app.state::<window::HotkeyState>()
                            .remember("capture", hotkey.shortcut);
                    }
                    Err(err) => {
                        eprintln!(
                            "[sense] capture hotkey FAILED ({}): {err}；可在设置里改",
                            hotkey.display
                        );
                    }
                }
            }

            // 仅供自动化验证：直接拉起选区窗口（模拟按快捷键）
            if std::env::var_os("SENSE_DEV_SELECT").is_some() {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(1200)).await;
                    let state = handle.state::<capture::CaptureState>();
                    if let Err(err) = capture::start_capture(handle.clone(), state).await {
                        eprintln!("[sense] dev select 失败: {err}");
                    }
                });
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

#[cfg(test)]
mod tests {
    use super::is_loopback;

    #[test]
    fn loopback_detection_covers_local_models() {
        assert!(is_loopback("http://127.0.0.1:11434/v1"));
        assert!(is_loopback("http://localhost:11434/v1"));
        assert!(is_loopback("http://[::1]:11434/v1"));
        assert!(!is_loopback("https://api.deepseek.com/v1"));
        // 域名里带 localhost 不算本地，别把远端请求也绕开代理
        assert!(!is_loopback("https://localhost.example.com/v1"));
    }
}
