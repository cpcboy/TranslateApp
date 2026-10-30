import { detectLang, type WordEntry } from "./schema";

/**
 * 中文输入的查词，现在是「先译成英文，再解析那个英文词」（提示词见 system-word-zh.txt）：
 * 输入「充电」，卡片是英文词 recharge 的义项表，不再是中文词「充电」本身的解析。
 *
 * 契约字段没动，但 `word` / `lang` 在中文输入时的含义变了：spec 写的是「原词」与「查询语言」，
 * 现在 `word` 是译出来的英文词、`lang` 是 en（词条本身的语言）；用户输入的中文由 store 的 `query` 带着。
 *
 * 判断「是中文输入」「词里有汉字」都以 Rust 的 `detect_lang` 为准（含汉字就是中文，schema.ts 的 `detectLang` 与它同一把尺）。
 */

/**
 * 中文输入拿回来的词条，是不是真的译成了英文。是就返回 null；不是就返回原因（给人看的短语）。
 *
 * 只看词本身：词里还有汉字，就是没译成。`lang` 不作判据——spec 里 `lang` 的字面含义是「查询语言」，
 * 模型照抄输入的 zh 也说得通，词是对的就不该为此重试或降级（见 `asEnglishEntry`）。
 *
 * 没译成的词条不能当正常结果渲染（不许假装成功）：调用方第一次碰到就重试，重试还是这样就降级并明说。
 */
export function translationProblem(query: string, entry: WordEntry): string | null {
  if (detectLang(query) !== "zh") return null;
  if (detectLang(entry.word) === "zh") return "词条里的词还是中文，没有译成英文";
  return null;
}

/**
 * 中文输入、词已经是英文，但模型把 `lang` 写成了 zh：词条的内容是对的，`lang` 归一成 en
 * （界面据此给英文例句配斜体），其余原样。其它情况（英文输入、没译成的词条）原样返回。
 */
export function asEnglishEntry(query: string, entry: WordEntry): WordEntry {
  if (detectLang(query) !== "zh" || translationProblem(query, entry) !== null) return entry;
  return entry.lang === "en" ? entry : { ...entry, lang: "en" };
}

/**
 * 流式预览要不要画。中文输入时，词还没流出来（预览里先用输入顶着）或模型给的就是中文词，
 * 画出来是一张中文卡，随后重试又整张换掉——这种先不画（界面显示骨架屏）；
 * 词是英文就画，lang 照抄成 zh 的按 en 画。
 */
export function previewForDisplay(query: string, preview: WordEntry | null): WordEntry | null {
  if (!preview || translationProblem(query, preview) !== null) return null;
  return asEnglishEntry(query, preview);
}

/** 卡片上「中文 → 英文」那一行要显示的中文原词；这次不是中译英的查询就返回 null */
export function translatedFrom(query: string, entry: WordEntry): string | null {
  const text = query.trim();
  return detectLang(text) === "zh" && translationProblem(text, entry) === null ? text : null;
}

/**
 * 重试之后中文输入仍没译成英文、降级展示模型给的词条时，卡片上方的说明。
 * 这句话在「词条是中文词」与「词里带着中文（如 WeChat (微信)）」两种情况下都成立。
 */
export function untranslatedNotice(query: string): string {
  return `没能把「${query.trim()}」译成英文：模型给的词条里仍是中文词，下面按原样展示`;
}
