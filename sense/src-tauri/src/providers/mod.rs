//! Provider 预设与解析。
//!
//! 预设只是起点：用户在设置里改过的 base_url / model / requires_key
//! 会覆盖预设值，并写进 `settings.json` 的 `providers` 字段。

pub mod openai_compat;

use serde::{Deserialize, Serialize};

use crate::settings;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub model: String,
    pub requires_key: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProviderOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_key: Option<bool>,
}

/// 内置预设。顺序就是设置页里的展示顺序。
pub fn presets() -> Vec<Provider> {
    vec![
        Provider {
            id: "deepseek".into(),
            label: "DeepSeek".into(),
            base_url: "https://api.deepseek.com/v1".into(),
            model: "deepseek-chat".into(),
            requires_key: true,
        },
        Provider {
            id: "qwen".into(),
            label: "通义千问".into(),
            base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1".into(),
            model: "qwen-plus".into(),
            requires_key: true,
        },
        Provider {
            id: "openrouter".into(),
            label: "OpenRouter".into(),
            base_url: "https://openrouter.ai/api/v1".into(),
            model: "deepseek/deepseek-chat-v3.1".into(),
            requires_key: true,
        },
        Provider {
            id: "ollama".into(),
            label: "Ollama（本地）".into(),
            base_url: "http://127.0.0.1:11434/v1".into(),
            model: "qwen2.5:7b".into(),
            requires_key: false,
        },
    ]
}

/// 预设 + 用户覆盖。找不到 id 时返回错误，不猜。
pub fn resolve(app: &tauri::AppHandle, id: &str) -> Result<Provider, String> {
    let mut provider = presets()
        .into_iter()
        .find(|item| item.id == id)
        .ok_or_else(|| format!("未知的 provider：{id}"))?;

    if let Ok(value) = settings::read_value(app) {
        if let Some(over) = value
            .pointer(&format!("/providers/{id}"))
            .and_then(|item| serde_json::from_value::<ProviderOverride>(item.clone()).ok())
        {
            if let Some(base_url) = over.base_url {
                if !base_url.trim().is_empty() {
                    provider.base_url = base_url.trim().to_string();
                }
            }
            if let Some(model) = over.model {
                if !model.trim().is_empty() {
                    provider.model = model.trim().to_string();
                }
            }
            if let Some(requires_key) = over.requires_key {
                provider.requires_key = requires_key;
            }
        }
    }

    Ok(provider)
}

/// 各家的「关掉思考」写法不同，认 id 不认模型名。
///
/// DeepSeek V4 系列的思考模式**默认开启**、effort 默认 high：不显式关掉，
/// 一次查词要先吐几 k token 的思维链，卡片就慢得没法看。
/// 参数名取自各家文档，写错不会静默降级——服务端会回 400，
/// 调用方会去掉这些字段再试一次（见 commands/lookup.rs）。
pub fn reasoning_fields(id: &str) -> Option<serde_json::Value> {
    match id {
        // OpenAI 格式：{"thinking": {"type": "enabled/disabled"}}
        "deepseek" => Some(serde_json::json!({ "thinking": { "type": "disabled" } })),
        // DashScope 兼容模式用 enable_thinking（qwen3 及之后的混合思考模型）
        "qwen" => Some(serde_json::json!({ "enable_thinking": false })),
        // OpenRouter 统一 reasoning 开关
        "openrouter" => Some(serde_json::json!({ "reasoning": { "enabled": false } })),
        // Ollama 的 OpenAI 兼容层认 reasoning_effort；不认的版本会忽略未知字段
        "ollama" => Some(serde_json::json!({ "reasoning_effort": "none" })),
        _ => None,
    }
}

/// 截屏翻译用：打开思考但压到 low，先理解整段再翻，质量优先。
///
/// 查词要快所以关掉；截屏是一次性动作，多等一两秒换准确度是值的。
pub fn understanding_fields(id: &str) -> Option<serde_json::Value> {
    match id {
        "deepseek" => Some(serde_json::json!({
            "thinking": { "type": "enabled" },
            "reasoning_effort": "low"
        })),
        "qwen" => Some(serde_json::json!({ "enable_thinking": true })),
        "openrouter" => Some(serde_json::json!({
            "reasoning": { "enabled": true, "effort": "low" }
        })),
        _ => None,
    }
}

/// 当前选中的 provider：settings.active_provider 优先，缺省 deepseek。
pub fn active_id(app: &tauri::AppHandle) -> String {
    settings::get_string(app, "active_provider").unwrap_or_else(|| "deepseek".to_string())
}

pub fn active(app: &tauri::AppHandle) -> Result<Provider, String> {
    resolve(app, &active_id(app))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_can_turn_thinking_off() {
        for preset in presets() {
            let fields = reasoning_fields(&preset.id);
            assert!(fields.is_some(), "{} 缺关思考参数，查词会慢", preset.id);
            let text = fields.unwrap().to_string();
            assert!(
                text.contains("disabled") || text.contains("false") || text.contains("\"none\""),
                "{} 的关思考参数看起来没关掉：{text}",
                preset.id
            );
        }
        assert!(reasoning_fields("unknown-provider").is_none());
    }

    #[test]
    fn presets_are_unique_and_complete() {
        let items = presets();
        assert!(items.len() >= 4, "至少四个预设");
        let mut ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), items.len(), "预设 id 不能重复");
        for item in &items {
            assert!(!item.base_url.is_empty(), "{} 缺 base_url", item.id);
            assert!(!item.model.is_empty(), "{} 缺 model", item.id);
        }
    }

    #[test]
    fn only_ollama_skips_key() {
        let items = presets();
        let ollama = items.iter().find(|item| item.id == "ollama").unwrap();
        assert!(!ollama.requires_key, "本地模型不需要 Key");
        assert!(items
            .iter()
            .filter(|item| item.id != "ollama")
            .all(|item| item.requires_key));
    }
}
