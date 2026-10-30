//! 系统提示词：编译期内联，运行时不依赖资源目录。
//!
//! 文件放在 `src-tauri/resources/prompts/`，打包时随 app 分发；
//! `include_str!` 保证 dev 与 release 读到同一份。

use std::sync::OnceLock;

const WORD_PROMPT: &str = include_str!("../resources/prompts/system-word.txt");
/// 中文输入的补充：先译成英文，再解析那个英文词。只追加在查词提示词后面，不改上文。
const WORD_ZH_PROMPT: &str = include_str!("../resources/prompts/system-word-zh.txt");
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

/// 一种输入语言对应的系统提示词，连同它的内容指纹。
struct WordPrompt {
    system: String,
    hash: String,
}

fn word_prompt(system: String) -> WordPrompt {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(system.as_bytes());
    let digest = hasher.finalize();
    WordPrompt {
        hash: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
        system,
    }
}

/// 按输入语言取查词提示词。
///
/// 英文输入用原提示词，一个字节都没动：英文词条的缓存 key、provider 的 prompt caching 都不受影响。
/// 中文输入在原提示词后面追加「先译成英文、再解析英文词」的一节，自己有一份稳定的前缀和指纹。
fn word_prompt_for(lang: &str) -> &'static WordPrompt {
    static EN: OnceLock<WordPrompt> = OnceLock::new();
    static ZH: OnceLock<WordPrompt> = OnceLock::new();
    let base = || normalize(WORD_PROMPT, WORD_INPUT_TEMPLATE);
    if lang == "zh" {
        ZH.get_or_init(|| word_prompt(format!("{}\n\n{}", base(), WORD_ZH_PROMPT.trim_end())))
    } else {
        EN.get_or_init(|| word_prompt(base()))
    }
}

/// prompt 内容指纹，进缓存 key：改提示词自动失效。中英文各有各的，互不连累。
pub fn word_prompt_hash(lang: &str) -> &'static str {
    &word_prompt_for(lang).hash
}

pub fn word_system(lang: &str) -> &'static str {
    &word_prompt_for(lang).system
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
        for lang in ["en", "zh"] {
            let system = word_system(lang);
            assert!(system.contains("# 角色"), "{lang}：角色段必须在");
            assert!(system.contains("# 规则"), "{lang}：规则段必须在");
            assert!(system.contains("# 输出结构"), "{lang}：输出结构必须在");
            assert!(system.contains("# 示例"), "{lang}：示例是质量的另一半，必须在");
            assert!(
                !system.contains("{{"),
                "{lang}：系统前缀里不能残留变量占位符，否则逐字节不稳定"
            );
            assert!(system.contains(INPUT_NOTE), "{lang}：应替换成静态输入说明");
        }
    }

    #[test]
    fn word_prompt_is_stable_across_calls() {
        for lang in ["en", "zh"] {
            assert_eq!(word_system(lang), word_system(lang));
            assert_eq!(
                word_system(lang).as_ptr(),
                word_system(lang).as_ptr(),
                "{lang}：应复用同一份字符串，不该每次重建"
            );
            assert_eq!(word_prompt_hash(lang), word_prompt_hash(lang));
        }
    }

    #[test]
    fn english_prompt_is_the_original_and_chinese_only_appends() {
        let english = word_system("en");
        let chinese = word_system("zh");

        // 英文：原提示词，没有中文输入那一节——英文词条的缓存 key 与 prompt caching 因此不变
        assert!(!english.contains("# 中文输入"), "英文提示词不能带中文输入那一节");
        assert_eq!(english, normalize(WORD_PROMPT, WORD_INPUT_TEMPLATE));

        // 中文：原提示词逐字在前，后面追加一节
        assert!(chinese.starts_with(english), "中文提示词必须以英文提示词原文开头");
        let appended = &chinese[english.len()..];
        assert!(appended.contains("# 中文输入"), "追加的一节必须在");
        assert!(appended.contains("先翻译，再解析"), "要说清楚先翻译再解析");
        assert!(appended.contains("\"en\""), "要要求词条 lang 填 en");
        assert!(appended.contains("recharge"), "选词的例子必须在（多义词要换成义项靠前的译词）");
        // 语种要求写成「哪些字段只写英文」的负面清单，没写成「中文只出现在…」的白名单：
        // 白名单漏了 scene / domains，模型会照字面把它们也写成英文，zod 又会把非法 scene 悄悄改成「日常」
        assert!(
            appended.contains("scene 与 domains 仍然是中文"),
            "必须交代 scene / domains 仍是中文"
        );
    }

    #[test]
    fn prompts_for_the_two_languages_have_separate_fingerprints() {
        assert_ne!(word_prompt_hash("en"), word_prompt_hash("zh"), "改一边不能连累另一边的缓存");
        assert_eq!(word_prompt_hash("en").len(), 64);
        assert_eq!(word_prompt_hash("zh").len(), 64);
        // 不认识的语言按英文处理，不会各自生出一份新提示词
        assert_eq!(word_prompt_hash("fr"), word_prompt_hash("en"));
        assert_eq!(word_system("fr").as_ptr(), word_system("en").as_ptr());
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
