/**
 * 输入框里的内容，是「查一个词」还是「翻译一段话」。
 *
 * 词典 prompt 的 JSON 契约放得下一个词、一个词组、一个成语，放不下一句话。
 * 以前只看长度（> 64 个 UTF-16 单位）：30 个字符的英文句子、50 个字的中文句子
 * 都会被当成「词」，送进词典 prompt 出一张怪卡片。
 *
 * 规则（任意一条成立就走长文翻译，按顺序判）：
 * 1. 超过 64 个字符。按码点数，与 Rust `lookup` 的 `chars().count() > 64` 同一把尺——
 *    emoji 之类占两个 UTF-16 单位，旧写法会在 32 个 emoji 处提前分流，两端不一致。
 * 2. 不止一行：词典输入是一个词，多行粘贴一定是文本。
 * 3. 含日文假名或韩文：词典只支持中英，这类文字交给翻译。
 * 4. 含汉字：超过 12 个字；或者至少 9 个字且句中有逗号、顿号、分号、句末标点；
 *    或者只在末尾有一个句末标点，但至少 8 个字。8 个字以内的成语、谚语、短问
 *    （「画蛇添足。」「塞翁失马，焉知非福」「怎么回事？」）仍查词典；代价是 8 个字以内的完整短句
 *    （「我不知道。」）也会进词典。引号、书名号包着的不影响判断。
 * 5. 拉丁文字（中英混排时也按这一条数英文词）：超过 8 个单词；或者至少 3 个单词且以句末标点结尾；
 *    或者至少 3 个单词且句末标点后面还有内容（多句）。缩写点（Dr. / Sen. / Inc. / etc. / U.S. / Ph.D.）
 *    不算句末，只对句号生效——问号、感叹号后面一定是句子边界；小数（3.14）、Node.js 这类也不是缩写。
 *
 * 一个词、短语、成语（take it for granted、说曹操曹操到）都留在词典。
 * 短语后面加了句末标点（「take it for granted.」）会被当成句子——去掉标点就是查词。
 */
export const WORD_MAX_CHARS = 64;

const HAN = /[\u3400-\u9fff]/g;
/** 假名、谚文（中点 ・ U+30FB 常出现在中文译名里，不算假名） */
const KANA_HANGUL = /[\u3040-\u30fa\u30fc-\u30ff\uac00-\ud7af]/;
/** 中文里的子句标点；ASCII 的逗号分号后面紧跟数字（1,000）不算 */
const CJK_CLAUSE = /[，、；。！？]|[,;!?](?!\d)/;
const CJK_TERMINATOR_END = /[。！？!?]$/;
/** 引号、书名号、括号：判断标点时先剥掉，「画蛇添足。」和 画蛇添足。 一样对待 */
const CJK_WRAPPERS = /^[「『“‘（(\[\s"']+|[」』”’）)\]\s"']+$/g;
/** 句末标点，后面可以跟引号、括号（含弯引号） */
const CLOSERS = `["'”’)\\]」』）]*`;
const LATIN_TERMINATOR = /[.!?]/;
const SENTENCE_BREAK = new RegExp(`(\\S*?)([.!?])${CLOSERS}\\s+(?=\\S)`, "g");
const ABBREVIATIONS = new Set(
  (
    "mr mrs ms dr prof sr jr st vs etc inc ltd co corp dept approx est fig vol ave blvd rd " +
    "sen gov gen capt col lt sgt cpl rev hon rep mt " +
    "jan feb mar apr jun jul aug sep sept oct nov dec"
  ).split(" "),
);

/**
 * 句号前面的那个词是不是缩写：Dr / Inc / etc、U.S / Ph.D / e.g / a.m 这类「一两个字母 + 点」。
 * 小数（3.14）、Node.js、孤立的数字和单个字母都不算——「We met in 2020.」「plan B.」后面就是下一句。
 */
function isAbbreviation(token: string): boolean {
  return /^(?:[A-Za-z]{1,2}\.)+[A-Za-z]{0,2}$/.test(token) || ABBREVIATIONS.has(token.toLowerCase());
}

/** 单个大写字母当名字缩写（J. K. Rowling、Harry S. Truman）：它前面得是文本开头、或者另一个缩写 / 大写开头的词 */
function isInitial(token: string, previous: string | undefined): boolean {
  if (!/^[A-Z]$/.test(token)) return false;
  return previous === undefined || /^[A-Z]/.test(previous);
}

function hasNextSentence(text: string): boolean {
  for (const match of text.matchAll(SENTENCE_BREAK)) {
    const token = match[1];
    // 问号、感叹号后面一定是句子边界；缩写只对句号生效
    if (match[2] !== ".") return true;
    const before = text.slice(0, match.index).split(/\s+/).filter(Boolean).pop();
    if (!isAbbreviation(token) && !isInitial(token, before)) return true;
  }
  return false;
}

/** 以句末标点结尾？「Smith and Co.」「Apples, oranges, etc.」的最后那个点是缩写，不算 */
function endsLikeSentence(text: string): boolean {
  const tail = text.replace(new RegExp(`${CLOSERS}$`), "");
  if (!LATIN_TERMINATOR.test(tail.slice(-1))) return false;
  if (!tail.endsWith(".")) return true;
  const last = tail.split(/\s+/).pop() ?? "";
  return !isAbbreviation(last.slice(0, -1)) && !isAbbreviation(last);
}

export function isLongText(input: string): boolean {
  const text = input.trim();
  if (!text) return false;
  if ([...text].length > WORD_MAX_CHARS) return true;
  if (text.split(/\r?\n/).filter((line) => line.trim()).length > 1) return true;
  if (KANA_HANGUL.test(text)) return true;

  const han = text.match(HAN)?.length ?? 0;
  if (han > 0) {
    if (han > 12) return true;
    const bare = text.replace(CJK_WRAPPERS, "");
    const interior = bare.replace(/[。！？!?]+$/, "");
    if (han >= 9 && CJK_CLAUSE.test(interior)) return true;
    if (han >= 8 && CJK_TERMINATOR_END.test(bare)) return true;
  }

  // 数「带英文字母或数字的词」：中英混排时，汉字那部分不算英文词
  const words = text.split(/\s+/).filter((token) => /[A-Za-z0-9]/.test(token)).length;
  if (words > 8) return true;
  return words >= 3 && (endsLikeSentence(text) || hasNextSentence(text));
}
