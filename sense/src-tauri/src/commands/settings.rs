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

/// 校验并规整一组 (base_url, model)：保存和「测试连接」用同一套规则，
/// 不然会出现「测试通过、保存却被拒」或反过来。
fn validate_endpoint(base_url: &str, model: &str) -> Result<(String, String), String> {
    let base_url = base_url.trim().trim_end_matches('/').to_string();
    let model = model.trim().to_string();
    if base_url.is_empty() || model.is_empty() {
        return Err("base_url 与 model 不能为空".to_string());
    }
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        return Err("base_url 必须以 http:// 或 https:// 开头".to_string());
    }
    // 明文 http 只允许本地模型（Ollama）；远端一律 https，别把 Key 裸奔发出去。
    // 「是不是本地」和选客户端用同一个判断：以前这里自己按 ':' 切主机名，
    // `http://localhost:80@evil.example` 会被当成本地放行（真正的主机是 @ 后面的），Key 就明文发给了 evil.example；
    // `http://[::1]:11434` 这种合法的本地写法反而被拒
    if base_url.starts_with("http://") && !crate::is_loopback(&base_url) {
        return Err("远端地址请用 https://；明文 http 只允许本地模型".to_string());
    }
    Ok((base_url, model))
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
    let (base_url, model) = validate_endpoint(&base_url, &model)?;

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
///
/// 测的是**表单里当前的值**，不是已保存的：`base_url` / `model` / `key` 有值就用它们，
/// 都不落盘。以前总是测已保存的配置，用户改了地址点「测试连接」，测的却是旧地址，
/// 通过了保存后才发现新地址不通。`key` 只在这一次请求里用，不写钥匙串、不写日志。
#[tauri::command]
pub async fn test_connection(
    app: AppHandle,
    id: String,
    base_url: Option<String>,
    model: Option<String>,
    key: Option<String>,
) -> Result<TestResult, String> {
    let mut provider = providers::resolve(&app, &id)?;
    if base_url.is_some() || model.is_some() {
        let (url, name) = validate_endpoint(
            base_url.as_deref().unwrap_or(&provider.base_url),
            model.as_deref().unwrap_or(&provider.model),
        )?;
        provider.base_url = url;
        provider.model = name;
    }

    let typed = key.as_deref().map(str::trim).filter(|item| !item.is_empty());
    let key = match typed {
        Some(typed) => Some(typed.to_string()),
        None if provider.requires_key => secrets::get_key(&id)?,
        None => None,
    };
    if provider.requires_key && key.is_none() {
        return Ok(TestResult {
            ok: false,
            model: provider.model.clone(),
            latency_ms: 0,
            message: "还没填 API Key。去上面填一个，30 秒搞定。".to_string(),
        });
    }

    // 本地模型要绕开系统代理，和真正查词走同一条路，否则「测试通过、查词不通」
    let client = crate::client_for(&provider.base_url);
    match openai_compat::list_models(client, &provider, key.as_deref()).await {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_is_trimmed_and_validated_like_save() {
        assert_eq!(
            validate_endpoint(" https://api.deepseek.com/v1/ ", " deepseek-chat ").unwrap(),
            ("https://api.deepseek.com/v1".to_string(), "deepseek-chat".to_string())
        );
        assert!(validate_endpoint("http://127.0.0.1:11434/v1", "m").is_ok(), "本地明文 http 放行");
        assert!(validate_endpoint("http://api.example.com/v1", "m").is_err(), "远端明文 http 拒绝");
        assert!(validate_endpoint("http://[::1]:11434/v1", "m").is_ok(), "IPv6 本地地址放行");
        assert!(validate_endpoint("http://LocalHost:11434/v1", "m").is_ok(), "主机名不分大小写");
        assert!(
            validate_endpoint("http://localhost:80@evil.example/v1", "m").is_err(),
            "userinfo 里写 localhost 不能骗过本地判断：Key 会明文发给 @ 后面的主机"
        );
        assert!(validate_endpoint("http://127.0.0.1:1@evil.example/v1", "m").is_err());
        for sneaky in [
            "http://evil.example?@localhost/v1",
            "http://evil.example#@localhost/v1",
            "http://evil.example\\@localhost/v1",
            "http://evil.example:80?@127.0.0.1/v1",
        ] {
            assert!(validate_endpoint(sneaky, "m").is_err(), "{sneaky}：真实主机是远端，明文 http 必须拒绝");
        }
        assert!(validate_endpoint("ftp://x", "m").is_err());
        assert!(validate_endpoint("", "m").is_err());
        assert!(validate_endpoint("https://a/v1", "  ").is_err());
    }
}
