//! SENSE 主进程。
//!
//! 应用装配：无边框置顶窗口、失焦隐藏、全局快捷键（⌃⌥⌘Space 查词 / ⌃⌥⌘X 截屏，均可配置）。
//! 模型请求、密钥（钥匙串）、SQLite 缓存、截屏翻译都在 Rust 侧，前端不接触 Key 也不直连模型。

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

/// 这个地址是不是本机。保存与「测试连接」的 https 规则、客户端选择共用它。
///
/// 用 reqwest 自己的 URL 解析器取主机：它才是真正决定请求发到哪里的那一个。
/// 以前自己按 `/` 切、按 `:` 取主机，`http://evil.example?@localhost/v1`、`...#@localhost`、
/// `...\@localhost` 这些写法会被当成本地（解析器认为主机是 evil.example），Key 就明文发了出去。
/// 解析不了的一律不算本地（宁可多要求 https）。
pub(crate) fn is_loopback(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    // host_str 已经小写；IPv6 带方括号；`127.1` 这类简写会被规整成 127.0.0.1
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
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
            // 同 http_client：只管非流式请求，流式请求单独放宽
            .timeout(Duration::from_secs(180))
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .expect("构建本地 HTTP 客户端失败")
    })
}

pub fn http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            // 连接要快。180 秒总时限只管非流式请求（/models 等）：reqwest 的 timeout 管到
            // 响应体读完，流式请求会按 max_tokens 单独放宽，见 openai_compat::stream_deadline。
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
            {
                let state = app.state::<window::HotkeyState>();
                let (hotkey, source) = window::load_hotkey(app.handle());
                let configured = hotkey.display.clone();
                let startup = window::register_with_fallback(
                    &state,
                    "lookup",
                    hotkey,
                    Some(window::default_hotkey()),
                    |shortcut| register_hotkey(app.handle(), "lookup", shortcut),
                );
                match (&startup.active, startup.fell_back) {
                    (Some(display), false) => {
                        println!("[sense] hotkey registered: {display}；来源：{source}");
                    }
                    (Some(display), true) => {
                        let reason = startup.errors.first().map(String::as_str).unwrap_or_default();
                        eprintln!("[sense] hotkey register FAILED ({configured}): {reason}");
                        eprintln!("[sense] 已回退到默认 {display}");
                    }
                    (None, _) => {
                        for err in &startup.errors {
                            eprintln!("[sense] hotkey register FAILED ({configured}): {err}");
                        }
                        eprintln!("[sense] 没有任何可用的查词快捷键");
                    }
                }
                // 热键是唯一入口：注册不上还躲着不露脸，用户会以为应用坏了。
                if startup.active.is_none() || startup.fell_back {
                    window::show(app.handle());
                }
            }

            // 截屏翻译的快捷键。注册不上不影响查词，只打日志并让用户在设置里改。
            {
                let state = app.state::<window::HotkeyState>();
                let (hotkey, source) = window::load_capture_hotkey(app.handle());
                let configured = hotkey.display.clone();
                let startup = window::register_with_fallback(&state, "capture", hotkey, None, |shortcut| {
                    register_hotkey(app.handle(), "capture", shortcut)
                });
                match startup.active {
                    Some(display) => {
                        println!("[sense] capture hotkey registered: {display}；来源：{source}");
                    }
                    None => {
                        let reason = startup.errors.first().map(String::as_str).unwrap_or_default();
                        eprintln!("[sense] capture hotkey FAILED ({configured}): {reason}；可在设置里改");
                    }
                }
            }

            // 仅供自动化验证：直接拉起选区窗口（模拟按快捷键）
            if window::dev_flag("SENSE_DEV_SELECT") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(1200)).await;
                    let state = handle.state::<capture::CaptureState>();
                    if let Err(err) = capture::start_capture(handle.clone(), state).await {
                        eprintln!("[sense] dev select 失败: {err}");
                    }
                });
            }

            // 仅供自动化验证（debug 构建）：拿一张现成的 PNG 走 OCR → 翻译 → 覆盖层，
            // 不需要屏幕录制权限，也不用人工拖框。
            if let Some(png) = window::dev_value("SENSE_DEV_CAPTURE_PNG") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(1200)).await;
                    let state = handle.state::<capture::CaptureState>();
                    match capture::dev_capture_png(&handle, &state, &png).await {
                        Ok(count) => println!("[sense] dev capture: {count} 块"),
                        Err(err) => eprintln!("[sense] dev capture 失败: {err}"),
                    }
                });
            }

            // 仅供本机验证（debug 构建）：窗口本来就是启动即显示（tauri.conf.json 里 visible: true），
            // 这个开关只是 600ms 后再走一遍 show（居中、抢焦点、刷新失焦宽限期），复现快捷键呼出的路径。
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
            // 点 Dock 图标时把窗口叫回来（失焦或按 Esc 之后窗口是隐藏的）。
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
        // userinfo 里写 localhost 骗不过它：真正的主机是 @ 后面的
        assert!(!is_loopback("http://localhost:80@evil.example/v1"));
        assert!(!is_loopback("http://127.0.0.1:1@evil.example/v1"));
        // 用 ? # \\ 把「@localhost」藏进查询串 / 片段 / 路径：URL 解析器认为主机仍是 evil.example
        for sneaky in [
            "http://evil.example?@localhost/v1",
            "http://evil.example#@localhost/v1",
            "http://evil.example\\@localhost/v1",
            "http://evil.example:80?@127.0.0.1/v1",
        ] {
            assert!(!is_loopback(sneaky), "{sneaky} 的真实主机是 evil.example，不能算本地");
        }
        // 解析不了的不算本地
        assert!(!is_loopback("localhost:11434/v1"));
        // 主机名不分大小写
        assert!(is_loopback("http://LocalHost:11434/v1"));
    }
}
