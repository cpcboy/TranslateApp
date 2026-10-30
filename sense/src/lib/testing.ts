import { wordEntrySchema, type WordEntry } from "./schema";

/** 单测用的最小合法词条：只关心 word / lang，其余字段给能过 zod 的固定值。不进生产包（只有测试 import 它）。 */
export function makeEntry(word: string, lang: "en" | "zh" = "en"): WordEntry {
  return wordEntrySchema.parse({
    schema_version: 1,
    word,
    lang,
    pronunciation: { ipa: lang === "en" ? "/x/" : null, pinyin: lang === "zh" ? "chōng diàn" : null },
    pos: ["v"],
    frequency: { zipf: 4.5, band: "mid" },
    senses: [
      {
        rank: 1,
        pos: "v",
        gloss_zh: "充电",
        gloss_en: "to charge a battery again",
        share: 1,
        register: "neutral",
        domains: ["技术"],
        collocations: ["recharge your phone"],
        examples: [{ text: "Recharge your phone.", translation: "给手机充电。", target_span: "Recharge", scene: "日常" }],
        pitfall: null,
        synonyms: [],
      },
    ],
    confusables: [],
    notes: null,
  });
}
