//! 英文硬折行段落的判定：一段话被按行折断（PDF 论文、等宽字体的 README / RFC、网页里的多行段落）时，把这些行合回一块再翻译。
//!
//! 为什么要合：整屏文字按「一行一条」发给模型时，模型会顺着句意把译文重新分配到各个断行上——编号对、内容错一格，
//! 应用还看不出来（CHANGELOG「截屏翻译：整页『译文条数对不上』改成带编号的协议」里的静默错位）。
//! 英文那条老路（`merge_lines` 里的 `joins_last`）只和上一块比、合并后用整块的高度比下一行，所以一段话只会两两合并，
//! 两栏的 PDF 页上行与行夹着另一栏的行，更是几乎合不起来（19 页论文 1227 行里只合并了 203 对）。
//!
//! 这里和 `zh_wrap` 一样改看**版面**：给每一行找「同列里紧挨在上面的那一行」，再逐对判定——上一行被折断得有理由
//! （写满了一行，或者下一行的第一个词放不下），上一行没说完，下一行是它的下文，这一对读起来像句子。
//! 取舍同样是宁可漏合、不可错合：漏合只是回到以前的样子，错合是把标题、列表项、代码、表格行并进段落，翻错加排错。
//!
//! 尺寸都以行高 h 为单位（取两行里较大的那个），1 倍图与 2 倍图通用。只处理**不含任何 CJK 字符**的行（汉字、假名、谚文、
//! CJK 标点、全角形式）：中文的折行归 `zh_wrap`，两条路不会同时认领同一行。判据的来由与实测见 CHANGELOG 2026-11-01
//! 「截屏翻译：按行硬折的英文段落合成一块再翻译」。

/// 竖直间隙（下一行的顶 − 上一行的底）在 [GAP_MIN, GAP_MAX] × h 之内（和中文那条同一个口径）
const GAP_MIN: f64 = -0.25;
const GAP_MAX: f64 = 0.55;
/// 两行行高相差不超过较大者的 45%。Vision 的框高抖动很大（同一字号的两行能差 14 / 16 / 18），比中文那条（30%）宽一些；
/// 字号相差的两行靠 `CHAR_WIDTH_DIFF_MAX` 另外挡
const HEIGHT_DIFF_MAX: f64 = 0.45;
/// 左缘对齐的容差
const LEFT_ALIGN_TOLERANCE: f64 = 0.6;
/// 首行缩进：这一列的第一行可以比下一行靠右，最多 3h
const INDENT_MAX: f64 = 3.0;
/// 列表项悬挂缩进：上一行是列表项的第一行，下一行可以比它靠右，最多 3h（靠右 0.6h 以内本来就算左缘对齐，不用再设下限）
const HANGING_MAX: f64 = 3.0;
/// 「写满一行」：上一行的右缘离这一列的右缘不超过这么多。一段的起手一对要求更严；成段之后放宽到 0.75h（容得下禁则让一行少一个字）
const FULL_FIRST: f64 = 0.5;
const FULL_NEXT: f64 = 0.75;
/// 上一行至少这么宽：窄栏里的短行不当折行
const MIN_WIDTH: f64 = 7.0;
/// 比这一列的右缘还宽出这么多的行是跨栏拼出来的长行（Vision 偶尔把左栏的一行和右栏同一高度的一行读成一条），不参与
const OVERWIDE: f64 = 2.0;
/// 列的右缘取各行右缘的这个分位（0.9）：一两条离群的长行不会把整列的「写满」标准拉高。比中位宽 1.5 倍的长行先排除
const RIGHT_QUANTILE: f64 = 0.9;
const RIGHT_OUTLIER: f64 = 1.5;
/// 参差右缘（网页、等宽文本）：上一行离列右缘的距离，小于「下一行的第一个词 + 一个空格」就是被折断的——那个词放不下。
/// 空格按 0.3h 算；左边减去 0.2h 的量框误差。上一行至少要占半栏宽
const SPACE_WIDTH: f64 = 0.3;
const RAGGED_SLACK: f64 = 0.2;
const RAGGED_MIN_FILL: f64 = 0.5;
/// 字色护栏：两行字色的亮度差超过这个就不是同一段文字（灰色的摘要、来源标签、页脚小字）。
/// 只比亮度、不比色相：论文里的引用链接是蓝色，整行蓝色的引用行与黑色的正文亮度差很小
const LUMINANCE_DIFF_MAX: f64 = 55.0;
/// 字号护栏：两行每个字母的平均宽度（词框宽 / 字母数，不含词间空格——两端对齐会把空格拉宽）相差超过 18% 不是同一段文字。
/// 两行都至少这么多个字符才比
const CHAR_WIDTH_DIFF_MAX: f64 = 0.18;
const CHAR_WIDTH_MIN_CHARS: usize = 20;
/// 同一视觉行被拆成几段（Vision 会把较长的一行从中间拆成两条）：右边那段的左缘离左边那段的右缘不超过 0.6h（左边最多重叠 0.3h），
/// 高度悬殊（竖排的 arXiv 戳、大标题旁的小字）、竖直不重叠的不算。栏间距一般 ≥ 1.0h，量到的碎片间隙 90% 在 0.5h 以内。
/// 竖直方向只要求两段的框至少重叠矮的那段的 0.6：高度比 ≤ 1.5 时，这已经把两段的中心差限制在较高那段的 0.43 倍以内，不用再查中心
const FRAGMENT_GAP_MAX: f64 = 0.6;
const FRAGMENT_OVERLAP_MAX: f64 = 0.3;
const FRAGMENT_HEIGHT_RATIO: f64 = 1.5;
const FRAGMENT_V_OVERLAP: f64 = 0.6;

/// 有下一行小写开头、上一行断在连接词处这类证据时，上一行至少要有这么多个词：表格里同一列的两个单元格
/// （`seconds, per request` / `before giving up`）几何上也像折行，但格子里的短语很少有四个词以上
const WEAK_MIN_WORDS: usize = 4;

/// 句子里常见的功能词：一行里有这些，才像英文句子（表格行、标签、代码里几乎没有）
const STOP_WORDS: &[&str] = &[
    "the", "of", "and", "to", "a", "in", "is", "that", "for", "we", "with", "by", "are", "as", "on", "be", "this", "it", "from", "an", "or", "can", "which", "our",
    "not", "at", "have", "has", "these", "their", "its", "than", "also", "more", "such", "each", "both", "between", "using", "used", "use", "been", "was", "were",
    "will", "would", "may", "other", "into", "only", "one", "two", "all", "but", "if", "when", "where", "while", "how", "what", "who", "whom", "whose", "then", "so",
    "do", "does", "did", "they", "them", "he", "she", "you", "your", "i", "my", "me", "us", "there", "here", "about", "over", "under", "after", "before", "during",
    "through", "since", "because", "however", "thus", "therefore", "whether", "should", "could", "must", "might", "shall", "any", "some", "many", "most", "much",
    "few", "several", "every", "no", "nor", "yet",
];

/// 句子可以在这些词后面被折断：冠词、介词、连词、助动词
const CONNECT_WORDS: &[&str] = &[
    "a", "an", "the", "of", "in", "on", "at", "to", "for", "with", "by", "from", "and", "or", "but", "nor", "as", "than", "that", "which", "who", "whom", "whose", "if",
    "when", "where", "while", "is", "are", "was", "were", "be", "been", "being", "has", "have", "had", "do", "does", "did", "will", "would", "can", "could", "may",
    "might", "must", "shall", "should", "not", "no", "into", "onto", "over", "under", "between", "among", "about", "across", "against", "along", "around", "before",
    "after", "during", "through", "within", "without", "per", "via", "vs",
];

/// 以句点收尾、但并不说明句子结束的缩写（小写比较）
const ABBREVIATIONS: &[&str] = &[
    "al.", "e.g.", "i.e.", "etc.", "fig.", "figs.", "eq.", "eqs.", "sec.", "secs.", "vs.", "cf.", "approx.", "no.", "dr.", "mr.", "mrs.", "ms.", "prof.", "st.",
    "w.r.t.", "resp.", "ch.", "vol.", "pp.", "p.",
];

/// 句子末尾可以跟着的收尾引号、括号
const CLOSERS: &str = "」』）)】]”’\"'》〉";

/// 一行 OCR 文字的版面（逻辑像素，原点左上）与文本（已 trim）。
pub(super) struct EnGeo<'a> {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub text: &'a str,
    /// 这一行的字色（sRGB 三个通道，0–255）；辅助程序没采到就是 None
    pub fg: Option<[f64; 3]>,
    /// 第一个词的宽度（逻辑像素）；没有逐词框就是 None
    pub first_word: Option<f64>,
    /// 逐词框里的总字数与总宽度（逻辑像素），用来算每个字母的平均宽度；没有逐词框就是 (0, 0.0)
    pub word_chars: usize,
    pub word_width: f64,
}

/// `wrap_links` 的结果。下标都是传进来的 `lines` 的下标。
pub(super) struct EnLinks {
    /// 第 j 项是 `Some(i)`：第 j 行（的那一视觉行）是第 i 行折断的下文，i < j
    pub wraps: Vec<Option<usize>>,
    /// 第 k 项是 `Some(h)`：第 k 行只是第 h 行（同一视觉行的左边那段）的右半截，要并进 h 所在的块；
    /// 只有这一视觉行自己在某条折行链里才算数，孤零零的两个小框（导航栏的相邻两项）不拼
    pub fragment_of: Vec<Option<usize>>,
}

/// 一条视觉行：一个 OCR 行，或者被 Vision 拆成几段的同一行。
struct Row {
    /// 行首（最左边那段）在 `lines` 里的下标
    head: usize,
    /// 同一视觉行里从左到右的各段（含行首）
    group: Vec<usize>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    text: String,
    fg: Option<[f64; 3]>,
    first_word: Option<f64>,
    word_chars: usize,
    word_width: f64,
}

impl Row {
    fn right(&self) -> f64 {
        self.x + self.w
    }

    fn bottom(&self) -> f64 {
        self.y + self.h
    }

    /// 每个字母的平均宽度。有逐词框（够 8 个字符）就只用词框，否则退回整行宽度除以字符数
    fn char_width(&self) -> f64 {
        if self.word_chars >= 8 {
            self.word_width / self.word_chars as f64
        } else {
            self.w / self.text.chars().count().max(1) as f64
        }
    }

    /// 下一行第一个词的宽度：没有逐词框就按字符数估
    fn first_word_width(&self) -> f64 {
        match self.first_word {
            Some(width) if width > 0.0 => width,
            _ => {
                let token = self.text.split_whitespace().next().unwrap_or("");
                token.chars().count() as f64 * self.w / self.text.chars().count().max(1) as f64
            }
        }
    }
}

// ───────────────────────────── 文字上的小判据 ─────────────────────────────

/// 任何一个 CJK 字符：汉字（含部首、符号区、扩展 B 以后）、假名、谚文（字母区与音节区）、兼容汉字、全角形式。
pub(super) fn has_cjk(text: &str) -> bool {
    text.chars().any(|ch| matches!(ch, '\u{1100}'..='\u{11ff}' | '\u{2e80}'..='\u{9fff}' | '\u{ac00}'..='\u{d7af}' | '\u{f900}'..='\u{faff}' | '\u{ff00}'..='\u{ffef}' | '\u{20000}'..='\u{2fa1f}'))
}

fn ascii_letters(text: &str) -> usize {
    text.chars().filter(|ch| ch.is_ascii_alphabetic()).count()
}

fn luminance(color: [f64; 3]) -> f64 {
    0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2]
}

/// 以项目符号、序号、字母序号、`[n]` 引用号、节号、代码注释符开头：这是列表里 / 文档里的新一条，不是上一行的下文。
/// `3.5 times`、`2018). These` 这种数字开头的不算。
fn starts_with_marker(text: &str) -> bool {
    let t = text.trim_start();
    let chars: Vec<char> = t.chars().take(12).collect();
    let Some(&first) = chars.first() else {
        return false;
    };
    if "•·●○▪◦*–—-".contains(first) {
        return true;
    }
    let second = chars.get(1).copied();
    let third = chars.get(2).copied();
    let is_space_or_end = |ch: Option<char>| ch.is_none_or(|ch| ch.is_whitespace());
    // 代码注释符：`//` `/*` `*/` `#<空白>` `#!` `--<空白>` `%<空白>`
    match first {
        '/' if matches!(second, Some('/') | Some('*')) => return true,
        '#' if second == Some('!') || second.is_some_and(|ch| ch.is_whitespace()) => return true,
        '-' if second == Some('-') && third.is_some_and(|ch| ch.is_whitespace()) => return true,
        '%' if second.is_some_and(|ch| ch.is_whitespace()) => return true,
        _ => {}
    }
    // `1.` `12)` 后面是空白或结尾（`1234.` `2018)` 的数字太多、`7.2` 后面不是空白，都不是）
    let digits = chars.iter().take_while(|ch| ch.is_ascii_digit()).count();
    if (1..=2).contains(&digits) && chars.get(digits).is_some_and(|ch| ".)".contains(*ch)) && is_space_or_end(chars.get(digits + 1).copied()) {
        return true;
    }
    // `(1)` `[12]` `(a)`
    if first == '(' || first == '[' {
        let inner = chars[1..].iter().take_while(|ch| ch.is_ascii_digit()).count();
        if (1..=3).contains(&inner) {
            let close = chars.get(1 + inner).copied();
            if close.is_some_and(|ch| ch == ')' || ch == ']') && is_space_or_end(chars.get(2 + inner).copied()) {
                return true;
            }
        }
        if first == '(' && second.is_some_and(|ch| ch.is_ascii_alphabetic()) && third == Some(')') && is_space_or_end(chars.get(3).copied()) {
            return true;
        }
    }
    // `a.` `B)` 单个字母
    if first.is_ascii_alphabetic() && second.is_some_and(|ch| ".)".contains(ch)) && is_space_or_end(third) {
        return true;
    }
    // `iv.` `XII)` 罗马数字（最多四个字母）
    let romans = chars.iter().take_while(|ch| "ivxIVX".contains(**ch)).count();
    if (1..=4).contains(&romans) && chars.get(romans).is_some_and(|ch| ".)".contains(*ch)) && is_space_or_end(chars.get(romans + 1).copied()) {
        return true;
    }
    // 节号 `7.2 The Larger…` `3.1.2) Foo`：1–2 位数字，一段或多段 `.1–2 位数字`，可选 `.` / `)`，空白，大写字母
    section_number(t)
}

/// `7.2 Title`、`3.1.2. Title`、`4.1) Title`：节号 + 空白 + 大写字母开头。
fn section_number(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    let digits = |from: usize| chars[from..].iter().take_while(|ch| ch.is_ascii_digit()).count();
    let first = digits(at);
    if !(1..=2).contains(&first) {
        return false;
    }
    at += first;
    let mut segments = 0;
    while chars.get(at) == Some(&'.') {
        let run = digits(at + 1);
        if !(1..=2).contains(&run) {
            break;
        }
        at += 1 + run;
        segments += 1;
    }
    if segments == 0 {
        return false;
    }
    if chars.get(at).is_some_and(|ch| ".)".contains(*ch)) {
        at += 1;
    }
    let spaces = chars[at.min(chars.len())..].iter().take_while(|ch| ch.is_whitespace()).count();
    spaces >= 1 && chars.get(at + spaces).is_some_and(|ch| ch.is_ascii_uppercase())
}

/// 圆括号 / 方括号还没合上：句子一定还在继续（引用清单 `(Bowman et al., 2015;` 以分号收尾）。
fn unclosed(text: &str) -> bool {
    let count = |ch: char| text.chars().filter(|c| *c == ch).count();
    count('(') > count(')') || count('[') > count(']')
}

/// 句末标点：句号、问号、叹号和省略号（被 `text-overflow` 截断的一行，后面不会再有下文；和 `zh_wrap` 的口径一样）。
const SENTENCE_FINAL: &str = ".!?。！？…⋯‥";

/// 这一行把一句话说完了：句末标点（可以后跟收尾引号 / 括号），冒号、分号、`$ > | / \` 也算（终端、代码、列表）。
/// 缩写（`et al.` `e.g.` `Fig.` `w.r.t.`、单个大写字母的人名缩写 `J.`）后面的句点不算；括号没合上的行没说完——
/// 但以句末标点收尾的先算说完了（`It works (mostly.`）。
fn ends_sentence(text: &str) -> bool {
    let t = text.trim_end();
    if t.is_empty() {
        return true;
    }
    let core = t.trim_end_matches(|ch| CLOSERS.contains(ch));
    let Some(last) = core.chars().last() else {
        return true;
    };
    // 括号没合上的行没说完（`(Bowman et al., 2015;`）——但以句号、问号、叹号收尾的先算说完了
    if unclosed(t) && !SENTENCE_FINAL.contains(last) {
        return false;
    }
    if SENTENCE_FINAL.contains(last) {
        if last == '.' {
            let token = core.split_whitespace().last().unwrap_or("");
            let lower = token.to_lowercase();
            if ABBREVIATIONS.contains(&lower.as_str()) || is_dotted_initialism(&lower) || is_single_capital_initial(token) {
                return false;
            }
        }
        return true;
    }
    ":;：；$>|/\\".contains(last)
}

/// `e.g.` `w.r.t.` 这种「字母. 字母. …」（至少两组）。
fn is_dotted_initialism(token: &str) -> bool {
    let chars: Vec<char> = token.chars().collect();
    chars.len() >= 4 && chars.len() % 2 == 0 && chars.chunks(2).all(|pair| pair[0].is_ascii_lowercase() && pair[1] == '.')
}

/// `J.` 单个大写字母加句点（人名缩写）。
fn is_single_capital_initial(token: &str) -> bool {
    let mut chars = token.chars();
    matches!((chars.next(), chars.next(), chars.next()), (Some(letter), Some('.'), None) if letter.is_ascii_uppercase())
}

/// 宽松一点的「像正文」：词数够、字母占一半以上、不像代码。已经有别的证据（下一行小写开头、上一行断在逗号 / 连接词处）时用它。
fn weak_prose(text: &str, min_words: usize) -> bool {
    if text.split_whitespace().count() < min_words {
        return false;
    }
    let nonspace = text.chars().filter(|ch| !ch.is_whitespace()).count();
    let letters = text.chars().filter(|ch| ch.is_alphabetic()).count();
    nonspace > 0 && (letters as f64) / (nonspace as f64) >= 0.5 && !codeish(text)
}

/// 代码里常见的符号（圆括号、逗号、句点、冒号、连字符、百分号不算：论文和正文里到处是）。
const CODE_SYMBOLS: &str = "{}[]<>=_;|\\^~`@#$";

/// 像代码 / 命令 / 路径：符号密度超过 5%，或者有 `->` `::` `=>` `()` `a_b` `--x`，或者以 `$ ` `# ` `> ` 开头。
/// 引用编号 `[12]` `[1, 2]` `[3–5]` 和 `[CLS]` 这种全大写的记号不算符号。
fn codeish(text: &str) -> bool {
    let nonspace = text.chars().filter(|ch| !ch.is_whitespace()).count();
    if nonspace == 0 {
        return false;
    }
    let cleaned = strip_bracketed_markers(text);
    let symbols = cleaned.chars().filter(|ch| CODE_SYMBOLS.contains(*ch)).count();
    if (symbols as f64) / (nonspace as f64) > 0.05 {
        return true;
    }
    if ["->", "::", "=>", "()"].iter().any(|pattern| cleaned.contains(pattern)) {
        return true;
    }
    let chars: Vec<char> = cleaned.chars().collect();
    let word = |ch: char| ch.is_alphanumeric() || ch == '_';
    if chars.windows(3).any(|w| w[1] == '_' && word(w[0]) && word(w[2])) || chars.windows(3).any(|w| w[0] == '-' && w[1] == '-' && word(w[2])) {
        return true;
    }
    let trimmed = cleaned.trim_start();
    let mut head = trimmed.chars();
    matches!((head.next(), head.next()), (Some('$' | '#' | '>'), Some(space)) if space.is_whitespace())
}

/// 把 `[12]` `[1, 2]` `[3–5]` `[1-3]`（数字、逗号、空白、连字符 / 短横）和 `[CLS]`（两个以上大写字母）换成空格。
fn strip_bracketed_markers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < chars.len() {
        if chars[at] == '[' {
            let mut end = at + 1;
            if chars.get(end).is_some_and(|ch| ch.is_ascii_digit()) {
                while chars.get(end).is_some_and(|ch| ch.is_ascii_digit() || ",–- ".contains(*ch) || ch.is_whitespace()) {
                    end += 1;
                }
            } else {
                while chars.get(end).is_some_and(|ch| ch.is_ascii_uppercase()) {
                    end += 1;
                }
                if end - (at + 1) < 2 {
                    end = at + 1;
                }
            }
            if end > at + 1 && chars.get(end) == Some(&']') {
                out.push(' ');
                at = end + 1;
                continue;
            }
        }
        out.push(chars[at]);
        at += 1;
    }
    out
}

/// 以引用编号开头：`[16], ByteNet …` `[9]. The …` `[1, 2]` `[3–5]`（每个数字至多 3 位，逗号 / 连字符 / 短横 / 空格分隔，中间没有别的字符）。
/// 句子在引用编号前面断行很常见（`…the foundation of the Extended Neural GPU` ⏎ `[16], ByteNet [18] …`），这样的行是上一行的下文；
/// 后面直接跟空白的 `[16] ByteNet` 是参考文献条目（`starts_with_marker`），`[INFO]` `[10:31:46]` `[img/sec/core]` 这种标签也不在这里。
fn starts_with_citation(text: &str) -> bool {
    let chars: Vec<char> = text.trim_start().chars().collect();
    if chars.first() != Some(&'[') {
        return false;
    }
    let (mut digits, mut any) = (0, false);
    for &ch in &chars[1..] {
        match ch {
            '0'..='9' => {
                digits += 1;
                any = true;
                if digits > 3 {
                    return false;
                }
            }
            ',' | '–' | '-' | ' ' => digits = 0,
            ']' => return any,
            _ => return false,
        }
    }
    false
}

/// 第一个字母串像一个词：至少三个字母，或者是常见的短功能词（`of` `to` `as` `is`……）。
/// 伪代码、公式里的变量名（`vo + 0 (Initialize …)` 的 `vo`、`t ← 0`）小写开头，却不是上一行的下文。
fn starts_with_a_word(text: &str) -> bool {
    let run: String = text.chars().skip_while(|ch| !ch.is_ascii_alphabetic()).take_while(|ch| ch.is_ascii_alphabetic()).collect::<String>().to_lowercase();
    run.chars().count() >= 3 || STOP_WORDS.contains(&run.as_str()) || CONNECT_WORDS.contains(&run.as_str())
}

/// 这一行最后一个英文词（小写）；去掉末尾的标点和收尾引号 / 括号再找。
fn last_word(text: &str) -> String {
    let trimmed = text.trim_end().trim_end_matches(|ch: char| CLOSERS.contains(ch) || ",;:.!?".contains(ch));
    let word: Vec<char> = trimmed.chars().rev().take_while(|ch| ch.is_ascii_alphabetic()).collect();
    word.into_iter().rev().collect::<String>().to_lowercase()
}

/// 两行都带「标签: 值」（冒号后面跟空白，不是行尾那一个）：键值列表，不是折行。
fn has_inner_colon(text: &str) -> bool {
    let trimmed = text.trim();
    let body = &trimmed[..trimmed.char_indices().last().map(|(at, _)| at).unwrap_or(0)];
    body.char_indices().any(|(at, ch)| ch == ':' && body[at + 1..].chars().next().is_some_and(|next| next.is_whitespace()))
}

// ───────────────────────────── 版面 ─────────────────────────────

/// 同一视觉行里被拆成几段的 OCR 行：返回每一行的「行首」（自己不是碎片就是 None）。
fn fragment_heads(lines: &[EnGeo]) -> Vec<Option<usize>> {
    let count = lines.len();
    let usable: Vec<bool> = lines.iter().map(|line| !has_cjk(line.text) && line.w.is_finite() && line.h.is_finite() && line.x.is_finite() && line.y.is_finite() && line.w > 0.0 && line.h > 0.0).collect();
    let mut by_x: Vec<usize> = (0..count).collect();
    by_x.sort_by(|a, b| lines[*a].x.total_cmp(&lines[*b].x));
    let mut left_neighbour: Vec<Option<usize>> = vec![None; count];
    for &b in &by_x {
        if !usable[b] {
            continue;
        }
        let right = &lines[b];
        let mut best: Option<(f64, usize)> = None;
        for &a in &by_x {
            let left = &lines[a];
            if a == b || !usable[a] || left.x >= right.x {
                continue;
            }
            let h = left.h.max(right.h);
            if h > FRAGMENT_HEIGHT_RATIO * left.h.min(right.h) {
                continue;
            }
            let overlap = (left.y + left.h).min(right.y + right.h) - left.y.max(right.y);
            if overlap < FRAGMENT_V_OVERLAP * left.h.min(right.h) {
                continue;
            }
            let gap = right.x - (left.x + left.w);
            if gap < -FRAGMENT_OVERLAP_MAX * h || gap > FRAGMENT_GAP_MAX * h {
                continue;
            }
            if best.is_none_or(|(smallest, _)| gap < smallest) {
                best = Some((gap, a));
            }
        }
        left_neighbour[b] = best.map(|(_, a)| a);
    }
    // 追到最左边那一段
    let mut heads = vec![None; count];
    for k in 0..count {
        let Some(mut at) = left_neighbour[k] else {
            continue;
        };
        let mut seen = vec![k];
        while let Some(next) = left_neighbour[at] {
            if seen.contains(&at) {
                break;
            }
            seen.push(at);
            at = next;
        }
        heads[k] = Some(at);
    }
    heads
}

/// 把同一视觉行的各段拼成一条 `Row`，按 y 从小到大排好。
fn build_rows(lines: &[EnGeo], heads: &[Option<usize>]) -> Vec<Row> {
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); lines.len()];
    for (k, head) in heads.iter().enumerate() {
        if let Some(head) = head {
            members[*head].push(k);
        }
    }
    let mut rows: Vec<Row> = Vec::new();
    for head in (0..lines.len()).filter(|k| heads[*k].is_none()) {
        let mut group = vec![head];
        let mut rest = std::mem::take(&mut members[head]);
        rest.sort_by(|a, b| lines[*a].x.total_cmp(&lines[*b].x));
        group.extend(rest);
        let first = &lines[head];
        if group.len() == 1 {
            rows.push(Row {
                head,
                group,
                x: first.x,
                y: first.y,
                w: first.w,
                h: first.h,
                text: first.text.to_string(),
                fg: first.fg,
                first_word: first.first_word,
                word_chars: first.word_chars,
                word_width: first.word_width,
            });
            continue;
        }
        let left = group.iter().map(|k| lines[*k].x).fold(f64::INFINITY, f64::min);
        let right = group.iter().map(|k| lines[*k].x + lines[*k].w).fold(f64::NEG_INFINITY, f64::max);
        let top = group.iter().map(|k| lines[*k].y).fold(f64::INFINITY, f64::min);
        let bottom = group.iter().map(|k| lines[*k].y + lines[*k].h).fold(f64::NEG_INFINITY, f64::max);
        rows.push(Row {
            head,
            x: left,
            y: top,
            w: right - left,
            h: bottom - top,
            text: group.iter().map(|k| lines[*k].text).collect::<Vec<_>>().join(" "),
            fg: first.fg,
            first_word: first.first_word,
            word_chars: group.iter().map(|k| lines[*k].word_chars).sum(),
            word_width: group.iter().map(|k| lines[*k].word_width).sum(),
            group,
        });
    }
    rows.sort_by(|a, b| a.y.total_cmp(&b.y));
    rows
}

/// 给每一条视觉行找「应当并进哪一行」：返回的第 j 项是 `Some(i)`，表示第 j 行是第 i 行折断的下文（i < j）。
///
/// `rows` 要按 y 从小到大排好。分两步：
/// 1. 版面：每一行找「同列里紧挨在它上面的那一行」（上方、左缘对齐的最近一行，不是排序里的上一行——两栏的页面上
///    左右两栏的行是交替排的）。连起来的一串行叫一列，列的右缘 R 取各行右缘的 0.9 分位。
/// 2. 判定：见 `wrap_links` 的各个关卡，每一关单独有测试。
fn row_links(rows: &[Row]) -> Vec<Option<usize>> {
    let count = rows.len();
    let finite = |row: &Row| [row.x, row.y, row.w, row.h].iter().all(|value| value.is_finite()) && row.w > 0.0 && row.h > 0.0;
    let eligible: Vec<bool> = rows.iter().map(|row| finite(row) && !has_cjk(&row.text) && ascii_letters(&row.text) >= 3).collect();

    // 1. 同列里紧挨着的上一行。每一行最多有一个下文
    let mut above: Vec<Option<usize>> = vec![None; count];
    let mut taken = vec![false; count];
    for (j, below) in rows.iter().enumerate() {
        if !eligible[j] || starts_with_marker(&below.text) {
            continue;
        }
        let mut closest: Option<usize> = None;
        for i in (0..j).rev() {
            let upper = &rows[i];
            if taken[i] || !eligible[i] {
                continue;
            }
            let h = upper.h.max(below.h);
            let gap = below.y - upper.bottom();
            if gap < GAP_MIN * h || gap > GAP_MAX * h {
                continue;
            }
            if (upper.h - below.h).abs() > HEIGHT_DIFF_MAX * h {
                continue;
            }
            let shift = below.x - upper.x;
            let indented = above[i].is_none() && shift < 0.0 && shift >= -INDENT_MAX * h;
            // 下一行比上一行靠右超过 0.6h 是缩进着起头的新段；只有上一行是列表项第一行时，下一行的悬挂缩进放行
            let hanging = starts_with_marker(&upper.text) && shift > 0.0 && shift <= HANGING_MAX * h;
            if shift.abs() > LEFT_ALIGN_TOLERANCE * h && !indented && !hanging {
                continue;
            }
            if closest.is_none_or(|best| upper.bottom() > rows[best].bottom()) {
                closest = Some(i);
            }
        }
        if let Some(i) = closest {
            above[j] = Some(i);
            taken[i] = true;
        }
    }

    // 一列的右缘：沿着连接找到列头，各行把自己记到列头上
    let mut head: Vec<usize> = (0..count).collect();
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); count];
    for j in 0..count {
        if let Some(i) = above[j] {
            head[j] = head[i];
        }
        members[head[j]].push(j);
    }
    let mut column_right = vec![f64::NEG_INFINITY; count];
    for (leader, group) in members.iter().enumerate() {
        if group.is_empty() {
            continue;
        }
        let mut widths: Vec<f64> = group.iter().map(|k| rows[*k].w).collect();
        widths.sort_by(|a, b| a.total_cmp(b));
        let median = widths[widths.len() / 2];
        let mut rights: Vec<f64> = group.iter().filter(|k| rows[**k].w <= RIGHT_OUTLIER * median).map(|k| rows[*k].right()).collect();
        if rights.is_empty() {
            rights = group.iter().map(|k| rows[*k].right()).collect();
        }
        rights.sort_by(|a, b| a.total_cmp(b));
        column_right[leader] = rights[(((RIGHT_QUANTILE * rights.len() as f64) as usize)).min(rights.len() - 1)];
    }

    // 2. 逐对判定
    let mut wraps: Vec<Option<usize>> = vec![None; count];
    for j in 0..count {
        let Some(i) = above[j] else {
            continue;
        };
        let (upper, below) = (&rows[i], &rows[j]);
        let h = upper.h.max(below.h);
        let right = column_right[head[j]];
        let established = wraps[i].is_some();
        let tolerance = h * if established { FULL_NEXT } else { FULL_FIRST };
        // 窄栏里的短行不当折行
        if upper.w < MIN_WIDTH * h {
            continue;
        }
        // 跨栏拼出来的长行（上一行或下一行比整列宽得多）不参与
        if upper.right() > right + OVERWIDE * h || below.right() > right + OVERWIDE * h {
            continue;
        }
        // 被折断得有理由：写满了一行；或者（参差右缘）下一行的第一个词放不下
        let shortfall = right - upper.right();
        let full = shortfall <= tolerance;
        let forced = full || (upper.w >= RAGGED_MIN_FILL * (right - upper.x) && shortfall - RAGGED_SLACK * h < SPACE_WIDTH * h + below.first_word_width());
        if !forced {
            continue;
        }
        // 字色亮度差得远：标题 + 灰色的摘要、正文 + 页脚小字。任何一边没采到字色就当作没差别
        if let (Some(a), Some(b)) = (upper.fg, below.fg) {
            if (luminance(a) - luminance(b)).abs() > LUMINANCE_DIFF_MAX {
                continue;
            }
        }
        let (upper_text, below_text) = (upper.text.as_str(), below.text.as_str());
        if codeish(upper_text) || codeish(below_text) {
            continue;
        }
        // 字号不一样（标题、小号注释）：每个字母的平均宽度差得远
        if upper_text.chars().count() >= CHAR_WIDTH_MIN_CHARS
            && below_text.chars().count() >= CHAR_WIDTH_MIN_CHARS
            && (upper.char_width() / below.char_width() - 1.0).abs() > CHAR_WIDTH_DIFF_MAX
        {
            continue;
        }
        // 两行都是「标签: 值」：键值列表
        if has_inner_colon(upper_text) && has_inner_colon(below_text) {
            continue;
        }
        // 上一行已经把话说完了（句末标点收尾）：各译各的不吃亏，也避开「这一行恰好是段落最后一行」的错合
        if ends_sentence(upper_text) {
            continue;
        }
        // 证据：下一行小写（或左圆括号 / 引号）开头；上一行断在逗号、连字符、连接词处，或括号还没合上；或者这一段已经成立（并且是真的写满）。
        // 方括号只认引用编号（`[16], ByteNet …`），花括号不认：`[INFO]` `[10:31:46]` `{INFO}` 开头的是日志 / 聊天里一条条独立的行
        let below_first = below_text.chars().next();
        let opens_like_a_continuation = below_first.is_some_and(|ch| "(\"“‘'".contains(ch)) || starts_with_citation(below_text);
        let lower_start = (below_first.is_some_and(|ch| ch.is_lowercase()) || (opens_like_a_continuation && !starts_with_marker(below_text))) && starts_with_a_word(below_text);
        let open_end = upper_text.trim_end().chars().last().is_some_and(|ch| ",-—(".contains(ch)) || CONNECT_WORDS.contains(&last_word(upper_text).as_str()) || unclosed(upper_text);
        // 没有任何证据（下一行大写 / 数字开头、上一行以实词结尾、这一段还没成立）就不合：一列互相独立的、不带标点的长行（侧栏菜单、
        // 标题列表）几何上和折行一模一样，只有「下一行小写开头」之类的文字证据分得开。量过：加上这一档只多合 0.3% 的真折行，
        // 却是真实页面上错合率最高的一档
        let linked = (lower_start || open_end || (established && full)) && weak_prose(upper_text, WEAK_MIN_WORDS) && (weak_prose(below_text, 1) || below_text.split_whitespace().count() <= 3);
        if linked {
            wraps[j] = Some(i);
        }
    }
    wraps
}

/// 英文硬折行：每一行应当并进哪一行（同一视觉行被拆开的碎片先拼好再判）。
pub(super) fn wrap_links(lines: &[EnGeo]) -> EnLinks {
    let heads = fragment_heads(lines);
    let rows = build_rows(lines, &heads);
    let row_wraps = row_links(&rows);
    let mut wraps: Vec<Option<usize>> = vec![None; lines.len()];
    let mut fragment_of: Vec<Option<usize>> = vec![None; lines.len()];
    for (r, row) in rows.iter().enumerate() {
        let linked_below = row_wraps.iter().any(|up| *up == Some(r));
        if let Some(up) = row_wraps[r] {
            wraps[row.head] = Some(rows[up].head);
        }
        if row.group.len() > 1 && (row_wraps[r].is_some() || linked_below) {
            for &k in &row.group[1..] {
                fragment_of[k] = Some(row.head);
            }
        }
    }
    EnLinks { wraps, fragment_of }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一行：带统一的逐词框（每个字母 7.0 宽，两行的字号一样），没有字色。测试里文字长度和宽度不必对得上，也不会撞上字号那一关
    fn geo(text: &str, x: f64, y: f64, w: f64, h: f64) -> EnGeo<'_> {
        EnGeo { x, y, w, h, text, fg: None, first_word: None, word_chars: 100, word_width: 700.0 }
    }

    /// 没有逐词框的一行（旧版辅助程序的输出）：每个字母的宽度按整行宽 / 字符数估。
    fn geo_plain(text: &str, x: f64, y: f64, w: f64, h: f64) -> EnGeo<'_> {
        EnGeo { x, y, w, h, text, fg: None, first_word: None, word_chars: 0, word_width: 0.0 }
    }

    fn geo_fg(text: &str, x: f64, y: f64, w: f64, h: f64, fg: [f64; 3]) -> EnGeo<'_> {
        EnGeo { fg: Some(fg), ..geo(text, x, y, w, h) }
    }

    fn links(lines: &[EnGeo]) -> Vec<Option<usize>> {
        wrap_links(lines).wraps
    }

    // 基准的两行：上一行写满、断在连接词 `than` 上，下一行小写开头。h = 16，两行紧挨着（间隙 2），左缘对齐
    const UPPER: &str = "Translating text that appears on a screen is harder than";
    const LOWER: &str = "translating a document, because the text has been";

    /// 基准两行加两行填充，让这一列有足够多的行（右缘 R = 520）：上面一行也写满，下面一行是下文。
    fn base<'a>() -> Vec<EnGeo<'a>> {
        vec![geo(UPPER, 100.0, 100.0, 420.0, 16.0), geo(LOWER, 100.0, 118.0, 368.0, 16.0)]
    }

    fn linked(lines: &[EnGeo]) -> bool {
        links(lines)[1] == Some(0)
    }

    // ───────────────────────── 文字上的小判据 ─────────────────────────

    #[test]
    fn any_cjk_character_keeps_a_line_out_of_the_english_path() {
        for inside in ['\u{1100}', '\u{11ff}', '\u{2e80}', '\u{3001}', '\u{3042}', '\u{4e00}', '\u{9fff}', '\u{ac00}', '\u{d7af}', '\u{f900}', '\u{faff}', '\u{ff00}', '\u{ff0c}', '\u{ffef}', '\u{20000}', '\u{2fa1f}'] {
            assert!(has_cjk(&format!("a{inside}b")), "{inside:?} 是 CJK");
        }
        for outside in ['\u{10ff}', '\u{1200}', '\u{1ffff}', '\u{2fa20}', '\u{2e7f}', '\u{a000}', '\u{abff}', '\u{d7b0}', '\u{f8ff}', '\u{fb00}', '\u{feff}', '\u{fff0}', 'a', 'é', 'ß', '“', '…', '—', 'я'] {
            assert!(!has_cjk(&format!("a{outside}b")), "{outside:?} 不是");
        }
        assert!(!has_cjk(""));
        // 含汉字的一行不参与：哪怕版面是一段完美的折行
        let lines = [geo("Translating 文字 that appears on a screen is harder than", 100.0, 100.0, 420.0, 16.0), geo(LOWER, 100.0, 118.0, 368.0, 16.0)];
        assert_eq!(links(&lines), vec![None, None]);
    }

    #[test]
    fn markers_comments_and_section_numbers_start_a_new_unit() {
        for marker in [
            "• item", "·item", "● x", "○ x", "▪ x", "◦ x", "* item", "– dash", "— dash", "- item", "   - indented", "1. First", "12) Twelfth", "9.", "3)", "(1) first", "(12) twelfth",
            "[7] Smith", "[123] x", "(a) letter", "a. one", "B) two", "I. roman", "iv. four", "XII) twelve", "7.2 The Larger Model", "3.1.2 Details", "4.1) Training", "10.5. Results",
            "// comment", "/* block", "*/ end", "# heading", "#!/bin/sh", "-- sql comment", "% latex",
        ] {
            assert!(starts_with_marker(marker), "「{marker}」是新的一条");
        }
        for plain in [
            "", "   ", "The quick", "2018). These include", "3.5 times faster", "1234. no", "12.5 percent", "7.2", "7.2 the lower case", "(1234) four digits", "[1234]", "(ab) two letters", "ab. two",
            "#hashtag", "%d items", "/path/to/file", "1.2.3.4 is an address", "vi. is not roman here? yes it is", "mixed, case",
        ] {
            // 最后两条是故意放进来的边界：`vi.` 是罗马数字、`1.2.3.4 is` 的节号后面不是大写字母
            let expect = plain == "vi. is not roman here? yes it is";
            assert_eq!(starts_with_marker(plain), expect, "「{plain}」");
        }
        assert!(section_number("7.2 The Larger"));
        assert!(section_number("10.12.3. Title"));
        assert!(!section_number("7.2 lower"), "节号后面要大写字母");
        assert!(!section_number("123.4 Big"), "节号第一段至多两位");
        assert!(!section_number("7 Big"), "至少一个小数点");
        assert!(!section_number("7.2Big"), "节号后面要有空白");
    }

    #[test]
    fn a_sentence_ends_at_its_final_punctuation_unless_it_is_an_abbreviation_or_a_bracket_is_open() {
        for done in ["It works.", "Does it work?", "It works!", "He said “done.”", "(see below.)", "结束。", "Done.)", "ends with a colon:", "ends with a semicolon;", "$", "> quoted >", "a | b |", "path/", "end\\", "a truncated card summary…", "cut off⋯", "cut off ‥", "He said “well…”"] {
            assert!(ends_sentence(done), "「{done}」说完了");
        }
        for open in [
            "it continues", "a comma,", "a hyphen-", "an em dash —", "as shown by Smith et al.", "for example, e.g.", "that is, i.e.", "see Fig.", "see Eq.", "w.r.t.", "approx.", "by J.", "by A. B.", "(Smith et al., 2018)", "(Bowman et al., 2015;",
            "(see section 3", "list [1,", "he said “well”",
        ] {
            assert!(!ends_sentence(open), "「{open}」没说完");
        }
        assert!(ends_sentence(""), "空串当说完了（没有可接的）");
        // 括号没合上但以句号收尾：算说完了；没合上又没句号：没说完
        assert!(ends_sentence("It works (mostly."));
        assert!(!ends_sentence("It works (mostly"));
        // 单个大写字母的缩写与点号缩写
        assert!(is_single_capital_initial("J."));
        assert!(!is_single_capital_initial("Jo."));
        assert!(!is_single_capital_initial("j."));
        assert!(is_dotted_initialism("e.g."));
        assert!(is_dotted_initialism("w.r.t."));
        assert!(!is_dotted_initialism("e.g"));
        assert!(!is_dotted_initialism("a."), "至少两组");
        assert!(!is_dotted_initialism("ab.c."));
        assert!(unclosed("(a") && unclosed("[a") && !unclosed("(a)") && !unclosed("[a]") && !unclosed(") ("), "只数个数，`) (` 数量相等不算没合上");
    }

    #[test]
    fn a_citation_number_at_the_start_of_a_line_is_recognised_but_tags_and_dates_are_not() {
        for yes in ["[16], ByteNet", "[9]. The", "[1, 2] and", "[3–5] show", "[12-13] show", "  [7], and", "[ 4 ], x"] {
            assert!(starts_with_citation(yes), "「{yes}」");
        }
        for no in ["", "16], x", "(16), x", "[INFO] x", "[10:31:46] x", "[2026-11-01] x", "[] x", "[,] x", "[16 x", "[1234] x", "[a1] x"] {
            assert!(!starts_with_citation(no), "「{no}」");
        }
    }

    #[test]
    fn the_last_word_and_the_first_word_are_looked_at_as_words() {
        assert_eq!(last_word("is harder than"), "than");
        assert_eq!(last_word("is harder than,"), "than");
        assert_eq!(last_word("said “and”"), "and");
        assert_eq!(last_word("year 2018"), "", "结尾是数字就没有");
        assert_eq!(last_word("A.4"), "", "`A.4` 的 A 不是最后一个词");
        assert_eq!(last_word(""), "");
        // 第一个字母串：至少三个字母，或者是常见的短功能词
        for word in ["than translating", "the text", "of it", "to be", "as shown", "is", "a b c", "(Smith et al.)", "“and” then", "was", "ISO"] {
            assert!(starts_with_a_word(word), "「{word}」");
        }
        for not_word in ["vo + 0 (Initialize)", "mo ← 0", "t + 1", "ml models", "xy", "q", "", "123", "(1) x", "é", "x-ray"] {
            assert!(!starts_with_a_word(not_word), "「{not_word}」：变量名 / 缩写 / 没有字母");
        }
    }

    #[test]
    fn code_looking_lines_are_recognised_but_citations_percentages_and_prose_symbols_are_not() {
        for code in [
            "let x = foo(bar);", "std::vec::Vec", "a -> b", "x => y", "call()", "snake_case_name here", "run --verbose now", "$ ls -la", "# comment here", "> quoted here", "array[i] = {1, 2}", "a || b && c ~ d",
        ] {
            assert!(codeish(code), "「{code}」像代码");
        }
        for prose in [
            "We recover whole paragraphs from the geometry (Smith et al., 2018).", "results improve by 7.7% on average", "see [12] and [1, 2] and [3–5] and [1-3] for details", "the [CLS] token is added in front of every example",
            "state-of-the-art (SOTA) models: rock, paper, scissors", "5 + 3 apples & 2 pears", "", "   ", "a --- b",
        ] {
            assert!(!codeish(prose), "「{prose}」是正文");
        }
        // 符号密度：5% 是边界（20 个非空白字符里 1 个符号 = 5%，不算；2 个才算）
        assert!(!codeish("aaaaaaaaa;aaaaaaaaaa"));
        assert!(codeish("aaaaaaaa;aaaaaaaa;aaa"));
        assert_eq!(strip_bracketed_markers("see [12] and [CLS] but [Ab] and [1, 2]"), "see   and   but [Ab] and  ");
        assert_eq!(strip_bracketed_markers("[x"), "[x");
        assert_eq!(strip_bracketed_markers("[1"), "[1");
        assert_eq!(strip_bracketed_markers("a [] b"), "a [] b");
    }

    #[test]
    fn weak_prose_needs_enough_words_mostly_letters_and_nothing_that_looks_like_code() {
        assert!(weak_prose("recover whole paragraphs geometry", 4));
        assert!(!weak_prose("recover whole paragraphs", 4), "词数不够");
        assert!(weak_prose("recover whole paragraphs", 3));
        assert!(!weak_prose("1234 5678 9012 3456", 4), "字母不到一半");
        assert!(weak_prose("12 345 abcd efgh ijkl", 4), "字母占一半以上");
        assert!(!weak_prose("let x = foo(bar); baz", 4), "像代码");
        assert!(!weak_prose("", 1));
    }

    #[test]
    fn a_label_colon_value_line_has_an_inner_colon_but_a_trailing_colon_does_not() {
        assert!(has_inner_colon("Name: value"));
        assert!(has_inner_colon("a: b: c"));
        assert!(!has_inner_colon("ends with a colon:"));
        assert!(!has_inner_colon("https://example.org/x"), "冒号后面不是空白");
        assert!(!has_inner_colon("no colon"));
        assert!(!has_inner_colon(""));
        assert!(!has_inner_colon(":"));
    }

    // ───────────────────────── 版面 ─────────────────────────

    #[test]
    fn the_base_pair_is_one_wrapped_sentence() {
        assert_eq!(links(&base()), vec![None, Some(0)]);
    }

    #[test]
    fn a_line_has_at_most_one_continuation_so_a_duplicate_box_stays_out_of_the_paragraph() {
        // 同一行的框被返回了两次（几乎重合）：上一行只认第一条当下文，重复的那条不并进段落
        let mut lines = base();
        lines.push(geo(LOWER, 100.0, 118.0, 368.0, 16.0));
        assert_eq!(links(&lines), vec![None, Some(0), None]);
    }

    #[test]
    fn a_justified_paragraph_links_every_line_to_the_one_above_including_the_short_last_line() {
        let texts = [
            "Translating text that appears on a screen is harder than",
            "translating a document, because the text has already been",
            "laid out by a typesetting engine and not by the author, so a",
            "sentence is often cut in the middle of a phrase.",
        ];
        let widths = [420.0, 421.0, 419.0, 260.0];
        let lines: Vec<EnGeo> = texts.iter().zip(widths).enumerate().map(|(k, (text, w))| geo(text, 100.0, 100.0 + 18.0 * k as f64, w, 16.0)).collect();
        assert_eq!(links(&lines), vec![None, Some(0), Some(1), Some(2)]);
    }

    #[test]
    fn two_columns_interleaved_by_y_link_within_their_own_column() {
        // 排序按 y：左右两栏的行交替出现；每一栏的行只和自己栏的上一行连
        let left = ["Translating text that appears on a screen is harder than", "translating a document, because the text has already been", "laid out by a typesetting engine and not by the author."];
        let right = ["We recover whole paragraphs from the geometry of the", "recognized lines instead of the sentence that was cut in", "half by the typesetting engine of the screen."];
        let mut lines: Vec<EnGeo> = Vec::new();
        for k in 0..3 {
            lines.push(geo(left[k], 100.0, 100.0 + 18.0 * k as f64, if k < 2 { 420.0 } else { 300.0 }, 16.0));
            lines.push(geo(right[k], 560.0, 101.0 + 18.0 * k as f64, if k < 2 { 420.0 } else { 300.0 }, 16.0));
        }
        assert_eq!(links(&lines), vec![None, None, Some(0), Some(1), Some(2), Some(3)]);
    }

    #[test]
    fn vertical_gap_height_and_alignment_each_have_an_exact_edge() {
        let with = |y: f64, x: f64, h: f64| -> bool {
            let lines = vec![geo(UPPER, 100.0, 100.0, 420.0, 16.0), geo(LOWER, x, y, 380.0, h)];
            linked(&lines)
        };
        // 竖直间隙：[-0.25h, 0.55h]，h = 16 → [-4, 8.8]；上一行底 = 116
        assert!(with(116.0 + 8.8, 100.0, 16.0), "间隙 8.8 在边上");
        assert!(!with(116.0 + 8.9, 100.0, 16.0), "间隙 8.9 超出");
        assert!(with(116.0 - 4.0, 100.0, 16.0), "重叠 4.0 在边上");
        assert!(!with(116.0 - 4.1, 100.0, 16.0), "重叠 4.1 超出");
        // 行高差 ≤ 0.45 × 较大者：upper 16、lower 29 → 差 13 = 0.4483 × 29 在边内；差 14 超出（lower 30，0.4667）
        assert!(with(118.0, 100.0, 29.0));
        assert!(!with(118.0, 100.0, 30.0));
        assert!(with(118.0, 100.0, 9.0), "下一行矮 7：7 ≤ 0.45 × 16 = 7.2");
        assert!(!with(118.0, 100.0, 8.7), "下一行矮 7.3 > 7.2");
        // 左缘：差 ≤ 0.6h = 9.6；下一行靠右 > 0.6h 是缩进着起头的新段，靠左则是首行缩进
        assert!(with(118.0, 109.6, 16.0));
        assert!(!with(118.0, 109.7, 16.0));
        assert!(with(118.0, 90.4, 16.0), "靠左 9.6 也在容差内");
        assert!(with(118.0, 52.0, 16.0), "首行缩进：下一行比上一行靠左 48（3h）仍算");
        assert!(!with(118.0, 51.9, 16.0), "靠左超过 3h 不算");
    }

    #[test]
    fn a_hanging_indent_is_allowed_only_under_a_list_item_and_within_three_line_heights() {
        let item = "• Translating text that appears on a screen is harder than";
        let with = |first: &'static str, x: f64| {
            let lines = vec![geo(first, 100.0, 100.0, 420.0, 16.0), geo(LOWER, x, 118.0, 380.0, 16.0)];
            linked(&lines)
        };
        assert!(with(item, 120.0), "列表项第一行下面缩进 20 的续行");
        assert!(with(item, 148.0), "缩进 48 = 3h 在边上");
        assert!(!with(item, 148.1));
        assert!(!with(UPPER, 120.0), "没有项目符号：缩进 20 是新的一段");
        // 续行本身是新的一条（又一个项目符号）就不连
        let lines = vec![geo(item, 100.0, 100.0, 420.0, 16.0), geo("• translating a document, because the text has already been laid out.", 100.0, 118.0, 380.0, 16.0)];
        assert!(!linked(&lines));
        let lines = vec![geo(UPPER, 100.0, 100.0, 420.0, 16.0), geo("2018). translating a document, because the text has already been", 100.0, 118.0, 380.0, 16.0)];
        assert!(linked(&lines), "`2018).` 不是序号");
    }

    #[test]
    fn a_line_must_be_at_least_seven_line_heights_wide_to_be_a_wrapped_line() {
        let with = |w: f64| {
            let lines = vec![geo("Translating text that appears on", 100.0, 100.0, w, 16.0), geo("a screen is harder than other things", 100.0, 118.0, w - 10.0, 16.0)];
            linked(&lines)
        };
        assert!(with(112.0), "7h = 112 在边上");
        assert!(!with(111.9));
    }

    #[test]
    fn a_wrapped_line_is_full_or_cannot_fit_the_next_word() {
        // 四行的一列：前三行右缘 520，被测的上一行（第三行）右缘 = 520 − shortfall，下一行是第四行
        let with = |shortfall: f64, first_word: Option<f64>| -> Option<usize> {
            let mut lines = vec![
                geo("Translating text that appears on a screen is harder than", 100.0, 100.0, 420.0, 16.0),
                geo("translating a document, because the text has already been", 100.0, 118.0, 420.0, 16.0),
                geo("laid out by a typesetting engine and not by the author, so", 100.0, 136.0, 420.0 - shortfall, 16.0),
                geo("a sentence is often cut in the middle of a phrase.", 100.0, 154.0, 300.0, 16.0),
            ];
            lines[3].first_word = first_word;
            links(&lines)[3]
        };
        // 写满：离列右缘 ≤ 0.5h = 8（起手一对）；成段之后 0.75h = 12
        assert_eq!(with(0.0, Some(1.0)), Some(2));
        assert_eq!(with(8.0, Some(1.0)), Some(2));
        assert_eq!(with(11.9, Some(1.0)), Some(2), "第三行是 established 的：下一行属于成段后的 0.75h");
        assert_eq!(with(12.0, Some(1.0)), Some(2), "12 在 0.75h 边上");
        assert_eq!(with(12.1, Some(1.0)), None, "12.1 > 0.75h：没写满，第一个词又短得放得下");
        // 参差右缘：下一行的第一个词放不下。距离 − 0.2h < 0.3h + 第一个词宽 → 距离 < 8 + 词宽
        assert_eq!(with(12.1, Some(10.0)), Some(2), "距离 12.1 < 8 + 10");
        assert_eq!(with(17.9, Some(10.0)), Some(2));
        assert_eq!(with(18.1, Some(10.0)), None, "距离 18.1 > 8 + 10：放得下");
        assert_eq!(with(40.0, Some(60.0)), Some(2), "第一个词很长，放不下");
        // 没有第一个词的宽度：按它的字符数估（`a` 一个字符，下一行 300 宽 / 50 个字符 = 6 / 字符）：距离 < 8 + 6 = 14
        assert_eq!(with(13.9, None), Some(2));
        assert_eq!(with(14.1, None), None);
        assert_eq!(with(8.0, None), Some(2));
        // 上一行至少占半栏：太短的行即使「放不下」也不算（不能是一行孤零零的短句）
        let lines = vec![
            geo("Translating text that appears on a screen is harder than", 100.0, 100.0, 420.0, 16.0),
            geo("translating a document, because the text has already been", 100.0, 118.0, 420.0, 16.0),
            geo("laid out by a typesetting engine, so", 100.0, 136.0, 209.0, 16.0),
            EnGeo { first_word: Some(500.0), ..geo("a sentence is often cut in the middle of a phrase.", 100.0, 154.0, 300.0, 16.0) },
        ];
        assert_eq!(links(&lines)[3], None, "209 < 0.5 × 420");
        let lines = vec![
            geo("Translating text that appears on a screen is harder than", 100.0, 100.0, 420.0, 16.0),
            geo("translating a document, because the text has already been", 100.0, 118.0, 420.0, 16.0),
            geo("laid out by a typesetting engine, so", 100.0, 136.0, 210.0, 16.0),
            EnGeo { first_word: Some(500.0), ..geo("a sentence is often cut in the middle of a phrase.", 100.0, 154.0, 300.0, 16.0) },
        ];
        assert_eq!(links(&lines)[3], Some(2), "210 = 0.5 × 420 在边上");
    }

    #[test]
    fn the_first_pair_of_a_paragraph_is_held_to_a_stricter_full_line_than_the_rest() {
        // 只有两行的一列：被测的上一行右缘 = R − shortfall（R 由下一行撑开）；第一对只放宽到 0.5h
        let with = |shortfall: f64| {
            let lines = vec![geo(UPPER, 100.0, 100.0, 420.0 - shortfall, 16.0), geo(LOWER, 100.0, 118.0, 420.0, 16.0)];
            let mut lines = lines;
            lines[1].first_word = Some(1.0);
            linked(&lines)
        };
        assert!(with(8.0), "起手一对：0.5h = 8 在边上");
        assert!(with(8.8), "8.8 − 3.2 = 5.6 < 4.8 + 1.0：参差右缘放行");
        assert!(!with(9.0), "9.0 − 3.2 = 5.8 不 < 5.8");
    }

    #[test]
    fn the_columns_right_edge_ignores_a_cross_column_line_and_uses_a_high_quantile() {
        // 跨栏拼出来的长行（右缘 800，宽 700 > 1.5 × 中位宽）不参与 R；它自己也不参与连接
        let mut lines: Vec<EnGeo> = (0..5).map(|k| geo("recover whole paragraphs from the geometry of the lines", 100.0, 100.0 + 18.0 * k as f64, 420.0, 16.0)).collect();
        lines.push(geo("recover whole paragraphs from the geometry of the lines", 100.0, 190.0, 700.0, 16.0));
        let result = links(&lines);
        assert_eq!(&result[..5], &[None, Some(0), Some(1), Some(2), Some(3)], "R = 520，前五行互相连着");
        assert_eq!(result[5], None, "第六行右缘 800 > R + 2h，不连");
        // 十二行的一列：最后一行比别的宽出一截。0.9 分位取到倒数第二档（520），所以那一行的右缘比 R 宽出 2h 以内还能连、超过就不连
        let with_last = |w: f64| {
            let mut lines: Vec<EnGeo> = (0..11).map(|k| geo("recover whole paragraphs from the geometry of the lines", 100.0, 100.0 + 18.0 * k as f64, 420.0, 16.0)).collect();
            lines.push(geo("recover whole paragraphs from the geometry of the lines", 100.0, 100.0 + 18.0 * 11.0, w, 16.0));
            links(&lines)[11]
        };
        assert_eq!(with_last(451.0), Some(10), "右缘 551 < R + 2h = 552");
        assert_eq!(with_last(453.0), None, "右缘 553 > 552");
        // 前十一行不受那一行的影响：R 没有被拉高到 553，各行仍然「写满」
        let mut lines: Vec<EnGeo> = (0..11).map(|k| geo("recover whole paragraphs from the geometry of the lines", 100.0, 100.0 + 18.0 * k as f64, 420.0, 16.0)).collect();
        lines.push(geo("recover whole paragraphs from the geometry of the lines", 100.0, 100.0 + 18.0 * 11.0, 453.0, 16.0));
        assert_eq!(links(&lines)[1..11], (0..10).map(Some).collect::<Vec<_>>()[..]);
        // 离群的界线是中位宽的 1.5 倍：宽 1.45 倍的行（609）还算这一列的，R 被它撑到 709，前面各行都「没写满」；宽 1.55 倍（651）才当作跨栏拼出来的
        let with_wide = |w: f64| -> Vec<Option<usize>> {
            let mut lines: Vec<EnGeo> = (0..5).map(|k| geo("recover whole paragraphs from the geometry of the lines", 100.0, 100.0 + 18.0 * k as f64, 420.0, 16.0)).collect();
            lines.push(geo("recover whole paragraphs from the geometry of the lines", 100.0, 190.0, w, 16.0));
            links(&lines)
        };
        assert_eq!(with_wide(609.0), vec![None; 6]);
        assert_eq!(with_wide(651.0), vec![None, Some(0), Some(1), Some(2), Some(3), None]);
        // 0.9 分位：十二行的一列里有两条宽出一截的行（右缘 616，宽 516 在 1.5 倍中位宽以内）时，倒数第二档就是它们的右缘——R = 616，
        // 其余各行离 R 有 6h，既没写满、下一行的第一个词也不是放不下：这一列不合；只有两条宽行之间（写满了）照常连
        let mut lines: Vec<EnGeo> = (0..10).map(|k| geo("recover whole paragraphs from the geometry of the lines", 100.0, 100.0 + 18.0 * k as f64, 420.0, 16.0)).collect();
        for k in 10..12 {
            lines.push(geo("recover whole paragraphs from the geometry of the lines", 100.0, 100.0 + 18.0 * k as f64, 516.0, 16.0));
        }
        let result = links(&lines);
        assert_eq!(&result[..11], &[None; 11]);
        assert_eq!(result[11], Some(10));
    }

    #[test]
    fn a_new_line_that_starts_with_a_marker_is_never_the_continuation_of_the_line_above() {
        for marker in ["1. translating a document, because the text has already been", "• translating a document, because the text has already been", "7.2 Translating a document, because the text has already been", "# translating a document, because the text has already been"] {
            let lines = vec![geo(UPPER, 100.0, 100.0, 420.0, 16.0), geo(marker, 100.0, 118.0, 380.0, 16.0)];
            assert_eq!(links(&lines)[1], None, "「{marker}」");
        }
    }

    #[test]
    fn the_text_color_gate_compares_luminance_not_hue() {
        let with = |a: [f64; 3], b: [f64; 3]| {
            let lines = vec![geo_fg(UPPER, 100.0, 100.0, 420.0, 16.0, a), geo_fg(LOWER, 100.0, 118.0, 380.0, 16.0, b)];
            linked(&lines)
        };
        assert!(with([0.0; 3], [0.0; 3]));
        assert!(with([0.0; 3], [55.0; 3]), "亮度差 55 在边上");
        assert!(!with([0.0; 3], [56.0; 3]), "亮度差 56");
        assert!(!with([0.0; 3], [90.0; 3]), "灰色的摘要");
        assert!(with([0.0; 3], [0.0, 0.0, 238.0]), "整行是蓝色的引用链接：亮度只差 17");
        assert!(with([10.0, 10.0, 10.0], [9.0, 9.0, 63.0]));
        assert!(!with([0.0; 3], [0.0, 255.0, 0.0]), "绿色的亮度差 182");
        // 缺字色（辅助程序没采到）当作没差别
        let lines = vec![geo_fg(UPPER, 100.0, 100.0, 420.0, 16.0, [0.0; 3]), geo(LOWER, 100.0, 118.0, 368.0, 16.0)];
        assert!(linked(&lines));
    }

    #[test]
    fn code_lines_key_value_lines_and_different_font_sizes_are_not_wrapped_text() {
        // 像代码
        let lines = vec![geo("Translating text that appears on a screen = harder_than", 100.0, 100.0, 420.0, 16.0), geo(LOWER, 100.0, 118.0, 368.0, 16.0)];
        assert_eq!(links(&lines)[1], None);
        let lines = vec![geo(UPPER, 100.0, 100.0, 420.0, 16.0), geo("translating a document; because the text has already been {laid} out.", 100.0, 118.0, 380.0, 16.0)];
        assert_eq!(links(&lines)[1], None);
        // 下一行很短（三个词以内）时 `weak_prose` 不要求它像正文，像代码这一关要另外查
        let lines = vec![geo(UPPER, 100.0, 100.0, 420.0, 16.0), geo("foo_bar() ran", 100.0, 118.0, 100.0, 16.0)];
        assert_eq!(links(&lines)[1], None);
        let lines = vec![geo(UPPER, 100.0, 100.0, 420.0, 16.0), geo("foo bar ran", 100.0, 118.0, 100.0, 16.0)];
        assert_eq!(links(&lines)[1], Some(0), "同样的短行不像代码就连");
        // 键值
        let lines = vec![geo("Translating text: that appears on a screen is harder than", 100.0, 100.0, 420.0, 16.0), geo("translating a document: because the text has already been", 100.0, 118.0, 380.0, 16.0)];
        assert_eq!(links(&lines)[1], None);
        let lines = vec![geo("Translating text: that appears on a screen is harder than", 100.0, 100.0, 420.0, 16.0), geo(LOWER, 100.0, 118.0, 368.0, 16.0)];
        assert_eq!(links(&lines)[1], Some(0), "只有一行有冒号：不是键值列表");
        // 字号：每个字母的平均宽度（词框宽 / 字母数）差 18% 以上
        let sized = |upper_cw: f64, lower_cw: f64| {
            let lines = vec![
                EnGeo { word_chars: 40, word_width: 40.0 * upper_cw, ..geo(UPPER, 100.0, 100.0, 420.0, 16.0) },
                EnGeo { word_chars: 40, word_width: 40.0 * lower_cw, ..geo(LOWER, 100.0, 118.0, 368.0, 16.0) },
            ];
            linked(&lines)
        };
        assert!(sized(7.0, 7.0));
        assert!(sized(7.0 * 1.18, 7.0), "差 18% 在边上");
        assert!(!sized(7.0 * 1.19, 7.0));
        assert!(!sized(7.0, 7.0 * 1.25), "下一行字大：7.0 / 8.75 = 0.8，差 20%");
        assert!(sized(7.0, 7.0 * 1.2), "下一行大 20%：7.0 / 8.4 = 0.833，差 16.7% 在内（不对称，只看上 / 下的比值）");
        // 没有逐词框（或词框里不够 8 个字符）的行退回整行宽度 / 字符数：上一行 57 个字符 420 宽 = 7.4，下一行 50 个字符
        let plain = |lower_w: f64, few_chars: bool| {
            let mut lines = vec![geo_plain(UPPER, 100.0, 100.0, 420.0, 16.0), geo_plain(LOWER, 100.0, 118.0, lower_w, 16.0)];
            if few_chars {
                lines[0].word_chars = 5;
                lines[0].word_width = 5000.0;
                lines[1].word_chars = 5;
                lines[1].word_width = 5000.0;
            }
            linked(&lines)
        };
        assert!(plain(368.0, false));
        assert!(!plain(250.0, false), "50 个字符只有 250 宽：每个字母 5.0 对 7.4");
        assert!(!plain(250.0, true), "词框里只有 5 个字符：不信词框，也退回整行");
        // 两行都至少 20 个字符才比：很短的尾行不比（19 个字符不比，20 个比）
        let tail = |text: &str| links(&[geo_plain(UPPER, 100.0, 100.0, 420.0, 16.0), geo_plain(text, 100.0, 118.0, 30.0, 16.0)])[1];
        assert_eq!(tail("translating a doc"), Some(0), "尾行 17 个字符、宽 30：不拿来比字号");
        assert_eq!(tail("translating a docum"), Some(0), "19 个字符");
        assert_eq!(tail("translating a docume"), None, "20 个字符、宽 30：每个字母 1.5 对 7.4，比了就不是同一段");
    }

    #[test]
    fn a_line_that_ends_a_sentence_is_never_continued_but_abbreviations_and_open_brackets_are() {
        let with = |first: &str, second: &str| {
            let lines = vec![geo(first, 100.0, 100.0, 420.0, 16.0), geo(second, 100.0, 118.0, 380.0, 16.0)];
            linked(&lines)
        };
        let next = "translating a document, because the text has already been";
        assert!(!with("Translating text that appears on a screen is harder.", next));
        assert!(!with("Translating text that appears on a screen is harder?", next));
        assert!(!with("Translating text that appears on a screen is harder!", next));
        assert!(!with("Translating text that appears on a screen is harder:", next));
        assert!(!with("Translating text that appears on a screen is harder;", next));
        assert!(!with("Translating text that appears on a screen is harder.”", next));
        assert!(with("Translating text that appears on a screen, as shown by Smith et al.", next), "缩写 et al.");
        assert!(with("Translating text that appears on a screen, for example, e.g.", next));
        assert!(with("Translating text that appears on a screen is harder (see Fig.", next));
        assert!(with("Translating text that appears on a screen (Bowman et al., 2015;", next), "括号没合上的分号");
        assert!(with("Translating text that appears on a screen is harder than", next));
    }

    #[test]
    fn the_evidence_for_a_continuation_is_a_lowercase_start_an_open_ending_or_an_established_run() {
        let with = |first: &str, second: &str| {
            let lines = vec![geo(first, 100.0, 100.0, 420.0, 16.0), geo(second, 100.0, 118.0, 380.0, 16.0)];
            linked(&lines)
        };
        let content_end = "Translating text that appears on a screen is harder";
        // 下一行小写开头（上一行以实词结尾）
        assert!(with(content_end, "than translating a document, because the text has been"));
        // 下一行以左圆括号 / 引号开头；以方括号、花括号开头的不算（`[INFO]` `[10:31:46]` 是日志里一条条独立的行）
        assert!(with(content_end, "(Smith et al., 2018) than translating a document, because"));
        assert!(!with(content_end, "[INFO] than translating a document, because the text has been"));
        assert!(with("Translating text that appears on a screen is harder", "[16], ByteNet [18] and ConvS2S [9], all of which use convolutional"), "引用编号开头：句子在它前面断行");
        assert!(with("Translating text that appears on a screen is harder", "[9]. The initial convolutional layers of the network extract"));
        assert!(with("Translating text that appears on a screen is harder", "[1, 2] and [3–5] showed that the layers of the network extract"));
        assert!(!with("Translating text that appears on a screen is harder", "[img/sec/core] Another quantity of interest is the largest batch"), "标签不是引用编号");
        assert!(!with("Translating text that appears on a screen is harder", "[10:31:46] Another quantity of interest is the largest batch"));
        assert!(!with("Translating text that appears on a screen is harder", "[2026-11-01] Another quantity of interest is the largest batch"), "四位数字不是引用编号");
        assert!(!with("Translating text that appears on a screen is harder", "[16] ByteNet and ConvS2S, all of which use convolutional layers"), "后面直接跟空白的是参考文献条目");
        assert!(!with(content_end, "{than} translating a document, because the text has been"));
        assert!(with(content_end, "“than” translating a document, because the text has been"));
        // 下一行大写 / 数字开头、上一行以实词结尾、这一段还没成立：没有任何证据，不合（一列不带标点的长行几何上和折行一模一样）
        assert!(!with(content_end, "Nevertheless the cost of the translation is low overall."));
        assert!(!with(content_end, "2018 was the year in which the cost of translation fell."));
        // 小写开头但第一个字母串不像词（变量名）：不算证据；常见的短功能词算
        assert!(!with(content_end, "vo + 0 (Initialize the second moment vector of the run)"));
        assert!(with(content_end, "of the translation, because the text has been laid out"));
        // 上一行断在逗号 / 连字符 / 破折号 / 左括号 / 连接词处：不管下一行怎么开头
        for open in ["Translating text that appears on a screen is harder,", "Translating text that appears on a screen is harder-", "Translating text that appears on a screen is harder —", "Translating text that appears on a screen is harder (", "Translating text that appears on a screen is harder than"] {
            assert!(with(open, "Nevertheless the cost of the translation is low overall."), "「{open}」：上一行断在这里，下一行怎么开头都行");
        }
        // 上一行的括号还没合上：句子一定还在继续，下一行怎么开头都行（合上了就没有这条证据）
        assert!(with("Translating text that appears on a screen (collected by the authors", "Google in 2019) and translating a document, because the text has been"));
        assert!(!with("Translating text that appears on a screen (collected by the authors)", "Google in 2019 and translating a document, because the text has been"));
        // 下一行小写开头、却大半是数字（表格里的一行）：不是下文；一行普通的文字照连
        assert!(!with("Table 2 reports the mean accuracy on the three benchmarks of the", "test 91.2 92.4 93.1 94.0"));
        assert!(with("Table 2 reports the mean accuracy on the three benchmarks of the", "test set, which contains 1,000 sentences in total"));
        // 上一行只有三个词：宽松一档要求至少四个词
        assert!(!with("Translating text that,", "translating a document, because the text has already been"));
        assert!(with("Translating text that appears,", "translating a document, because the text has already been"));
    }

    #[test]
    fn an_established_paragraph_goes_on_across_capital_letters_only_while_lines_are_really_full() {
        // 前两行靠小写开头连上，第三行大写开头（专有名词）、第二行没有任何「断在哪儿」的证据：成段 + 写满就连
        let texts = ["Translating text that appears on a screen is harder than", "translating a document, because the text has already been laid", "out by a typesetting engine and not by the author, so Alice"];
        let build = |third_width: f64| -> Vec<Option<usize>> {
            let mut lines: Vec<EnGeo> = vec![
                geo(texts[0], 100.0, 100.0, 420.0, 16.0),
                geo("translating a document, because the text has already been laid", 100.0, 118.0, 420.0, 16.0),
                geo("Alice and Bob wrote the sentence out by a typesetting engine.", 100.0, 136.0, third_width, 16.0),
            ];
            lines[2].first_word = Some(30.0);
            links(&lines)
        };
        let _ = texts;
        assert_eq!(build(300.0), vec![None, Some(0), Some(1)], "第二行写满、成段：大写开头的下一行连上");
        // 第二行不是真的写满（离列右缘 13 > 0.75h = 12），只是「下一个词放不下」：参差右缘不能靠成段兜底
        let mut lines = vec![
            geo(texts[0], 100.0, 100.0, 420.0, 16.0),
            geo("translating a document, because the text has already changed", 100.0, 118.0, 407.0, 16.0),
            geo("Alice and Bob wrote the sentence out by a typesetting engine.", 100.0, 136.0, 300.0, 16.0),
        ];
        lines[2].first_word = Some(40.0);
        assert_eq!(links(&lines), vec![None, Some(0), None]);
    }

    #[test]
    fn a_new_paragraph_starts_with_an_indent_or_a_larger_gap_and_is_not_merged_into_the_one_above() {
        // 上一行写满、不以句号收尾（段落恰好在这里结束）、下一行缩进着起头：新的一段
        let lines = vec![geo(UPPER, 100.0, 100.0, 420.0, 16.0), geo("Translating a document, because the text has already been", 124.0, 118.0, 396.0, 16.0)];
        assert_eq!(links(&lines)[1], None);
        // 间隙比一行内的行距大得多：段间距
        let lines = vec![geo(UPPER, 100.0, 100.0, 420.0, 16.0), geo(LOWER, 100.0, 136.0, 380.0, 16.0)];
        assert_eq!(links(&lines)[1], None);
    }

    #[test]
    fn something_that_is_not_text_never_links() {
        let ok = geo(UPPER, 100.0, 100.0, 420.0, 16.0);
        for broken in [
            geo(LOWER, f64::NAN, 118.0, 380.0, 16.0),
            geo(LOWER, 100.0, f64::INFINITY, 380.0, 16.0),
            geo(LOWER, 100.0, 118.0, 0.0, 16.0),
            geo(LOWER, 100.0, 118.0, 380.0, 0.0),
            geo(LOWER, 100.0, 118.0, -5.0, 16.0),
            geo("12 34 56 78", 100.0, 118.0, 380.0, 16.0),
            geo("", 100.0, 118.0, 380.0, 16.0),
        ] {
            let lines = [geo(UPPER, 100.0, 100.0, 420.0, 16.0), broken];
            assert_eq!(links(&lines)[1], None);
        }
        assert_eq!(links(&[ok]), vec![None]);
        assert!(links(&[]).is_empty());
    }

    #[test]
    fn geometry_in_double_pixels_decides_the_same_as_in_single_pixels() {
        let lines = |scale: f64| -> Vec<Option<usize>> {
            let texts = ["Translating text that appears on a screen is harder than", "translating a document, because the text has already been", "laid out by a typesetting engine and not by the author, so a"];
            let lines: Vec<EnGeo> = texts.iter().enumerate().map(|(k, text)| geo(text, 100.0 * scale, (100.0 + 18.0 * k as f64) * scale, 420.0 * scale, 16.0 * scale)).collect();
            links(&lines)
        };
        assert_eq!(lines(1.0), lines(2.0));
        assert_eq!(lines(2.0), vec![None, Some(0), Some(1)]);
    }

    // ───────────────────────── 同一视觉行的碎片 ─────────────────────────

    #[test]
    fn a_long_line_that_vision_split_in_two_is_joined_before_the_fullness_test() {
        // 第二行被拆成左右两段：左半截只到 360，右半截 364–520。只看左半截，它「没写满」，段落会在这里断开
        let mut lines = vec![
            geo("Translating text that appears on a screen is harder than", 100.0, 100.0, 420.0, 16.0),
            geo("translating a document, because the text has already", 100.0, 118.0, 260.0, 16.0),
            geo("been laid out by a typesetting", 364.0, 118.0, 156.0, 16.0),
            geo("engine and not by the author, so a sentence is often cut.", 100.0, 136.0, 340.0, 16.0),
        ];
        lines[3].first_word = Some(40.0);
        let result = wrap_links(&lines);
        assert_eq!(result.wraps, vec![None, Some(0), None, Some(1)], "右半截不单独连；第四行连到第二行（的整行）");
        assert_eq!(result.fragment_of, vec![None, None, Some(1), None], "右半截跟着左半截走");
    }

    #[test]
    fn fragments_are_only_glued_when_the_row_itself_is_in_a_wrapped_run() {
        // 导航栏里相邻的两项（间隙很小）：没有任何上下文连着它们，不拼
        let lines = [geo("Home", 100.0, 40.0, 50.0, 16.0), geo("Products", 156.0, 40.0, 70.0, 16.0)];
        let result = wrap_links(&lines);
        assert_eq!(result.fragment_of, vec![None, None]);
        assert_eq!(result.wraps, vec![None, None]);
    }

    #[test]
    fn the_fragment_gap_overlap_height_and_vertical_alignment_have_exact_edges() {
        // 行高 20：间隙上限 0.6h = 12，重叠上限 0.3h = 6，高度比 1.5，竖直重叠至少 0.6 × 矮的那段
        let split = |gap: f64, right_h: f64, right_y: f64| -> Vec<Option<usize>> {
            let lines = vec![geo("Translating text that appears on", 100.0, 100.0, 260.0, 20.0), geo("a screen is harder than", 360.0 + gap, right_y, 100.0, right_h)];
            fragment_heads(&lines)
        };
        assert_eq!(split(11.0, 20.0, 100.0), vec![None, Some(0)]);
        assert_eq!(split(13.0, 20.0, 100.0), vec![None, None], "间隙 13 > 12");
        assert_eq!(split(-5.8, 20.0, 100.0), vec![None, Some(0)], "重叠 5.8 < 6");
        assert_eq!(split(-6.2, 20.0, 100.0), vec![None, None], "重叠 6.2 > 6");
        assert_eq!(split(2.0, 29.0, 100.0), vec![None, Some(0)], "高度比 1.45");
        assert_eq!(split(2.0, 31.0, 100.0), vec![None, None], "高度比 1.55");
        assert_eq!(split(2.0, 20.0, 107.0), vec![None, Some(0)], "竖直重叠 13 ≥ 0.6 × 20 = 12");
        assert_eq!(split(2.0, 20.0, 109.0), vec![None, None], "竖直重叠 11 < 12");
        assert_eq!(split(2.0, 20.0, 125.0), vec![None, None], "竖直方向不重叠的不是同一行");
        // 三段连在一起：都追到最左边那一段
        let three = vec![geo("one two three", 100.0, 100.0, 100.0, 16.0), geo("four five", 205.0, 100.0, 80.0, 16.0), geo("six seven", 290.0, 100.0, 80.0, 16.0)];
        assert_eq!(fragment_heads(&three), vec![None, Some(0), Some(0)]);
        // 含汉字、不是有限数的不拼
        let cjk = vec![geo("one two three", 100.0, 100.0, 100.0, 16.0), geo("四 five", 205.0, 100.0, 80.0, 16.0)];
        assert_eq!(fragment_heads(&cjk), vec![None, None]);
        let nan = vec![geo("one two three", 100.0, 100.0, 100.0, 16.0), geo("four five", f64::NAN, 100.0, 80.0, 16.0)];
        assert_eq!(fragment_heads(&nan), vec![None, None]);
    }

    // ───────────────────────── 评审补的用例（独立日志行、边界、各关卡的权重） ─────────────────────────

    const TXT: &str = "recover whole paragraphs from the geometry of the lines";

    #[test]
    fn independent_lines_that_start_with_a_square_or_curly_bracket_are_not_continuations() {
        // 日志、聊天里一条条独立的行（几何取自真 Vision 对渲染页的识别）：每一行都以标签开头，不是上一行的下文
        for prefix in ["[INFO]", "[WARN]", "[ERROR]", "[main]", "[10:31:46]", "[2026-11-01 10:31:46]", "{INFO}"] {
            let texts = [
                format!("{prefix} Downloading from central the artifact for the plugin"),
                format!("{prefix} Compiling 14 source files to the target directory"),
                format!("{prefix} Building demo-app 1.0-SNAPSHOT"),
                format!("{prefix} Tests run 12 Failures 0 Errors 0 Skipped 0 in the module"),
            ];
            let widths = [462.0, 438.0, 292.0, 494.0];
            let lines: Vec<EnGeo> = texts.iter().zip(widths).enumerate().map(|(k, (text, w))| geo(text, 24.0, 134.0 + 18.0 * k as f64, w, 16.0)).collect();
            assert!(links(&lines).iter().all(|link| link.is_none()), "「{prefix}」");
        }
    }

    #[test]
    fn a_line_cut_off_by_an_ellipsis_is_finished() {
        let with = |first: &str| links(&[geo(first, 100.0, 100.0, 420.0, 16.0), geo(LOWER, 100.0, 118.0, 380.0, 16.0)])[1];
        assert_eq!(with("Translating text that appears on a screen is harder than"), Some(0));
        assert_eq!(with("Translating text that appears on a screen is harder than…"), None, "被截断的摘要后面不会有下文");
        assert_eq!(with("Translating text that appears on a screen is harder than⋯"), None);
    }

    #[test]
    fn a_split_first_or_last_line_of_a_run_is_still_glued() {
        // 一段的第一行被拆成两段（只有下面连着它）
        let first = [geo("Translating text that appears on", 100.0, 100.0, 260.0, 16.0), geo("a screen is harder than", 364.0, 100.0, 156.0, 16.0), geo("translating a document, because the text has already been", 100.0, 118.0, 420.0, 16.0)];
        assert_eq!(wrap_links(&first).fragment_of, vec![None, Some(0), None]);
        // 一段的最后一行被拆成两段（只有上面连着它）
        let last = [geo("Translating text that appears on a screen is harder than", 100.0, 100.0, 420.0, 16.0), geo("translating a document, because", 100.0, 118.0, 250.0, 16.0), geo("the text has already been", 354.0, 118.0, 120.0, 16.0)];
        assert_eq!(wrap_links(&last).fragment_of, vec![None, None, Some(1)]);
    }

    #[test]
    fn luminance_uses_all_three_channels_with_their_own_weights() {
        let with = |a: [f64; 3], b: [f64; 3]| linked(&[geo_fg(UPPER, 100.0, 100.0, 420.0, 16.0, a), geo_fg(LOWER, 100.0, 118.0, 368.0, 16.0, b)]);
        assert!(!with([0.0; 3], [0.0, 60.0, 255.0]), "亮度 61.3 > 55（蓝色通道的权重）");
        assert!(!with([0.0; 3], [255.0, 30.0, 0.0]), "亮度 75.7 > 55（红色通道的权重）");
        assert!(with([0.0; 3], [255.0, 0.0, 0.0]), "纯红 54.2 在内");
        assert!(with([200.0; 3], [200.0; 3]) && with([230.0; 3], [200.0; 3]), "深色底上的浅色字：同一种颜色，亮度差 30");
    }

    #[test]
    fn weak_prose_letter_ratio_is_exactly_one_half() {
        assert!(weak_prose("ab cd 12 34", 4), "恰好一半是字母");
        assert!(weak_prose("ab cd 1 2 3", 4), "57%");
        assert!(!weak_prose("ab cd 1 2 3 4 5", 4), "44%");
    }

    #[test]
    fn a_row_needs_at_least_three_ascii_letters_to_take_part() {
        let with = |lower: &str| links(&[geo(UPPER, 100.0, 100.0, 420.0, 16.0), geo(lower, 100.0, 118.0, 30.0, 16.0)])[1];
        assert_eq!(with("to"), None, "两个字母");
        assert_eq!(with("tox"), Some(0), "三个字母");
    }

    #[test]
    fn the_gap_limit_and_the_fullness_tolerance_use_the_taller_of_the_two_lines() {
        // 上一行高 16、下一行高 29：间隙 12 ≤ 0.55 × 29 = 15.95（用较矮的 16 算则只有 8.8）
        assert!(linked(&[geo(UPPER, 100.0, 100.0, 420.0, 16.0), geo(LOWER, 100.0, 116.0 + 12.0, 380.0, 29.0)]));
        // 上一行高 16（右缘 508），下一行高 29（右缘 520 = R）：差 12 ≤ 0.5 × 29；用较矮的 16 算则 12 > 8，第一个词又只有 1 宽，也不是「放不下」
        let mut lines = vec![geo(UPPER, 100.0, 100.0, 408.0, 16.0), geo(LOWER, 100.0, 118.0, 420.0, 29.0)];
        lines[1].first_word = Some(1.0);
        assert_eq!(links(&lines)[1], Some(0));
    }

    #[test]
    fn a_lowercase_ordinary_word_is_a_start_even_when_it_is_not_a_function_word() {
        let lines = vec![geo("Translating text that appears on a screen is harder", 100.0, 100.0, 420.0, 16.0), geo("translating a document, because the text has been", 100.0, 118.0, 380.0, 16.0)];
        assert!(linked(&lines));
        assert!(starts_with_a_word("translating"));
        assert!(starts_with_a_word("we need"), "we 只在 STOP_WORDS 里");
        assert!(starts_with_a_word("vs the"), "vs 只在 CONNECT_WORDS 里");
    }

    #[test]
    fn a_fragment_that_is_the_first_input_line_still_finds_its_left_half() {
        let lines = vec![geo("a screen is harder than", 364.0, 100.0, 156.0, 16.0), geo("Translating text that appears on", 100.0, 100.0, 260.0, 16.0)];
        assert_eq!(fragment_heads(&lines), vec![Some(1), None]);
    }

    #[test]
    fn words_that_begin_with_roman_numeral_letters_are_not_list_markers() {
        for text in ["Install the package first", "Visit the settings page", "Xylophones are loud", "Vision models are", "ill-formed input"] {
            assert!(!starts_with_marker(text), "「{text}」");
        }
        assert!(!starts_with_marker("[ab c"));
    }

    #[test]
    fn single_digit_citations_do_not_make_a_line_look_like_code() {
        assert!(!codeish("results improve over earlier work [1] and [2] and [3] a lot"));
        assert_eq!(strip_bracketed_markers("a [1] b"), "a   b");
    }

    #[test]
    fn the_column_right_edge_is_the_ninetieth_percentile_so_one_wide_row_of_six_sets_it() {
        // 6 行：5 行右缘 520、1 行右缘 560（宽 460，不算离群）。0.9 分位取到最大值 560，其余各行距 R = 40 > 0.5h，
        // 第一个词只有 10 宽（放得下）：都不连；0.8 分位会取到第二大的 520，各行写满，连起来
        let mut v: Vec<EnGeo> = (0..5).map(|k| EnGeo { first_word: Some(10.0), ..geo(TXT, 100.0, 100.0 + 18.0 * k as f64, 420.0, 16.0) }).collect();
        v.push(EnGeo { first_word: Some(10.0), ..geo(TXT, 100.0, 100.0 + 18.0 * 5.0, 460.0, 16.0) });
        assert_eq!(links(&v)[1..5], [None, None, None, None]);
    }

    #[test]
    fn without_word_boxes_the_font_size_gate_divides_the_line_width_by_its_character_count() {
        let lower: String = LOWER.chars().take(40).collect();
        let lines = vec![geo_plain(UPPER, 100.0, 100.0, 420.0, 16.0), geo_plain(&lower, 100.0, 118.0, 400.0, 16.0)];
        assert_eq!(links(&lines)[1], None, "每个字母 7.4 对 10：差 26%");
    }

    #[test]
    fn a_short_tail_line_is_at_most_three_words() {
        let open = "Translating text that appears on a screen is harder,";
        let with = |tail: &str| links(&[geo(open, 100.0, 100.0, 420.0, 16.0), geo(tail, 100.0, 118.0, 150.0, 16.0)])[1];
        assert_eq!(with("abc 12345 67890"), Some(0), "三个词的尾行（字母不到一半）");
        assert_eq!(with("abc 12345 67890 12345"), None, "四个词、字母不到一半");
    }
}
