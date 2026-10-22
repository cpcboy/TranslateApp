//! 设置页的命令：provider 列表、保存、Key 管理、测试连接。
//!
//! 这里没有任何命令会把 Key 的明文交给前端。

use serde::Serialize;
use serde_json::json;
use tauri::AppHandle;

use crate::providers::{self, openai_compat};
use crate::settings;
use crate::secrets;

#[derive(Debug, Serialize)]
pub struct ProviderView {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub model: String,
    pub requires_key: bool,
    pub key_masked: Option<String>,
    pub active: bool,
}

#[derive(Debug, Serialize)]
pub struct TestResult {
    pub ok: bool,
    pub model: String,
    pub latency_ms: u128,
    pub message: String,
}

fn key_mask_of(app: &AppHandle, id: &str) -> Option<String> {
    settings::read_value(app)
        .ok()
        .and_then(|value| {
            value
                .pointer(&format!("/providers/{id}/key_mask"))
                .and_then(|item| item.as_str())
                .map(str::to_string)
        })
}

#[tauri::command]
pub fn list_providers(app: AppHandle) -> Vec<ProviderView> {
    let active_id = providers::active_id(&app);
    providers::presets()
        .into_iter()
        .map(|preset| {
            let resolved = providers::resolve(&app, &preset.id).unwrap_or(preset);
            ProviderView {
                key_masked: key_mask_of(&app, &resolved.id),
                active: resolved.id == active_id,
                id: resolved.id,
                label: resolved.label,
                base_url: resolved.base_url,
                model: resolved.model,
                requires_key: resolved.requires_key,
            }
        })
        .collect()
}

#[tauri::command]
pub fn save_provider(
    app: AppHandle,
    id: String,
    base_url: String,
    model: String,
) -> Result<(), String> {
    if !providers::presets().iter().any(|preset| preset.id == id) {
        return Err(format!("未知的 provider：{id}"));
    }
    let base_url = base_url.trim().trim_end_matches('/').to_string();
    let model = model.trim().to_string();
    if base_url.is_empty() || model.is_empty() {
        return Err("base_url 与 model 不能为空".to_string());
    }
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        return Err("base_url 必须以 http:// 或 https:// 开头".to_string());
    }
    // 明文 http 只允许本地模型（Ollama）；远端一律 https，别把 Key 裸奔发出去。
    if base_url.starts_with("http://") {
        let host = base_url
            .trim_start_matches("http://")
            .split(['/', ':'])
            .next()
            .unwrap_or_default();
        let loopback = matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]");
        if !loopback {
            return Err("远端地址请用 https://；明文 http 只允许本地模型".to_string());
        }
    }

    // 逐字段合并：不能整个对象覆盖，否则会把 key_mask 冲掉。
    settings::update(&app, |map| {
        let entry = map
            .entry("providers")
            .or_insert_with(|| json!({}))
            .as_object_mut();
        if let Some(providers) = entry {
            let provider = providers
                .entry(id.clone())
                .or_insert_with(|| json!({}))
                .as_object_mut();
            if let Some(provider) = provider {
                provider.insert("base_url".into(), json!(base_url));
                provider.insert("model".into(), json!(model));
            }
        }
    })?;

    Ok(())
}

#[tauri::command]
pub fn set_active_provider(app: AppHandle, id: String) -> Result<(), String> {
    if !providers::presets().iter().any(|preset| preset.id == id) {
        return Err(format!("未知的 provider：{id}"));
    }
    settings::update(&app, |map| {
        map.insert("active_provider".into(), json!(id));
    })?;
    Ok(())
}

/// 存 Key：钥匙串 + 只把掩码写进配置（用于设置页回显，避免每次读钥匙串）。
#[tauri::command]
pub fn set_api_key(app: AppHandle, id: String, key: String) -> Result<String, String> {
    let trimmed = key.trim();
    secrets::set_key(&id, trimmed)?;
    let mask = secrets::mask(trimmed);

    settings::update(&app, |map| {
        let providers = map
            .entry("providers")
            .or_insert_with(|| json!({}))
            .as_object_mut();
        if let Some(providers) = providers {
            let provider = providers
                .entry(id.clone())
                .or_insert_with(|| json!({}))
                .as_object_mut();
            if let Some(provider) = provider {
                provider.insert("key_mask".into(), json!(mask));
            }
        }
    })?;

    Ok(mask)
}

#[tauri::command]
pub fn clear_api_key(app: AppHandle, id: String) -> Result<(), String> {
    secrets::delete_key(&id)?;
    settings::update(&app, |map| {
        let providers = map.get_mut("providers").and_then(|item| item.as_object_mut());
        if let Some(providers) = providers {
            if let Some(provider) = providers.get_mut(&id).and_then(|item| item.as_object_mut()) {
                provider.remove("key_mask");
            }
        }
    })?;
    Ok(())
}

/// 开发期开关：只在 debug 构建里读环境变量，发布版永远返回 null。
#[tauri::command]
pub fn dev_flags() -> serde_json::Value {
    let dev_lookup = if cfg!(debug_assertions) {
        std::env::var("SENSE_DEV_LOOKUP")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    } else {
        None
    };
    if dev_lookup.is_some() {
        println!("[sense] dev_flags: 触发自动查询 {dev_lookup:?}");
    }
    json!({ "dev_lookup": dev_lookup })
}

/// 测试连接：走 `/models`，免费且能拿到模型名与延迟。
#[tauri::command]
pub async fn test_connection(app: AppHandle, id: String) -> Result<TestResult, String> {
    let provider = providers::resolve(&app, &id)?;

    let key = if provider.requires_key {
        match secrets::get_key(&id)? {
            Some(key) => Some(key),
            None => {
                return Ok(TestResult {
                    ok: false,
                    model: provider.model.clone(),
                    latency_ms: 0,
                    message: "还没填 API Key。去上面填一个，30 秒搞定。".to_string(),
                });
            }
        }
    } else {
        None
    };

    let client = crate::http_client();
    match openai_compat::list_models(&client, &provider, key.as_deref()).await {
        Ok((latency_ms, models)) => {
            if !models.is_empty() && !models.iter().any(|item| item == &provider.model) {
                return Ok(TestResult {
                    ok: false,
                    model: provider.model.clone(),
                    latency_ms,
                    message: format!(
                        "连上了，但服务端没有 `{}` 这个模型。检查一下模型名（共 {} 个可用）。",
                        provider.model,
                        models.len()
                    ),
                });
            }
            Ok(TestResult {
                ok: true,
                model: provider.model.clone(),
                latency_ms,
                message: format!("连接正常，服务端报告 {} 个模型", models.len()),
            })
        }
        Err(err) => Ok(TestResult {
            ok: false,
            model: provider.model.clone(),
            latency_ms: 0,
            message: err.message(),
        }),
    }
}
