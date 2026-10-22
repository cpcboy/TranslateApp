//! 系统提示词：编译期内联，运行时不依赖资源目录。
//!
//! 文件放在 `src-tauri/resources/prompts/`，打包时随 app 分发；
//! `include_str!` 保证 dev 与 release 读到同一份。

use std::sync::OnceLock;

const WORD_PROMPT: &str = include_str!("../resources/prompts/system-word.txt");
#[allow(dead_code)] // D9 校对管线才用
const PROOFREAD_PROMPT: &str = include_str!("../resources/prompts/system-proofread.txt");

/// 原文里唯一带变量的那一行。
const WORD_INPUT_TEMPLATE: &str =
    r#"{"word":"{{WORD}}","lang":"{{LANG}}","level":"{{LEVEL}}","context":{{CONTEXT}}}"#;

const INPUT_NOTE: &str =
    "输入由 user 消息给出：一个 JSON 对象，含 word / lang / level / context 四个字段。";

/// 把变量行换成静态说明。
///
/// 这样系统前缀逐字节稳定（能命中 provider 的 prompt caching），
/// 变量一律由 user 消息承载，也不会因为单词里的引号破坏 JSON。
fn normalize(raw: &str, template: &str) -> String {
    raw.replace(template, INPUT_NOTE).trim_end().to_string()
}

/// prompt 内容指纹，进缓存 key：改提示词自动失效。
pub fn word_prompt_hash() -> &'static str {
    static CELL: OnceLock<String> = OnceLock::new();
    CELL.get_or_init(|| {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(word_system().as_bytes());
        let digest = hasher.finalize();
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    })
}

pub fn word_system() -> &'static str {
    static CELL: OnceLock<String> = OnceLock::new();
    CELL.get_or_init(|| normalize(WORD_PROMPT, WORD_INPUT_TEMPLATE))
}

#[allow(dead_code)] // D9 校对管线才用
pub fn proofread_system() -> &'static str {
    PROOFREAD_PROMPT.trim_end()
}

/// 用户消息就是一个 JSON 行，用 serde_json 转义。
pub fn user_message(word: &str, lang: &str, context: Option<&str>) -> String {
    serde_json::json!({
        "word": word,
        "lang": lang,
        "level": "B2",
        "context": context,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_prompt_keeps_examples_and_drops_placeholders() {
        let system = word_system();
        assert!(system.contains("# 角色"), "角色段必须在");
        assert!(system.contains("# 规则"), "规则段必须在");
        assert!(system.contains("# 输出结构"), "输出结构必须在");
        assert!(system.contains("# 示例"), "示例是质量的另一半，必须在");
        assert!(
            !system.contains("{{"),
            "系统前缀里不能残留变量占位符，否则逐字节不稳定"
        );
        assert!(system.contains(INPUT_NOTE), "应替换成静态输入说明");
    }

    #[test]
    fn word_prompt_is_stable_across_calls() {
        assert_eq!(word_system(), word_system());
        assert_eq!(
            word_system().as_ptr(),
            word_system().as_ptr(),
            "应复用同一份字符串，不该每次重建"
        );
    }

    #[test]
    fn user_message_is_valid_json_and_escapes_quotes() {
        let message = user_message("he said \"hi\"", "en", None);
        let value: serde_json::Value = serde_json::from_str(&message).expect("必须是合法 JSON");
        assert_eq!(value["word"], "he said \"hi\"");
        assert_eq!(value["lang"], "en");
        assert!(value["context"].is_null());
    }

    #[test]
    fn context_is_passed_through() {
        let message = user_message("charge", "en", Some("He was charged with fraud."));
        let value: serde_json::Value = serde_json::from_str(&message).expect("合法 JSON");
        assert_eq!(value["context"], "He was charged with fraud.");
    }
}
