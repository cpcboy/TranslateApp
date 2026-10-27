/**
 * 展示上限整形：校验之前，先把「只是多给了一点」的内容按 spec 的上限截断。
 *
 * spec 给每个数组、每段文字都定了上限，但模型偶尔会多给：round 本来就有 5 个词性，
 * 近义词顺手写了 4 个，notes 多写几个字。这些不影响结构，却会让 zod 判整张卡作废，
 * 刚流式画出来的义项又被收回——违反「部分成功优先」。
 *
 * 规则：
 * - 数组保留前 N 项；文字截到上限，最后一位换成「…」，让截断看得出来；
 * - 截掉了什么汇总成一条提示（降级要明说），由 schema.ts 放进 warnings；
 * - band / zipf 是展示标签，越界就当未知，与枚举兜底同一原则；
 * - 结构性问题（缺字段、类型不对、lang 不是 en/zh）不在这里处理，仍由 zod 判失败。
 *
 * zod 镜像本身不动，仍与 spec 一一对应；这里不改任何字段名。
 */

type Json = Record<string, unknown>;

const isObject = (value: unknown): value is Json =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** 与 spec/word-entry.schema.json 的 maxItems / maxLength 一一对应（有测试对拍）。 */
export const LIMITS = {
  pos: 4,
  senses: 6,
  confusables: 3,
  notes: 200,
  pronunciation: 80,
  confusableWord: 40,
  difference: 120,
  glossZh: 40,
  glossEn: 90,
  domains: 2,
  domain: 12,
  collocations: 3,
  collocation: 60,
  examples: 2,
  exampleText: 200,
  translation: 200,
  targetSpan: 60,
  pitfall: 120,
  synonyms: 3,
  synonym: 40,
} as const;

const BANDS = ["high", "mid", "low"];

export type Normalized = {
  /** 整形后的副本；原对象不动（预览用的是 store 里的状态，不能原地改） */
  value: unknown;
  /** 截掉了什么，给人看的短语，例如「词性 5→4」「备注过长」；校验阶段还可能追加「义项 7→6」 */
  trimmed: string[];
};

export function normalizeEntry(raw: unknown): Normalized {
  if (!isObject(raw)) return { value: raw, trimmed: [] };
  const entry = structuredClone(raw);
  const notes = new Notes();

  notes.list(entry, "pos", LIMITS.pos, "词性");
  // 义项条数不在这里截：要先逐条救援再截到 6 条，否则前面一条坏义项会连累后面合法的（见 schema.ts）
  notes.list(entry, "confusables", LIMITS.confusables, "易混词");
  notes.text(entry, "notes", LIMITS.notes, "备注");

  if (isObject(entry.pronunciation)) {
    notes.text(entry.pronunciation, "ipa", LIMITS.pronunciation, "音标");
    notes.text(entry.pronunciation, "pinyin", LIMITS.pronunciation, "拼音");
  }

  if (isObject(entry.frequency)) {
    // 有值但不认识（越界、写成数字或字符串）就当未知；缺字段仍交给 zod
    const { band, zipf } = entry.frequency;
    if (band != null && !BANDS.includes(band as string)) entry.frequency.band = null;
    if (zipf != null && (typeof zipf !== "number" || zipf < 0 || zipf > 8)) entry.frequency.zipf = null;
  }

  if (Array.isArray(entry.confusables)) {
    for (const item of entry.confusables) {
      if (!isObject(item)) continue;
      notes.text(item, "word", LIMITS.confusableWord, "易混词");
      notes.text(item, "difference", LIMITS.difference, "易混词说明");
    }
  }

  if (Array.isArray(entry.senses)) {
    for (const sense of entry.senses) {
      if (isObject(sense)) trimSense(sense, notes);
    }
  }

  return { value: entry, trimmed: notes.summary() };
}

function trimSense(sense: Json, notes: Notes) {
  notes.text(sense, "gloss_zh", LIMITS.glossZh, "释义");
  notes.text(sense, "gloss_en", LIMITS.glossEn, "英文释义");
  notes.list(sense, "domains", LIMITS.domains, "领域");
  notes.eachText(sense, "domains", LIMITS.domain, "领域");
  notes.list(sense, "collocations", LIMITS.collocations, "搭配");
  notes.eachText(sense, "collocations", LIMITS.collocation, "搭配");
  notes.list(sense, "examples", LIMITS.examples, "例句");
  if (Array.isArray(sense.examples)) {
    for (const example of sense.examples) {
      if (!isObject(example)) continue;
      notes.text(example, "text", LIMITS.exampleText, "例句");
      notes.text(example, "translation", LIMITS.translation, "例句译文");
      notes.text(example, "target_span", LIMITS.targetSpan, "例句");
    }
  }
  notes.text(sense, "pitfall", LIMITS.pitfall, "易错点");
  notes.list(sense, "synonyms", LIMITS.synonyms, "近义词");
  notes.eachText(sense, "synonyms", LIMITS.synonym, "近义词");
}

/** 记录截断，最后汇总成一条提示；同一类只记超得最多的那次。 */
class Notes {
  private lists = new Map<string, { from: number; to: number }>();
  private texts = new Set<string>();

  list(owner: Json, key: string, max: number, label: string) {
    const value = owner[key];
    if (!Array.isArray(value) || value.length <= max) return;
    owner[key] = value.slice(0, max);
    const seen = this.lists.get(label);
    if (!seen || value.length > seen.from) this.lists.set(label, { from: value.length, to: max });
  }

  text(owner: Json, key: string, max: number, label: string) {
    const value = owner[key];
    if (typeof value !== "string" || value.length <= max) return;
    owner[key] = clip(value, max);
    this.texts.add(label);
  }

  eachText(owner: Json, key: string, max: number, label: string) {
    const value = owner[key];
    if (!Array.isArray(value)) return;
    owner[key] = value.map((item) => {
      if (typeof item !== "string" || item.length <= max) return item;
      this.texts.add(label);
      return clip(item, max);
    });
  }

  summary(): string[] {
    const parts = [...this.lists].map(([label, { from, to }]) => `${label} ${from}→${to}`);
    for (const label of this.texts) parts.push(`${label}过长`);
    return parts;
  }
}

/** 截到 max 个 UTF-16 单元（zod 的 max 就是这么数的），末位换成「…」；不把代理对切成两半。 */
function clip(text: string, max: number): string {
  let end = max - 1;
  const code = text.charCodeAt(end - 1);
  if (code >= 0xd800 && code <= 0xdbff) end -= 1;
  return `${text.slice(0, end)}…`;
}
