import { z } from "zod";

/**
 * `SENSE-v1.0/spec/word-entry.schema.json` 的 zod 镜像。
 *
 * 契约冻结：这里只允许加可选字段，不允许改字段名。
 * 未知字段用默认行为（strip）丢掉，不因此判失败——模型偶尔多写一个字段，
 * 不值得让整张卡作废。
 */

export const SCHEMA_VERSION = 1;
/** prompt 版本进缓存 key：改 prompt 后旧缓存自然失效。 */
export const PROMPT_VERSION = 1;

// 词性 / 语域 / 场景都只用于展示，不该因为它们不在名单里就让整张卡作废。
// 之前 `interj`（shit、wow 这类感叹词）缺失，导致这条词无论重试多少次都判非法。
const KNOWN_POS = [
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

export const posSchema = z.enum(KNOWN_POS).catch("n");

export const sceneSchema = z
  .enum(["日常", "职场", "学术", "新闻", "法律", "技术", "旅行", "消费", "医疗", "教育", "口语", "网络", "娱乐", "商业"])
  .catch("日常");

export const registerSchema = z
  .enum(["neutral", "formal", "informal", "technical", "legal", "slang", "vulgar", "colloquial", "idiom", "literary"])
  .nullable()
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
 * 策略：先整包校验；整包不过就逐个义项救援，能救几个救几个。
 * 一个义项都不合法才算失败——那时才需要重试。
 */
export function parseWordEntry(raw: unknown): ParseResult {
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
  return path ? `${path}: ${issue.message}` : issue.message;
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
  const draft = partial as Record<string, unknown> | null;
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
    senses,
    confusables: [],
    notes: null,
  });

  return shell.success ? shell.data : null;
}
