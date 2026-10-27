//! 阅读视图的两个辅助能力：语法标注、划词解析。
//!
//! 都走同一个 provider，但用独立的小提示词：要的是短 JSON，
//! 不套 WordEntry 契约（那是查词卡片用的，太重）。

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::providers as provider_registry;
use crate::providers::openai_compat;
use crate::secrets;

#[derive(Clone, Serialize, Deserialize)]
pub struct GrammarIssue {
    /// 原文里逐字复制的片段。前端靠字符串匹配定位，比让模型给偏移可靠。
    pub fragment: String,
    #[serde(default)]
    pub replacement: String,
    #[serde(default)]
    pub explanation: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WordGloss {
    #[serde(default)]
    pub term: String,
    #[serde(default)]
    pub gloss: String,
    #[serde(default)]
    pub pos: String,
    #[serde(default)]
    pub note: String,
}

/// 从模型输出里抠出第一个 JSON 数组/对象，容忍 ```json 包裹和前后废话。
fn extract_json(raw: &str) -> Result<String, String> {
    let text = raw.trim().trim_start_matches("```json").trim_start_matches("```");
    let text = text.trim_end_matches("```").trim();
    let start = text.find(['[', '{']).ok_or("模型没有返回 JSON")?;
    let end = text.rfind([']', '}']).ok_or("JSON 不完整")?;
    if end <= start {
        return Err("JSON 不完整".to_string());
    }
    Ok(text[start..=end].to_string())
}

async fn ask(app: &AppHandle, system: &str, user: &str, max_tokens: u32) -> Result<String, String> {
    let provider = provider_registry::active(app)?;
    let key = if provider.requires_key {
        secrets::get_key(&provider.id)?
    } else {
        None
    };
    let reasoning = provider_registry::reasoning_fields(&provider.id);
    let body = openai_compat::build_body(
        &provider.model,
        system,
        user,
        0.1,
        reasoning.as_ref(),
        max_tokens,
    );

    let outcome = openai_compat::stream_chat_adaptive(
        crate::client_for(&provider.base_url),
        &provider,
        key.as_deref(),
        &body,
        reasoning.as_ref(),
        &mut |_delta: &str| {},
        None,
    )
    .await
    .map_err(|err| err.message())?;

    Ok(outcome.text)
}

/// 语法/用词检查。返回的每个 fragment 都是原文片段，前端据此画波浪线。
#[tauri::command]
pub async fn check_grammar(app: AppHandle, text: String) -> Result<Vec<GrammarIssue>, String> {
    if text.trim().chars().count() < 20 {
        return Ok(Vec::new());
    }

    let system = "你是英语语法检查器。找出 user 文本中的语法、用词、拼写错误。\n\
        只输出 JSON 数组，元素形如 {\"fragment\":\"...\",\"replacement\":\"...\",\"explanation\":\"...\"}。\n\
        规则：\n\
        1. fragment 必须是原文里**逐字连续**的一段（不超过 80 字符），便于程序定位；\n\
        2. 忽略代码、命令、URL、邮箱、版本号、专有名词、引用块；\n\
        3. explanation 用中文，一句话说清错在哪、为什么；\n\
        4. 没有错误就输出 []。不要输出任何解释性文字。";

    let raw = ask(&app, system, &text, 2000).await?;
    let json = extract_json(&raw)?;
    let mut issues: Vec<GrammarIssue> =
        serde_json::from_str(&json).map_err(|err| format!("解析语法结果失败：{err}"))?;
    issues.retain(|issue| !issue.fragment.trim().is_empty() && text.contains(&issue.fragment));
    Ok(issues)
}

/// 划词解析：这个词在**这句话里**是什么意思。
#[tauri::command]
pub async fn explain_word(
    app: AppHandle,
    term: String,
    context: String,
) -> Result<WordGloss, String> {
    let term = term.trim().to_string();
    if term.is_empty() {
        return Err("没有选中内容".to_string());
    }

    let system = "你是词典。根据上下文解释 user 给的词或短语。\n\
        只输出 JSON 对象：{\"gloss\":\"这个语境下的中文意思\",\"pos\":\"词性缩写，如 n./v./adj.\",\"note\":\"一句话讲解，讲清搭配或易错点\"}。\n\
        不要输出其它文字。";
    let user = serde_json::json!({ "term": term, "context": context }).to_string();

    let raw = ask(&app, system, &user, 600).await?;
    let json = extract_json(&raw)?;
    let mut gloss: WordGloss =
        serde_json::from_str(&json).map_err(|err| format!("解析失败：{err}"))?;
    if gloss.term.is_empty() {
        gloss.term = term;
    }
    Ok(gloss)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_survives_code_fence_and_chatter() {
        let raw = "好的，结果如下：\n```json\n[{\"fragment\":\"a b\"}]\n```\n以上。";
        assert_eq!(extract_json(raw).unwrap(), "[{\"fragment\":\"a b\"}]");
    }

    #[test]
    fn broken_json_is_reported() {
        assert!(extract_json("没有 JSON").is_err());
    }
}
