import { z } from "zod";
import { LIMITS, normalizeEntry } from "./normalize";

/**
 * `SENSE-v1.0/spec/word-entry.schema.json` 的 zod 镜像。
 *
 * 契约冻结：这里只允许加可选字段，不允许改字段名。
 * 未知字段用默认行为（strip）丢掉，不因此判失败——模型偶尔多写一个字段，
 * 不值得让整张卡作废。
 */

export const SCHEMA_VERSION = 1;

// 词性 / 语域 / 场景都只用于展示，不该因为它们不在名单里就让整张卡作废。
// 之前 `interj`（shit、wow 这类感叹词）缺失，导致这条词无论重试多少次都判非法。
//
// 词性不再兜底成某个具体值：名单外的词性（modal 之类）原样保留，界面直接显示原文；
// 只有根本不是字符串时才记成 "other"（界面写「其他」）。以前一律改成 "n"，
// 卡片上会凭空多出一个错的「n.」。已知词性的写法见 labels.ts（有测试保证名单里的都有标签）。
export const KNOWN_POS = [
  "n",
  "v",
  "adj",
  "adv",
  "prep",
  "conj",
  "pron",
  "num",
  "phr",
  "interj",
  "aux",
  "det",
  "art",
  "part",
  "abbr",
] as const;

const POS_ALIASES: Record<string, string> = {
  noun: "n",
  verb: "v",
  adjective: "adj",
  adverb: "adv",
  preposition: "prep",
  conjunction: "conj",
  pronoun: "pron",
  numeral: "num",
  number: "num",
  phrase: "phr",
  interjection: "interj",
  auxiliary: "aux",
  determiner: "det",
  article: "art",
  particle: "part",
  abbreviation: "abbr",
};

/** 去空白、转小写，单个词性的尾点去掉（`adj.` → `adj`）；常见全称归到缩写：「N」「Adj.」「noun」都能认出来。
 *  `n./v.` 这类复合写法不动，免得被削成 `n./v` */
function normalizePos(value: string): string {
  const trimmed = value.trim().toLowerCase();
  const key = /^[a-z]+\.$/.test(trimmed) ? trimmed.slice(0, -1) : trimmed;
  return Object.prototype.hasOwnProperty.call(POS_ALIASES, key) ? POS_ALIASES[key] : key;
}

export const posSchema = z
  .string()
  .transform(normalizePos)
  .pipe(z.string().min(1).max(16))
  .catch("other");

export const sceneSchema = z
  .enum(["日常", "职场", "学术", "新闻", "法律", "技术", "旅行", "消费", "医疗", "教育", "口语", "网络", "娱乐", "商业"])
  .catch("日常");

export const KNOWN_REGISTER = [
  "neutral",
  "formal",
  "informal",
  "technical",
  "legal",
  "slang",
  "vulgar",
  "colloquial",
  "idiom",
  "literary",
] as const;

/**
 * 模型常把「粗俗」写成别的词。提示词里的语域名单只有 6 个、没有 vulgar，
 * 不归一的话，一个骂人的词会因为被兜底成 neutral 而不带任何提醒。
 * 按「整词」匹配：`very rude`、`offensive slur`、`vulgar slang` 都算；`prudent` 里的 rude 不算。
 */
const VULGAR_WORDS =
  /\b(?:vulgar|offensive|profan\w*|obscen\w*|taboo|rude|crude|coarse|swear\w*|slur|derogator\w*|pejorat\w*|explicit|nsfw|impolite)\b/;

/** 「没有特别语域」的各种写法：不画标签（原来兜底成 neutral 的就是这些） */
const NEUTRAL_WORDS = new Set(["none", "standard", "general", "normal", "n/a", "na", "unmarked", "unknown", "default", "common"]);

const REGISTER_ALIASES: Record<string, string> = {
  casual: "informal",
  colloquialism: "colloquial",
};

function normalizeRegister(value: string | null): string | null {
  if (value === null) return null;
  const key = value.trim().toLowerCase();
  if (VULGAR_WORDS.test(key)) return "vulgar";
  if (NEUTRAL_WORDS.has(key)) return "neutral";
  return Object.prototype.hasOwnProperty.call(REGISTER_ALIASES, key) ? REGISTER_ALIASES[key] : key;
}

/**
 * 语域名单外的值（archaic 之类）原样保留，界面直接显示；
 * 只有根本不是字符串（或空串）时才当作 neutral，不画标签。KNOWN_REGISTER 里的都有中文标签。
 */
export const registerSchema = z
  .string()
  .nullable()
  .transform(normalizeRegister)
  .pipe(z.string().min(1).max(16).nullable())
  .catch("neutral");

export const exampleSchema = z.object({
  text: z.string().min(1).max(200),
  translation: z.string().min(1).max(200),
  target_span: z.string().min(1).max(60),
  scene: sceneSchema,
});

export const senseSchema = z.object({
  rank: z.number().int().min(1).max(6),
  pos: posSchema,
  gloss_zh: z.string().min(1).max(40),
  gloss_en: z.string().max(90),
  share: z.number().min(0).max(1),
  register: registerSchema,
  domains: z.array(z.string().max(12)).max(2),
  collocations: z.array(z.string().max(60)).max(3),
  examples: z.array(exampleSchema).min(1).max(2),
  pitfall: z.string().max(120).nullable(),
  synonyms: z.array(z.string().max(40)).max(3),
});

export const wordEntrySchema = z.object({
  schema_version: z.literal(SCHEMA_VERSION),
  word: z.string().min(1).max(64),
  lang: z.enum(["en", "zh"]),
  pronunciation: z.object({
    ipa: z.string().max(80).nullable(),
    pinyin: z.string().max(80).nullable(),
  }),
  pos: z.array(posSchema).max(4),
  frequency: z.object({
    zipf: z.number().min(0).max(8).nullable(),
    band: z.enum(["high", "mid", "low"]).nullable(),
  }),
  senses: z.array(senseSchema).min(1).max(6),
  confusables: z
    .array(
      z.object({
        word: z.string().max(40),
        difference: z.string().max(120),
      }),
    )
    .max(3),
  notes: z.string().max(200).nullable(),
});

export type WordEntry = z.infer<typeof wordEntrySchema>;
export type Sense = z.infer<typeof senseSchema>;
export type Example = z.infer<typeof exampleSchema>;
export type Scene = z.infer<typeof sceneSchema>;

export type ParseOk = {
  ok: true;
  entry: WordEntry;
  /** 被丢掉的义项数量、share 越界等提示，用于「降级要明说」 */
  warnings: string[];
  /** 有义项被丢弃或做了修正 */
  degraded: boolean;
};

export type ParseFail = { ok: false; error: string };

export type ParseResult = ParseOk | ParseFail;

const SHARE_MIN = 0.95;
const SHARE_MAX = 1.05;

/**
 * 校验模型输出。
 *
 * 策略：先按 spec 上限截掉「多给的一点」（见 normalize.ts，截断要明说）；
 * 再整包校验，整包不过就逐个义项救援，能救几个救几个。
 * 一个义项都不合法才算失败——那时才需要重试。
 */
export function parseWordEntry(raw: unknown): ParseResult {
  const { value, trimmed } = normalizeEntry(raw);
  const result = parseStrict(value, trimmed);
  if (result.ok && trimmed.length > 0) {
    result.warnings.push(`部分内容超出展示上限，已截断：${trimmed.join("、")}`);
  }
  return result;
}

/**
 * 整形之后按 zod 镜像校验（展示类枚举的兜底仍在上面的 schema 里），这里不再额外放宽。
 * 义项超过 6 条时在救援之后再截，截掉的条数记进 trimmed。
 */
function parseStrict(raw: unknown, trimmed: string[]): ParseResult {
  const whole = wordEntrySchema.safeParse(raw);
  if (whole.success) {
    const warnings: string[] = [];
    const sum = whole.data.senses.reduce((acc, sense) => acc + sense.share, 0);
    if (sum < SHARE_MIN || sum > SHARE_MAX) {
      warnings.push(`义项占比合计 ${sum.toFixed(2)}，偏离 1.0，排序可能不准`);
    }
    return { ok: true, entry: whole.data, warnings, degraded: false };
  }

  // 整包失败：尝试只救 senses
  const draft = raw as Record<string, unknown> | null;
  if (!draft || typeof draft !== "object" || !Array.isArray(draft.senses)) {
    return { ok: false, error: firstIssue(whole.error) };
  }

  const kept: Sense[] = [];
  let dropped = 0;
  for (const item of draft.senses) {
    const parsed = senseSchema.safeParse(item);
    if (parsed.success) kept.push(parsed.data);
    else dropped += 1;
  }

  if (kept.length === 0) {
    return { ok: false, error: firstIssue(whole.error) };
  }

  // 先救再截：模型按常用度排序，留前 6 条合法的
  if (kept.length > LIMITS.senses) {
    trimmed.push(`义项 ${kept.length}→${LIMITS.senses}`);
    kept.length = LIMITS.senses;
  }

  const shell = wordEntrySchema.safeParse({
    ...draft,
    senses: kept,
  });

  if (!shell.success) {
    return { ok: false, error: firstIssue(shell.error) };
  }

  const warnings: string[] = [];
  if (dropped > 0) warnings.push(`有 ${dropped} 个义项格式不合法，已略过`);

  const sum = shell.data.senses.reduce((acc, sense) => acc + sense.share, 0);
  if (sum < SHARE_MIN || sum > SHARE_MAX) {
    warnings.push(`义项占比合计 ${sum.toFixed(2)}，偏离 1.0`);
  }

  return { ok: true, entry: shell.data, warnings, degraded: dropped > 0 };
}

function firstIssue(error: z.ZodError): string {
  const issue = error.issues[0];
  if (!issue) return "格式不符合契约";
  const path = issue.path.join(".");
  const what = describeIssue(issue);
  return path ? `${path}：${what}` : what;
}

/** zod 的英文原因换成中文：这句话会直接出现在错误卡片上。 */
function describeIssue(issue: z.core.$ZodIssue): string {
  switch (issue.code) {
    case "invalid_type":
      return issue.message.includes("received undefined") ? "缺少这个字段" : `类型不对，应为 ${issue.expected}`;
    case "too_big":
      if (issue.origin === "string") return `超过 ${issue.maximum} 个字符`;
      if (issue.origin === "array") return `超过 ${issue.maximum} 项`;
      return `大于上限 ${issue.maximum}`;
    case "too_small":
      if (issue.origin === "string") return `少于 ${issue.minimum} 个字符`;
      if (issue.origin === "array") return `少于 ${issue.minimum} 项`;
      return `小于下限 ${issue.minimum}`;
    case "invalid_value":
      return "取值不在允许范围内";
    default:
      return issue.message;
  }
}

/** 判断查询语言：含中日韩统一表意文字就当中文。 */
export function detectLang(text: string): "en" | "zh" {
  return /[\u4e00-\u9fff]/.test(text) ? "zh" : "en";
}

/**
 * 流式预览：把「已经完整的义项」拼成一张能渲染的 Card。
 *
 * 单个义项仍然过 zod，避免把半截数据画到界面上；顶层字段缺失时用占位，
 * 不假装它是完整结果（调用方会额外显示「还在加载」）。
 */
export function buildPreview(partial: unknown, fallbackWord: string): WordEntry | null {
  // 预览也先整形：多一个近义词的义项不该在流式阶段消失（截断提示交给最终卡片）
  const draft = normalizeEntry(partial).value as Record<string, unknown> | null;
  if (!draft || !Array.isArray(draft.senses)) return null;

  const senses: Sense[] = [];
  for (const item of draft.senses) {
    const parsed = senseSchema.safeParse(item);
    if (parsed.success) senses.push(parsed.data);
  }
  if (senses.length === 0) return null;

  const pronunciation = (draft.pronunciation ?? {}) as Record<string, unknown>;
  const frequency = (draft.frequency ?? {}) as Record<string, unknown>;

  const shell = wordEntrySchema.safeParse({
    schema_version: SCHEMA_VERSION,
    word: typeof draft.word === "string" && draft.word.length > 0 ? draft.word : fallbackWord,
    lang: draft.lang === "zh" ? "zh" : "en",
    pronunciation: {
      ipa: typeof pronunciation.ipa === "string" ? pronunciation.ipa : null,
      pinyin: typeof pronunciation.pinyin === "string" ? pronunciation.pinyin : null,
    },
    pos: [],
    frequency: {
      zipf: typeof frequency.zipf === "number" ? frequency.zipf : null,
      band: null,
    },
    // 与最终卡片一致：先逐条校验，再留前 6 条
    senses: senses.slice(0, LIMITS.senses),
    confusables: [],
    notes: null,
  });

  return shell.success ? shell.data : null;
}
