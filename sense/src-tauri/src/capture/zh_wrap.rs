//! 中文折行段落的判定：一句话被折成多行时，把这些行合回一块再翻译。
//!
//! 英文那套（上一行以逗号结尾、或下一行以小写字母开头）对中文基本不起作用：汉字没有大小写，行尾也不一定有标点，
//! 一句话在词中间被折断（`…可以翻译你在屏幕` / `上选中的任何内容。`）就会分成两条各译一遍。
//! 这里改看**版面**：上一行写满了一行、话没说完、下一行紧挨着并且对齐、这一对读起来像正文（有逗号句号）。
//!
//! 取舍是宁可漏合、不可错合：每行各译一遍只是译文可能不通顺，把菜单、侧栏、表格、标题合成一块则是翻错加排错。
//! 所以菜单项、标签、按钮、标题、文件名、键值对这类**没有逗号句号**的行，这里一对也合不了（见 `has_prose_punctuation`）；
//! 「几何上像折行、又带逗号」的独立行（等宽的诗句、目录、名言、标题加一句话摘要、一列逗号短句）靠几道关挡：
//! 每一对的字色要一样；起手一对不是对仗、不能只靠「碰巧一样宽」（见 `wrap_links` 的第 2 步）。
//!
//! 尺寸都以行高 h 为单位（取两行里较大的那个），1 倍图与 2 倍图通用。判据的来由与实测见 CHANGELOG 2026-10-31
//! 「截屏翻译：折成多行的中文段落合成一块再翻译」。

use super::SENTENCE_END;

/// 竖直间隙（下一行的顶 − 上一行的底）在 [GAP_MIN, GAP_MAX] × h 之内。上限沿用英文那套的 0.55
const GAP_MIN: f64 = -0.25;
const GAP_MAX: f64 = 0.55;
/// 两行行高相差不超过较大者的 30%
const HEIGHT_DIFF_MAX: f64 = 0.3;
/// 左缘对齐的容差
const LEFT_ALIGN_TOLERANCE: f64 = 0.6;
/// 首行缩进：这一列的第一行可以比下一行靠右，最多 3h（缩进两个字约 1.5h，留一倍余量）
const INDENT_MAX: f64 = 3.0;
/// 「写满一行」：上一行的右缘离这一列最右的右缘不超过这么多。一段的起手一对要求更严（0.5h）：
/// 一列宽度相近、又各带一个逗号的短句（功能列表）靠它挡住；成段之后放宽到 0.75h，容得下禁则让一行少一个字
const FULL_FIRST: f64 = 0.5;
const FULL_NEXT: f64 = 0.75;
/// 上一行至少这么宽（约 9 个字）：窄栏里的短行不当折行
const MIN_WIDTH: f64 = 7.0;
/// 字色护栏：两行字色（sRGB 三个通道，0–255）有任何一个通道差得比这个多，就不是同一段文字
/// （标题 + 灰色的摘要、正文 + 页脚小字）。标题和摘要常常只差一档灰，所以容差取得很小
const COLOR_TOLERANCE: f64 = 24.0;

/// 一行 OCR 文字的版面（逻辑像素，原点左上）与文本（已 trim）。
pub(super) struct LineGeo<'a> {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub text: &'a str,
    /// 这一行的字色（sRGB 三个通道，0–255）；辅助程序没采到就是 None
    pub fg: Option<[f64; 3]>,
}

impl LineGeo<'_> {
    fn right(&self) -> f64 {
        self.x + self.w
    }

    fn bottom(&self) -> f64 {
        self.y + self.h
    }

    /// 版面是有限数、宽高为正，才谈得上「对齐」「紧挨着」。
    fn has_usable_box(&self) -> bool {
        [self.x, self.y, self.w, self.h].iter().all(|value| value.is_finite()) && self.w > 0.0 && self.h > 0.0
    }
}

/// 汉字与拉丁字母各有多少个。翻译方向（`lang::detect` 里汉字圈对拉丁字母的比较）与这里的「中文为主」是同一个口径。
pub(super) fn script_counts(text: &str) -> (usize, usize) {
    let han = text.chars().filter(|ch| ('\u{4e00}'..='\u{9fff}').contains(ch)).count();
    let latin = text.chars().filter(|ch| ch.is_ascii_alphabetic()).count();
    (han, latin)
}

/// 中文为主：至少一个汉字，并且汉字 ×2 ≥ 拉丁字母（`SENSE 是一款运行在 macOS 上的 AI 翻译器` 算，`Settings 设置` 不算）。
fn is_chinese(text: &str) -> bool {
    let (han, latin) = script_counts(text);
    han >= 1 && han * 2 >= latin
}

/// 汉字、假名与全角标点（和前端 `overlayLayout.ts` 的 `isCjk` 同一个范围）。
fn is_cjk(ch: char) -> bool {
    matches!(ch, '\u{2e80}'..='\u{9fff}' | '\u{f900}'..='\u{faff}' | '\u{ff00}'..='\u{ffef}')
}

/// 两行文字接起来：两侧边界字符都是汉字 / 全角标点就紧挨着，否则（拉丁词之间、汉字与拉丁字母之间）隔一个空格。
/// 英文那条老路也走这里：边界上有拉丁字符时与以前一样（隔一个空格），只有汉字之间不再塞半角空格。
/// 另外，英文单词被连字符断开的（`inter-` + `national`、`state-of-the-` + `art`：字母加连字符结尾、下一行小写字母开头）
/// 也紧挨着、连字符留着——`inter-national` 模型读得懂，`state-of-the-art` 本来就该是这个样子；隔一个空格的话
/// 译文里会冒出「inter- national」。
pub(super) fn join_lines(previous: &str, next: &str) -> String {
    let glue = match (previous.chars().last(), next.chars().next()) {
        (Some(before), Some(after)) if is_cjk(before) && is_cjk(after) => "",
        (Some('-'), Some(after)) if after.is_ascii_lowercase() && previous.chars().rev().nth(1).is_some_and(|ch| ch.is_ascii_alphabetic()) => "",
        _ => " ",
    };
    format!("{previous}{glue}{next}")
}

/// 句子末尾可以跟着的收尾引号、括号。
const CLOSERS: &str = "」』）)】]”’\"'》〉";

/// 这一行已经把一句话说完了：以句末标点结尾（现有那套收尾字符），或者句末标点后面只跟着收尾引号 / 括号（`……。”`、`共 3 条…`）。
/// 省略号也算：截断的一行（CSS 的 `text-overflow`）后面不会再有下文；Vision 常把它读成 `⋯`（U+22EF）。
fn finished_sentence(text: &str) -> bool {
    let text = text.trim_end();
    let Some(last) = text.chars().last() else {
        return true;
    };
    if SENTENCE_END.contains(last) {
        return true;
    }
    text.trim_end_matches(|ch| CLOSERS.contains(ch))
        .chars()
        .last()
        .is_some_and(|ch| "。！？.!?…⋯‥；;：:".contains(ch))
}

/// 这一行以句号、问号、叹号或省略号收尾（可以后跟收尾引号 / 括号）：像一段话的最后一行（被 `line-clamp` 截断的摘要也是这样收尾的）。
fn ends_with_full_stop(text: &str) -> bool {
    text.trim_end()
        .trim_end_matches(|ch| CLOSERS.contains(ch))
        .chars()
        .last()
        .is_some_and(|ch| "。！？.!?…⋯‥".contains(ch))
}

/// 以项目符号、序号、时间戳开头：这是列表里的新一条，不是上一行的下文。
/// `6.1%`、`3.5 倍` 这种小数不算序号。
fn starts_with_list_marker(text: &str) -> bool {
    let head: Vec<char> = text.trim_start().chars().take(8).collect();
    let Some(&first) = head.first() else {
        return false;
    };
    if "•·-–—*●○▪◦".contains(first) || ('①'..='⑳').contains(&first) {
        return true;
    }
    let second = head.get(1).copied();
    match first {
        // `[10:31:46]`、`［3］`
        '[' | '［' => return second.is_some_and(|ch| ch.is_ascii_digit()),
        // `（1）`、`(2)`
        '(' | '（' => {
            let digits = head[1..].iter().take_while(|ch| ch.is_ascii_digit()).count();
            return (1..=2).contains(&digits) && head.get(1 + digits).is_some_and(|ch| ")）".contains(*ch));
        }
        _ => {}
    }
    // `1.` `12、` `3)`（点后面不是数字）
    let digits = head.iter().take_while(|ch| ch.is_ascii_digit()).count();
    if (1..=2).contains(&digits)
        && head.get(digits).is_some_and(|ch| ".、)）".contains(*ch))
        && !head.get(digits + 1).is_some_and(|ch| ch.is_ascii_digit())
    {
        return true;
    }
    // `一、` `二.`
    let numerals = head.iter().take_while(|ch| "一二三四五六七八九十".contains(**ch)).count();
    numerals >= 1 && head.get(numerals).is_some_and(|ch| "、.".contains(*ch))
}

/// 正文才有的标点：`，。；！？`，以及半角的 `, ; ! ?`（数字之间的逗号 `1,299` 不算；顿号 `、` 也不算：
/// 目录项、成语、并列的短语里到处是，不说明这是一句话）。
/// 菜单、标签、按钮、标题、文件名、键值对里没有这些，所以这条是「一列独立的短行永远合不起来」的保证。
fn has_prose_punctuation(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    chars.iter().enumerate().any(|(index, &ch)| match ch {
        '，' | '。' | '；' | '！' | '？' | ';' | '!' | '?' => true,
        ',' => {
            let digit_before = index > 0 && chars[index - 1].is_ascii_digit();
            let digit_after = chars.get(index + 1).is_some_and(|next| next.is_ascii_digit());
            !(digit_before && digit_after)
        }
        _ => false,
    })
}

/// 这一行里有句末标点（句号、问号、叹号）：整句话写完过，像成篇的正文。
fn has_sentence_end_inside(text: &str) -> bool {
    text.chars().any(|ch| "。！？".contains(ch))
}

/// 两行的字色差得很远：标题 + 灰色的摘要、正文 + 页脚小字。任何一边没采到字色就当作没差别。
fn colors_differ(upper: &LineGeo, below: &LineGeo) -> bool {
    match (upper.fg, below.fg) {
        (Some(a), Some(b)) => a.iter().zip(b.iter()).any(|(x, y)| (x - y).abs() > COLOR_TOLERANCE),
        _ => false,
    }
}

/// 对仗：两行一样长（相差不超过一个字），第一个句内标点（逗号、分号、句号、问号、叹号）落在同一个字位——
/// 诗句、对联、成语对，以及「团结奋进！开拓创新」这类等宽的标语、问答。两个都是逗号时字位相差一个字还算（五言、七言、
/// 对联常有长短差一个字的）；别的标点要落在完全相同的字位。
/// 正文折行的两行长度也一样，但第一个标点很少落在同一个位置。
fn parallel_clauses(upper: &str, below: &str) -> bool {
    let first_mark = |text: &str| text.chars().enumerate().find(|(_, ch)| "，,；;。！？".contains(*ch));
    match (first_mark(upper), first_mark(below)) {
        (Some((x, a)), Some((y, b))) => {
            let window = if "，,".contains(a) && "，,".contains(b) { 1 } else { 0 };
            x.abs_diff(y) <= window && upper.chars().count().abs_diff(below.chars().count()) <= 1
        }
        _ => false,
    }
}

/// 给每一行找「应当并进哪一行」：返回的第 j 项是 `Some(i)`，表示第 j 行是第 i 行折断的下文（i < j）。
///
/// `lines` 要按 y 从小到大排好。分两步：
/// 1. 版面：每一行找「同列里紧挨在它上面的那一行」——上方、水平对齐的最近一行，不是排序里的上一行
///    （侧栏的行夹在正文的两行之间时，正文的行仍然连得起来）。连起来的一串行叫一列，`R` = 这一列里最右的右缘，
///    即「同列里最宽的那一行」。
/// 2. 判定：上一行没说完、写满了一行（右缘接近 `R`）、这一对像正文、下一行有凭据，才算折断。
///    每一对的字色要一样。起手一对（上一行自己还不是并进来的）再多两道关：不是对仗；只靠「两行都写满」成立、
///    并且这两行里一个句末标点也没有时，后面的行也要连得上（第三行以句号收尾，或者第三行写满、第四行也连得上）
///    ——一列互相独立的带逗号的短句宽度碰巧一样，是巧合，连着几行都一样才不是。
pub(super) fn wrap_links(lines: &[LineGeo]) -> Vec<Option<usize>> {
    let count = lines.len();
    let eligible: Vec<bool> = lines.iter().map(|line| line.has_usable_box() && is_chinese(line.text)).collect();

    // 1. 同列里紧挨着的上一行。每一行最多有一个下文：同一行不会被两行都连上
    let mut above: Vec<Option<usize>> = vec![None; count];
    let mut taken = vec![false; count];
    for (j, below) in lines.iter().enumerate() {
        if !eligible[j] || starts_with_list_marker(below.text) {
            continue;
        }
        let mut closest: Option<usize> = None;
        for i in (0..j).rev() {
            let upper = &lines[i];
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
            // 首行缩进：只有这一列的第一行（上面还没有连着的行）才能比下一行靠右
            let indented = above[i].is_none() && shift < 0.0 && shift >= -INDENT_MAX * h;
            if shift.abs() > LEFT_ALIGN_TOLERANCE * h && !indented {
                continue;
            }
            if closest.is_none_or(|best| upper.bottom() > lines[best].bottom()) {
                closest = Some(i);
            }
        }
        if let Some(i) = closest {
            above[j] = Some(i);
            taken[i] = true;
        }
    }

    // 每一行的下文（几何上连在它下面的那一行）
    let mut follower: Vec<Option<usize>> = vec![None; count];
    for (k, upper) in above.iter().enumerate() {
        if let Some(i) = *upper {
            follower[i] = Some(k);
        }
    }

    // 一列的最右右缘：沿着连接找到列头，各行把自己的右缘记到列头上
    let mut head: Vec<usize> = (0..count).collect();
    let mut column_right = vec![f64::NEG_INFINITY; count];
    for j in 0..count {
        if let Some(i) = above[j] {
            head[j] = head[i];
        }
        column_right[head[j]] = column_right[head[j]].max(lines[j].right());
    }

    // 2. 逐对判定
    let mut wraps: Vec<Option<usize>> = vec![None; count];
    for j in 0..count {
        let Some(i) = above[j] else {
            continue;
        };
        let (upper, below) = (&lines[i], &lines[j]);
        let h = upper.h.max(below.h);
        // 上一行已经把话说完了（句号收尾）：各译各的不吃亏
        if finished_sentence(upper.text) {
            continue;
        }
        // 上一行并进来过：这一段已经成立，往后的行放宽一档
        let established = wraps[i].is_some();
        let tolerance = h * if established { FULL_NEXT } else { FULL_FIRST };
        let right = column_right[head[j]];
        if upper.right() < right - tolerance || upper.w < MIN_WIDTH * h {
            continue;
        }
        // 这一对要像正文。菜单、标题、文件名里没有这些标点，这一关永远过不去
        if !(established || has_prose_punctuation(upper.text) || has_prose_punctuation(below.text)) {
            continue;
        }
        // 字色不一样是两个元素（标题 + 灰色的摘要、正文 + 灰色的来源标签）。每一对都要查，成段之后也一样：
        // 标题折成两行时，灰色的摘要不能被前面已经成立的一对带进来。代价是整行是链接色的行会把一段切开（切开的各段要各自重新过起手一对的几道关，链接密集的段落合并率约减半）
        if colors_differ(upper, below) {
            continue;
        }
        // 下一行要有凭据：也写满了一行（还会继续），或者以句号收尾（段落的最后一行），或者这一段已经成立
        let both_full = below.right() >= right - tolerance;
        let closes = ends_with_full_stop(below.text);
        if !(both_full || established || closes) {
            continue;
        }
        // 起手一对只靠「两行都写满」成立（没有句号收尾）：宽度碰巧一样的独立行也是这样。再多两道关
        if !established && !closes {
            // 对仗（诗句、对联、成语对）不是折行
            if parallel_clauses(upper.text, below.text) {
                continue;
            }
            // 这两行里一个句末标点也没有：第三行也要连得上，一次巧合不算。第三行以句号收尾就够了；
            // 只是写满的话，第四行也要连得上（写满了，或者以句号收尾）——连着三行等宽的标题不算折行
            if !has_sentence_end_inside(upper.text) && !has_sentence_end_inside(below.text) {
                let is_full = |index: usize| lines[index].right() >= right - FULL_NEXT * h;
                let confirmed = follower[j].is_some_and(|third| {
                    ends_with_full_stop(lines[third].text) || (is_full(third) && follower[third].is_some_and(|fourth| is_full(fourth) || ends_with_full_stop(lines[fourth].text)))
                });
                if !confirmed {
                    continue;
                }
            }
        }
        wraps[j] = Some(i);
    }
    wraps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geo(text: &str, x: f64, y: f64, w: f64, h: f64) -> LineGeo<'_> {
        LineGeo { x, y, w, h, text, fg: None }
    }

    /// 带字色的一行。
    fn geo_fg(text: &str, x: f64, y: f64, w: f64, h: f64, fg: [f64; 3]) -> LineGeo<'_> {
        LineGeo { fg: Some(fg), ..geo(text, x, y, w, h) }
    }

    #[test]
    fn script_counts_and_chinese_dominance_follow_the_translation_direction_rule() {
        assert_eq!(script_counts("SENSE 是一款运行在 macOS 上的 AI 翻译器"), (11, 12));
        assert!(is_chinese("SENSE 是一款运行在 macOS 上的 AI 翻译器"), "汉字 ×2 ≥ 字母");
        assert!(is_chinese("要使用 Claude"), "3 个汉字 ×2 = 6 ≥ 6 个字母：边界算中文");
        assert!(!is_chinese("要使用 Claude Code"), "汉字 ×2 < 字母");
        assert!(!is_chinese("Settings 设置"), "英文为主的中英混排走英文那套");
        assert!(!is_chinese("The quick brown fox"));
        assert!(!is_chinese("2026-10-31 10:21"), "没有汉字");
        assert!(is_chinese("口"), "整行只剩一个汉字（折行落下的尾巴）也算");
        // 范围两头的字都算汉字；数字不是拉丁字母（`2026 年 10 月` 的 6 个数字不能把汉字比下去）
        assert_eq!(script_counts("\u{4e00}\u{9fff}"), (2, 0));
        assert_eq!(script_counts("2026 年 10 月"), (2, 0));
        assert!(is_chinese("2026 年 10 月 31 日 上午 10 点"));
    }

    #[test]
    fn lines_join_without_a_space_only_between_han_or_fullwidth_characters() {
        assert_eq!(join_lines("可以翻译你在屏幕", "上选中的任何内容。"), "可以翻译你在屏幕上选中的任何内容。");
        assert_eq!(join_lines("以逗号结尾，", "下一行"), "以逗号结尾，下一行", "全角逗号后面接汉字：不插空格（以前会塞一个半角空格）");
        assert_eq!(join_lines("使用（见下文）", "继续"), "使用（见下文）继续", "全角括号也算");
        assert_eq!(join_lines("打开 macOS", "设置"), "打开 macOS 设置", "拉丁字母与汉字之间仍隔一个空格");
        assert_eq!(join_lines("设置", "macOS 上"), "设置 macOS 上");
        assert_eq!(join_lines("The quick brown,", "fox jumps"), "The quick brown, fox jumps", "英文不变");
        assert_eq!(join_lines("ends with,", "继续"), "ends with, 继续", "半角逗号不是全角标点：与以前一样隔一个空格");
        assert_eq!(join_lines("", "x"), " x", "边界缺一侧时退回以前的行为（调用方不会传空串）");
        // 汉字、假名、CJK 标点区（`。` `、` `「` `」`）、全角区都算「紧挨着」
        assert_eq!(join_lines("你好。", "世界"), "你好。世界");
        assert_eq!(join_lines("甲、", "乙"), "甲、乙");
        assert_eq!(join_lines("他说「好」", "然后"), "他说「好」然后");
        assert_eq!(join_lines("こんにちは", "世界"), "こんにちは世界");
        assert_eq!(join_lines("第一行（", "ＡＢ"), "第一行（ＡＢ", "全角字母也在全角区里");
        assert_eq!(join_lines("他说“好”", "然后"), "他说“好” 然后", "弯引号不在汉字 / 全角区：与以前一样隔一个空格");
    }

    #[test]
    fn a_hyphen_that_breaks_a_latin_word_joins_without_a_space_but_nothing_else_changes() {
        // 字母 + 连字符结尾、下一行小写字母开头：英文单词被连字符断开——紧挨着、连字符留着
        assert_eq!(join_lines("an inter-", "national event"), "an inter-national event");
        assert_eq!(join_lines("state-of-the-", "art models"), "state-of-the-art models");
        assert_eq!(join_lines("representa-", "tion"), "representa-tion");
        // 其余一律与以前一样：下一行大写 / 数字 / 汉字开头、连字符前面不是字母、破折号、孤零零的连字符
        assert_eq!(join_lines("an inter-", "National event"), "an inter- National event");
        assert_eq!(join_lines("an inter-", "2018 event"), "an inter- 2018 event");
        assert_eq!(join_lines("an inter-", "中文"), "an inter- 中文");
        assert_eq!(join_lines("see 5-", "10 pages"), "see 5- 10 pages");
        assert_eq!(join_lines("see 5-", "year plan"), "see 5- year plan", "连字符前面是数字，不是被断开的单词");
        assert_eq!(join_lines("a -", "dash"), "a - dash", "连字符前面是空格");
        assert_eq!(join_lines("-", "dash"), "- dash", "整行只有一个连字符");
        assert_eq!(join_lines("an em dash—", "continues"), "an em dash— continues", "破折号不是连字符");
        assert_eq!(join_lines("an inter-", "éclat"), "an inter- éclat", "只认 ASCII 小写字母");
    }

    #[test]
    fn a_sentence_is_finished_by_its_final_punctuation_with_or_without_closing_quotes() {
        for done in ["内容。", "真的吗？", "太好了！", "结束.", "完了!", "Why?", "他说：", "清单；", "如下:", "共 3 条…", "（见下文）", "「好」", "他说“好。”", "结束。）", "他说：“你好！”", "他说『好。』", "他说“好；”", "他说“注意：”", "他说“好…”"] {
            assert!(finished_sentence(done), "「{done}」说完了");
        }
        for open in ["可以翻译你在屏幕", "你好，", "你好,", "使用 macOS 的", "他说“好”", "他说“好”，然后", "继续——"] {
            assert!(!finished_sentence(open), "「{open}」还没说完");
        }
        assert!(finished_sentence(""), "空串当说完了（没有可接的）");
        assert!(finished_sentence("   "), "空白也是");
    }

    #[test]
    fn only_a_closing_full_stop_marks_the_last_line_of_a_paragraph() {
        for last in ["上选中的任何内容。", "是这样吗？", "太好了！", "End.", "他说“好。”", "完了…"] {
            assert!(ends_with_full_stop(last), "「{last}」像段落的最后一行");
        }
        for not_last in ["还有后面", "分号；", "冒号：", "逗号，", "结束）"] {
            assert!(!ends_with_full_stop(not_last), "「{not_last}」不像");
        }
    }

    #[test]
    fn list_markers_are_recognised_but_decimals_and_ordinary_numbers_are_not() {
        for marker in ["• 查词卡片", "·离线识别", "- 支持", "– 支持", "— 支持", "* 注意", "● 重点", "○ 重点", "▪ 项目", "◦ 项目", "1. 打开设置", "12、第十二条", "3) 其次", "1）条目", "（1）第一项", "(2) 第二项", "（12）第十二项", "(3）混合括号", "① 第一", "⑩ 第十", "⑳ 第二十", "一、概述", "三、概述", "九.", "十二.", "[10:31:46] 已安装", "［3］引用", "   • 前面有空格", "四、概述", "五.", "六、概述", "七.", "八、概述"] {
            assert!(starts_with_list_marker(marker), "「{marker}」是列表的新一条");
        }
        for plain in ["6.1%，城乡收入差距继续缩小", "3.5 倍", "2026 年第三季度", "2026.年度计划", "2026、2027 年的数据", "12.5 公里", "3 到 5 个工作日", "1000 多家企业", "你好", "口", "十分钟", "三个人", "", "   ", "（见下文）继续", "[注]", "（123）三位数不算序号", "1234.", "(a) 字母", "12.5%", "123. 之后", "365、366 天", "100) 以内", "100）以内"] {
            assert!(!starts_with_list_marker(plain), "「{plain}」不是列表符号");
        }
    }

    #[test]
    fn prose_punctuation_is_what_menus_labels_and_file_names_never_have() {
        for prose in ["你好，世界", "完了。", "是吗？", "太好了！", "第一；第二", "a, b", "wow!", "why?", "x; y", "1,299,和", "1,299, 又"] {
            assert!(has_prose_punctuation(prose), "「{prose}」像正文");
        }
        for label in ["新建任务", "设置与偏好", "使用说明与帮助中心", "屏幕截图 2026-10-31 上午 10.21.03.png", "版本：1.2.0（构建 2026103101）", "偏好设置…", "1,299.00 元", "Wi-Fi", "A.B.C", "", "3.2 训练策略、超参数设置", "甲、乙、丙", "山清水秀、鸟语花香"] {
            assert!(!has_prose_punctuation(label), "「{label}」是标签 / 文件名 / 并列短语，不算正文标点（顿号不算）");
        }
    }

    #[test]
    fn every_closing_quote_and_bracket_may_follow_the_final_punctuation() {
        // 收尾字符的集合在这里逐个钉死：光遍历 `CLOSERS` 自己，往里删一个字符这条用例也不会红
        let closers = ['」', '』', '）', ')', '】', ']', '”', '’', '"', '\'', '》', '〉'];
        assert_eq!(CLOSERS.chars().count(), closers.len(), "收尾字符的个数");
        for closer in closers {
            for stop in ['。', '！', '？', '.', '!', '?'] {
                let line = format!("他说一定可以做到{stop}{closer}");
                assert!(finished_sentence(&line), "「{line}」说完了");
                assert!(ends_with_full_stop(&line), "「{line}」是段落的最后一行");
            }
            for soft in ['；', ';', '：', ':'] {
                let line = format!("他说一定可以做到{soft}{closer}");
                assert!(finished_sentence(&line), "「{line}」说完了（分号、冒号也算）");
                assert!(!ends_with_full_stop(&line), "「{line}」不是段落的最后一行");
            }
            // 光有收尾引号 / 括号、前面没有标点：右括号、右直角引号本来就在现有的句末字符表里；弯引号、直引号、书名号不在（引号里可能还没说完）
            let bare = format!("他说一定可以做到{closer}");
            assert_eq!(finished_sentence(&bare), SENTENCE_END.contains(closer), "「{bare}」");
            assert!(!ends_with_full_stop(&bare), "「{bare}」");
        }
        for stop in ['。', '！', '？', '.', '!', '?', '…', '⋯', '‥'] {
            assert!(ends_with_full_stop(&format!("上选中的任何内容{stop}")), "以「{stop}」收尾");
        }
    }

    #[test]
    fn a_sentence_end_inside_a_line_means_the_fullwidth_marks_only() {
        // 整句话写完过：全角的句号、问号、叹号。半角的 `.` `!` `?` 在版本号、文件名、网址里到处是（`Python 3.12`），不算
        for inside in ["第一句。第二句", "真的吗？是的", "太好了！继续"] {
            assert!(has_sentence_end_inside(inside), "「{inside}」");
        }
        for not_inside in ["Python 3.12 发布", "文件 a.txt 已保存", "wow! 太好了", "why? 因为", "分号；冒号：逗号，顿号、", ""] {
            assert!(!has_sentence_end_inside(not_inside), "「{not_inside}」");
        }
    }

    #[test]
    fn a_comma_counts_unless_it_sits_between_two_digits() {
        for prose in [",abc", "你好,", "9,好", ",5", "a,5", "5,a", "1,299,和", "，", "第一，第二"] {
            assert!(has_prose_punctuation(prose), "「{prose}」里的逗号算");
        }
        for number in ["5,5", "1,299", "1,299,000", "10,000.5"] {
            assert!(!has_prose_punctuation(number), "「{number}」是千分位，不算");
        }
    }

    #[test]
    fn the_cjk_ranges_include_their_edges_and_nothing_just_outside() {
        for inside in ['\u{2e80}', '\u{3001}', '\u{3042}', '\u{4e00}', '\u{9fff}', '\u{f900}', '\u{faff}', '\u{ff00}', '\u{ff0c}', '\u{ffef}'] {
            assert!(is_cjk(inside), "{inside:?} 是汉字 / 假名 / 全角");
        }
        for outside in ['\u{2e7f}', '\u{a000}', '\u{f8ff}', '\u{fb00}', '\u{feff}', '\u{fff0}', 'a', ' ', '“', '…', '—'] {
            assert!(!is_cjk(outside), "{outside:?} 不是");
        }
    }

    #[test]
    fn a_wrapped_paragraph_links_every_line_to_the_one_above() {
        // 真 Vision 在 1 倍图上量到的三行（CHANGELOG 里的例子）：前两行写满、第三行是段尾
        let lines = [
            geo("为了保护你的隐私，所有的文字识别都在本机完成，图", 22.0, 120.0, 358.0, 20.0),
            geo("片不会离开你的电脑。只有识别出来的文字会发送给你", 24.0, 144.0, 356.0, 22.0),
            geo("选择的翻译服务，而密钥只保存在系统钥匙串里。", 22.0, 170.0, 324.0, 20.0),
        ];
        assert_eq!(wrap_links(&lines), vec![None, Some(0), Some(1)]);
    }

    #[test]
    fn a_two_line_paragraph_needs_a_closing_full_stop_on_the_second_line() {
        let lines = [geo("SENSE 是一款运行在 macOS上的 AI 翻译器，可以翻译", 22.0, 56.0, 370.0, 20.0), geo("你在屏幕上选中的任何内容。", 22.0, 82.0, 190.0, 20.0)];
        assert_eq!(wrap_links(&lines), vec![None, Some(0)], "题目里的例子：写满、有逗号、第二行以句号收尾");
        // 第二行短、也没有句末标点：两行的段落没有第二个凭据，保持现状（各译各的）
        let no_stop = [geo("SENSE 是一款运行在 macOS上的 AI 翻译器，可以翻译", 22.0, 56.0, 370.0, 20.0), geo("你在屏幕上选中的任何内容", 22.0, 82.0, 190.0, 20.0)];
        assert_eq!(wrap_links(&no_stop), vec![None, None]);
        // 两行都写满、却没有句号：只有「宽度碰巧一样」这一次巧合，不合（要第三行也连得上，见下一个用例）
        let both_full = [geo("所有的文字识别都在本机完成，图片不会离开你的电脑，", 22.0, 56.0, 370.0, 20.0), geo("只有识别出来的文字会发送给你选择的翻译服务，而", 22.0, 82.0, 366.0, 20.0)];
        assert_eq!(wrap_links(&both_full), vec![None, None]);
        // 两行里有一个句号（说明这是成篇的正文），两行都写满：有凭据
        let prose = [geo("所有的文字识别都在本机完成。图片不会离开你的电脑，", 22.0, 56.0, 370.0, 20.0), geo("只有识别出来的文字会发送给你选择的翻译服务，而", 22.0, 82.0, 366.0, 20.0)];
        assert_eq!(wrap_links(&prose), vec![None, Some(0)]);
    }

    #[test]
    fn a_pair_that_is_only_equally_wide_needs_the_lines_after_it_to_continue_too() {
        // 起手一对只靠「两行都写满」成立、两行里又一个句末标点也没有：宽度碰巧一样的独立行也是这样，要后面的行也连得上
        let first = "所有的文字识别都在本机完成，图片不会离开你的电脑";
        let second = "只有识别出来的文字会发送给你选择的翻译服务而";
        let third = "密钥只保存在系统钥匙串里不会出现在日志中而";
        // 第三行也写满了、但没有句号，也没有第四行：三次一样宽还不够——连着三条等宽的新闻标题就是这样
        let three_full = [geo(first, 22.0, 100.0, 300.0, 20.0), geo(second, 22.0, 126.0, 298.0, 20.0), geo(third, 22.0, 152.0, 296.0, 20.0)];
        assert_eq!(wrap_links(&three_full), vec![None, None, None]);
        // 第四行也写满了：四次一样宽，不是巧合
        let four_full = [geo(first, 22.0, 100.0, 300.0, 20.0), geo(second, 22.0, 126.0, 298.0, 20.0), geo(third, 22.0, 152.0, 296.0, 20.0), geo("所有的设置都可以在设置页里随时改变而且很快", 22.0, 178.0, 294.0, 20.0)];
        assert_eq!(wrap_links(&four_full), vec![None, Some(0), Some(1), Some(2)]);
        // 第四行以句号收尾（段落的最后一行）：连得上
        let four_closing = [geo(first, 22.0, 100.0, 300.0, 20.0), geo(second, 22.0, 126.0, 298.0, 20.0), geo(third, 22.0, 152.0, 296.0, 20.0), geo("所有的设置都可以随时改。", 22.0, 178.0, 150.0, 20.0)];
        assert_eq!(wrap_links(&four_closing), vec![None, Some(0), Some(1), Some(2)]);
        // 第四行很短、又没有句号：第三行的写满只是又一次巧合——不合
        let short_fourth = [geo(first, 22.0, 100.0, 300.0, 20.0), geo(second, 22.0, 126.0, 298.0, 20.0), geo(third, 22.0, 152.0, 296.0, 20.0), geo("所有的设置", 22.0, 178.0, 80.0, 20.0)];
        assert_eq!(wrap_links(&short_fourth), vec![None, None, None, None]);
        // 第三行以句号收尾（段落的最后一行）：连得上
        let closing = [geo(first, 22.0, 100.0, 300.0, 20.0), geo(second, 22.0, 126.0, 298.0, 20.0), geo("密钥只保存在系统钥匙串里。", 22.0, 152.0, 130.0, 20.0)];
        assert_eq!(wrap_links(&closing), vec![None, Some(0), Some(1)]);
        // 第三行很短、又没有句号：可能是段尾，也可能是一列长短不一的句子——不合（保守）
        let short_tail = [geo(first, 22.0, 100.0, 300.0, 20.0), geo(second, 22.0, 126.0, 298.0, 20.0), geo("密钥只保存在系统钥匙串里", 22.0, 152.0, 130.0, 20.0)];
        assert_eq!(wrap_links(&short_tail), vec![None, None, None]);
        // 只有两行：没有第三行可以印证，不合
        assert_eq!(wrap_links(&short_tail[..2]), vec![None, None]);
        // 第三行不是「连得上」的那种（隔了一大段空白，成了另一段）：不算印证
        let far_third = [geo(first, 22.0, 100.0, 300.0, 20.0), geo(second, 22.0, 126.0, 298.0, 20.0), geo("密钥只保存在系统钥匙串里。", 22.0, 200.0, 130.0, 20.0)];
        assert_eq!(wrap_links(&far_third), vec![None, None, None]);
        // 两行里有一个句号（成篇的正文）：不需要第三行
        let prose = [geo("所有的文字识别都在本机完成。图片不会离开你的电脑，", 22.0, 100.0, 300.0, 20.0), geo(second, 22.0, 126.0, 298.0, 20.0)];
        assert_eq!(wrap_links(&prose), vec![None, Some(0)]);
        // B 自己以句号收尾：本来就有凭据，不需要第三行
        let closes = [geo(first, 22.0, 100.0, 300.0, 20.0), geo("只有识别出来的文字会发送给你选择的翻译服务。", 22.0, 126.0, 120.0, 20.0)];
        assert_eq!(wrap_links(&closes), vec![None, Some(0)]);
    }

    #[test]
    fn once_a_paragraph_is_established_a_short_unpunctuated_last_line_still_joins() {
        let lines = [
            geo("按下快捷键，拖出一个框，译文会直接盖在原文的", 52.0, 208.0, 328.0, 22.0),
            geo("位置上，字号和颜色都和原文一致，不用再来回切换窗", 22.0, 234.0, 358.0, 20.0),
            geo("口和复制粘贴。", 24.0, 260.0, 98.0, 19.0),
        ];
        // 首行缩进（52 → 22）算对齐；前两行写满、第三行以句号收尾
        assert_eq!(wrap_links(&lines), vec![None, Some(0), Some(1)]);
        // 末行只剩一个字、没有句号：起手一对只靠「写满」，第三行印证不了——不合
        let orphan = [
            geo("按下快捷键，拖出一个框，译文会直接盖在原文的", 52.0, 208.0, 328.0, 22.0),
            geo("位置上，字号和颜色都和原文一致，不用再来回切换窗", 22.0, 234.0, 358.0, 20.0),
            geo("口", 24.0, 260.0, 14.0, 16.0),
        ];
        assert_eq!(wrap_links(&orphan), vec![None, None, None]);
        // 四行：起手一对里有句号（成篇的正文），不需要后面的行印证；成段之后第四行是只剩一个字的末行，不要求标点也并进来
        let four = [
            geo("按下快捷键。拖出一个框，译文会直接盖在原文的", 52.0, 208.0, 328.0, 22.0),
            geo("位置上，字号和颜色都和原文一致，不用再来回切换窗", 22.0, 234.0, 358.0, 20.0),
            geo("口和复制粘贴，所有的设置都可以在设置页里改变化", 22.0, 260.0, 356.0, 20.0),
            geo("口", 24.0, 286.0, 14.0, 16.0),
        ];
        assert_eq!(wrap_links(&four), vec![None, Some(0), Some(1), Some(2)]);
    }

    #[test]
    fn menus_sidebars_and_labels_never_link_whatever_their_geometry() {
        // 没有任何逗号句号：即使宽度完全一样、紧挨着、左对齐，也一对都连不起来（这是整条规则的保证）
        let labels = ["使用说明与帮助中心入口", "管理已授权的应用和设备", "查看所有已保存的搜索记录", "屏幕录制与辅助功能权限", "隐私与安全性相关设置"];
        let lines: Vec<LineGeo> = labels.iter().enumerate().map(|(index, label)| geo(label, 14.0, 6.0 + 22.0 * index as f64, 160.0, 18.0)).collect();
        assert_eq!(wrap_links(&lines), vec![None; labels.len()]);
        // 同样的几何，换成「有逗号、下一行以句号收尾」的正文，就能连起来——证明上面不是因为几何不合格
        let prose = [geo("使用说明与帮助中心入口，", 14.0, 6.0, 160.0, 18.0), geo("管理已授权的应用和设备。", 14.0, 28.0, 160.0, 18.0)];
        assert_eq!(wrap_links(&prose), vec![None, Some(0)]);
        // 只有顿号（目录项、成语、并列短语）不算正文标点
        let enumerations = [geo("使用说明、帮助中心入口", 14.0, 6.0, 160.0, 18.0), geo("管理已授权的应用、设备", 14.0, 28.0, 160.0, 18.0)];
        assert_eq!(wrap_links(&enumerations), vec![None, None]);
    }

    #[test]
    fn a_column_of_short_clauses_with_commas_is_not_a_paragraph() {
        // 真 Vision 量到的功能列表：每条中间带逗号，宽度 182 / 168 / 142 / 170——差 14px = 0.7h，超过起手一对的 0.5h
        let lines = [
            geo("支持中英互译，识别准确率高", 170.0, 202.0, 182.0, 22.0),
            geo("离线识别，不上传任何图片", 170.0, 224.0, 168.0, 20.0),
            geo("译文原位替换到截图上", 170.0, 244.0, 142.0, 20.0),
            geo("查词卡片按常用度排序义项", 170.0, 266.0, 170.0, 18.0),
        ];
        assert_eq!(wrap_links(&lines), vec![None; 4]);
        // 两条宽度一样：只有一次巧合，不合（以前会合）
        let same_width = [geo("支持中英互译，识别准确率高", 170.0, 202.0, 182.0, 20.0), geo("离线识别，不上传任何图片啊", 170.0, 224.0, 182.0, 20.0)];
        assert_eq!(wrap_links(&same_width), vec![None, None]);
        // 三条宽度都一样、逗号的位置也不对仗：只有三次一样宽，没有句号，不合
        let three_same = [
            geo("支持中英互译，识别准确率高", 170.0, 202.0, 182.0, 20.0),
            geo("离线识别，不上传任何图片啊", 170.0, 224.0, 182.0, 20.0),
            geo("译文原位替换到截图，上去啊", 170.0, 246.0, 182.0, 20.0),
        ];
        assert_eq!(wrap_links(&three_same), vec![None; 3]);
        // 连着四条都一样宽、逗号的位置也各不相同：被当成折行——已知的、写进文档的残余风险（要四次巧合）
        let four_same = [
            geo("支持中英互译，识别准确率高", 170.0, 202.0, 182.0, 20.0),
            geo("离线识别，不上传任何图片啊", 170.0, 224.0, 182.0, 20.0),
            geo("译文原位替换到截图，上去啊", 170.0, 246.0, 182.0, 20.0),
            geo("离线工作不依赖网络，更安全啊", 170.0, 268.0, 182.0, 20.0),
        ];
        assert_eq!(wrap_links(&four_same), vec![None, Some(0), Some(1), Some(2)]);
    }

    #[test]
    fn parallel_clauses_are_poetry_or_couplets_not_a_wrapped_sentence() {
        // 对仗：一样长（± 1 字）、第一个逗号落在同一个字位（± 1）
        assert!(parallel_clauses("春眠不觉晓，处处闻啼鸟", "夜来风雨声，花落知多少"));
        assert!(parallel_clauses("山清水秀，鸟语花香", "天高云淡，秋高气爽"));
        assert!(parallel_clauses("红豆生南国,春来发几枝", "愿君多采撷，此物最相思"), "半角逗号也算");
        assert!(parallel_clauses("春眠不觉晓，处处闻啼鸟", "夜来风雨声，花落知多少啊"), "长度差 1 还算");
        assert!(parallel_clauses("春眠不觉晓，处处闻啼鸟", "夜来风雨声息，花落知多少"), "逗号位置差 1 还算");
        assert!(!parallel_clauses("春眠不觉晓，处处闻啼鸟", "夜来风雨声息息，花落知多少"), "逗号位置差 2：不是");
        assert!(!parallel_clauses("春眠不觉晓，处处闻啼鸟", "夜来风雨声，花落知多少啊啊"), "长度差 2：不是");
        assert!(!parallel_clauses("支持中英互译，识别准确率高", "离线识别不上传任何图片"), "有一行没有逗号：不是");
        assert!(!parallel_clauses("无逗号的一行", "也没有逗号的行"));
        // 逗号以外的标点（分号、句号、问号、叹号）也是分隔：等宽的标语、问答、分号列表，第一个标点落在同一个字位
        assert!(parallel_clauses("团结奋进！开拓创新", "诚信为本！服务至上"), "标语：叹号对叹号");
        assert!(parallel_clauses("什么是截屏翻译？就是", "怎样才能装好呢？先下"), "问答：问号对问号");
        assert!(parallel_clauses("先打开设置；再选择语言", "再点保存键；然后退出啊"), "分号对分号，字位相同");
        assert!(!parallel_clauses("先打开设置；再选择语言", "接着点保存键；然后退出"), "分号 5 对分号 6：不放宽");
        assert!(parallel_clauses("先打开设置;再选择语言", "再点保存键;然后退出啊"), "半角分号也算");
        assert!(parallel_clauses("春天来了。花都开了呀", "夏天到了。蝉鸣阵阵啊"), "句号对句号");
        assert!(parallel_clauses("团结奋进！开拓创新", "诚信为本，服务至上"), "叹号对逗号，字位一样：也算（位置相同）");
        // 逗号对逗号位置可以差一个字；逗号以外的标点要完全相同——正文折行里逗号对句号、差一个字的情形很常见，不能当对仗
        assert!(parallel_clauses("春眠不觉晓，处处闻啼鸟", "夜来风雨声息，花落知多少"), "逗号对逗号，差 1：算");
        assert!(parallel_clauses("春眠不觉晓,处处闻啼鸟", "夜来风雨声息,花落知多少"), "半角逗号对半角逗号，差 1：算");
        assert!(parallel_clauses("春眠不觉晓，处处闻啼鸟", "夜来风雨声息,花落知多少"), "全角逗号对半角逗号，差 1：算");
        assert!(!parallel_clauses("为了保护你的隐私，所有的文字识别都在本机完成，图", "片不会离开你的电脑。只有识别出来的文字会发送给你"), "真折行的两行：逗号 8 对句号 9");
        assert!(!parallel_clauses("团结奋进！开拓创新", "诚信为本服！务至上啊"), "叹号 4 对叹号 5：不放宽");
        assert!(!parallel_clauses("团结奋进！开拓创新", "诚信为本服务至上啊啊"), "只有一行有标点：不是");
        // 只看第一个逗号：第一个逗号对齐、后面的逗号各在各的位置，照样是对仗；第一个逗号不齐、只有最后一个对齐，不是
        assert!(parallel_clauses("春眠不觉晓，处处闻，啼鸟", "夜来风雨声，花落知多少啊"));
        assert!(!parallel_clauses("甲，丙丁戊己庚，辛", "甲乙丙丁戊己庚，辛"));
        // 正文折行的两行：长度一样，逗号的位置各不相同
        assert!(!parallel_clauses("所有的文字识别都在本机完成，图片不会离开你的电脑", "只有识别出来的文字会发送给你选择的翻译服务，而"));
    }

    #[test]
    fn an_evenly_wide_couplet_or_poem_is_not_merged_but_a_wrapped_paragraph_with_the_same_commas_is() {
        // 五言 / 七言诗，一行一联：等宽、各带一个逗号、行末没有标点。几何上和折行一模一样，靠对仗挡住
        let poem = ["春眠不觉晓，处处闻啼鸟", "夜来风雨声，花落知多少", "红豆生南国，春来发几枝", "愿君多采撷，此物最相思"];
        let lines: Vec<LineGeo> = poem.iter().enumerate().map(|(index, verse)| geo(verse, 30.0, 20.0 + 24.0 * index as f64, 180.0, 20.0)).collect();
        assert_eq!(wrap_links(&lines), vec![None; 4]);
        // 第二行以句号收尾（真的是一句话折成了两行）：对仗不适用，照连
        let wrapped = [geo("春眠不觉晓，处处闻啼鸟", 30.0, 20.0, 180.0, 20.0), geo("夜来风雨声，花落知多少。", 30.0, 44.0, 180.0, 20.0)];
        assert_eq!(wrap_links(&wrapped), vec![None, Some(0)]);
    }

    #[test]
    fn equally_wide_slogans_questions_and_semicolon_lists_are_not_paragraphs() {
        // 一列等宽的标语（叹号）、问答（问号）、分号列表：行里有句末标点或分号，几何上和折行一模一样，
        // 但每一行的第一个标点都落在同一个字位——对仗，不是一句话折成了几行
        let lists: [&[&str]; 3] = [
            &["团结奋进！开拓创新", "诚信为本！服务至上", "客户第一！永不止步"],
            &["什么是截屏翻译？就是", "怎样才能装好呢？先下", "离线能不能使用？可以"],
            &["先打开设置；再选择语言", "再点保存键；然后退出啊", "最后重启应用；完成设置"],
        ];
        for list in lists {
            let lines: Vec<LineGeo> = list.iter().enumerate().map(|(index, text)| geo(text, 30.0, 24.0 + 24.0 * index as f64, 150.0, 18.0)).collect();
            assert_eq!(wrap_links(&lines), vec![None; lines.len()], "{list:?}");
        }
        // 两行的版本同样不连（以前只认逗号，这里行里没有逗号，对仗关不起作用，叹号又让第三行印证也免了）
        let pair = [geo("团结奋进！开拓创新", 30.0, 24.0, 150.0, 18.0), geo("诚信为本！服务至上", 30.0, 48.0, 150.0, 18.0)];
        assert_eq!(wrap_links(&pair), vec![None, None]);
    }

    #[test]
    fn a_title_and_its_gray_summary_are_two_elements_because_their_text_colors_differ() {
        // 真页面（36kr）：标题（深色）+ 一句话摘要（灰色），摘要以句号收尾。字色差得远，不连
        let title = [38.0, 38.0, 38.0];
        let gray = [127.0, 127.0, 127.0];
        let lines = [geo_fg("电科金仓发布 KES AI 数据库，面向多模态场景", 430.0, 462.0, 226.0, 22.0, title), geo_fg("让大模型看懂私有数据。", 430.0, 496.0 - 12.0, 140.0, 20.0, gray)];
        assert_eq!(wrap_links(&lines), vec![None, None]);
        // 字色一样（同一段文字）：照连
        let same = [geo_fg("电科金仓发布 KES AI 数据库，面向多模态场景", 430.0, 462.0, 226.0, 22.0, title), geo_fg("让大模型看懂私有数据。", 430.0, 484.0, 140.0, 20.0, [41.0, 40.0, 40.0])];
        assert_eq!(wrap_links(&same), vec![None, Some(0)]);
        // 任何一边没采到字色：当作没差别（不能因为缺数据就不合并）
        let missing = [geo_fg("电科金仓发布 KES AI 数据库，面向多模态场景", 430.0, 462.0, 226.0, 22.0, title), geo("让大模型看懂私有数据。", 430.0, 484.0, 140.0, 20.0)];
        assert_eq!(wrap_links(&missing), vec![None, Some(0)]);
        // 差 24 整：还算同色；差 25：不是（任何一个通道，正负方向都算）。标题与摘要常常只差一档灰（深灰 38 对中灰 61），所以容差取得很小
        let edge = |delta: f64| [geo_fg("所有的文字识别都在本机完成，图片不会离开你的电脑", 22.0, 100.0, 300.0, 20.0, [30.0, 30.0, 30.0]), geo_fg("只有识别出来的文字会发送给你选择的翻译服务。", 22.0, 126.0, 120.0, 20.0, [30.0, 30.0, 30.0 + delta])];
        assert_eq!(wrap_links(&edge(24.0)), vec![None, Some(0)]);
        assert_eq!(wrap_links(&edge(25.0)), vec![None, None]);
        assert_eq!(wrap_links(&edge(-25.0)), vec![None, None], "负方向也算");
        let near = [geo_fg("国家统计局发布三季度居民收入数据，城乡差距继续缩小", 30.0, 24.0, 330.0, 22.0, [29.0, 29.0, 31.0]), geo_fg("前三季度全国居民人均可支配收入同比增长百分之五点二。", 30.0, 50.0, 300.0, 20.0, [61.0, 61.0, 64.0])];
        assert_eq!(wrap_links(&near), vec![None, None], "标题 29 对摘要 61（差 32，一档灰）：不连");
    }

    #[test]
    fn the_color_guard_applies_to_every_pair_so_a_gray_summary_is_not_pulled_into_a_headline_or_a_paragraph() {
        let ink = [30.0, 30.0, 30.0];
        let gray = [140.0, 140.0, 145.0];
        let blue = [6.0, 69.0, 173.0];
        // 标题折成两行（第二行也写满），后面跟灰色的一句话摘要：标题的两行连起来，摘要自己一块
        // （摘要以句号收尾，印证了标题两行；但摘要的字色不一样，不能被带进来）
        let headline = [
            geo_fg("国家统计局发布三季度居民收入数据，城乡居民收入差距继续", 30.0, 24.0, 380.0, 22.0, ink),
            geo_fg("缩小，农村居民收入增速快于城镇居民收入增速且保持稳", 30.0, 48.0, 378.0, 22.0, ink),
            geo_fg("前三季度全国居民人均可支配收入同比增长百分之五点二。", 30.0, 76.0, 300.0, 20.0, gray),
        ];
        assert_eq!(wrap_links(&headline), vec![None, Some(0), None]);
        // 摘要和标题同色：几何上就是一段话，照连（字色一样的标题 + 摘要是写进文档的残余风险）
        let same = [
            geo_fg("国家统计局发布三季度居民收入数据，城乡居民收入差距继续", 30.0, 24.0, 380.0, 22.0, ink),
            geo_fg("缩小，农村居民收入增速快于城镇居民收入增速且保持稳", 30.0, 48.0, 378.0, 22.0, ink),
            geo_fg("前三季度全国居民人均可支配收入同比增长百分之五点二。", 30.0, 76.0, 300.0, 20.0, ink),
        ];
        assert_eq!(wrap_links(&same), vec![None, Some(0), Some(1)]);
        // 成段之后跟着灰色的来源标签：不并进来
        let source = [
            geo_fg("所有的文字识别都在本机完成。图片不会离开你的电脑", 22.0, 100.0, 300.0, 20.0, ink),
            geo_fg("只有识别出来的文字会发送给你选择的翻译服务而已啊", 22.0, 126.0, 298.0, 20.0, ink),
            geo_fg("来源：新华社", 22.0, 152.0, 90.0, 18.0, gray),
        ];
        assert_eq!(wrap_links(&source), vec![None, Some(0), None]);
        // 代价：整行是链接蓝色的行（维基百科）把一段切开——切开后的各段要各自重新过起手一对的几道关，这里切开的是单独的一行，不会再合并
        let wiki = [
            geo_fg("所有的文字识别都在本机完成，图片不会离开你的电脑", 22.0, 100.0, 300.0, 20.0, ink),
            geo_fg("只有识别出来的文字会发送给你选择的翻译服务，而", 22.0, 126.0, 298.0, 20.0, ink),
            geo_fg("密钥只保存在系统钥匙串里，不会出现在日志中，而", 22.0, 152.0, 296.0, 20.0, blue),
            geo_fg("这样即使日志被分享出去，也不会泄露任何密钥。", 22.0, 178.0, 220.0, 20.0, ink),
        ];
        assert_eq!(wrap_links(&wiki), vec![None, Some(0), None, None]);
    }

    #[test]
    fn a_truncated_line_ends_its_text_and_a_clamped_summary_may_end_with_an_ellipsis() {
        // CSS 截断的一行：Vision 常读成 ⋯（U+22EF）；它是这一块文字的最后一行，后面不会有下文
        for done in ["流，20 元刷差评商家需 50 元⋯", "目录项⋯", "截断‥", "共 3 条…"] {
            assert!(finished_sentence(done), "「{done}」是被截断的一行，说完了");
        }
        // 反过来：被 `line-clamp` 截成两行的摘要，最后一行以省略号收尾，是段落的最后一行
        for last in ["全免费使用。该软件支持文件降序、⋯", "如存肌肉”的时代来了…", "后面还有‥"] {
            assert!(ends_with_full_stop(last), "「{last}」像被截断的摘要的最后一行");
        }
        // 真页面（虎嗅）：一条折成两行、第二行被截断的标题 + 红色的来源标签。前两行本来就是一个标题；
        // 第二行以 ⋯ 结尾说完了，下面的来源标签是另一个元素，不并（以前会把它也拖进来）
        let lines = [
            geo("职业差评灰产调查：团队暗语交", 88.0, 558.0, 252.0, 20.0),
            geo("流，20 元刷差评商家需 50 元⋯", 88.0, 584.0, 244.0, 24.0),
            geo("四 界面新闻。", 88.0, 612.0, 92.0, 19.0),
        ];
        assert_eq!(wrap_links(&lines), vec![None, Some(0), None]);
    }

    /// 一对能连起来的基准：每个用例只破坏一个条件，证明各条件各自管用。
    fn base_pair() -> [LineGeo<'static>; 2] {
        [geo("所有的文字识别都在本机完成，图片不会离开你的电脑", 22.0, 100.0, 300.0, 20.0), geo("只有识别出来的文字会发送给你选择的翻译服务。", 22.0, 126.0, 120.0, 20.0)]
    }

    #[test]
    fn each_condition_alone_can_stop_a_pair_from_linking() {
        assert_eq!(wrap_links(&base_pair()), vec![None, Some(0)], "基准本身能连");
        let case = |name: &str, mutate: &dyn Fn(&mut [LineGeo<'static>; 2])| {
            let mut pair = base_pair();
            mutate(&mut pair);
            assert_eq!(wrap_links(&pair), vec![None, None], "{name}");
        };
        case("间隙太大（> 0.55h）", &|pair| pair[1].y = 100.0 + 20.0 + 0.6 * 20.0);
        case("两行重叠太多（< −0.25h）", &|pair| pair[1].y = 100.0 + 20.0 - 0.3 * 20.0);
        case("行高相差超过 30%", &|pair| pair[1].h = 13.0);
        case("左缘不齐", &|pair| pair[1].x = 22.0 + 0.7 * 20.0);
        case("下一行比上一行靠左太多（超过首行缩进）", &|pair| {
            pair[0].x = 22.0 + 3.1 * 20.0;
            pair[0].w = 300.0 - 3.1 * 20.0;
        });
        case("上一行不够宽（< 7h）", &|pair| {
            pair[0].text = "所有的文字识别，图片不会";
            pair[0].w = 139.0;
        });
        case("上一行没写满（右缘比下一行靠左）", &|pair| {
            pair[1].w = 300.0;
            pair[0].w = 289.0;
        });
        case("上一行以句号结尾", &|pair| pair[0].text = "所有的文字识别都在本机完成，图片不会离开你的电脑。");
        case("下一行以项目符号开头", &|pair| pair[1].text = "• 只有识别出来的文字会发送给你选择的翻译服务。");
        case("上一行不是中文为主", &|pair| pair[0].text = "The quick brown fox jumps over the lazy dog, 然后");
        case("下一行不是中文为主", &|pair| pair[1].text = "Only the recognised text is sent to the translation service.");
        case("整对没有逗号句号", &|pair| {
            pair[0].text = "所有的文字识别都在本机完成图片不会离开你的电脑";
            pair[1].text = "只有识别出来的文字会发送给你选择的翻译服务";
        });
        case("下一行短、也没有句末标点", &|pair| pair[1].text = "只有识别出来的文字会发送给你选择的翻译服务");
    }

    /// 把一组行整体放大（2 倍图）。
    fn at_scale<'a>(lines: &[LineGeo<'a>], factor: f64) -> Vec<LineGeo<'a>> {
        lines.iter().map(|l| LineGeo { x: l.x * factor, y: l.y * factor, w: l.w * factor, h: l.h * factor, text: l.text, fg: l.fg }).collect()
    }

    #[test]
    fn thresholds_are_in_line_heights_so_a_double_resolution_image_gives_the_same_verdicts() {
        let check = |name: &str, mutate: &dyn Fn(&mut [LineGeo<'static>; 2]), linked: bool| {
            let mut pair = base_pair();
            mutate(&mut pair);
            for factor in [1.0, 2.0] {
                let want = if linked { vec![None, Some(0)] } else { vec![None, None] };
                assert_eq!(wrap_links(&at_scale(&pair, factor)), want, "{name} ×{factor}");
            }
        };
        check("左缘差 0.5h", &|pair| pair[1].x = 22.0 + 10.0, true);
        check("左缘差 0.65h", &|pair| pair[1].x = 22.0 + 13.0, false);
        check("间隙 0.5h", &|pair| pair[1].y = 130.0, true);
        check("间隙 0.6h", &|pair| pair[1].y = 132.0, false);
        check("重叠 0.1h", &|pair| pair[1].y = 118.0, true);
        check("重叠 0.3h", &|pair| pair[1].y = 114.0, false);
        check("行高差 25%", &|pair| pair[1].h = 15.0, true);
        check("行高差 35%", &|pair| pair[1].h = 13.0, false);
        check("上一行刚好 7h", &|pair| pair[0].w = 140.0, true);
        check("上一行 6.9h", &|pair| pair[0].w = 138.0, false);
    }

    #[test]
    fn the_gap_and_width_limits_use_the_larger_of_the_two_line_heights() {
        let text = "所有的文字识别都在本机完成，图片不会离开你的电脑";
        let tail = "只有识别出来的文字会发送给你选择的翻译服务。";
        // 上一行 26 高、下一行 20 高，间隙 12：按较大者（26）算上限 14.3，连；若按下一行（20）算上限 11，就不连了
        assert_eq!(wrap_links(&[geo(text, 22.0, 100.0, 300.0, 26.0), geo(tail, 22.0, 138.0, 120.0, 20.0)]), vec![None, Some(0)]);
        // 反过来，上一行 20 高、下一行 26 高：按较大者算，同样连（若按上一行算上限 11，就不连了）
        assert_eq!(wrap_links(&[geo(text, 22.0, 100.0, 300.0, 20.0), geo(tail, 22.0, 132.0, 120.0, 26.0)]), vec![None, Some(0)]);
        // 最小宽度：上一行 150 宽，两行都是 20 高 → 7h = 140，够宽；下一行 24 高 → 7h = 168，不够
        assert_eq!(wrap_links(&[geo("所有的文字识别都在本机完成，图片", 22.0, 100.0, 150.0, 20.0), geo("只有识别出来的文字。", 22.0, 124.0, 60.0, 20.0)]), vec![None, Some(0)]);
        assert_eq!(wrap_links(&[geo("所有的文字识别都在本机完成，图片", 22.0, 100.0, 150.0, 20.0), geo("只有识别出来的文字。", 22.0, 124.0, 60.0, 24.0)]), vec![None, None]);
    }

    #[test]
    fn the_right_edge_tolerance_also_uses_the_larger_of_the_two_line_heights() {
        let text = "所有的文字识别都在本机完成，图片不会离开你的电脑";
        let tail = "只有识别出来的文字会发送给你选择的翻译服务。";
        // 上一行 20 高、下一行 16 高：容差按较大者（20）是 10。整列的最右缘是 331，上一行的右缘 322 离它 9，在容差内；
        // 若按下一行（16）算只有 8，就不连了
        assert_eq!(wrap_links(&[geo(text, 22.0, 100.0, 300.0, 20.0), geo(tail, 22.0, 126.0, 309.0, 16.0)]), vec![None, Some(0)]);
        // 反过来，上一行 16 高、下一行 20 高：同样按较大者
        assert_eq!(wrap_links(&[geo(text, 22.0, 100.0, 300.0, 16.0), geo(tail, 22.0, 122.0, 309.0, 20.0)]), vec![None, Some(0)]);
        // 差 11 就超出了：322 离 333 有 11，大于 10
        assert_eq!(wrap_links(&[geo(text, 22.0, 100.0, 300.0, 20.0), geo(tail, 22.0, 126.0, 311.0, 16.0)]), vec![None, None]);
    }

    #[test]
    fn the_second_line_is_measured_against_the_widest_line_of_the_column_not_against_the_first() {
        // 第三行把整列的最右缘撑到 322：第一行（右缘 314）还在容差内，第二行（309）已落在容差线（312）之外。
        // 第二行相对第一行只差 5，但相对「同列里最宽的那一行」没写满——不能当凭据
        let lines = [
            geo("所有的文字识别都在本机完成，图片不会离开你的电脑", 22.0, 100.0, 292.0, 20.0),
            geo("只有识别出来的文字会发送给你选择的翻译服务而", 22.0, 126.0, 287.0, 20.0),
            geo("这一行把整列撑宽了，所以前面两行都不算写满。", 22.0, 152.0, 300.0, 20.0),
        ];
        assert_eq!(wrap_links(&lines)[1], None);
    }

    #[test]
    fn every_threshold_accepts_a_pair_just_inside_it() {
        // 只测「不连」的话，把阈值往里收一点没人发现：每条阈值内侧一点点也要连得上
        let check = |name: &str, mutate: &dyn Fn(&mut [LineGeo<'static>; 2])| {
            let mut pair = base_pair();
            mutate(&mut pair);
            assert_eq!(wrap_links(&pair), vec![None, Some(0)], "{name}");
        };
        check("间隙 0.5h（< 0.55h）", &|pair| pair[1].y = 100.0 + 20.0 + 0.5 * 20.0);
        check("两行重叠 0.1h（> −0.25h）：行高紧的段落常见", &|pair| pair[1].y = 100.0 + 20.0 - 0.1 * 20.0);
        check("下一行矮 25%（< 30%）", &|pair| pair[1].h = 15.0);
        check("左缘差 0.5h（< 0.6h）", &|pair| pair[1].x = 22.0 + 0.5 * 20.0);
        check("首行缩进 2.9h（< 3h）", &|pair| {
            pair[0].x = 22.0 + 2.9 * 20.0;
            pair[0].w = 300.0 - 2.9 * 20.0;
        });
        check("上一行刚好 7h 宽", &|pair| pair[0].w = 140.0);
        check("两行重叠刚好 0.25h（边界算连）", &|pair| pair[1].y = 100.0 + 20.0 - 5.0);
        check("首行缩进刚好 3h（边界算连）", &|pair| {
            pair[0].x = 22.0 + 60.0;
            pair[0].w = 300.0 - 60.0;
        });
        check("上一行的右缘刚好落在容差线上（离最右缘 0.5h）", &|pair| {
            pair[0].w = 290.0;
            pair[1].w = 300.0;
        });
        check("下一行的右缘刚好落在容差线上（且没有句号；第一行里有句号，不需要第三行）", &|pair| {
            pair[0].text = "所有的文字识别都在本机完成。图片不会离开你的电脑";
            pair[1].text = "只有识别出来的文字会发送给你选择的翻译服务";
            pair[1].w = 290.0;
        });
        check("逗号句号只在下一行", &|pair| pair[0].text = "所有的文字识别都在本机完成图片不会离开你的电脑");
        // 下面两条正好落在阈值上：用 20 的行高，0.3h = 6、0.6h = 12 都是二进制里能精确表示的数，`>` 与 `>=` 在这里才分得出来
        check("下一行矮刚好 30%（边界算连）", &|pair| pair[1].h = 14.0);
        check("左缘差刚好 0.6h（边界算连）", &|pair| pair[1].x = 22.0 + 12.0);
    }

    #[test]
    fn a_sentence_end_inside_either_line_stands_in_for_the_third_line() {
        // 两行一样宽、都不以句号收尾、也没有第三行：要是其中一行里整句话写完过（。！？），就不需要第三行来印证
        for stop in ['。', '！', '？'] {
            let comma_line = "所有的文字识别都在本机完成，图片不会离开你的电脑啊啊";
            let stop_in_below = format!("只有识别出来的文字会发送{stop}给你选择的翻译服务而已啊");
            assert_eq!(wrap_links(&[geo(comma_line, 22.0, 100.0, 300.0, 20.0), geo(&stop_in_below, 22.0, 126.0, 300.0, 20.0)]), vec![None, Some(0)], "「{stop}」在下一行里");
            let stop_in_upper = format!("所有的文字识别都在本机完成{stop}图片不会离开你的电脑啊啊");
            assert_eq!(wrap_links(&[geo(&stop_in_upper, 22.0, 100.0, 300.0, 20.0), geo("只有识别出来的文字会发送给你选择的翻译服务而已啊", 22.0, 126.0, 300.0, 20.0)]), vec![None, Some(0)], "「{stop}」在上一行里");
        }
        // 对照：两行里都没有句末标点，又没有第三行，就不连
        assert_eq!(wrap_links(&[geo("所有的文字识别都在本机完成，图片不会离开你的电脑啊啊", 22.0, 100.0, 300.0, 20.0), geo("只有识别出来的文字会发送给你选择的翻译服务而已啊", 22.0, 126.0, 300.0, 20.0)]), vec![None, None]);
    }

    #[test]
    fn the_third_line_confirms_a_pair_with_the_wider_tolerance_used_inside_a_paragraph() {
        // 前三行里一个句末标点也没有：第三行的右缘离整列最右缘 12（0.6h），比起手一对的容差（0.5h）远，在后续的容差（0.75h）之内，算写满了；
        // 第三行只是写满、没有句号，所以第四行（以句号收尾）也要连得上
        let with_third = |width: f64| {
            [
                geo("所有的文字识别都在本机完成，图片不会离开你的电脑", 22.0, 100.0, 300.0, 20.0),
                geo("只有识别出来的文字会发送给你选择的翻译服务而已啊", 22.0, 126.0, 300.0, 20.0),
                geo("密钥只保存在系统钥匙串里不会出现在日志当中而已", 22.0, 152.0, width, 20.0),
                geo("所有的设置都可以随时改。", 22.0, 178.0, 150.0, 20.0),
            ]
        };
        assert_eq!(wrap_links(&with_third(288.0)), vec![None, Some(0), Some(1), Some(2)]);
        // 离最右缘刚好 15（0.75h，20 的行高下是二进制里能精确表示的数）：边界算写满
        assert_eq!(wrap_links(&with_third(285.0)), vec![None, Some(0), Some(1), Some(2)]);
        // 再往里缩 4（离最右缘 16，超过 0.75h = 15）就不算了
        assert_eq!(wrap_links(&with_third(284.0)), vec![None; 4]);
    }

    #[test]
    fn once_a_paragraph_is_established_the_following_lines_need_neither_punctuation_nor_a_closing_stop() {
        // 第二、三、四行里一个标点都没有：起手一对（第一行里有句号）成立之后，往后不再要求标点和句号收尾
        let lines = [
            geo("所有的文字识别都在本机完成。图片不会离开你的电脑", 22.0, 100.0, 300.0, 20.0),
            geo("只有识别出来的文字会发送给你选择的翻译服务而", 22.0, 126.0, 292.0, 20.0),
            geo("密钥只保存在系统钥匙串里不会出现在日志当中而", 22.0, 152.0, 290.0, 20.0),
            geo("密钥只保存在系统钥匙串里", 22.0, 178.0, 130.0, 20.0),
        ];
        assert_eq!(wrap_links(&lines), vec![None, Some(0), Some(1), Some(2)]);
    }

    #[test]
    fn the_indent_limit_uses_the_taller_of_the_two_line_heights() {
        // 上一行 20 高、下一行 26 高：首行缩进上限按较大者（26）算是 78；缩进 70 在内。若按上一行（20）算只有 60，就不连了
        let text = "所有的文字识别都在本机完成，图片不会离开你的电脑";
        let tail = "只有识别出来的文字会发送给你选择的翻译服务。";
        assert_eq!(wrap_links(&[geo(text, 22.0 + 70.0, 100.0, 300.0 - 70.0, 20.0), geo(tail, 22.0, 126.0, 300.0, 26.0)]), vec![None, Some(0)]);
        // 缩进 80 超出（> 78）
        assert_eq!(wrap_links(&[geo(text, 22.0 + 80.0, 100.0, 300.0 - 80.0, 20.0), geo(tail, 22.0, 126.0, 300.0, 26.0)]), vec![None, None]);
    }

    #[test]
    fn only_the_first_line_of_a_column_may_sit_to_the_right_of_the_next_line() {
        // 首行缩进只发生在一段的第一行：第二行比第三行靠右 2h 不是缩进，第三行不并
        let lines = [
            geo("所有的文字识别都在本机完成。图片不会离开你的电脑，", 62.0, 100.0, 260.0, 20.0),
            geo("只有识别出来的文字会发送给你选择的翻译服务，而", 62.0, 126.0, 260.0, 20.0),
            geo("密钥只保存在系统钥匙串里。", 22.0, 152.0, 130.0, 20.0),
        ];
        assert_eq!(wrap_links(&lines), vec![None, Some(0), None]);
    }

    #[test]
    fn the_right_edge_tolerance_is_tighter_for_the_first_pair_than_afterwards() {
        let first = "所有的文字识别都在本机完成。图片不会离开你的电脑";
        let second = "只有识别出来的文字会发送给你选择的翻译服务，而";
        let third = "密钥只保存在系统钥匙串里，不会出现在日志中，而";
        let last = "这样即使日志被分享出去，也不会泄露任何密钥。";
        // 起手一对：第二行比第一行短 0.6h（12px），超过 0.5h，连不起来
        let tight_start = [geo(first, 22.0, 100.0, 300.0, 20.0), geo(second, 22.0, 126.0, 288.0, 20.0)];
        assert_eq!(wrap_links(&tight_start), vec![None, None], "起手一对差 0.6h：不合");
        // 差 0.4h（8px）：合
        let ok_start = [geo(first, 22.0, 100.0, 300.0, 20.0), geo(second, 22.0, 126.0, 292.0, 20.0)];
        assert_eq!(wrap_links(&ok_start), vec![None, Some(0)]);
        // 成段之后（第四行起）放宽到 0.75h：第三行比最宽的短 0.7h（14px），第四行仍并进来——禁则让一行少一个字很常见
        let relaxed = [
            geo(first, 22.0, 100.0, 300.0, 20.0),
            geo(second, 22.0, 126.0, 292.0, 20.0),
            geo(third, 22.0, 152.0, 286.0, 20.0),
            geo(last, 22.0, 178.0, 220.0, 20.0),
        ];
        assert_eq!(wrap_links(&relaxed), vec![None, Some(0), Some(1), Some(2)]);
        // 短 0.8h（16px）：第三行并进来（成段后下一行不要求写满），但它够不上「写满」，第四行不再并
        let mut too_short = relaxed;
        too_short[2].w = 284.0;
        assert_eq!(wrap_links(&too_short), vec![None, Some(0), Some(1), None]);
    }

    #[test]
    fn the_column_is_measured_against_its_widest_line_not_just_the_pair() {
        // 第三行比前两行都宽：前两行相对「同列里最宽的那一行」都没写满，不是折行
        let lines = [
            geo("所有的文字识别都在本机完成，图片不会离开你的电脑", 22.0, 100.0, 300.0, 20.0),
            geo("只有识别出来的文字会发送给你选择的翻译服务，而", 22.0, 126.0, 300.0, 20.0),
            geo("这一行比上面两行都宽，所以它们不是写满了一行的折行", 22.0, 152.0, 340.0, 20.0),
        ];
        assert_eq!(wrap_links(&lines), vec![None, None, None]);
    }

    #[test]
    fn a_sidebar_line_between_two_paragraph_lines_does_not_break_the_paragraph() {
        // 按 y 排序后两栏交错：正文的第二行不挨着正文的第一行，中间夹着侧栏的一项
        let lines = [
            geo("所有的文字识别都在本机完成，图片不会离开你的电脑", 200.0, 100.0, 300.0, 20.0),
            geo("设置与偏好", 14.0, 104.0, 76.0, 20.0),
            geo("只有识别出来的文字会发送给你选择的翻译服务，而", 200.0, 126.0, 296.0, 20.0),
            geo("使用说明与帮助中心", 14.0, 134.0, 136.0, 20.0),
            geo("密钥只保存在系统钥匙串里。", 200.0, 152.0, 150.0, 20.0),
        ];
        assert_eq!(wrap_links(&lines), vec![None, None, Some(0), None, Some(2)]);
    }

    #[test]
    fn the_nearest_upper_line_wins_and_a_line_never_gets_two_followers() {
        let text = "所有的文字识别都在本机完成，图片不会离开你的电脑";
        let tail = "只有识别出来的文字会发送给你选择的翻译服务。";
        // 重复识别出来的两条几乎重叠的行（y 差 4px）都能当「上一行」：取底边离下一行最近的（第二条）
        let doubled = [geo(text, 22.0, 100.0, 300.0, 20.0), geo(text, 22.0, 104.0, 300.0, 20.0), geo(tail, 22.0, 124.0, 150.0, 20.0)];
        assert_eq!(wrap_links(&doubled), vec![None, None, Some(1)]);
        // 两条下文都够得着同一个上一行：先到先得，另一条不会再连一次
        let twins = [geo(text, 22.0, 100.0, 300.0, 20.0), geo(tail, 22.0, 124.0, 150.0, 20.0), geo(tail, 22.0, 126.0, 150.0, 20.0)];
        assert_eq!(wrap_links(&twins), vec![None, Some(0), None]);
        // 底边一样低的两个候选：取排序里更靠后的（离下一行更近的那条）
        let tie = [geo(text, 22.0, 100.0, 300.0, 24.0), geo(text, 22.0, 104.0, 300.0, 20.0), geo(tail, 22.0, 126.0, 150.0, 22.0)];
        assert_eq!(wrap_links(&tie), vec![None, None, Some(1)]);
        // 候选的先后和底边不一致（排在前面的行更高，底边反而更低）：取底边离下一行最近的，不是排序里紧挨着的
        let tall_first = [geo(text, 22.0, 100.0, 300.0, 24.0), geo("另一行文字", 22.0, 102.0, 200.0, 20.0), geo(tail, 22.0, 126.0, 150.0, 22.0)];
        assert_eq!(wrap_links(&tall_first), vec![None, None, Some(0)]);
    }

    #[test]
    fn non_finite_or_degenerate_boxes_never_link() {
        let nan = [geo("所有的文字识别都在本机完成，图片不会离开你的电脑", f64::NAN, 100.0, 300.0, 20.0), geo("只有识别出来的文字会发送给你选择的翻译服务。", 22.0, 126.0, 120.0, 20.0)];
        assert_eq!(wrap_links(&nan), vec![None, None]);
        let zero_width = [geo("所有的文字识别都在本机完成，图片不会离开你的电脑", 22.0, 100.0, 0.0, 20.0), geo("只有识别出来的文字会发送给你选择的翻译服务。", 22.0, 126.0, 120.0, 20.0)];
        assert_eq!(wrap_links(&zero_width), vec![None, None]);
        let zero_width_below = [geo("所有的文字识别都在本机完成，图片不会离开你的电脑", 22.0, 100.0, 300.0, 20.0), geo("只有识别出来的文字会发送给你选择的翻译服务。", 22.0, 126.0, 0.0, 20.0)];
        assert_eq!(wrap_links(&zero_width_below), vec![None, None]);
        let infinite = [geo("所有的文字识别都在本机完成，图片不会离开你的电脑", 22.0, 100.0, 300.0, f64::INFINITY), geo("只有识别出来的文字会发送给你选择的翻译服务。", 22.0, 126.0, 120.0, 20.0)];
        assert_eq!(wrap_links(&infinite), vec![None, None]);
        let nan_y = [geo("所有的文字识别都在本机完成，图片不会离开你的电脑", 22.0, f64::NAN, 300.0, 20.0), geo("只有识别出来的文字会发送给你选择的翻译服务。", 22.0, 126.0, 120.0, 20.0)];
        assert_eq!(wrap_links(&nan_y), vec![None, None]);
        for height in [0.0, -20.0] {
            let flat = [geo("所有的文字识别都在本机完成，图片不会离开你的电脑", 22.0, 100.0, 300.0, height), geo("只有识别出来的文字会发送给你选择的翻译服务。", 22.0, 126.0, 120.0, 20.0)];
            assert_eq!(wrap_links(&flat), vec![None, None], "上一行高 {height}");
            let flat_below = [geo("所有的文字识别都在本机完成，图片不会离开你的电脑", 22.0, 100.0, 300.0, 20.0), geo("只有识别出来的文字会发送给你选择的翻译服务。", 22.0, 126.0, 120.0, height)];
            assert_eq!(wrap_links(&flat_below), vec![None, None], "下一行高 {height}");
        }
        assert_eq!(wrap_links(&[]), Vec::<Option<usize>>::new());
    }

    #[test]
    fn the_verdict_does_not_depend_on_the_image_scale() {
        // 2 倍图：所有长度翻倍，行高也翻倍——判据只看相对 h 的比例
        let double = |lines: &[LineGeo<'static>; 2]| -> Vec<Option<usize>> {
            let scaled: Vec<LineGeo> = lines.iter().map(|line| geo(line.text, line.x * 2.0, line.y * 2.0, line.w * 2.0, line.h * 2.0)).collect();
            wrap_links(&scaled)
        };
        assert_eq!(double(&base_pair()), wrap_links(&base_pair()));
    }
}
