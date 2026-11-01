//! 翻译的目标语言：偏好的读写，以及给前端的"这段话会怎么译"。
//!
//! 偏好存在 `settings.json` 的 `translate_target`（`auto` 或语言表里的代码），由 Rust 持有：
//! 长文翻译、截屏翻译都在 Rust 里取它，前端只负责读写和显示。取舍规则在 `lang.rs`。

use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::AppHandle;

use crate::lang::{self, Lang, Plan};
use crate::settings;

const SETTINGS_KEY: &str = "translate_target";

fn pref_in(doc: &Value) -> &'static str {
    lang::normalize_pref(doc.get(SETTINGS_KEY).and_then(Value::as_str))
}

/// 当前的目标语言偏好。配置读不出来、或写了认不得的值，都当 auto：偏好不该挡住翻译。
pub fn load_pref(app: &AppHandle) -> &'static str {
    settings::read_value(app).map_or(lang::AUTO, |doc| pref_in(&doc))
}

/// 能存的偏好：`auto` 或语言表里的代码。
fn validate_pref(target: &str) -> Result<&'static str, String> {
    if target == lang::AUTO {
        return Ok(lang::AUTO);
    }
    lang::lang_by_code(target)
        .map(|lang| lang.code)
        .ok_or_else(|| format!("不支持的目标语言：{target}"))
}

fn write_pref(map: &mut Map<String, Value>, pref: &str) {
    map.insert(SETTINGS_KEY.to_string(), json!(pref));
}

/// 一段话的两端语言：长文翻译的事件、结果和 `plan_translation` 共用这个形状。
#[derive(Clone, Serialize)]
pub struct Languages {
    /// 认出的原文语言；认不出为 null
    pub source: Option<&'static Lang>,
    /// 有把握才为 true；false 时界面写"自动识别"，不报语种
    pub source_confident: bool,
    pub target: &'static Lang,
}

impl Languages {
    pub fn of(plan: &Plan) -> Self {
        Self {
            source: plan.source.lang,
            source_confident: plan.source.confident,
            target: plan.target,
        }
    }
}

#[derive(Serialize)]
pub struct LanguageSettings {
    /// `auto` 或语言代码
    pub target: &'static str,
    pub languages: &'static [Lang],
}

/// 选择器要的全部：当前偏好 + 能选的语言（语言表只有 Rust 里这一份）。
#[tauri::command]
pub fn translate_languages(app: AppHandle) -> LanguageSettings {
    LanguageSettings {
        target: load_pref(&app),
        languages: lang::LANGS,
    }
}

/// 存偏好，返回实际存下的值。
#[tauri::command]
pub fn set_translate_target(app: AppHandle, target: String) -> Result<String, String> {
    let pref = validate_pref(&target)?;
    settings::update(&app, |map| write_pref(map, pref))?;
    Ok(pref.to_string())
}

#[derive(Serialize)]
pub struct TranslationPlan {
    #[serde(flatten)]
    pub languages: Languages,
    /// 要不要走查词卡（只有中 ↔ 英；见 `Plan::dictionary`）。调用方先挡掉整句和长文
    pub dictionary: bool,
}

/// 这段输入会怎么译：前端据此在"查词卡"和"普通翻译"之间分流。
#[tauri::command]
pub fn plan_translation(app: AppHandle, text: String) -> TranslationPlan {
    let plan = lang::plan(&text, load_pref(&app));
    TranslationPlan {
        languages: Languages::of(&plan),
        dictionary: plan.dictionary(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_is_read_from_the_settings_document() {
        assert_eq!(pref_in(&json!({})), "auto");
        assert_eq!(pref_in(&json!({ "translate_target": "ja" })), "ja");
        assert_eq!(pref_in(&json!({ "translate_target": "zh-Hant" })), "zh-Hant");
        // 手改坏的配置：认不得的、类型不对的，都当 auto
        assert_eq!(pref_in(&json!({ "translate_target": "klingon" })), "auto");
        assert_eq!(pref_in(&json!({ "translate_target": 5 })), "auto");
        assert_eq!(pref_in(&json!({ "translate_target": null })), "auto");
        assert_eq!(pref_in(&json!({ "hotkey": "x" })), "auto");
    }

    #[test]
    fn only_auto_and_table_codes_can_be_saved() {
        assert_eq!(validate_pref("auto"), Ok("auto"));
        assert_eq!(validate_pref("ja"), Ok("ja"));
        assert_eq!(validate_pref("zh-Hant"), Ok("zh-Hant"));
        assert!(validate_pref("klingon").unwrap_err().contains("klingon"));
        assert!(validate_pref("").is_err());
        assert!(validate_pref("日语").is_err(), "存的是代码，不是中文名");
        assert!(validate_pref("JA").is_err(), "代码大小写精确");
    }

    #[test]
    fn saving_the_preference_leaves_every_other_field_alone() {
        let mut path = std::env::temp_dir();
        path.push(format!("sense-language-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        settings::write_at(
            &path,
            &json!({ "hotkey": "ctrl+k", "active_provider": "deepseek", "providers": { "deepseek": { "key_mask": "sk-****abcd" } }, "未来字段": { "a": 1 } }),
        )
        .expect("写入");

        let doc = settings::update_at(&path, |map| write_pref(map, "ja")).expect("更新");
        assert_eq!(pref_in(&doc), "ja");
        let again = settings::read_at(&path).expect("重新读");
        assert_eq!(pref_in(&again), "ja", "真的落了盘");
        assert_eq!(again["hotkey"], "ctrl+k");
        assert_eq!(again["active_provider"], "deepseek");
        assert_eq!(again.pointer("/providers/deepseek/key_mask").and_then(Value::as_str), Some("sk-****abcd"));
        assert_eq!(again.pointer("/未来字段/a").and_then(Value::as_i64), Some(1), "未知字段不能被冲掉");

        // 改回 auto 也是显式写下的值
        let back = settings::update_at(&path, |map| write_pref(map, "auto")).expect("更新");
        assert_eq!(back["translate_target"], "auto");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_plan_has_the_shape_the_frontend_reads() {
        let plan = lang::plan("こんにちは", lang::AUTO);
        let value = serde_json::to_value(TranslationPlan {
            languages: Languages::of(&plan),
            dictionary: plan.dictionary(),
        })
        .expect("序列化");
        assert_eq!(value["source"], json!({ "code": "ja", "name": "日语", "native": "日本語" }));
        assert_eq!(value["source_confident"], true);
        assert_eq!(value["target"], json!({ "code": "zh", "name": "中文", "native": "简体中文" }));
        assert_eq!(value["dictionary"], false);

        // 认不出的书写系统：source 是 null，不是缺字段
        let plan = lang::plan("নমস্কার", lang::AUTO);
        let value = serde_json::to_value(Languages::of(&plan)).expect("序列化");
        assert!(value["source"].is_null());
        assert_eq!(value["source_confident"], false);

        // 没把握的拉丁字母：有 source（按英文处理），但 source_confident 是 false
        let plan = lang::plan("charge", lang::AUTO);
        let value = serde_json::to_value(Languages::of(&plan)).expect("序列化");
        assert_eq!(value["source"]["code"], "en");
        assert_eq!(value["source_confident"], false);
    }

    #[test]
    fn the_settings_payload_lists_every_language_in_table_order() {
        let value = serde_json::to_value(LanguageSettings { target: "auto", languages: lang::LANGS }).expect("序列化");
        let codes: Vec<&str> = value["languages"]
            .as_array()
            .expect("数组")
            .iter()
            .filter_map(|item| item["code"].as_str())
            .collect();
        assert_eq!(codes.len(), lang::LANGS.len());
        assert_eq!(&codes[..3], ["zh", "zh-Hant", "en"]);
        assert_eq!(value["target"], "auto");
    }
}
