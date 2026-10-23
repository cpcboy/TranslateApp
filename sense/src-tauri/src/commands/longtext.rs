//! 长文翻译：粘贴一大段文章，按段落切块逐块翻译。
//!
//! 和查词是两条完全不同的链路：查词要的是 WordEntry 的 JSON，
//! 长文要的是「把这段话说成人话」，所以用独立的提示词，不套 schema。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::providers as provider_registry;
use crate::providers::openai_compat;
use crate::secrets;

#[derive(Clone, Serialize)]
pub struct LongTextProgress {
    pub done: usize,
    pub total: usize,
}

/// 每块的最大字符数。太小请求多、太大会被上下文截断。
const CHUNK_CHARS: usize = 1500;

/// 按段落聚合成块，尽量不把一个自然段劈开。
fn chunk(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for paragraph in text.split_inclusive('\n') {
        if !current.is_empty() && current.chars().count() + paragraph.chars().count() > CHUNK_CHARS {
            chunks.push(std::mem::take(&mut current));
        }
        // 单段就超长时硬切
        if paragraph.chars().count() > CHUNK_CHARS {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            let mut buffer = String::new();
            for ch in paragraph.chars() {
                buffer.push(ch);
                if buffer.chars().count() >= CHUNK_CHARS {
                    chunks.push(std::mem::take(&mut buffer));
                }
            }
            if !buffer.is_empty() {
                chunks.push(buffer);
            }
            continue;
        }
        current.push_str(paragraph);
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

fn looks_chinese(text: &str) -> bool {
    let cjk = text.chars().filter(|ch| ('\u{4e00}'..='\u{9fff}').contains(ch)).count();
    let latin = text.chars().filter(|ch| ch.is_ascii_alphabetic()).count();
    cjk * 2 >= latin && cjk > 0
}

#[tauri::command]
pub async fn translate_text(app: AppHandle, text: String) -> Result<String, String> {
    let source = text.trim().to_string();
    if source.is_empty() {
        return Err("还没有内容".to_string());
    }

    let provider = provider_registry::active(&app)?;
    let key = if provider.requires_key {
        secrets::get_key(&provider.id)?
    } else {
        None
    };

    let target = if looks_chinese(&source) { "英文" } else { "中文" };
    let system = format!(
        "你是翻译引擎。把 user 给的文本翻译成{target}。\n\
         要求：保留原有分段与换行；专有名词、代码、命令、数字原样保留；\
         只输出译文本身，不要解释、不要加引号或 Markdown 代码块。"
    );

    let chunks = chunk(&source);
    let total = chunks.len();
    let mut output = String::new();

    for (index, piece) in chunks.iter().enumerate() {
        let reasoning = provider_registry::reasoning_fields(&provider.id);
        let body = openai_compat::build_body(
            &provider.model,
            &system,
            piece,
            0.3,
            reasoning.as_ref(),
            openai_compat::MAX_TOKENS,
        );

        let mut raw = String::new();
        let outcome = openai_compat::stream_chat(
            crate::client_for(&provider.base_url),
            &provider,
            key.as_deref(),
            &body,
            &mut |delta: &str| raw.push_str(delta),
        )
        .await
        .map_err(|err| err.message())?;

        output.push_str(outcome.text.trim_end());
        if index + 1 < total {
            output.push_str("\n\n");
        }

        let _ = app.emit(
            "sense://longtext",
            LongTextProgress {
                done: index + 1,
                total,
            },
        );
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_is_split_without_losing_content() {
        let text = "第一段。\n".repeat(400);
        let chunks = chunk(&text);
        assert!(chunks.len() > 1);
        assert_eq!(chunks.concat(), text);
        assert!(chunks.iter().all(|item| item.chars().count() <= CHUNK_CHARS));
    }

    #[test]
    fn target_language_follows_source() {
        assert!(looks_chinese("这是一段中文"));
        assert!(!looks_chinese("this is english"));
    }
}
