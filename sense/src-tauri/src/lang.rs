//! 多语言翻译的规则：语言表、原文语言识别、目标语言的取舍。
//!
//! 需求（负责人 2026-10-31）：自动识别原文语言；默认译成中文，原文是中文时默认译成英文；
//! 另提供多国语言的目标语言选项。长文翻译、截屏翻译、查词的分流都从这里取结论，各处不再自己判断。
//!
//! 规则：
//! 1. 偏好是 `auto`（默认）：原文是中文 → 英文，其它 → 中文。
//! 2. 偏好是语言表里的某种语言：就译成它；但原文**确认**就是这种语言时（译成自己没有意义），退回第 1 条。
//!    简体 → 繁体不算同一种：那是一次有意义的转换。
//! 3. 查词卡（义项卡）只做中 ↔ 英：契约里 `lang` 只有 en / zh，冻结（见 [`Plan::dictionary`]）。
//!    别的语言、或者手选了别的目标语言，一律走普通翻译。
//!
//! 识别只靠本地规则，不发请求、不加依赖：先看文字用的书写系统（假名 → 日语、谚文 → 韩语、
//! 西里尔 → 俄语 / 乌克兰语……），拉丁字母再按高频虚词和特征字母打分。**有把握才标 `confident`**；
//! 没把握（太短、没有证据）时按"拉丁字母 = 英文"处理，但不声称认出了语种，界面写"自动识别"。
//! 识别错了的代价是被动的：目标语言由偏好决定，原文语种只影响"默认方向""是否同语种""要不要发语法检查"。

use std::borrow::Cow;

use serde::Serialize;

/// 语言表里的一种语言。
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Lang {
    /// 稳定代码（BCP 47 的简写）：存进配置、前后端对话都用它。
    pub code: &'static str,
    /// 中文名：提示词里写"翻译成{name}"，界面里也用它。
    /// 中文、英文沿用两条老提示词里一直写的"中文""英文"，所以默认方向的提示词一个字节都没变。
    pub name: &'static str,
    /// 这种语言自己的写法，选择器里跟中文名并排放，认得出来。
    pub native: &'static str,
}

/// 偏好的取值之一：不手选，按原文语言自动取舍。
pub const AUTO: &str = "auto";

/// 能选的目标语言，同时也是能认出来的原文语言（顺序就是选择器里的顺序）。
pub const LANGS: &[Lang] = &[
    Lang { code: "zh", name: "中文", native: "简体中文" },
    Lang { code: "zh-Hant", name: "繁体中文", native: "繁體中文" },
    Lang { code: "en", name: "英文", native: "English" },
    Lang { code: "ja", name: "日语", native: "日本語" },
    Lang { code: "ko", name: "韩语", native: "한국어" },
    Lang { code: "fr", name: "法语", native: "Français" },
    Lang { code: "de", name: "德语", native: "Deutsch" },
    Lang { code: "es", name: "西班牙语", native: "Español" },
    Lang { code: "pt", name: "葡萄牙语", native: "Português" },
    Lang { code: "it", name: "意大利语", native: "Italiano" },
    Lang { code: "ru", name: "俄语", native: "Русский" },
    Lang { code: "uk", name: "乌克兰语", native: "Українська" },
    Lang { code: "ar", name: "阿拉伯语", native: "العربية" },
    Lang { code: "he", name: "希伯来语", native: "עברית" },
    Lang { code: "th", name: "泰语", native: "ไทย" },
    Lang { code: "vi", name: "越南语", native: "Tiếng Việt" },
    Lang { code: "id", name: "印度尼西亚语", native: "Bahasa Indonesia" },
    Lang { code: "tr", name: "土耳其语", native: "Türkçe" },
    Lang { code: "hi", name: "印地语", native: "हिन्दी" },
    Lang { code: "nl", name: "荷兰语", native: "Nederlands" },
    Lang { code: "pl", name: "波兰语", native: "Polski" },
    Lang { code: "el", name: "希腊语", native: "Ελληνικά" },
];

pub fn lang_by_code(code: &str) -> Option<&'static Lang> {
    LANGS.iter().find(|lang| lang.code == code)
}

/// 表里一定有的代码（`known` 的调用点都是写死的字面量，单测逐个盯着）。
fn known(code: &str) -> &'static Lang {
    lang_by_code(code).expect("语言表里必须有这个代码")
}

/// 把配置里读到的偏好规整成 `auto` 或语言表里有的代码。
/// 认不得的一律当 auto：配置被手改坏了，不该挡住翻译。
pub fn normalize_pref(raw: Option<&str>) -> &'static str {
    raw.and_then(lang_by_code).map_or(AUTO, |lang| lang.code)
}

fn is_chinese(lang: &Lang) -> bool {
    lang.code.starts_with("zh")
}

// ───────────────────────── 识别 ─────────────────────────

/// 识别结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Detected {
    /// 认出的语言；认不出（只有数字符号，或文字不在语言表里）为 None。
    pub lang: Option<&'static Lang>,
    /// 有把握才为 true：文字本身决定了语言（假名、谚文、西里尔……），或拉丁字母里证据明确。
    /// 没把握时 `lang` 是 "拉丁字母 = 英文" 的默认值，不能当成认出了英文。
    pub confident: bool,
    /// 文本里有没有任何字母（只有数字、标点、符号、表情时为 false）。
    pub letters: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Script {
    Han,
    Kana,
    Hangul,
    Latin,
    Cyrillic,
    Arabic,
    Hebrew,
    Thai,
    Devanagari,
    Greek,
    /// 字母，但书写系统不在表里（孟加拉文、格鲁吉亚文……）
    Other,
}

/// 一个字符属于哪种书写系统；不是字母（数字、标点、符号、空白、表情）返回 None。
fn script_of(ch: char) -> Option<Script> {
    if !ch.is_alphabetic() {
        return None;
    }
    Some(match ch {
        '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}' | '\u{20000}'..='\u{2fa1f}' => {
            Script::Han
        }
        // 中点 ・ (U+30FB) 不是字母，进不到这里；长音符 ー (U+30FC) 只在日语里用
        '\u{3040}'..='\u{309f}' | '\u{30a0}'..='\u{30ff}' | '\u{31f0}'..='\u{31ff}' | '\u{ff66}'..='\u{ff9f}' => {
            Script::Kana
        }
        '\u{1100}'..='\u{11ff}' | '\u{3130}'..='\u{318f}' | '\u{ac00}'..='\u{d7af}' => Script::Hangul,
        'a'..='z' | 'A'..='Z' | '\u{00c0}'..='\u{024f}' | '\u{1e00}'..='\u{1eff}' => Script::Latin,
        '\u{0400}'..='\u{052f}' => Script::Cyrillic,
        '\u{0600}'..='\u{06ff}' | '\u{0750}'..='\u{077f}' | '\u{fb50}'..='\u{fdff}' | '\u{fe70}'..='\u{feff}' => {
            Script::Arabic
        }
        '\u{0590}'..='\u{05ff}' => Script::Hebrew,
        '\u{0e00}'..='\u{0e7f}' => Script::Thai,
        '\u{0900}'..='\u{097f}' => Script::Devanagari,
        '\u{0370}'..='\u{03ff}' | '\u{1f00}'..='\u{1fff}' => Script::Greek,
        _ => Script::Other,
    })
}

#[derive(Default)]
struct Counts {
    han: usize,
    kana: usize,
    hangul: usize,
    latin: usize,
    cyrillic: usize,
    arabic: usize,
    hebrew: usize,
    thai: usize,
    devanagari: usize,
    greek: usize,
    other: usize,
}

impl Counts {
    fn of(text: &str) -> Self {
        let mut counts = Counts::default();
        for script in text.chars().filter_map(script_of) {
            match script {
                Script::Han => counts.han += 1,
                Script::Kana => counts.kana += 1,
                Script::Hangul => counts.hangul += 1,
                Script::Latin => counts.latin += 1,
                Script::Cyrillic => counts.cyrillic += 1,
                Script::Arabic => counts.arabic += 1,
                Script::Hebrew => counts.hebrew += 1,
                Script::Thai => counts.thai += 1,
                Script::Devanagari => counts.devanagari += 1,
                Script::Greek => counts.greek += 1,
                Script::Other => counts.other += 1,
            }
        }
        counts
    }

    fn cjk(&self) -> usize {
        self.han + self.kana + self.hangul
    }

    fn letters(&self) -> usize {
        self.cjk()
            + self.latin
            + self.cyrillic
            + self.arabic
            + self.hebrew
            + self.thai
            + self.devanagari
            + self.greek
            + self.other
    }
}

/// 引号、书名号、括号括起来的部分去掉之后剩下的文字（只看开头 `SCAN_CHARS` 个字符）。
/// 没有配上对的开括号原样保留；`"` 开合同形，按先后配对。英文里的 `’` 是撇号，不当引号。
fn outside_quotes(text: &str) -> String {
    const PAIRS: [(char, char); 10] = [
        ('“', '”'),
        ('‘', '’'),
        ('「', '」'),
        ('『', '』'),
        ('《', '》'),
        ('〈', '〉'),
        ('（', '）'),
        ('(', ')'),
        ('【', '】'),
        ('[', ']'),
    ];
    // 一对引号之间最多看这么远：一长串没配上对的开括号，不该把扫描拖成平方
    const MAX_QUOTED: usize = 400;

    let chars: Vec<char> = text.chars().take(SCAN_CHARS).collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        let closer = if ch == '"' { Some('"') } else { PAIRS.iter().find(|(open, _)| *open == ch).map(|(_, close)| *close) };
        if let Some(close) = closer {
            if let Some(offset) = chars[index + 1..].iter().take(MAX_QUOTED).position(|candidate| *candidate == close) {
                index += offset + 2; // 连引号带内容一起跳过
                continue;
            }
        }
        out.push(ch);
        index += 1;
    }
    out
}

/// 这些字日语、韩语不用：简体中文里常见、而日语用另一种写法（们 → 達、这 → 這、说 → 説、语 → 語、电 → 電……）的字，
/// 加上只有中文才有的字（你、您、吗、吧、呢、啊、谁、哪、怎）。文字里有一个，就不是日语 / 韩语。
/// （「机」「国」「学」「会」这类日语里也用的字不在里面。）
const CHINESE_ONLY: &str = "们这个说话为对时发过还开关么吗吧呢啊请谢译语书门见车长马鸟鱼风电买卖读听课问题认让给从边进动现样经业应间线义实两东谁哪怎你您嘛咱啥";

fn has_chinese_only(text: &str) -> bool {
    text.chars().any(|ch| CHINESE_ONLY.contains(ch))
}

/// 汉字圈（汉字 + 假名 + 谚文）里是哪一种。
///
/// **日语**：有假名，并且（没有汉字，或平假名紧挨着汉字——日语的送假名和助词（検索履歴**を**消去、経済安全保障**で**）几乎总是这样，
/// 或假名有汉字的两倍）。**韩语**：有谚文，并且（没有汉字，或谚文有汉字的两倍）。
///
/// 有歧义时偏向中文，因为两种误判的代价不对称：把中文句认成日语 / 韩语，结果是"译成中文"的空转（最糟）；
/// 把夹少量汉字的日语 / 韩语认成中文，只是被译成英文（仍是一次有用的翻译）。所以中文句里夹一个片假名词（`我喜欢看アニメ`）、
/// 一个韩文词（`我在用 카카오톡`）、引一句日语（`“ありがとう”是谢谢的意思`）仍是中文；
/// 代价是 `東京タワー`、`韓國語 한국어` 这种汉字和假名 / 谚文混着的短语也认成中文。纯汉字的日语（`東京`）和中文分不开，同样是已知局限。
fn cjk_code(counts: &Counts, text: &str) -> &'static str {
    // 中文句子里夹的日语词、韩语词（`日语里的ありがとう是谢谢的意思`）：引号外面有一个中文独有的字，整句就是中文。
    // 只查引号外面：`「你好」は中国語です。` 是日语的句子，引了一句中文
    if has_chinese_only(&outside_quotes(text)) {
        return "zh";
    }
    if counts.hangul > 0 && counts.hangul >= counts.kana && (counts.han == 0 || counts.hangul >= counts.han * 2) {
        "ko"
    } else if counts.kana > 0 && (counts.han == 0 || counts.kana >= counts.han * 2 || hiragana_beside_han(text)) {
        "ja"
    } else {
        "zh"
    }
}

/// 有没有一个平假名紧挨着汉字（前面或后面）。
fn hiragana_beside_han(text: &str) -> bool {
    let is_hiragana = |ch: char| matches!(ch, '\u{3041}'..='\u{3096}' | '\u{309d}'..='\u{309f}');
    let is_han = |ch: char| script_of(ch) == Some(Script::Han);
    // 组合符号（NFD 里 で 写成 て + U+3099）不算一个字：跳过它，才不会把平假名和汉字隔开
    let is_mark = |ch: char| is_combining(ch) || matches!(ch, '\u{3099}' | '\u{309a}');
    let mut previous: Option<char> = None;
    for ch in text.chars().filter(|ch| !is_mark(*ch)) {
        if let Some(before) = previous {
            if (is_hiragana(before) && is_han(ch)) || (is_han(before) && is_hiragana(ch)) {
                return true;
            }
        }
        previous = Some(ch);
    }
    false
}

/// 识别一段文字的语言。
pub fn detect(text: &str) -> Detected {
    let counts = Counts::of(text);
    if counts.letters() == 0 {
        return Detected { lang: None, confident: false, letters: false };
    }

    // 书写系统按字母数比。汉字圈一个字顶两三个拉丁字母，权重 ×2——这就是截屏翻译一直用的
    // 「汉字 ×2 ≥ 拉丁字母就是中文」；并列时排在前面的赢（汉字圈在最前）。
    #[derive(Clone, Copy)]
    enum Pick {
        Cjk,
        Latin,
        Cyrillic,
        Arabic,
        Hebrew,
        Thai,
        Devanagari,
        Greek,
        Other,
    }
    let candidates = [
        (Pick::Cjk, counts.cjk() * 2),
        (Pick::Latin, counts.latin),
        (Pick::Cyrillic, counts.cyrillic),
        (Pick::Arabic, counts.arabic),
        (Pick::Hebrew, counts.hebrew),
        (Pick::Thai, counts.thai),
        (Pick::Devanagari, counts.devanagari),
        (Pick::Greek, counts.greek),
        (Pick::Other, counts.other),
    ];
    let (pick, _) = candidates
        .iter()
        .fold(candidates[0], |best, item| if item.1 > best.1 { *item } else { best });

    let sure = |code: &str| Detected { lang: Some(known(code)), confident: true, letters: true };
    // 中文句子里引的外文（`请把“よろしくお願いします”翻译成中文`、`“Привет”是什么意思`）：引号 / 书名号 / 括号外面
    // 是一句像样的中文，引文相对短，整句就是中文——引号里的内容不决定整句是什么语言。要同时满足：
    // ①外面只有汉字、至少 4 个，并且有一个中文独有的字（`请`、`译`、`么`……：它是一句中文，不是一个标签或人名）；
    // ②引文的字母数不超过外面汉字数的 3 倍（引文压倒外面时，外面只是个标签：`错误信息（Unable to connect to the server）`
    // 是英文消息、`村上春樹『ノルウェイの森』` 是日文书名，按整段判）；其中拉丁字母仍按 2 倍——和上面「汉字 ×2 ≥ 拉丁字母」
    // 是同一把尺，引的是英文时结果和原来的规则一样（`请翻译下面这句话：“Where is the station?”` 是英文，译成中文才是想要的）。
    // 整段都在引号里时外面什么也没有，同样按整段判。
    if counts.han > 0 {
        let head: String = text.chars().take(SCAN_CHARS).collect();
        let outside = outside_quotes(&head);
        let outside_counts = Counts::of(&outside);
        let whole = Counts::of(&head);
        let quoted = whole.letters().saturating_sub(outside_counts.letters());
        // 外面只有汉字（下面的条件保证），所以整段里的拉丁字母都在引号里
        if outside_counts.han >= 4
            && outside_counts.letters() == outside_counts.han
            && quoted <= outside_counts.han * 3
            && whole.latin <= outside_counts.han * 2
            && has_chinese_only(&outside)
        {
            return sure("zh");
        }
    }
    match pick {
        Pick::Cjk => sure(cjk_code(&counts, text)),
        Pick::Latin => {
            let (lang, confident) = latin_language(text);
            Detected { lang: Some(lang), confident, letters: true }
        }
        // 俄语还是乌克兰语按特征字母和虚词打分；保加利亚语、塞尔维亚语……没有证据，按俄语处理但不声称（已知局限）
        Pick::Cyrillic => {
            let (lang, confident) = cyrillic_language(text);
            Detected { lang: Some(lang), confident, letters: true }
        }
        // 波斯语、乌尔都语用阿拉伯字母但有自己的字：这些字出现就不声称是阿拉伯语
        Pick::Arabic => Detected {
            lang: Some(known("ar")),
            confident: !text.chars().any(|ch| "پچژگکیٹڈڑںے".contains(ch)),
            letters: true,
        },
        Pick::Hebrew => sure("he"),
        Pick::Thai => sure("th"),
        Pick::Devanagari => sure("hi"),
        Pick::Greek => sure("el"),
        Pick::Other => Detected { lang: None, confident: false, letters: true },
    }
}

// ───────────────────────── 拉丁字母：哪一种语言 ─────────────────────────

/// 打分的权重。最高分要到 `SURE` 才算"有把握"：两个虚词，或一个特征词，或一个特征字母；
/// 英文例外：至少要有一个核心词，门槛是 `SURE - SOFT`（见 `latin_language`）。
/// 英文里与别的语言撞形的"软词"（in、is、no、me、to、do……荷兰语、西班牙语、波兰语里同样高频）只算半分：
/// 否则一句荷兰语里的两个 `in` 就能把它"确认"成英文。
const WORD: u32 = 2;
const SOFT: u32 = 1;
const MARK: u32 = 4;
const STRONG: u32 = 6;
const MEDIUM: u32 = 4;
/// 共用的音符（à ê ô ç ã…）：每种语言最多加这么多分，单靠它们够不到 `SURE`
const HINT: u32 = 2;
const SURE: u32 = 4;

/// 只看开头这么多个字符：判语种用不着整篇，几千字符的证据足够，也免得百万字的粘贴白白算很久。
const SCAN_CHARS: usize = 3000;

struct LatinLang {
    code: &'static str,
    /// 高频虚词，命中一个加 `WORD` 分。该语言里很常见的词都收，**包括外语之间共有的**（en、la、de、un……）——
    /// 共有的词给每个收了它的语言都加分，不影响外语之间的排名，却能让真正的那种语言不被只收了这个词的邻居抢走。
    /// 只排除在**英文**里也高频的词（no、do、war、to、as、com、eu、var、ben、over……）和单个 ASCII 字母，
    /// 那些词出现在英文里会白白给外语加分。
    words: &'static [&'static str],
    /// 只有英文用：英文里与别的语言撞形的高频词，命中一个只加 `SOFT` 分（见上面的权重说明）。
    /// 荷兰语的 of / back / up / out / had、西班牙语的 he / use、波兰语的 one、德语的 also 在真实语料里都出现过：
    /// 当核心词会把外语句子"确认"成英文（`No he recibido el correo`、`Even wachten of annuleren`、`Also was jetzt?`）。
    soft: &'static [&'static str],
    /// 问候、致谢一类的特征词，命中一个加 `MARK` 分：一两个词的输入（`bonjour`、`danke`）也要认得出来。
    marks: &'static [&'static str],
}

const EN: usize = 0;
const FR: usize = 1;
const DE: usize = 2;
const ES: usize = 3;
const PT: usize = 4;
const IT: usize = 5;
const NL: usize = 6;
const PL: usize = 7;
const TR: usize = 8;
const VI: usize = 9;
// 印尼语（下标 10）没有特征字母，只靠词，不需要下标常量

const LATIN: [LatinLang; 11] = [
    LatinLang {
        code: "en",
        words: &[
            "the", "and", "that", "it", "for", "with", "are", "be", "this", "have", "from", "by", "not",
            "at", "they", "you", "she", "his", "their", "which", "can", "if", "about", "there", "what",
            "would", "more", "when", "who", "does", "did", "your", "our", "its", "these", "those", "than",
            "then", "just", "into", "only", "some", "could", "should", "how", "why", "where", "were",
            "them", "being", "other", "such", "very", "because", "him", "us", "new",
            "any", "may", "get", "see", "now", "way", "each", "both", "many", "most", "much", "own",
            "same", "too", "still", "after", "before", "between", "through", "during", "under", "again", "here",
            "while", "since", "until", "every", "never", "always", "often", "already", "another", "without",
            "within", "against", "among", "across", "upon", "yet", "nor", "either", "neither", "whether",
            "although", "though", "however", "therefore", "make", "like", "need", "want", "know", "take", "give",
            "find", "think", "look", "well", "first", "last", "good",
        ],
        soft: &[
            "in", "is", "no", "me", "as", "on", "an", "do", "to", "so", "am", "was", "her", "will", "has", "been",
            "my", "we", "all", "even", "or", "but", "come",
            "of", "he", "also", "back", "up", "use", "one", "had", "out",
        ],
        marks: &[],
    },
    LatinLang {
        code: "fr",
        words: &[
            "le", "la", "les", "un", "une", "des", "du", "de", "en", "et", "est", "que", "qui", "dans", "pour",
            "pas", "sur", "au", "aux", "avec", "ce", "cette", "ces", "il", "elle", "nous", "vous", "ils",
            "elles", "je", "mais", "ou", "où", "par", "sont", "être", "avoir", "fait", "comme", "sa", "ses",
            "leur", "leurs", "très", "aussi", "bien", "tout", "tous", "même", "ne", "se", "si", "ont", "sans",
            "entre", "après", "avant", "chez", "donc", "alors", "encore", "toujours", "jamais", "rien", "peu",
            "beaucoup", "ça", "cela", "ceci", "dont", "lors", "cet", "sous", "vers", "depuis", "puis", "mon",
            "ma", "mes", "notre", "votre", "nos", "vos", "peut", "peuvent", "doit", "faire", "été", "était",
            "sera",
        ],
        soft: &[],
        marks: &["bonjour", "merci", "salut", "bonsoir", "oui", "monsieur", "madame", "bienvenue"],
    },
    LatinLang {
        code: "de",
        words: &[
            "der", "die", "das", "den", "dem", "des", "und", "ist", "nicht", "ein", "eine", "einen", "einem",
            "einer", "eines", "zu", "mit", "von", "auf", "für", "im", "dass", "sie", "es", "ich", "wir", "ihr",
            "er", "auch", "nach", "wie", "aber", "wenn", "oder", "sich", "noch", "wird", "sind", "haben", "als",
            "aus", "nur", "über", "kann", "diese", "dieser", "dieses", "mehr", "durch", "sein", "werden",
            "wurde", "sehr", "schon", "immer", "doch", "mir", "mich", "dir", "dich", "uns", "bei", "um", "vom",
            "zum", "zur", "ins", "ob", "weil", "dann", "denn", "kein", "keine", "wer", "hier", "jetzt",
            "gegen", "ohne", "unter", "zwischen",
        ],
        soft: &[],
        marks: &["danke", "bitte", "hallo", "guten", "tschüss", "nein", "herr", "frau", "willkommen"],
    },
    LatinLang {
        code: "es",
        words: &[
            "el", "la", "los", "las", "un", "una", "unos", "unas", "de", "del", "al", "en", "y", "que", "es",
            "se", "por", "con", "para", "su", "sus", "lo", "como", "más", "pero", "muy", "este", "esta",
            "estos", "estas", "está", "están", "hay", "fue", "ser", "han", "también", "sobre", "entre",
            "cuando", "donde", "sin", "todo", "todos", "yo", "ella", "ellos", "nosotros", "ya", "porque", "mi",
            "tu", "nos", "ese", "esa", "eso", "qué", "cómo", "puede", "tiene", "hace", "le", "les", "mis",
            "tus", "esto", "aquí", "así", "bien", "otro", "otra", "otros", "cada", "siempre", "nunca",
            "después", "antes", "hasta", "desde", "durante", "mismo", "pueden", "debe", "hemos", "fueron",
            "si", "te",
        ],
        soft: &[],
        marks: &["hola", "gracias", "buenos", "buenas", "adiós", "señor", "señora", "bienvenido"],
    },
    LatinLang {
        code: "pt",
        words: &[
            "os", "um", "uma", "uns", "umas", "de", "da", "dos", "das", "em", "na", "nas", "nos", "não",
            "para", "por", "mais", "como", "mas", "foi", "ao", "aos", "ele", "ela", "eles", "elas", "você",
            "vocês", "nós", "meu", "minha", "seu", "sua", "também", "muito", "são", "está", "tem", "isso",
            "este", "esta", "já", "ou", "quando", "entre", "sobre", "até", "ser", "é", "que", "se", "pelo",
            "pela", "pelos", "pelas", "num", "numa", "às", "tudo", "todos", "todas", "ainda", "sempre",
            "nunca", "depois", "antes", "desde", "durante", "mesmo", "pode", "podem", "deve", "foram",
        ],
        soft: &[],
        marks: &["obrigado", "obrigada", "olá", "tchau", "português"],
    },
    LatinLang {
        code: "it",
        words: &[
            "il", "lo", "la", "gli", "le", "un", "uno", "una", "di", "da", "con", "su", "per", "tra", "fra",
            "che", "è", "non", "si", "come", "ma", "più", "sono", "del", "dello", "della", "dei", "degli",
            "delle", "al", "allo", "alla", "ai", "agli", "alle", "nel", "nello", "nella", "nei", "negli",
            "nelle", "sul", "sulla", "questo", "questa", "questi", "queste", "anche", "essere", "hanno",
            "lui", "lei", "noi", "voi", "loro", "mi", "ti", "quando", "tutto", "molto", "già", "perché", "cui",
            "ecco", "quello", "quella", "stato", "fare", "sia", "suo", "sua", "suoi", "tutti", "ancora",
            "sempre", "mai", "dopo", "prima", "però", "quindi", "se", "né", "oppure",
        ],
        soft: &[],
        marks: &["ciao", "grazie", "prego", "buongiorno", "buonasera", "arrivederci", "scusi", "benvenuto"],
    },
    LatinLang {
        code: "nl",
        words: &[
            "de", "het", "een", "en", "van", "ik", "te", "dat", "die", "niet", "op", "aan", "met", "voor",
            "zijn", "er", "maar", "om", "ook", "als", "bij", "nog", "naar", "wel", "ze", "je", "wij", "hij",
            "zij", "hebben", "heeft", "werd", "worden", "deze", "dit", "uit", "nu", "wat", "hoe", "kan", "zou",
            "bent", "wordt", "waar", "geen", "meer", "veel", "mijn", "jouw", "onze", "hun", "dus", "toen",
            "want", "zich", "alle", "tot", "door", "hadden", "zal", "zullen", "moet", "ons", "uw", "jullie",
            "omdat", "al", "zo", "nee", "ja", "goed", "heel", "andere", "tegen", "tussen", "onder", "zonder",
        ],
        soft: &[],
        marks: &["bedankt", "alstublieft", "goedemorgen", "goedendag", "hallo", "welkom"],
    },
    LatinLang {
        code: "pl",
        words: &[
            "nie", "na", "że", "się", "jest", "jak", "ale", "po", "tak", "za", "od", "czy", "już",
            "tylko", "jego", "jej", "ich", "być", "ma", "są", "był", "było", "może", "przez", "dla", "tego",
            "tym", "ta", "oraz", "lub", "bardzo", "który", "która", "które", "przy", "tych", "tej", "też",
            "więc", "gdy", "jeśli", "można", "należy", "będzie", "jako", "jestem", "mnie",
        ],
        soft: &[],
        marks: &["dziękuję", "cześć", "dzień", "dobry", "proszę", "witamy"],
    },
    LatinLang {
        code: "tr",
        words: &[
            "ve", "bir", "bu", "için", "ile", "de", "da", "çok", "daha", "gibi", "ama", "ne", "sen",
            "biz", "siz", "olarak", "kadar", "sonra", "ya", "mı", "mu", "mü", "değil", "ise", "olan", "ki",
            "şey", "en", "bunu", "şu", "çünkü", "ancak", "veya", "hem", "yok", "tüm", "bütün", "arasında",
            "üzerinde", "bana", "sana", "onu", "bunlar", "olduğu", "oldu", "olmak", "yapmak", "gerekir",
        ],
        soft: &[],
        marks: &["merhaba", "teşekkürler", "teşekkür", "selam", "günaydın", "evet", "hayır", "hoşgeldiniz"],
    },
    LatinLang {
        code: "vi",
        words: &[
            "và", "của", "là", "có", "không", "được", "trong", "một", "những", "cho", "với", "các", "này",
            "đã", "để", "khi", "người", "tôi", "bạn", "rất", "cũng", "như", "nhưng", "đó", "thì", "mà",
            "sẽ", "đang", "từ", "nhiều", "hơn", "ở", "đến", "tại",
        ],
        soft: &[],
        marks: &["xin", "chào", "cảm", "ơn"],
    },
    LatinLang {
        code: "id",
        words: &[
            "yang", "dan", "di", "ini", "itu", "dengan", "untuk", "dari", "pada", "tidak", "adalah", "akan",
            "saya", "kamu", "kami", "mereka", "juga", "atau", "karena", "dalam", "sudah", "bisa", "oleh", "ke",
            "lebih", "sebagai", "harus", "seperti", "kita", "anda", "apa", "bagaimana", "jika", "jangan",
            "dapat", "telah", "para", "tersebut", "sebuah", "satu", "dua", "belum", "masih", "hanya", "semua",
            "baru", "sangat", "lagi", "saja", "pun", "antara", "bahwa", "serta", "namun", "tetapi", "sehingga",
            "ketika", "setelah", "sebelum", "tanpa", "tentang",
        ],
        soft: &[],
        marks: &["terima", "kasih", "selamat", "tolong", "maaf"],
    },
];

/// 基字母 + 组合符号 → 预组合字符：NFC 的一小部分（拉丁字母，含越南语的叠加符号，加西里尔的 й ё ї）。
/// 按（基字母，组合符号）排序，二分查找；表是用 Python 的 unicodedata 生成的，没有加依赖。
const COMPOSED: &[(char, char, char)] = &[
    ('A', '\u{300}', '\u{c0}'), ('A', '\u{301}', '\u{c1}'), ('A', '\u{302}', '\u{c2}'), ('A', '\u{303}', '\u{c3}'),
    ('A', '\u{304}', '\u{100}'), ('A', '\u{306}', '\u{102}'), ('A', '\u{307}', '\u{226}'), ('A', '\u{308}', '\u{c4}'),
    ('A', '\u{309}', '\u{1ea2}'), ('A', '\u{30a}', '\u{c5}'), ('A', '\u{30c}', '\u{1cd}'), ('A', '\u{323}', '\u{1ea0}'),
    ('A', '\u{328}', '\u{104}'), ('B', '\u{307}', '\u{1e02}'), ('B', '\u{323}', '\u{1e04}'), ('C', '\u{301}', '\u{106}'),
    ('C', '\u{302}', '\u{108}'), ('C', '\u{307}', '\u{10a}'), ('C', '\u{30c}', '\u{10c}'), ('C', '\u{327}', '\u{c7}'),
    ('D', '\u{307}', '\u{1e0a}'), ('D', '\u{30c}', '\u{10e}'), ('D', '\u{323}', '\u{1e0c}'), ('D', '\u{327}', '\u{1e10}'),
    ('E', '\u{300}', '\u{c8}'), ('E', '\u{301}', '\u{c9}'), ('E', '\u{302}', '\u{ca}'), ('E', '\u{303}', '\u{1ebc}'),
    ('E', '\u{304}', '\u{112}'), ('E', '\u{306}', '\u{114}'), ('E', '\u{307}', '\u{116}'), ('E', '\u{308}', '\u{cb}'),
    ('E', '\u{309}', '\u{1eba}'), ('E', '\u{30c}', '\u{11a}'), ('E', '\u{323}', '\u{1eb8}'), ('E', '\u{327}', '\u{228}'),
    ('E', '\u{328}', '\u{118}'), ('F', '\u{307}', '\u{1e1e}'), ('G', '\u{301}', '\u{1f4}'), ('G', '\u{302}', '\u{11c}'),
    ('G', '\u{304}', '\u{1e20}'), ('G', '\u{306}', '\u{11e}'), ('G', '\u{307}', '\u{120}'), ('G', '\u{30c}', '\u{1e6}'),
    ('G', '\u{327}', '\u{122}'), ('H', '\u{302}', '\u{124}'), ('H', '\u{307}', '\u{1e22}'), ('H', '\u{308}', '\u{1e26}'),
    ('H', '\u{30c}', '\u{21e}'), ('H', '\u{323}', '\u{1e24}'), ('H', '\u{327}', '\u{1e28}'), ('I', '\u{300}', '\u{cc}'),
    ('I', '\u{301}', '\u{cd}'), ('I', '\u{302}', '\u{ce}'), ('I', '\u{303}', '\u{128}'), ('I', '\u{304}', '\u{12a}'),
    ('I', '\u{306}', '\u{12c}'), ('I', '\u{307}', '\u{130}'), ('I', '\u{308}', '\u{cf}'), ('I', '\u{309}', '\u{1ec8}'),
    ('I', '\u{30c}', '\u{1cf}'), ('I', '\u{323}', '\u{1eca}'), ('I', '\u{328}', '\u{12e}'), ('J', '\u{302}', '\u{134}'),
    ('K', '\u{301}', '\u{1e30}'), ('K', '\u{30c}', '\u{1e8}'), ('K', '\u{323}', '\u{1e32}'), ('K', '\u{327}', '\u{136}'),
    ('L', '\u{301}', '\u{139}'), ('L', '\u{30c}', '\u{13d}'), ('L', '\u{323}', '\u{1e36}'), ('L', '\u{327}', '\u{13b}'),
    ('M', '\u{301}', '\u{1e3e}'), ('M', '\u{307}', '\u{1e40}'), ('M', '\u{323}', '\u{1e42}'), ('N', '\u{300}', '\u{1f8}'),
    ('N', '\u{301}', '\u{143}'), ('N', '\u{303}', '\u{d1}'), ('N', '\u{307}', '\u{1e44}'), ('N', '\u{30c}', '\u{147}'),
    ('N', '\u{323}', '\u{1e46}'), ('N', '\u{327}', '\u{145}'), ('O', '\u{300}', '\u{d2}'), ('O', '\u{301}', '\u{d3}'),
    ('O', '\u{302}', '\u{d4}'), ('O', '\u{303}', '\u{d5}'), ('O', '\u{304}', '\u{14c}'), ('O', '\u{306}', '\u{14e}'),
    ('O', '\u{307}', '\u{22e}'), ('O', '\u{308}', '\u{d6}'), ('O', '\u{309}', '\u{1ece}'), ('O', '\u{30b}', '\u{150}'),
    ('O', '\u{30c}', '\u{1d1}'), ('O', '\u{31b}', '\u{1a0}'), ('O', '\u{323}', '\u{1ecc}'), ('O', '\u{328}', '\u{1ea}'),
    ('P', '\u{301}', '\u{1e54}'), ('P', '\u{307}', '\u{1e56}'), ('R', '\u{301}', '\u{154}'), ('R', '\u{307}', '\u{1e58}'),
    ('R', '\u{30c}', '\u{158}'), ('R', '\u{323}', '\u{1e5a}'), ('R', '\u{327}', '\u{156}'), ('S', '\u{301}', '\u{15a}'),
    ('S', '\u{302}', '\u{15c}'), ('S', '\u{307}', '\u{1e60}'), ('S', '\u{30c}', '\u{160}'), ('S', '\u{323}', '\u{1e62}'),
    ('S', '\u{327}', '\u{15e}'), ('T', '\u{307}', '\u{1e6a}'), ('T', '\u{30c}', '\u{164}'), ('T', '\u{323}', '\u{1e6c}'),
    ('T', '\u{327}', '\u{162}'), ('U', '\u{300}', '\u{d9}'), ('U', '\u{301}', '\u{da}'), ('U', '\u{302}', '\u{db}'),
    ('U', '\u{303}', '\u{168}'), ('U', '\u{304}', '\u{16a}'), ('U', '\u{306}', '\u{16c}'), ('U', '\u{308}', '\u{dc}'),
    ('U', '\u{309}', '\u{1ee6}'), ('U', '\u{30a}', '\u{16e}'), ('U', '\u{30b}', '\u{170}'), ('U', '\u{30c}', '\u{1d3}'),
    ('U', '\u{31b}', '\u{1af}'), ('U', '\u{323}', '\u{1ee4}'), ('U', '\u{328}', '\u{172}'), ('V', '\u{303}', '\u{1e7c}'),
    ('V', '\u{323}', '\u{1e7e}'), ('W', '\u{300}', '\u{1e80}'), ('W', '\u{301}', '\u{1e82}'), ('W', '\u{302}', '\u{174}'),
    ('W', '\u{307}', '\u{1e86}'), ('W', '\u{308}', '\u{1e84}'), ('W', '\u{323}', '\u{1e88}'), ('X', '\u{307}', '\u{1e8a}'),
    ('X', '\u{308}', '\u{1e8c}'), ('Y', '\u{300}', '\u{1ef2}'), ('Y', '\u{301}', '\u{dd}'), ('Y', '\u{302}', '\u{176}'),
    ('Y', '\u{303}', '\u{1ef8}'), ('Y', '\u{304}', '\u{232}'), ('Y', '\u{307}', '\u{1e8e}'), ('Y', '\u{308}', '\u{178}'),
    ('Y', '\u{309}', '\u{1ef6}'), ('Y', '\u{323}', '\u{1ef4}'), ('Z', '\u{301}', '\u{179}'), ('Z', '\u{302}', '\u{1e90}'),
    ('Z', '\u{307}', '\u{17b}'), ('Z', '\u{30c}', '\u{17d}'), ('Z', '\u{323}', '\u{1e92}'), ('a', '\u{300}', '\u{e0}'),
    ('a', '\u{301}', '\u{e1}'), ('a', '\u{302}', '\u{e2}'), ('a', '\u{303}', '\u{e3}'), ('a', '\u{304}', '\u{101}'),
    ('a', '\u{306}', '\u{103}'), ('a', '\u{307}', '\u{227}'), ('a', '\u{308}', '\u{e4}'), ('a', '\u{309}', '\u{1ea3}'),
    ('a', '\u{30a}', '\u{e5}'), ('a', '\u{30c}', '\u{1ce}'), ('a', '\u{323}', '\u{1ea1}'), ('a', '\u{328}', '\u{105}'),
    ('b', '\u{307}', '\u{1e03}'), ('b', '\u{323}', '\u{1e05}'), ('c', '\u{301}', '\u{107}'), ('c', '\u{302}', '\u{109}'),
    ('c', '\u{307}', '\u{10b}'), ('c', '\u{30c}', '\u{10d}'), ('c', '\u{327}', '\u{e7}'), ('d', '\u{307}', '\u{1e0b}'),
    ('d', '\u{30c}', '\u{10f}'), ('d', '\u{323}', '\u{1e0d}'), ('d', '\u{327}', '\u{1e11}'), ('e', '\u{300}', '\u{e8}'),
    ('e', '\u{301}', '\u{e9}'), ('e', '\u{302}', '\u{ea}'), ('e', '\u{303}', '\u{1ebd}'), ('e', '\u{304}', '\u{113}'),
    ('e', '\u{306}', '\u{115}'), ('e', '\u{307}', '\u{117}'), ('e', '\u{308}', '\u{eb}'), ('e', '\u{309}', '\u{1ebb}'),
    ('e', '\u{30c}', '\u{11b}'), ('e', '\u{323}', '\u{1eb9}'), ('e', '\u{327}', '\u{229}'), ('e', '\u{328}', '\u{119}'),
    ('f', '\u{307}', '\u{1e1f}'), ('g', '\u{301}', '\u{1f5}'), ('g', '\u{302}', '\u{11d}'), ('g', '\u{304}', '\u{1e21}'),
    ('g', '\u{306}', '\u{11f}'), ('g', '\u{307}', '\u{121}'), ('g', '\u{30c}', '\u{1e7}'), ('g', '\u{327}', '\u{123}'),
    ('h', '\u{302}', '\u{125}'), ('h', '\u{307}', '\u{1e23}'), ('h', '\u{308}', '\u{1e27}'), ('h', '\u{30c}', '\u{21f}'),
    ('h', '\u{323}', '\u{1e25}'), ('h', '\u{327}', '\u{1e29}'), ('i', '\u{300}', '\u{ec}'), ('i', '\u{301}', '\u{ed}'),
    ('i', '\u{302}', '\u{ee}'), ('i', '\u{303}', '\u{129}'), ('i', '\u{304}', '\u{12b}'), ('i', '\u{306}', '\u{12d}'),
    ('i', '\u{308}', '\u{ef}'), ('i', '\u{309}', '\u{1ec9}'), ('i', '\u{30c}', '\u{1d0}'), ('i', '\u{323}', '\u{1ecb}'),
    ('i', '\u{328}', '\u{12f}'), ('j', '\u{302}', '\u{135}'), ('j', '\u{30c}', '\u{1f0}'), ('k', '\u{301}', '\u{1e31}'),
    ('k', '\u{30c}', '\u{1e9}'), ('k', '\u{323}', '\u{1e33}'), ('k', '\u{327}', '\u{137}'), ('l', '\u{301}', '\u{13a}'),
    ('l', '\u{30c}', '\u{13e}'), ('l', '\u{323}', '\u{1e37}'), ('l', '\u{327}', '\u{13c}'), ('m', '\u{301}', '\u{1e3f}'),
    ('m', '\u{307}', '\u{1e41}'), ('m', '\u{323}', '\u{1e43}'), ('n', '\u{300}', '\u{1f9}'), ('n', '\u{301}', '\u{144}'),
    ('n', '\u{303}', '\u{f1}'), ('n', '\u{307}', '\u{1e45}'), ('n', '\u{30c}', '\u{148}'), ('n', '\u{323}', '\u{1e47}'),
    ('n', '\u{327}', '\u{146}'), ('o', '\u{300}', '\u{f2}'), ('o', '\u{301}', '\u{f3}'), ('o', '\u{302}', '\u{f4}'),
    ('o', '\u{303}', '\u{f5}'), ('o', '\u{304}', '\u{14d}'), ('o', '\u{306}', '\u{14f}'), ('o', '\u{307}', '\u{22f}'),
    ('o', '\u{308}', '\u{f6}'), ('o', '\u{309}', '\u{1ecf}'), ('o', '\u{30b}', '\u{151}'), ('o', '\u{30c}', '\u{1d2}'),
    ('o', '\u{31b}', '\u{1a1}'), ('o', '\u{323}', '\u{1ecd}'), ('o', '\u{328}', '\u{1eb}'), ('p', '\u{301}', '\u{1e55}'),
    ('p', '\u{307}', '\u{1e57}'), ('r', '\u{301}', '\u{155}'), ('r', '\u{307}', '\u{1e59}'), ('r', '\u{30c}', '\u{159}'),
    ('r', '\u{323}', '\u{1e5b}'), ('r', '\u{327}', '\u{157}'), ('s', '\u{301}', '\u{15b}'), ('s', '\u{302}', '\u{15d}'),
    ('s', '\u{307}', '\u{1e61}'), ('s', '\u{30c}', '\u{161}'), ('s', '\u{323}', '\u{1e63}'), ('s', '\u{327}', '\u{15f}'),
    ('t', '\u{307}', '\u{1e6b}'), ('t', '\u{308}', '\u{1e97}'), ('t', '\u{30c}', '\u{165}'), ('t', '\u{323}', '\u{1e6d}'),
    ('t', '\u{327}', '\u{163}'), ('u', '\u{300}', '\u{f9}'), ('u', '\u{301}', '\u{fa}'), ('u', '\u{302}', '\u{fb}'),
    ('u', '\u{303}', '\u{169}'), ('u', '\u{304}', '\u{16b}'), ('u', '\u{306}', '\u{16d}'), ('u', '\u{308}', '\u{fc}'),
    ('u', '\u{309}', '\u{1ee7}'), ('u', '\u{30a}', '\u{16f}'), ('u', '\u{30b}', '\u{171}'), ('u', '\u{30c}', '\u{1d4}'),
    ('u', '\u{31b}', '\u{1b0}'), ('u', '\u{323}', '\u{1ee5}'), ('u', '\u{328}', '\u{173}'), ('v', '\u{303}', '\u{1e7d}'),
    ('v', '\u{323}', '\u{1e7f}'), ('w', '\u{300}', '\u{1e81}'), ('w', '\u{301}', '\u{1e83}'), ('w', '\u{302}', '\u{175}'),
    ('w', '\u{307}', '\u{1e87}'), ('w', '\u{308}', '\u{1e85}'), ('w', '\u{30a}', '\u{1e98}'), ('w', '\u{323}', '\u{1e89}'),
    ('x', '\u{307}', '\u{1e8b}'), ('x', '\u{308}', '\u{1e8d}'), ('y', '\u{300}', '\u{1ef3}'), ('y', '\u{301}', '\u{fd}'),
    ('y', '\u{302}', '\u{177}'), ('y', '\u{303}', '\u{1ef9}'), ('y', '\u{304}', '\u{233}'), ('y', '\u{307}', '\u{1e8f}'),
    ('y', '\u{308}', '\u{ff}'), ('y', '\u{309}', '\u{1ef7}'), ('y', '\u{30a}', '\u{1e99}'), ('y', '\u{323}', '\u{1ef5}'),
    ('z', '\u{301}', '\u{17a}'), ('z', '\u{302}', '\u{1e91}'), ('z', '\u{307}', '\u{17c}'), ('z', '\u{30c}', '\u{17e}'),
    ('z', '\u{323}', '\u{1e93}'), ('\u{c2}', '\u{300}', '\u{1ea6}'), ('\u{c2}', '\u{301}', '\u{1ea4}'), ('\u{c2}', '\u{303}', '\u{1eaa}'),
    ('\u{c2}', '\u{309}', '\u{1ea8}'), ('\u{c2}', '\u{323}', '\u{1eac}'), ('\u{c4}', '\u{304}', '\u{1de}'), ('\u{c5}', '\u{301}', '\u{1fa}'),
    ('\u{c7}', '\u{301}', '\u{1e08}'), ('\u{ca}', '\u{300}', '\u{1ec0}'), ('\u{ca}', '\u{301}', '\u{1ebe}'), ('\u{ca}', '\u{303}', '\u{1ec4}'),
    ('\u{ca}', '\u{309}', '\u{1ec2}'), ('\u{ca}', '\u{323}', '\u{1ec6}'), ('\u{cf}', '\u{301}', '\u{1e2e}'), ('\u{d2}', '\u{31b}', '\u{1edc}'),
    ('\u{d3}', '\u{31b}', '\u{1eda}'), ('\u{d4}', '\u{300}', '\u{1ed2}'), ('\u{d4}', '\u{301}', '\u{1ed0}'), ('\u{d4}', '\u{303}', '\u{1ed6}'),
    ('\u{d4}', '\u{309}', '\u{1ed4}'), ('\u{d4}', '\u{323}', '\u{1ed8}'), ('\u{d5}', '\u{301}', '\u{1e4c}'), ('\u{d5}', '\u{304}', '\u{22c}'),
    ('\u{d5}', '\u{308}', '\u{1e4e}'), ('\u{d5}', '\u{31b}', '\u{1ee0}'), ('\u{d6}', '\u{304}', '\u{22a}'), ('\u{d9}', '\u{31b}', '\u{1eea}'),
    ('\u{da}', '\u{31b}', '\u{1ee8}'), ('\u{dc}', '\u{300}', '\u{1db}'), ('\u{dc}', '\u{301}', '\u{1d7}'), ('\u{dc}', '\u{304}', '\u{1d5}'),
    ('\u{dc}', '\u{30c}', '\u{1d9}'), ('\u{e2}', '\u{300}', '\u{1ea7}'), ('\u{e2}', '\u{301}', '\u{1ea5}'), ('\u{e2}', '\u{303}', '\u{1eab}'),
    ('\u{e2}', '\u{309}', '\u{1ea9}'), ('\u{e2}', '\u{323}', '\u{1ead}'), ('\u{e4}', '\u{304}', '\u{1df}'), ('\u{e5}', '\u{301}', '\u{1fb}'),
    ('\u{e7}', '\u{301}', '\u{1e09}'), ('\u{ea}', '\u{300}', '\u{1ec1}'), ('\u{ea}', '\u{301}', '\u{1ebf}'), ('\u{ea}', '\u{303}', '\u{1ec5}'),
    ('\u{ea}', '\u{309}', '\u{1ec3}'), ('\u{ea}', '\u{323}', '\u{1ec7}'), ('\u{ef}', '\u{301}', '\u{1e2f}'), ('\u{f2}', '\u{31b}', '\u{1edd}'),
    ('\u{f3}', '\u{31b}', '\u{1edb}'), ('\u{f4}', '\u{300}', '\u{1ed3}'), ('\u{f4}', '\u{301}', '\u{1ed1}'), ('\u{f4}', '\u{303}', '\u{1ed7}'),
    ('\u{f4}', '\u{309}', '\u{1ed5}'), ('\u{f4}', '\u{323}', '\u{1ed9}'), ('\u{f5}', '\u{301}', '\u{1e4d}'), ('\u{f5}', '\u{304}', '\u{22d}'),
    ('\u{f5}', '\u{308}', '\u{1e4f}'), ('\u{f5}', '\u{31b}', '\u{1ee1}'), ('\u{f6}', '\u{304}', '\u{22b}'), ('\u{f9}', '\u{31b}', '\u{1eeb}'),
    ('\u{fa}', '\u{31b}', '\u{1ee9}'), ('\u{fc}', '\u{300}', '\u{1dc}'), ('\u{fc}', '\u{301}', '\u{1d8}'), ('\u{fc}', '\u{304}', '\u{1d6}'),
    ('\u{fc}', '\u{30c}', '\u{1da}'), ('\u{102}', '\u{300}', '\u{1eb0}'), ('\u{102}', '\u{301}', '\u{1eae}'), ('\u{102}', '\u{303}', '\u{1eb4}'),
    ('\u{102}', '\u{309}', '\u{1eb2}'), ('\u{102}', '\u{323}', '\u{1eb6}'), ('\u{103}', '\u{300}', '\u{1eb1}'), ('\u{103}', '\u{301}', '\u{1eaf}'),
    ('\u{103}', '\u{303}', '\u{1eb5}'), ('\u{103}', '\u{309}', '\u{1eb3}'), ('\u{103}', '\u{323}', '\u{1eb7}'), ('\u{106}', '\u{327}', '\u{1e08}'),
    ('\u{107}', '\u{327}', '\u{1e09}'), ('\u{112}', '\u{300}', '\u{1e14}'), ('\u{112}', '\u{301}', '\u{1e16}'), ('\u{113}', '\u{300}', '\u{1e15}'),
    ('\u{113}', '\u{301}', '\u{1e17}'), ('\u{114}', '\u{327}', '\u{1e1c}'), ('\u{115}', '\u{327}', '\u{1e1d}'), ('\u{14c}', '\u{300}', '\u{1e50}'),
    ('\u{14c}', '\u{301}', '\u{1e52}'), ('\u{14c}', '\u{328}', '\u{1ec}'), ('\u{14d}', '\u{300}', '\u{1e51}'), ('\u{14d}', '\u{301}', '\u{1e53}'),
    ('\u{14d}', '\u{328}', '\u{1ed}'), ('\u{15a}', '\u{307}', '\u{1e64}'), ('\u{15b}', '\u{307}', '\u{1e65}'), ('\u{160}', '\u{307}', '\u{1e66}'),
    ('\u{161}', '\u{307}', '\u{1e67}'), ('\u{168}', '\u{301}', '\u{1e78}'), ('\u{168}', '\u{31b}', '\u{1eee}'), ('\u{169}', '\u{301}', '\u{1e79}'),
    ('\u{169}', '\u{31b}', '\u{1eef}'), ('\u{16a}', '\u{308}', '\u{1e7a}'), ('\u{16b}', '\u{308}', '\u{1e7b}'), ('\u{1a0}', '\u{300}', '\u{1edc}'),
    ('\u{1a0}', '\u{301}', '\u{1eda}'), ('\u{1a0}', '\u{303}', '\u{1ee0}'), ('\u{1a0}', '\u{309}', '\u{1ede}'), ('\u{1a0}', '\u{323}', '\u{1ee2}'),
    ('\u{1a1}', '\u{300}', '\u{1edd}'), ('\u{1a1}', '\u{301}', '\u{1edb}'), ('\u{1a1}', '\u{303}', '\u{1ee1}'), ('\u{1a1}', '\u{309}', '\u{1edf}'),
    ('\u{1a1}', '\u{323}', '\u{1ee3}'), ('\u{1af}', '\u{300}', '\u{1eea}'), ('\u{1af}', '\u{301}', '\u{1ee8}'), ('\u{1af}', '\u{303}', '\u{1eee}'),
    ('\u{1af}', '\u{309}', '\u{1eec}'), ('\u{1af}', '\u{323}', '\u{1ef0}'), ('\u{1b0}', '\u{300}', '\u{1eeb}'), ('\u{1b0}', '\u{301}', '\u{1ee9}'),
    ('\u{1b0}', '\u{303}', '\u{1eef}'), ('\u{1b0}', '\u{309}', '\u{1eed}'), ('\u{1b0}', '\u{323}', '\u{1ef1}'), ('\u{1ea}', '\u{304}', '\u{1ec}'),
    ('\u{1eb}', '\u{304}', '\u{1ed}'), ('\u{226}', '\u{304}', '\u{1e0}'), ('\u{227}', '\u{304}', '\u{1e1}'), ('\u{228}', '\u{306}', '\u{1e1c}'),
    ('\u{229}', '\u{306}', '\u{1e1d}'), ('\u{22e}', '\u{304}', '\u{230}'), ('\u{22f}', '\u{304}', '\u{231}'), ('\u{406}', '\u{308}', '\u{407}'),
    ('\u{410}', '\u{306}', '\u{4d0}'), ('\u{410}', '\u{308}', '\u{4d2}'), ('\u{413}', '\u{301}', '\u{403}'), ('\u{415}', '\u{300}', '\u{400}'),
    ('\u{415}', '\u{306}', '\u{4d6}'), ('\u{415}', '\u{308}', '\u{401}'), ('\u{416}', '\u{306}', '\u{4c1}'), ('\u{416}', '\u{308}', '\u{4dc}'),
    ('\u{417}', '\u{308}', '\u{4de}'), ('\u{418}', '\u{300}', '\u{40d}'), ('\u{418}', '\u{304}', '\u{4e2}'), ('\u{418}', '\u{306}', '\u{419}'),
    ('\u{418}', '\u{308}', '\u{4e4}'), ('\u{41a}', '\u{301}', '\u{40c}'), ('\u{41e}', '\u{308}', '\u{4e6}'), ('\u{423}', '\u{304}', '\u{4ee}'),
    ('\u{423}', '\u{306}', '\u{40e}'), ('\u{423}', '\u{308}', '\u{4f0}'), ('\u{423}', '\u{30b}', '\u{4f2}'), ('\u{427}', '\u{308}', '\u{4f4}'),
    ('\u{42b}', '\u{308}', '\u{4f8}'), ('\u{42d}', '\u{308}', '\u{4ec}'), ('\u{430}', '\u{306}', '\u{4d1}'), ('\u{430}', '\u{308}', '\u{4d3}'),
    ('\u{433}', '\u{301}', '\u{453}'), ('\u{435}', '\u{300}', '\u{450}'), ('\u{435}', '\u{306}', '\u{4d7}'), ('\u{435}', '\u{308}', '\u{451}'),
    ('\u{436}', '\u{306}', '\u{4c2}'), ('\u{436}', '\u{308}', '\u{4dd}'), ('\u{437}', '\u{308}', '\u{4df}'), ('\u{438}', '\u{300}', '\u{45d}'),
    ('\u{438}', '\u{304}', '\u{4e3}'), ('\u{438}', '\u{306}', '\u{439}'), ('\u{438}', '\u{308}', '\u{4e5}'), ('\u{43a}', '\u{301}', '\u{45c}'),
    ('\u{43e}', '\u{308}', '\u{4e7}'), ('\u{443}', '\u{304}', '\u{4ef}'), ('\u{443}', '\u{306}', '\u{45e}'), ('\u{443}', '\u{308}', '\u{4f1}'),
    ('\u{443}', '\u{30b}', '\u{4f3}'), ('\u{447}', '\u{308}', '\u{4f5}'), ('\u{44b}', '\u{308}', '\u{4f9}'), ('\u{44d}', '\u{308}', '\u{4ed}'),
    ('\u{456}', '\u{308}', '\u{457}'), ('\u{1e36}', '\u{304}', '\u{1e38}'), ('\u{1e37}', '\u{304}', '\u{1e39}'), ('\u{1e5a}', '\u{304}', '\u{1e5c}'),
    ('\u{1e5b}', '\u{304}', '\u{1e5d}'), ('\u{1e60}', '\u{323}', '\u{1e68}'), ('\u{1e61}', '\u{323}', '\u{1e69}'), ('\u{1e62}', '\u{307}', '\u{1e68}'),
    ('\u{1e63}', '\u{307}', '\u{1e69}'), ('\u{1ea0}', '\u{302}', '\u{1eac}'), ('\u{1ea0}', '\u{306}', '\u{1eb6}'), ('\u{1ea1}', '\u{302}', '\u{1ead}'),
    ('\u{1ea1}', '\u{306}', '\u{1eb7}'), ('\u{1eb8}', '\u{302}', '\u{1ec6}'), ('\u{1eb9}', '\u{302}', '\u{1ec7}'), ('\u{1ecc}', '\u{302}', '\u{1ed8}'),
    ('\u{1ecc}', '\u{31b}', '\u{1ee2}'), ('\u{1ecd}', '\u{302}', '\u{1ed9}'), ('\u{1ecd}', '\u{31b}', '\u{1ee3}'), ('\u{1ece}', '\u{31b}', '\u{1ede}'),
    ('\u{1ecf}', '\u{31b}', '\u{1edf}'), ('\u{1ee4}', '\u{31b}', '\u{1ef0}'), ('\u{1ee5}', '\u{31b}', '\u{1ef1}'), ('\u{1ee6}', '\u{31b}', '\u{1eec}'),
    ('\u{1ee7}', '\u{31b}', '\u{1eed}'),
];

fn is_combining(ch: char) -> bool {
    ('\u{0300}'..='\u{036f}').contains(&ch)
}

/// 把分解形式（NFD）组合成预组合形式（NFC）：词表和特征字母写的都是预组合形式。
/// 从访达、终端复制出来的文字常是分解形式（`é` 写成 `e` + 组合重音），不组合的话词表一个都对不上。
/// 没有组合符号时原样返回，不分配；表里没有的组合原样留着（后面分词时组合符号仍算进词里）。
fn compose(text: &str) -> Cow<'_, str> {
    if !text.chars().any(is_combining) {
        return Cow::Borrowed(text);
    }
    let mut out: Vec<char> = Vec::with_capacity(text.len());
    for ch in text.chars() {
        if is_combining(ch) {
            if let Some(last) = out.last_mut() {
                if let Ok(index) = COMPOSED.binary_search_by(|entry| (entry.0, entry.1).cmp(&(*last, ch))) {
                    *last = COMPOSED[index].2;
                    continue;
                }
            }
        }
        out.push(ch);
    }
    Cow::Owned(out.into_iter().collect())
}

/// 词里的字符：字母，加上组合符号（U+0300–036F）。
/// 从访达、终端复制出来的文字常是分解形式（NFD）：`é` 写成 `e` + 组合重音。组合符号不是字母，
/// 要是当分隔符，`Tạo` 会被拆成 `Ta` 和 `o`，拆出来的碎片还会碰巧命中别的语言的虚词。
fn is_word_char(ch: char) -> bool {
    ch.is_alphabetic() || ('\u{0300}'..='\u{036f}').contains(&ch)
}

/// 特征字母。分两档：
///
/// - **强证据**（`score` 里加 `STRONG` 分）：基本只在某一种语言里出现的字母——ß、ñ、¿ ¡、土耳其语的 ğ ş ı İ、波兰语的 ł ń ś ź ż ą ę ć、
///   越南语的 đ ơ ư 和带声调的元音，以及葡萄牙语的 `ão` / `õe`。
/// - **弱证据**（`weak`，每种语言**最多加 `HINT` 分**）：à è ì ò ù ê â ô î û ë ï ä ö ü ç ã õ 这类，几种语言共用——
///   越南语满篇都是 à ê ô ò ã，法语、意大利语、葡萄牙语也有，逐个累加会把越南语句子拉到法语那边去。
///   封顶之后它只能拿来打破平局，不能单独撑起"有把握"。
///
/// 常见的 é á í ó ú 不收（多种语言都有，英文里的外来词 café、résumé 也有）。
fn add_char_evidence(
    previous: Option<char>,
    ch: char,
    score: &mut [u32; LATIN.len()],
    weak: &mut [bool; LATIN.len()],
) {
    let mut hint = |languages: &[usize]| {
        for &index in languages {
            weak[index] = true;
        }
    };
    match ch {
        'ß' | 'ẞ' => score[DE] += STRONG,
        'ñ' | 'Ñ' | '¿' | '¡' => score[ES] += STRONG,
        // 葡萄牙语的鼻化元音总是 ão / õe（não、coração、opções）；越南语的 ã 后面跟的是辅音
        'o' | 'O' if matches!(previous, Some('ã' | 'Ã')) => score[PT] += STRONG,
        'e' | 'E' if matches!(previous, Some('õ' | 'Õ')) => score[PT] += STRONG,
        'ã' | 'õ' | 'Ã' | 'Õ' => hint(&[PT, VI]),
        'ç' | 'Ç' => hint(&[FR, PT, TR]),
        'œ' | 'Œ' => score[FR] += MEDIUM,
        'à' | 'À' => hint(&[FR, IT, PT, VI]),
        'è' | 'È' => hint(&[FR, IT]),
        'ù' | 'Ù' => hint(&[FR, IT, VI]),
        'ì' | 'Ì' | 'ò' | 'Ò' => hint(&[IT, VI]),
        'ê' | 'Ê' | 'â' | 'Â' | 'ô' | 'Ô' => hint(&[FR, PT, VI]),
        'î' | 'Î' | 'û' | 'Û' | 'ÿ' => hint(&[FR]),
        'ë' | 'Ë' | 'ï' | 'Ï' => hint(&[FR, NL]),
        'ä' | 'Ä' => hint(&[DE]),
        'ö' | 'Ö' | 'ü' | 'Ü' => hint(&[DE, TR]),
        'ğ' | 'Ğ' | 'ş' | 'Ş' | 'ı' | 'İ' => score[TR] += STRONG,
        'ł' | 'Ł' | 'ń' | 'Ń' | 'ś' | 'Ś' | 'ź' | 'Ź' | 'ż' | 'Ż' | 'ą' | 'Ą' | 'ę' | 'Ę' | 'ć' | 'Ć' => {
            score[PL] += STRONG
        }
        'đ' | 'Đ' | 'ơ' | 'Ơ' | 'ư' | 'Ư' => score[VI] += STRONG,
        'ă' | 'Ă' => score[VI] += MEDIUM,
        // 越南语的带声调元音（Latin Extended Additional 里 ạ 到 ỹ）
        '\u{1ea0}'..='\u{1ef9}' => score[VI] += STRONG,
        // 分解形式（NFD）的同类证据：下点、上钩、角这三个组合符号几乎只出现在越南语里
        '\u{0323}' | '\u{0309}' | '\u{031b}' => score[VI] += STRONG,
        _ => {}
    }
}

/// 拉丁字母的文字是哪种语言，以及有没有把握。
///
/// 有把握：最高分 ≥ 2（两个虚词，或一个特征词 / 特征字母），并且严格高于第二名。
/// 没把握：按英文处理（拉丁字母的默认），但不声称认出了英文。
fn latin_language(text: &str) -> (&'static Lang, bool) {
    let head: String = text.chars().take(SCAN_CHARS).collect();
    // 词表和特征字母写的都是预组合形式；分解形式（NFD）的文字先组合
    let head = compose(&head);
    let mut score = [0u32; LATIN.len()];
    // 有小写字母的文字里，全大写的短缩写（2–4 个字母：MIT、UN、IT、ID、API、HTML……）不当词看：`MIT License` 里的 MIT 不是德语的 mit。
    // 5 个字母以上的大写词（`DANKE schön`、`MERCI beaucoup`）多半是在喊一个词，照常匹配；整段全大写的（标题、招牌）也照常匹配
    let has_lowercase = head.chars().any(char::is_lowercase);

    // 英文命中了几个"核心词"（不含糊的英文词，不算软词）：英文要至少有一个才能算"有把握"
    let mut english_core = 0;

    for token in head.split(|ch: char| !is_word_char(ch)).filter(|token| !token.is_empty()) {
        if has_lowercase && (2..=4).contains(&token.chars().count()) && token.chars().all(char::is_uppercase) {
            continue;
        }
        let word = token.to_lowercase();
        for (index, (slot, language)) in score.iter_mut().zip(LATIN.iter()).enumerate() {
            if language.marks.contains(&word.as_str()) {
                *slot += MARK;
            } else if language.words.contains(&word.as_str()) {
                *slot += WORD;
                if index == EN {
                    english_core += 1;
                }
            } else if language.soft.contains(&word.as_str()) {
                *slot += SOFT;
            }
        }
    }
    let mut weak = [false; LATIN.len()];
    let mut previous = None;
    for ch in head.chars() {
        add_char_evidence(previous, ch, &mut score, &mut weak);
        previous = Some(ch);
    }
    for (slot, hinted) in score.iter_mut().zip(weak) {
        if hinted {
            *slot += HINT;
        }
    }

    // 分最高的；并列时英文（下标 0）在前，不被后面的挤掉
    let mut best = EN;
    for index in 1..score.len() {
        if score[index] > score[best] {
            best = index;
        }
    }
    let second = score
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != best)
        .map(|(_, value)| *value)
        .max()
        .unwrap_or(0);

    // 英文：软词凑不出"有把握"（一句荷兰语里的两个 in 不能算英文），至少要有一个核心词；
    // 有核心词时，一个核心词 + 一个软词（`React is a library for…`）就够，不然真实的英文短句会掉出去一大截
    let bar = if best == EN {
        if english_core == 0 {
            u32::MAX
        } else {
            SURE - SOFT
        }
    } else {
        SURE
    };
    if score[best] >= bar && score[best] > second {
        (known(LATIN[best].code), true)
    } else {
        (known("en"), false)
    }
}

// ───────────────────────── 西里尔字母：俄语还是乌克兰语 ─────────────────────────

struct CyrillicLang {
    code: &'static str,
    words: &'static [&'static str],
    marks: &'static [&'static str],
    /// 只在这种语言里用的字母，出现一个 +3
    letters: &'static str,
}

/// 两种语言共有的词（в、на、не、по、за……）不收；收的是两边写法不同的那些（что / що、это / це、как / як……）。
const CYRILLIC: [CyrillicLang; 2] = [
    CyrillicLang {
        code: "ru",
        words: &[
            "что", "это", "как", "он", "из", "от", "его", "они", "быть", "был", "была", "были", "она", "мы", "вы",
            "все", "или", "уже", "только", "еще", "если", "бы", "нет", "чтобы", "может", "есть",
        ],
        marks: &["привет", "здравствуйте", "спасибо", "пожалуйста", "добрый"],
        letters: "ыэёъЫЭЁЪ",
    },
    CyrillicLang {
        code: "uk",
        words: &[
            "що", "це", "як", "він", "із", "від", "його", "вони", "бути", "був", "була", "були", "вона", "ми",
            "ви", "всі", "або", "вже", "тільки", "ще", "якщо", "ні", "щоб", "може", "є", "та", "й", "чи",
            "дуже", "їх", "про", "після", "між",
        ],
        marks: &["привіт", "дякую", "будь", "ласка", "вітаю", "добрий"],
        letters: "іїєґІЇЄҐ",
    },
];

/// 西里尔文字是俄语还是乌克兰语，以及有没有把握。打分和拉丁字母同一个规矩：最高分 ≥ 2 且严格高于另一边才有把握；
/// 没证据（一个没有特征字母的词，或保加利亚语、塞尔维亚语这些）按俄语处理但不声称。
fn cyrillic_language(text: &str) -> (&'static Lang, bool) {
    let head: String = text.chars().take(SCAN_CHARS).collect();
    let head = compose(&head); // й ё ї 的分解形式先组合
    let mut score = [0u32; CYRILLIC.len()];
    for token in head.split(|ch: char| !is_word_char(ch)).filter(|token| !token.is_empty()) {
        let word = token.to_lowercase();
        for (slot, language) in score.iter_mut().zip(CYRILLIC.iter()) {
            if language.marks.contains(&word.as_str()) {
                *slot += MARK;
            } else if language.words.contains(&word.as_str()) {
                *slot += WORD;
            }
        }
    }
    for ch in head.chars() {
        for (slot, language) in score.iter_mut().zip(CYRILLIC.iter()) {
            if language.letters.contains(ch) {
                *slot += STRONG;
            }
        }
    }
    let (best, other) = if score[1] > score[0] { (1, 0) } else { (0, 1) };
    if score[best] >= SURE && score[best] > score[other] {
        (known(CYRILLIC[best].code), true)
    } else {
        (known("ru"), false)
    }
}

// ───────────────────────── 目标语言的取舍 ─────────────────────────

/// 这一段话该怎么译：认出的原文语言 + 取定的目标语言。
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub source: Detected,
    pub target: &'static Lang,
}

pub fn plan(text: &str, pref: &str) -> Plan {
    let source = detect(text);
    Plan { source, target: resolve_target(source, pref) }
}

/// 目标语言。偏好见模块说明的第 1、2 条。
pub fn resolve_target(source: Detected, pref: &str) -> &'static Lang {
    let default = || {
        if source.lang.is_some_and(is_chinese) {
            known("en")
        } else {
            known("zh")
        }
    };
    match lang_by_code(pref) {
        None => default(),
        Some(chosen) => {
            // 只有"确认"是同一种语言才退回默认：没把握的识别不该推翻用户明确的选择
            // （`Joyeux anniversaire` 没认出法语，选了英文就该译成英文，而不是当成"已经是英文"）
            let already = source.confident && source.lang.is_some_and(|lang| lang.code == chosen.code);
            if already { default() } else { chosen }
        }
    }
}

impl Plan {
    /// 这段话该不该走查词卡：只有中 → 英、英 → 中两个方向（契约里 `lang` 只有 en / zh，冻结）。
    /// 调用方先用 `isLongText` 挡掉整句和长文，这里只回答方向。
    ///
    /// - 原文是中文：目标是英文才走（先译成英文再解析那个英文词）
    /// - 原文是英文（包括没把握、按英文处理的拉丁字母）：目标是中文才走
    /// - 没有任何字母（`12345`）：沿用老办法，当英文输入
    /// - 其它语言、或者手选了别的目标语言：普通翻译
    pub fn dictionary(&self) -> bool {
        let to = self.target.code;
        match (self.source.lang, self.source.letters) {
            (Some(lang), _) if is_chinese(lang) => to == "en",
            (Some(lang), _) if lang.code == "en" => to == "zh",
            (None, false) => to == "zh",
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(text: &str) -> Option<&'static str> {
        detect(text).lang.map(|lang| lang.code)
    }

    #[test]
    fn language_table_is_consistent() {
        // 代码唯一，且不和 auto 撞
        for (index, lang) in LANGS.iter().enumerate() {
            assert!(!lang.code.is_empty() && !lang.name.is_empty() && !lang.native.is_empty(), "{lang:?}");
            assert_ne!(lang.code, AUTO);
            assert!(LANGS[..index].iter().all(|other| other.code != lang.code), "代码重复：{}", lang.code);
            assert!(LANGS[..index].iter().all(|other| other.name != lang.name), "中文名重复：{}", lang.name);
        }
        // 默认方向的提示词用的就是这两个名字：改了名字，两条老提示词就不再逐字节不变
        assert_eq!(known("zh").name, "中文");
        assert_eq!(known("en").name, "英文");
        // 每个 detect 里写死的代码都在表里（`known` 才不会 panic）
        for code in ["zh", "en", "ja", "ko", "uk", "ru", "ar", "he", "th", "hi", "el"] {
            assert!(lang_by_code(code).is_some(), "{code}");
        }
    }

    #[test]
    fn latin_table_matches_the_index_constants() {
        for (index, expected) in [
            (EN, "en"),
            (FR, "fr"),
            (DE, "de"),
            (ES, "es"),
            (PT, "pt"),
            (IT, "it"),
            (NL, "nl"),
            (PL, "pl"),
            (TR, "tr"),
            (VI, "vi"),
            (10, "id"),
        ] {
            assert_eq!(LATIN[index].code, expected, "下标常量和表的顺序要一致");
            assert!(lang_by_code(expected).is_some());
        }
        assert_eq!(LATIN.len(), 11);
        for language in &LATIN {
            // 软词只有英文用：外语的词表里没有"撞形"这回事
            assert!(language.code == "en" || language.soft.is_empty(), "{} 不该有软词", language.code);
            // 全小写、没有空词：to_lowercase 之后才对得上
            for word in language.words.iter().chain(language.marks.iter()).chain(language.soft.iter()) {
                assert_eq!(*word, word.to_lowercase(), "{}：{word}", language.code);
                assert!(!word.is_empty() && word.chars().all(char::is_alphabetic), "{}：{word}", language.code);
            }
            // 单个 ASCII 字母太容易撞上英文里的 a、I、s、t……；带音符的单字母（é è ở）英文里不会单独成词，
            // 西班牙语的连词 y 是唯一留下的 ASCII 单字母（英文里几乎不会单独出现）
            for word in language.words.iter().filter(|word| word.chars().count() == 1) {
                assert!(
                    !word.is_ascii() || (language.code, *word) == ("es", "y"),
                    "{} 的虚词里不该有单个字母 {word}",
                    language.code
                );
            }
        }
    }

    /// 英文里高频的词。外语的词表里一个都不能有：它们出现在英文里，会白白给外语加分，把英文句子拉走。
    const COMMON_ENGLISH: &[&str] = &[
        "the", "of", "and", "to", "in", "is", "that", "it", "for", "with", "as", "was", "on", "are", "be", "this",
        "have", "from", "or", "by", "not", "but", "at", "they", "you", "we", "he", "she", "his", "her", "their",
        "which", "will", "can", "if", "about", "there", "what", "all", "would", "has", "been", "more", "when", "who",
        "do", "does", "did", "my", "your", "our", "its", "these", "those", "than", "then", "also", "just", "into",
        "only", "some", "could", "should", "how", "why", "where", "were", "had", "them", "being", "other", "such",
        "very", "because", "no", "an", "so", "me", "us", "him", "am", "up", "out", "one", "new", "any", "may", "get",
        "man", "son", "war", "hat", "over", "plus", "com", "org", "net", "eu", "var", "ben", "dan", "mag", "ten",
        "ton", "era", "ha", "ci", "vi", "io", "co", "id", "to", "go", "now", "see", "use", "way",
        "day", "time", "make", "like", "back", "well", "even", "good", "work", "first", "last", "long", "great",
        "little", "own", "old", "right", "big", "high", "small", "large", "next", "early", "young", "important",
    ];

    #[test]
    fn foreign_word_lists_never_contain_a_common_english_word() {
        for language in LATIN.iter().filter(|language| language.code != "en") {
            for word in language.words.iter().chain(language.marks.iter()) {
                // 例外：印尼语 / 荷兰语等里"dan"这类，在上面的表里是英文人名；它们在对应语言里是最高频的词，
                // 英文句子里偶尔出现一个只加一分，压不过英文自己的虚词
                let allowed = matches!((language.code, *word), ("id", "dan"));
                assert!(
                    allowed || !COMMON_ENGLISH.contains(word),
                    "{} 的词表里有英文高频词 {word}",
                    language.code
                );
            }
        }
    }

    #[test]
    fn preference_is_normalized() {
        assert_eq!(normalize_pref(None), AUTO);
        assert_eq!(normalize_pref(Some("auto")), AUTO);
        assert_eq!(normalize_pref(Some("ja")), "ja");
        assert_eq!(normalize_pref(Some("zh-Hant")), "zh-Hant");
        // 手改坏的配置：认不得的当 auto，大小写也不放过（代码是精确的）
        assert_eq!(normalize_pref(Some("klingon")), AUTO);
        assert_eq!(normalize_pref(Some("")), AUTO);
        assert_eq!(normalize_pref(Some("JA")), AUTO);
    }

    #[test]
    fn no_letters_means_nothing_to_recognize() {
        for text in ["", "   ", "12345 ——", "😀", "3.14 + 2 = 5.14", "¿¡!?…"] {
            assert_eq!(detect(text), Detected { lang: None, confident: false, letters: false }, "{text:?}");
        }
    }

    #[test]
    fn chinese_japanese_korean_are_told_apart() {
        for text in ["落实", "充电", "这是一段中文", "AI 助手", "我今天去了超市，买了很多东西。", "说曹操曹操到"] {
            assert_eq!(code(text), Some("zh"), "{text}");
            assert!(detect(text).confident, "{text}");
        }
        for text in ["こんにちは", "今日は天気がいいですね", "ラーメン", "私は学生です。"] {
            assert_eq!(code(text), Some("ja"), "{text}");
            assert!(detect(text).confident, "{text}");
        }
        for text in ["안녕하세요", "한국어를 공부합니다", "한국 大統領은 오늘 발표했다"] {
            assert_eq!(code(text), Some("ko"), "{text}");
            assert!(detect(text).confident, "{text}");
        }
        // 中文里偶尔夹一个假名不算日语
        assert_eq!(code("这是一个很长的中文句子，里面偶尔出现了一个ー符号，其余全是汉字"), Some("zh"));
        // 中文句夹一个外文词、引一句日语：仍是中文（以前会被"确认"成日语 / 韩语，译成中文等于白翻）
        for text in [
            "我喜欢看アニメ",
            "日语里的“ありがとう”是谢谢的意思",
            "我在用 카카오톡",
            "我们用 카카오톡 聊天",
            "这个词在日语里写成コーヒー，意思是咖啡，很常见",
        ] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some("zh"), "{text}");
            assert!(found.confident, "{text}");
        }
        // 汉字多、假名少的日语：平假名紧挨着汉字（送假名、助词）就是日语（以前会被认成中文）
        for text in [
            "検索履歴を消去",
            "日米首脳会談、経済安全保障で連携強化",
            "コーヒーを飲む",
            "漢字漢字漢字あ",
        ] {
            assert_eq!(code(text), Some("ja"), "{text}");
        }
        // 汉字和假名 / 谚文混着、又没有平假名挨着汉字的短语：有歧义，偏向中文（译成英文总是一次有用的翻译；认成日语 / 韩语却可能是空转）
        for text in ["東京タワー", "韓國語 한국어"] {
            assert_eq!(code(text), Some("zh"), "{text}");
        }
        // 平假名没有挨着汉字、又比汉字少：仍是中文
        assert_eq!(code("漢字漢字漢字 あ"), Some("zh"));
        // 假名不比汉字少：不看挨不挨着
        assert_eq!(code("漢字 ありがとう"), Some("ja"));
        // 纯汉字的日语和中文分不开：认作中文（已知局限）
        assert_eq!(code("東京"), Some("zh"));
    }

    #[test]
    fn other_scripts_are_recognized_by_their_letters() {
        assert_eq!(code("Привет, как дела?"), Some("ru"));
        assert_eq!(code("Привіт, як справи? Їжак і ґанок"), Some("uk"));
        // 俄语 / 乌克兰语靠特征字母和写法不同的虚词分开
        assert_eq!(code("Это очень хорошая идея, и мы все согласны"), Some("ru"));
        assert_eq!(code("Це дуже гарна ідея, і ми всі згодні"), Some("uk"));
        assert!(detect("Это очень хорошая идея, и мы все согласны").confident);
        assert!(detect("Це дуже гарна ідея, і ми всі згодні").confident);
        // 只有特征字母、没有任何虚词也认得出：і ї є ґ 是乌克兰语独有的，ы э ё ъ 是俄语独有的
        for (text, expected) in [("Їжак", "uk"), ("Ґанок", "uk"), ("Ёлка", "ru"), ("Объект", "ru"), ("Ыы", "ru")] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some(expected), "{text}");
            assert!(found.confident, "{text}");
        }
        // 没有任何证据（一个共有写法的词、保加利亚语 / 塞尔维亚语）：按俄语处理但不声称
        for text in ["Назад", "Настройки", "Добре дошли"] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some("ru"), "{text}");
            assert!(!found.confident, "{text}：没证据就不能声称是俄语");
        }
        assert_eq!(code("مرحبا بالعالم"), Some("ar"));
        assert_eq!(code("שלום עולם"), Some("he"));
        assert_eq!(code("สวัสดีครับ"), Some("th"));
        assert_eq!(code("नमस्ते दुनिया"), Some("hi"));
        assert_eq!(code("Γειά σου Κόσμε"), Some("el"));
        for text in ["Привет", "مرحبا", "שלום", "สวัสดี", "नमस्ते", "Γειά"] {
            assert!(detect(text).confident, "{text}");
        }
        // 波斯语用阿拉伯字母但有自己的字：不声称是阿拉伯语
        let persian = detect("سلام، حال شما چطور است؟ گربه");
        assert_eq!(persian.lang.map(|lang| lang.code), Some("ar"));
        assert!(!persian.confident);
        // 表里没有的书写系统：认不出，但明确"有字母"（不是只有数字符号）
        assert_eq!(detect("নমস্কার পৃথিবী"), Detected { lang: None, confident: false, letters: true });
    }

    #[test]
    fn latin_languages_are_recognized_when_the_evidence_is_clear() {
        for (text, expected) in [
            ("The quick brown fox jumps over the lazy dog", "en"),
            ("I do not know what to do in this case", "en"),
            ("Le chat est sur la table et il mange", "fr"),
            ("Das ist ein Test und ich bin müde", "de"),
            ("El niño come una manzana en la cocina", "es"),
            ("Eu não sei o que fazer com isso", "pt"),
            ("Il gatto è sul tavolo e non mangia", "it"),
            ("Ik heb een hond en een kat", "nl"),
            ("To jest bardzo dobre, dziękuję", "pl"),
            ("Bu bir test ve çok güzel", "tr"),
            ("Xin chào, tôi là sinh viên", "vi"),
            ("Saya tidak tahu apa yang harus dilakukan", "id"),
        ] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some(expected), "{text}");
            assert!(found.confident, "{text}：应当有把握");
        }
    }

    #[test]
    fn one_or_two_words_are_recognized_from_greetings_and_special_letters() {
        for (text, expected) in [
            ("Bonjour", "fr"),
            ("merci", "fr"),
            ("Danke", "de"),
            ("Straße", "de"),
            ("Hola", "es"),
            ("¿Cómo estás?", "es"),
            ("jalapeño", "es"),
            ("Obrigado", "pt"),
            ("coração", "pt"),
            ("Ciao", "it"),
            ("Grazie mille", "it"),
            ("bedankt", "nl"),
            ("Dziękuję", "pl"),
            ("Łódź", "pl"),
            ("Teşekkürler", "tr"),
            ("İstanbul", "tr"),
            ("Terima kasih", "id"),
            ("Việt Nam", "vi"),
        ] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some(expected), "{text}");
            assert!(found.confident, "{text}");
        }
    }

    #[test]
    fn shared_accents_are_only_a_hint_and_cannot_carry_the_verdict_alone() {
        // à è ì ò ù ê â ô ç ã ö ü……几种语言共用：每种语言最多加 1 分，单靠它们不会"有把握"
        // 后五个的音符只有一种语言用（î û 法语、ä 德语）：加分要是不封顶，一个字就够让它"有把握"
        for text in ["Français", "Düsseldorf", "À", "ê", "ò", "Thêm", "façade", "île", "naître", "coût", "Käse", "Bär"] {
            let found = detect(text);
            assert!(!found.confident, "{text}：只有共用的音符，不该声称认出了语种");
            assert_eq!(found.lang.map(|lang| lang.code), Some("en"), "{text}");
        }
        // 越南语满篇都是 à ê ô ò ã：不能被逐个累加成法语 / 意大利语 / 葡萄牙语
        for text in [
            "Thêm thành viên vào Danh bạ công ty",
            "Tạo tài khoản",
            "Trình phát video",
            "Watermark hai dòng",
            "Co giãn",
        ] {
            let found = detect(text);
            assert!(
                found.lang.map(|lang| lang.code) == Some("vi") || !found.confident,
                "{text}：不能被认成别的语言 {:?}",
                found.lang.map(|lang| lang.code)
            );
        }
        assert_eq!(code("Thêm thành viên vào Danh bạ công ty"), Some("vi"));
        // 葡萄牙语的 ão / õe 是强证据（单个 ã 不是）
        assert_eq!(code("informação"), Some("pt"));
        assert_eq!(code("opções"), Some("pt"));
        assert!(detect("informação").confident);
        assert_eq!(code("São Paulo"), Some("pt"), "ão 本身就是强证据");
    }

    #[test]
    fn decomposed_text_is_recognized_like_composed_text() {
        // 从访达、终端复制出来的文字常是分解形式：基字母 + 组合符号。词表写的是预组合形式，不组合就一个都对不上
        for (composed, decomposed, expected) in [
            ("Dziękuję bardzo", "Dzie\u{328}kuje\u{328} bardzo", "pl"),
            ("informação", "informac\u{327}a\u{303}o", "pt"),
            ("Teşekkürler", "Tes\u{327}ekku\u{308}rler", "tr"),
            ("¿Cómo estás?", "¿Co\u{301}mo estas\u{301}?", "es"),
            ("Le chat est très content", "Le chat est tre\u{300}s content", "fr"),
            ("Das ist schön und gut für uns", "Das ist scho\u{308}n und gut fu\u{308}r uns", "de"),
            ("Ёлка", "Е\u{308}лка", "ru"),
            ("Їжак", "І\u{308}жак", "uk"),
        ] {
            let a = detect(composed);
            let b = detect(decomposed);
            assert_eq!(a.lang.map(|lang| lang.code), Some(expected), "{composed}");
            assert_eq!((a.lang, a.confident), (b.lang, b.confident), "{decomposed}：分解形式要和预组合形式同一个结论");
            assert!(b.confident, "{decomposed}");
        }
    }

    #[test]
    fn composition_handles_stacked_marks_and_leaves_unknown_ones_alone() {
        assert_eq!(compose("e\u{301}"), "é");
        assert_eq!(compose("n\u{303}"), "ñ");
        assert_eq!(compose("e\u{302}\u{301}"), "ế", "越南语的叠加符号：ê + 重音 → ế");
        assert_eq!(compose("\u{438}\u{306}"), "й");
        // 表里没有的组合原样留着（x 没有带重音的预组合形式）；孤立的组合符号也不丢
        assert_eq!(compose("x\u{301}"), "x\u{301}");
        assert_eq!(compose("\u{301}a"), "\u{301}a");
        // 没有组合符号：原样返回，不分配
        assert!(matches!(compose("plain text 文字"), Cow::Borrowed(_)));
        // 表是按（基字母，组合符号）排序的，二分查找才成立
        assert!(COMPOSED.windows(2).all(|pair| (pair[0].0, pair[0].1) < (pair[1].0, pair[1].1)));
    }

    #[test]
    fn decomposed_accents_do_not_tear_words_apart() {
        // 从访达、终端复制出来的文字常是分解形式（NFD）：基字母 + 组合符号。组合符号算进词里，
        // 否则 `Tạo` 变成 `Ta` + `o`，碎片还会命中别的语言的虚词（`ta` 是波兰语的）
        let decomposed = "Ta\u{0323}o ta\u{0300}i khoa\u{0309}n";
        let found = detect(decomposed);
        assert_eq!(found.lang.map(|lang| lang.code), Some("vi"), "下点、上钩是越南语特有的组合符号");
        assert!(found.confident);
        // 同一句的预组合形式（NFC）结论一致
        assert_eq!(code("Tạo tài khoản"), Some("vi"));
        // 法语：分解形式的 `été` `à` 照样被当成词的一部分，虚词仍然命中
        let french = "Le chat est tre\u{0300}s content d'e\u{0301}te\u{0301}";
        assert_eq!(code(french), Some("fr"));
        assert!(detect(french).confident);
    }

    #[test]
    fn a_combining_mark_without_a_precomposed_form_stays_in_its_word() {
        // U+0348 在组合表里没有预组合形式，compose 不动它：它仍是词的一部分，不是分隔符。
        // 当成分隔符的话，`das͈ist` 会被拆成 `das` 和 `ist`，两个德语虚词就凑出了"有把握"
        let stray = "das\u{0348}ist ein\u{0348}test";
        let found = detect(stray);
        assert!(!found.confident, "{found:?}：带着杂散组合符号的词不能被拆开去命中虚词");
        // 对照：真有空格，就是德语
        let spaced = detect("das ist ein test");
        assert_eq!(spaced.lang.map(|lang| lang.code), Some("de"));
        assert!(spaced.confident);
    }

    #[test]
    fn english_and_unknown_latin_text_is_never_claimed_as_a_foreign_language() {
        // 没证据：按英文处理，但不声称认出了英文
        for text in [
            "charge", "run", "Hello", "iPhone 15 Pro", "npm install foo", "good morning",
            "café", "résumé", "naïve", "façade", "non-linear", "a non-linear function", "var x = 1;",
            "Joyeux anniversaire",
        ] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some("en"), "{text}");
            assert!(!found.confident, "{text}：没有证据，不该声称认出了语种");
            assert!(found.letters);
        }
        // 真正的英文句子：有把握，且绝不被认成外语
        for text in [
            "This is a fairly long English paragraph that clearly has more than forty letters in it.",
            "Install the package using npm and then run the build script from the project root.",
            "He was charged with fraud, but the case was dropped when the witness did not show up.",
            "We will meet at the station on Monday, and then we can go to the museum together.",
            "No problem, I do not mind waiting for you in the lobby if it takes a few more minutes.",
            "Terms and conditions apply to all of the offers that are listed on this page.",
            "The function returns a promise that resolves when all of the requests have been completed.",
            "Please do not hesitate to contact us if you have any questions about your order.",
        ] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some("en"), "{text}");
            assert!(found.confident, "{text}");
        }
    }

    #[test]
    fn english_words_that_other_languages_use_too_cannot_confirm_english_alone() {
        // in / is / no / me / to / do……在荷兰语、西班牙语、波兰语里同样高频：只算半分，凑不够"有把握"
        for text in [
            "Open mappen in tabbladen, niet in vensters",
            "No me gusta",
            "To jest do dyspozycji",
            "no me",
            "in is on",
        ] {
            let found = detect(text);
            assert!(!found.confident, "{text}：不能被确认成任何语言");
            assert_eq!(found.lang.map(|lang| lang.code), Some("en"), "{text}");
        }
        // 外语自己的虚词盖得过英文的软词
        for (text, expected) in [("Hij is in het huis", "nl"), ("Dit is een test van het systeem", "nl")] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some(expected), "{text}");
            assert!(found.confident, "{text}");
        }
        // 真正的英文句子照样确认：核心词（the、of、and……）够用
        for text in ["Where is the library?", "He was in the house and she was on the roof", "What is this and why is it here"] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some("en"), "{text}");
            assert!(found.confident, "{text}");
        }
    }

    #[test]
    fn english_needs_one_core_word_and_then_a_core_plus_a_soft_word_is_enough() {
        // 真实英文文档里的短句：一个核心词 + 一个软词（`for` + `to`、`the` + `on`、`for` + `is`）
        for text in [
            "Add inline source map to transformed source for better debugging.",
            "Detects the current CI/CD provider based on environment variables.",
            "React is a JavaScript library for creating user interfaces.",
            "Offers an async require.resolve function. It's highly configurable.",
            "Uncaught Error: @vitejs/plugin-react can't detect preamble. Something is wrong.",
        ] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some("en"), "{text}");
            assert!(found.confident, "{text}");
        }
        // 只有软词（哪怕很多个）：没有一个不含糊的英文词，不能算英文
        for text in ["in is on an do to so", "No me gusta", "Open mappen in tabbladen, niet in vensters"] {
            let found = detect(text);
            assert!(!found.confident, "{text}");
        }
        // 只有一个核心词：太少，仍然没把握
        assert!(!detect("Create a regular expression from a parsed glob pattern.").confident);
        // 外语自己的虚词照样盖得过英文的软词
        assert_eq!(code("Hij is in het huis"), Some("nl"));
    }

    #[test]
    fn english_core_words_that_other_languages_use_cannot_confirm_english() {
        // 评审员的复现：一个"核心英文词" + 一个软词，就凑够了英文的门槛——而这些词在别的语言里同样高频
        // （西班牙语 he = 我有、荷兰语 of = 或、德语 also = 所以）
        for text in [
            "No he recibido el correo",
            "Even wachten of annuleren",
            "Also was jetzt?",
            "Het back-up bestand is niet up-to-date",
            "Ik had een uur out nodig",
            "Use la función de búsqueda",
            "One of the most common",
        ] {
            let found = detect(text);
            if text.starts_with("One of") {
                // 这句是真的英文：of / one 是软词，但 most / the 够用
                assert_eq!(found.lang.map(|lang| lang.code), Some("en"), "{text}");
                assert!(found.confident, "{text}");
            } else {
                assert!(!(found.lang.map(|lang| lang.code) == Some("en") && found.confident), "{text}：不能被确认成英文");
            }
        }
        // 这九个词在英文表里是软词，不是核心词（改回去，上面几句就会被"确认"成英文）
        let en = &LATIN[EN];
        for word in ["of", "he", "also", "back", "up", "use", "one", "had", "out"] {
            assert!(en.soft.contains(&word), "{word} 应该是软词");
            assert!(!en.words.contains(&word), "{word} 不该是核心词");
        }
    }

    #[test]
    fn a_quote_in_a_chinese_sentence_does_not_decide_the_language() {
        // 外面是一句像样的中文（≥4 个汉字、有中文独有的字），引文相对短：中文。引的是日语、俄语、英语都一样
        for text in [
            "请把“よろしくお願いします”翻译成中文",
            "这个词怎么读：「ありがとう」",
            "请翻译《カタカナ》这个词",
            "“Привет”是什么意思",
            "请把\"Install npm\"翻译成中文",
            "（お願いします）是什么意思",
        ] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some("zh"), "{text}");
            assert!(found.confident, "{text}");
        }
        // 引文的字母数落在外面汉字数的 2 到 3 倍之间（汉字圈 ×2 的权重输给引文的文字）：不靠这条规则就会被判成引文的语言
        for text in ["请把“Доброе утро друзья”翻译成中文", "请把“صباح الخير يا أصدقائي”翻译成中文"] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some("zh"), "{text}");
            assert!(found.confident, "{text}");
        }
        // 整段都在引号里：外面什么也没有，仍按整段判（不能因此全变成中文）
        assert_eq!(code("「検索履歴を消去」"), Some("ja"));
        assert_eq!(code("（検索履歴を消去）"), Some("ja"));
        assert_eq!(code("“Привет”"), Some("ru"));
        // 外面有假名：是日语的句子，引文只是句子的一部分
        assert_eq!(code("彼は「ありがとう」と言った"), Some("ja"));
        // 英文句子引了一段日语：外面是英文
        assert_eq!(code("He said “ありがとう” and left the room."), Some("en"));
    }

    #[test]
    fn a_short_chinese_label_or_a_kanji_name_outside_the_quotes_does_not_make_the_quote_chinese() {
        // 评审员的复现：引文压倒外面的标签，整段是英文 / 日文（上一版把它们判成了中文，目标语言就反了）
        for text in [
            "提示：“Your session has expired. Please sign in again.”",
            "错误信息（Unable to connect to the server）",
            "警告：\"The file was modified outside of the editor and has not been saved.\"",
        ] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some("en"), "{text}");
            assert!(found.confident, "{text}");
        }
        for text in ["首相「再発防止に努める」", "村上春樹『ノルウェイの森』", "「你好」は中国語です。"] {
            assert_eq!(code(text), Some("ja"), "{text}");
        }
        // 外面是中文句子、引的英文很长（引文压倒外面）：回到按字数比，英文；目标语言是中文——正是这句话要的
        let long_quote = detect("请把“Install the package using npm and then run the build script from the project root”翻译成中文");
        assert_eq!(long_quote.lang.map(|lang| lang.code), Some("en"));
        // 引的是西里尔 / 阿拉伯文字、字母数超过外面汉字数的 3 倍：同样是引文压倒外面，按整段判。
        // （拉丁字母有自己的 2 倍上限；这条 3 倍的上限现在只有别的文字管得到，所以单独钉着）
        assert_eq!(code("请把“Привет, как дела? Всё хорошо, спасибо большое”翻译成中文"), Some("ru"));
        assert_eq!(code("请把“مرحبا بك في تطبيقنا الجديد، نتمنى لك يوما سعيدا”翻译成中文"), Some("ar"));
        // 引的英文在外面汉字数的 2–3 倍之间：拉丁字母按 2 倍算（和「汉字 ×2 ≥ 拉丁字母」同一把尺），仍是英文——
        // 评审员的复现：上一版把它改判成了中文，目标语言就从中文变成了英文
        for text in ["请翻译下面这句话：“Where is the station?”", "请翻译这句话：“What time is it now?”"] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some("en"), "{text}");
            assert!(found.confident, "{text}");
        }
        // 引的是别的文字不受这把尺影响：假名、西里尔字母在外面汉字数的 3 倍以内仍是中文
        assert_eq!(code("请翻译下面这句话：“ありがとうございます”"), Some("zh"));
    }

    #[test]
    fn the_quote_rule_has_known_costs() {
        // 极短的中文包装句（外面不到 4 个汉字）引一段非拉丁文字：按整段判成那种文字。译成中文，通常也正是想要的
        assert_eq!(code("“صباح الخير”怎么说"), Some("ar"));
        // 繁体中文里夹没有引号的日语词：中文独有字表是简体的，认不出
        assert_eq!(code("他說ありがとう"), Some("ja"));
    }

    #[test]
    fn outside_quotes_cuts_only_matched_pairs() {
        assert_eq!(outside_quotes("请把“abc”翻译"), "请把翻译");
        assert_eq!(outside_quotes("读「あ」和『い』吧"), "读和吧");
        assert_eq!(outside_quotes("见（括号）里"), "见里");
        assert_eq!(outside_quotes("a \"x\" b \"y\" c"), "a  b  c", "ASCII 双引号按先后配对");
        // 没配上对的开括号原样保留，英文里的撇号不是引号
        assert_eq!(outside_quotes("没有结尾“引号"), "没有结尾“引号");
        assert_eq!(outside_quotes("don’t stop"), "don’t stop");
        assert_eq!(outside_quotes("（没闭上"), "（没闭上");
        // 引号里面再有引号：按最先闭上的那个收
        assert_eq!(outside_quotes("他说“你说‘好’吗”完了"), "他说完了");
        // 一长串没配上对的开括号：线性，不是平方
        let many = "“".repeat(100_000);
        let started = std::time::Instant::now();
        assert_eq!(outside_quotes(&many).chars().count(), SCAN_CHARS);
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
    }

    #[test]
    fn a_chinese_only_character_means_the_sentence_is_chinese_even_with_unquoted_japanese() {
        for text in [
            "日语里的ありがとう是谢谢的意思",
            "我在用 카카오톡 聊天",
            "你好、ありがとう",
            "这家店的ラーメン很好吃",
        ] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some("zh"), "{text}");
            assert!(found.confident, "{text}");
        }
        // 日语的句子里没有这些字：照常是日语
        for text in ["私は学生です。", "今日は天気がいいですね", "検索履歴を消去", "日米首脳会談、経済安全保障で連携強化"] {
            assert_eq!(code(text), Some("ja"), "{text}");
        }
        // 日语里也用的字不能放进去：放进去，这些日语句子就认成中文了
        for ch in "机国学会来点当体里与着都的事".chars() {
            assert!(!CHINESE_ONLY.contains(ch), "{ch} 日语里也用，不该在 CHINESE_ONLY 里");
        }
    }

    #[test]
    fn nfd_japanese_still_finds_the_hiragana_next_to_kanji() {
        // で 的分解形式是 て + U+3099（浊点）：浊点不能把平假名和汉字隔开
        let decomposed = "WeChat\u{3066}\u{3099}接続";
        let composed = "WeChatで接続";
        assert_eq!(code(composed), Some("ja"));
        assert_eq!(code(decomposed), Some("ja"));
        assert!(hiragana_beside_han("経済安全保障\u{3066}\u{3099}連携"));
        assert!(!hiragana_beside_han("漢字漢字 \u{3066}\u{3099} 漢字"), "中间隔着空格：不挨着");
    }

    #[test]
    fn shouted_foreign_words_still_count_but_short_acronyms_do_not() {
        // 5 个字母以上的大写词多半是在喊一个词：照常匹配（评审员：跳过所有大写词会让这两句由确认变成没把握）
        for (text, expected) in [("DANKE schön", "de"), ("MERCI beaucoup", "fr"), ("Gracias, BUENOS días", "es")] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some(expected), "{text}");
            assert!(found.confident, "{text}");
        }
        // 2–4 个字母的大写缩写仍不当词
        assert!(!detect("Licensed under the MIT or API terms").lang.is_some_and(|lang| lang.code == "de"));
        assert_eq!(code("Licensed under the MIT License"), Some("en"));
    }

    #[test]
    fn all_caps_acronyms_are_not_words_unless_the_whole_text_is_in_capitals() {
        // 英文文档里的 MIT 是许可证，不是德语的 mit（真实英文文档里确实出现过：被认成德语）
        for text in ["MIT or MIT/Apache 2.0 where applicable.", "Licensed under the MIT License", "MIT"] {
            let found = detect(text);
            assert_eq!(found.lang.map(|lang| lang.code), Some("en"), "{text}");
        }
        // 有一个核心词（where），一个软词（or）：确认是英文——不是德语
        assert!(detect("MIT or MIT/Apache 2.0 where applicable.").confident);
        // 只有缩写：没有任何证据
        assert!(!detect("MIT").confident);
        // 整段全大写（标题、招牌）：照常匹配
        let shouting = detect("DAS IST EIN TEST UND ICH BIN MÜDE");
        assert_eq!(shouting.lang.map(|lang| lang.code), Some("de"));
        assert!(shouting.confident);
        // 有小写的德语句子里夹一个全大写的缩写：别的词照样认
        let mixed = detect("Wir benutzen die MIT Lizenz und das ist gut");
        assert_eq!(mixed.lang.map(|lang| lang.code), Some("de"));
        assert!(mixed.confident);
    }

    #[test]
    fn a_tie_between_russian_and_ukrainian_claims_neither() {
        // ы 只有俄语用、ї 只有乌克兰语用：各加一次，平手——不声称，按俄语处理
        let tie = detect("ыї");
        assert_eq!(tie.lang.map(|lang| lang.code), Some("ru"));
        assert!(!tie.confident);
    }

    #[test]
    fn a_single_ambiguous_hint_is_not_enough() {
        // 一个撞形的虚词、一个共有的字母，不足以把英文句子拉走
        let found = detect("Let me know if you want a non-trivial example");
        assert_eq!(found.lang.map(|lang| lang.code), Some("en"));
        // 两种语言平手：不声称，退回英文默认
        let tie = detect("hallo");
        assert!(!tie.confident, "hallo 德语荷兰语都有");
        assert_eq!(tie.lang.map(|lang| lang.code), Some("en"));
    }

    #[test]
    fn script_comparison_keeps_the_rule_screen_translation_always_used() {
        // 老规则：汉字 ×2 ≥ 拉丁字母（只数 ASCII）就是中文。没有假名、谚文、带音符字母时，新规则必须与它逐例一致
        fn old_is_chinese(text: &str) -> bool {
            let han = text.chars().filter(|ch| ('\u{4e00}'..='\u{9fff}').contains(ch)).count();
            let latin = text.chars().filter(|ch| ch.is_ascii_alphabetic()).count();
            han >= 1 && han * 2 >= latin
        }
        let pieces = [
            "", "你", "你好", "设置", "要使用", "Settings", "Claude", "Claude Code", "macOS", "AI", "2026", "——", "！",
            "SENSE 是一款运行在 macOS 上的 AI 翻译器",
        ];
        let mut checked = 0;
        for a in pieces {
            for b in pieces {
                for c in pieces {
                    let text = format!("{a} {b} {c}");
                    let found = detect(&text);
                    if !found.letters {
                        continue;
                    }
                    let is_zh = found.lang.map(|lang| lang.code) == Some("zh");
                    assert_eq!(is_zh, old_is_chinese(&text), "{text:?}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 2000, "{checked}");
        // 几个钉死的边界
        assert_eq!(code("要使用 Claude"), Some("zh"), "3 个汉字 ×2 = 6 ≥ 6 个字母：边界算中文");
        assert_eq!(code("要使用 Claude Code"), Some("en"), "汉字 ×2 < 字母");
        assert_eq!(code("Settings 设置"), Some("en"), "英文为主的中英混排");
        assert_eq!(code("2026 年 10 月 31 日 上午 10 点"), Some("zh"), "数字不是字母");
    }

    #[test]
    fn a_huge_paste_is_not_a_problem() {
        // 判语种只看开头；整段文字的书写系统数一遍，线性
        let text = "The quick brown fox jumps over the lazy dog. ".repeat(40_000);
        let found = detect(&text);
        assert_eq!(found.lang.map(|lang| lang.code), Some("en"));
        assert!(found.confident);
    }

    fn pick(source: &str, pref: &str) -> &'static str {
        resolve_target(detect(source), pref).code
    }

    #[test]
    fn auto_target_is_chinese_unless_the_source_is_chinese() {
        assert_eq!(pick("Hello, how are you today?", AUTO), "zh");
        assert_eq!(pick("こんにちは", AUTO), "zh");
        assert_eq!(pick("Bonjour tout le monde", AUTO), "zh");
        assert_eq!(pick("안녕하세요", AUTO), "zh");
        assert_eq!(pick("Привет", AUTO), "zh");
        assert_eq!(pick("你好，世界", AUTO), "en");
        assert_eq!(pick("charge", AUTO), "zh", "没把握的拉丁字母按英文，译成中文（和以前一样）");
        assert_eq!(pick("12345", AUTO), "zh", "没有字母：默认中文");
        assert_eq!(pick("নমস্কার", AUTO), "zh", "认不出的书写系统：默认中文");
        // 偏好是认不得的值：当 auto
        assert_eq!(pick("你好", "klingon"), "en");
        assert_eq!(pick("hello", ""), "zh");
    }

    #[test]
    fn a_chosen_target_wins_unless_the_source_is_confirmed_to_be_that_language() {
        assert_eq!(pick("Hello, how are you today?", "ja"), "ja");
        assert_eq!(pick("你好，世界", "ja"), "ja");
        assert_eq!(pick("你好，世界", "fr"), "fr");
        assert_eq!(pick("こんにちは", "en"), "en");
        assert_eq!(pick("Bonjour tout le monde", "de"), "de");

        // 选的就是原文语言：译成自己没有意义，退回默认方向
        assert_eq!(pick("你好，世界", "zh"), "en", "中文 → 选了中文：退回英文");
        assert_eq!(pick("The weather is nice and we can go out today.", "en"), "zh", "英文 → 选了英文：退回中文");
        assert_eq!(pick("こんにちは", "ja"), "zh");
        assert_eq!(pick("Bonjour tout le monde", "fr"), "zh");
        assert_eq!(pick("안녕하세요", "ko"), "zh");

        // 简体 → 繁体不是同一种：这是一次转换，照译
        assert_eq!(pick("你好，世界", "zh-Hant"), "zh-Hant");
        assert_eq!(pick("Hello, how are you today?", "zh-Hant"), "zh-Hant");
    }

    #[test]
    fn an_unsure_guess_never_overrides_what_the_user_chose() {
        // 短短几个词没认出法语，被当成"英文（没把握）"：选了英文，就是要英文，不能当成"已经是英文"
        assert_eq!(pick("Joyeux anniversaire", "en"), "en");
        assert_eq!(pick("charge", "en"), "en");
        assert_eq!(pick("charge", "zh"), "zh");
    }

    #[test]
    fn dictionary_cards_only_exist_for_chinese_english_in_either_direction() {
        // 默认：英文词 → 义项卡（中文释义）；中文词 → 英文词的义项卡
        assert!(plan("charge", AUTO).dictionary());
        assert!(plan("落实", AUTO).dictionary());
        assert!(plan("take it for granted", AUTO).dictionary());
        // 选了中文 / 英文：方向仍落在中英之间，照走查词卡
        assert!(plan("charge", "zh").dictionary());
        assert!(plan("落实", "en").dictionary());
        assert!(plan("落实", "zh").dictionary(), "选了中文，原文是中文：退回英文，仍是 中 → 英");
        assert!(
            plan("take it for granted", "en").dictionary(),
            "选了英文，原文确认是英文：退回中文，仍是 英 → 中"
        );
    }

    #[test]
    fn dictionary_route_is_closed_for_every_other_language_and_target() {
        // 手选了别的目标
        assert!(!plan("charge", "ja").dictionary());
        assert!(!plan("落实", "ja").dictionary());
        assert!(!plan("charge", "zh-Hant").dictionary());
        // 选了英文、原文是没把握的英文词：照选的译（原样返回），不去查词卡——
        // 否则 `Joyeux anniversaire` 这种没认出的外语短语会被塞进英文词典
        assert!(!plan("charge", "en").dictionary());
        assert!(!plan("Joyeux anniversaire", "en").dictionary());
        // 别的原文语言
        assert!(!plan("こんにちは", AUTO).dictionary());
        assert!(!plan("Bonjour", AUTO).dictionary());
        assert!(!plan("Привет", AUTO).dictionary());
        assert!(!plan("안녕하세요", AUTO).dictionary());
        assert!(!plan("নমস্কার", AUTO).dictionary());
        // 没有字母：沿用老办法（当英文输入），译成中文才走
        assert!(plan("12345", AUTO).dictionary());
        assert!(!plan("12345", "ja").dictionary());
    }

    #[test]
    fn the_plan_reports_how_sure_the_source_is() {
        let japanese = plan("こんにちは", AUTO).source;
        assert_eq!((japanese.lang.map(|lang| lang.code), japanese.confident), (Some("ja"), true));
        let french = plan("Bonjour", AUTO).source;
        assert_eq!((french.lang.map(|lang| lang.code), french.confident), (Some("fr"), true));
        // 没把握：lang 是"拉丁字母 = 英文"的默认值，confident 为 false——界面据此写"自动识别"而不是"英语"
        let unsure = plan("charge", AUTO).source;
        assert_eq!((unsure.lang.map(|lang| lang.code), unsure.confident), (Some("en"), false));
        assert!(!plan("12345", AUTO).source.letters);
    }
}
