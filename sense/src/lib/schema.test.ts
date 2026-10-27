import Ajv2020 from "ajv/dist/2020";
import { describe, expect, it } from "vitest";
import { buildPreview, detectLang, parseWordEntry, wordEntrySchema } from "./schema";
import { parseFinal, tryParsePartial } from "./stream";
import { LIMITS } from "./normalize";
import specSchema from "../../src-tauri/resources/schema/word-entry.json";

export const CHARGE = {
  schema_version: 1,
  word: "charge",
  lang: "en",
  pronunciation: { ipa: "/tʃɑːdʒ/", pinyin: null },
  pos: ["v", "n"],
  frequency: { zipf: 5.6, band: "high" },
  senses: [
    {
      rank: 1,
      pos: "v",
      gloss_zh: "收费；要价",
      gloss_en: "to ask money for goods or a service",
      share: 0.5,
      register: "neutral",
      domains: ["消费", "旅行"],
      collocations: ["charge sb for sth", "charge a fee"],
      examples: [
        {
          text: "The hotel charges 200 dollars a night.",
          translation: "这家酒店每晚收费 200 美元。",
          target_span: "charges",
          scene: "旅行",
        },
      ],
      pitfall: "人作主语：They charged me ¥30。",
      synonyms: ["bill"],
    },
    {
      rank: 2,
      pos: "v",
      gloss_zh: "指控；控告",
      gloss_en: "to accuse someone formally of a crime",
      share: 0.3,
      register: "legal",
      domains: ["法律", "新闻"],
      collocations: ["be charged with sth"],
      examples: [
        {
          text: "He was charged with fraud last Friday.",
          translation: "他上周五被控欺诈。",
          target_span: "charged",
          scene: "法律",
        },
      ],
      pitfall: null,
      synonyms: ["accuse"],
    },
    {
      rank: 3,
      pos: "v",
      gloss_zh: "充电",
      gloss_en: "to put electricity into a device",
      share: 0.2,
      register: "neutral",
      domains: ["技术", "日常"],
      collocations: ["charge your phone"],
      examples: [
        {
          text: "Charge your phone before we leave.",
          translation: "走之前把手机充上电。",
          target_span: "Charge",
          scene: "日常",
        },
      ],
      pitfall: null,
      synonyms: [],
    },
  ],
  confusables: [{ word: "cost", difference: "cost 是主语自身花费，charge 是向别人收取。" }],
  notes: null,
};

describe("wordEntrySchema（与 spec JSON Schema 对拍）", () => {
  // 直接 import JSON：由 Vite 解析，省掉 Node 类型依赖
  const schema = specSchema;
  const ajv = new Ajv2020({ strict: false, allErrors: true });
  const validate = ajv.compile(schema);

  it("样例在 JSON Schema 与 zod 两边都通过", () => {
    expect(validate(CHARGE), JSON.stringify(validate.errors)).toBe(true);
    expect(wordEntrySchema.safeParse(CHARGE).success).toBe(true);
  });

  it("缺必填字段时两边都拒绝", () => {
    const broken = { ...CHARGE, senses: undefined };
    expect(validate(broken)).toBe(false);
    expect(wordEntrySchema.safeParse(broken).success).toBe(false);
  });

  it("展示类枚举非法时：zod 兜底成默认值，spec 仍然拒绝", () => {
    // 这是刻意的分歧：spec 是给模型看的严格契约，
    // 应用侧不能因为一个展示标签（场景/词性/语域）丢掉整张卡。
    // （词性、语域现在不兜底成默认值，而是归一后原样保留，见 labels.test.ts；这里只验场景）
    const broken = structuredClone(CHARGE) as typeof CHARGE;
    broken.senses[0].examples[0].scene = "办公室";
    expect(validate(broken)).toBe(false);

    const parsed = wordEntrySchema.safeParse(broken);
    expect(parsed.success).toBe(true);
    if (parsed.success) {
      expect(parsed.data.senses[0].examples[0].scene).toBe("日常");
    }
  });

  it("interj 这类词性不再让整条记录作废", () => {
    const entry = structuredClone(CHARGE) as typeof CHARGE;
    (entry.senses[0] as { pos: string }).pos = "interj";
    const parsed = parseWordEntry(entry);
    expect(parsed.ok).toBe(true);
    if (parsed.ok) expect(parsed.entry.senses[0].pos).toBe("interj");
  });

  it("多一个字段：zod 容忍（strip），不因此判失败", () => {
    const extra = { ...CHARGE, 模型自创字段: "x" };
    const parsed = parseWordEntry(extra);
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect("模型自创字段" in parsed.entry).toBe(false);
    }
  });
});

describe("parseWordEntry 的降级策略", () => {
  it("整包合法时不降级", () => {
    const parsed = parseWordEntry(CHARGE);
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(parsed.degraded).toBe(false);
      expect(parsed.warnings).toEqual([]);
      expect(parsed.entry.senses).toHaveLength(3);
    }
  });

  it("单个义项坏掉时丢掉它，保留其余并标记降级", () => {
    const broken = structuredClone(CHARGE) as typeof CHARGE;
    // @ts-expect-error 故意删字段
    delete broken.senses[1].gloss_zh;
    const parsed = parseWordEntry(broken);
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(parsed.entry.senses).toHaveLength(2);
      expect(parsed.degraded).toBe(true);
      expect(parsed.warnings.join()).toContain("1 个义项");
    }
  });

  it("全部义项都坏掉时判失败（交给重试）", () => {
    const broken = { ...CHARGE, senses: [{ rank: 1 }, { rank: 2 }] };
    expect(parseWordEntry(broken).ok).toBe(false);
  });

  it("share 之和越界只警告，不判失败", () => {
    const skewed = structuredClone(CHARGE) as typeof CHARGE;
    skewed.senses[0].share = 0.9;
    const parsed = parseWordEntry(skewed);
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(parsed.warnings.some((item) => item.includes("占比"))).toBe(true);
    }
  });
});

describe("展示上限整形：只是多给了一点，不让整张卡作废", () => {
  const withSenses = (count: number) => {
    const entry = structuredClone(CHARGE) as typeof CHARGE;
    entry.senses = Array.from({ length: count }, (_, index) => ({
      ...structuredClone(CHARGE.senses[0]),
      rank: Math.min(index + 1, 6),
    }));
    return entry;
  };
  const trimWarning = (warnings: string[]) => warnings.find((item) => item.includes("超出展示上限"));

  it("round 这类词有 5 个词性：保留前 4 个，并明说截断了", () => {
    const entry = structuredClone(CHARGE) as typeof CHARGE;
    entry.pos = ["adj", "n", "prep", "adv", "v"];
    const parsed = parseWordEntry(entry);
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(parsed.entry.pos).toEqual(["adj", "n", "prep", "adv"]);
      expect(trimWarning(parsed.warnings)).toContain("词性 5→4");
      expect(parsed.degraded).toBe(false);
    }
  });

  it("7 个义项保留前 6 个", () => {
    const parsed = parseWordEntry(withSenses(7));
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(parsed.entry.senses).toHaveLength(6);
      expect(trimWarning(parsed.warnings)).toContain("义项 7→6");
    }
  });

  it("近义词、例句、易混词超量都截断，不丢义项", () => {
    const entry = structuredClone(CHARGE) as typeof CHARGE;
    for (const sense of entry.senses) {
      sense.synonyms = ["a", "b", "c", "d"];
      sense.examples = [sense.examples[0], sense.examples[0], sense.examples[0]];
    }
    entry.confusables = [1, 2, 3, 4].map((index) => ({ word: `w${index}`, difference: "区别" }));
    const parsed = parseWordEntry(entry);
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(parsed.entry.senses).toHaveLength(3);
      expect(parsed.entry.senses.every((sense) => sense.synonyms.length === 3)).toBe(true);
      expect(parsed.entry.senses.every((sense) => sense.examples.length === 2)).toBe(true);
      expect(parsed.entry.confusables).toHaveLength(3);
      const warning = trimWarning(parsed.warnings) ?? "";
      expect(warning).toContain("近义词 4→3");
      expect(warning).toContain("例句 3→2");
      expect(warning).toContain("易混词 4→3");
    }
  });

  it("超长文字截到上限并以省略号结尾", () => {
    const entry = structuredClone(CHARGE) as typeof CHARGE;
    (entry as { notes: string | null }).notes = "注".repeat(201);
    entry.confusables = [{ word: "cost", difference: "差".repeat(121) }];
    entry.senses[0].domains = ["Information Technology"];
    const parsed = parseWordEntry(entry);
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(parsed.entry.notes).toHaveLength(200);
      expect(parsed.entry.notes?.endsWith("…")).toBe(true);
      expect(parsed.entry.confusables[0].difference).toHaveLength(120);
      expect(parsed.entry.senses[0].domains[0]).toHaveLength(12);
      expect(trimWarning(parsed.warnings)).toContain("备注过长");
    }
  });

  it("截断不会把 emoji 的代理对切成两半", () => {
    const entry = structuredClone(CHARGE) as typeof CHARGE;
    (entry as { notes: string | null }).notes = "注".repeat(198) + "😀😀";
    const parsed = parseWordEntry(entry);
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(/[\ud800-\udbff](?![\udc00-\udfff])/.test(parsed.entry.notes ?? "")).toBe(false);
    }
  });

  it("不认识的词频档位当未知，不作废、不提示", () => {
    const entry = structuredClone(CHARGE) as typeof CHARGE;
    (entry.frequency as { band: string | null }).band = "very high";
    entry.frequency.zipf = 8.4;
    const parsed = parseWordEntry(entry);
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(parsed.entry.frequency).toEqual({ zipf: null, band: null });
      expect(trimWarning(parsed.warnings)).toBeUndefined();
    }
  });

  it("结构性问题仍然判失败，原因是中文且带字段路径", () => {
    const parsed = parseWordEntry({ ...CHARGE, lang: "ja" });
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.error).toBe("lang：取值不在允许范围内");

    const missing = parseWordEntry({ ...CHARGE, senses: undefined });
    expect(missing.ok).toBe(false);
    if (!missing.ok) expect(missing.error).toBe("senses：缺少这个字段");
  });

  it("整形不改动传进来的对象", () => {
    const entry = structuredClone(CHARGE) as typeof CHARGE;
    entry.pos = ["adj", "n", "prep", "adv", "v"];
    parseWordEntry(entry);
    expect(entry.pos).toHaveLength(5);
  });

  it("合法输入没有截断提示", () => {
    const parsed = parseWordEntry(CHARGE);
    expect(parsed.ok && trimWarning(parsed.warnings)).toBeFalsy();
  });

  it("先救再截：前面一条坏义项不会连累后面合法的", () => {
    const ranks = [7, 1, 2, 3, 4, 5, 6]; // 第一条 rank 越界，其余 6 条合法
    const entry = structuredClone(CHARGE) as typeof CHARGE;
    entry.senses = ranks.map((rank) => ({ ...structuredClone(CHARGE.senses[0]), rank }));
    const parsed = parseWordEntry(entry);
    expect(parsed.ok).toBe(true);
    if (parsed.ok) expect(parsed.entry.senses.map((sense) => sense.rank)).toEqual([1, 2, 3, 4, 5, 6]);
  });

  it("词频写成了别的类型也当未知", () => {
    const entry = structuredClone(CHARGE) as unknown as { frequency: { zipf: unknown; band: unknown } };
    entry.frequency = { zipf: "5.6", band: 3 };
    const parsed = parseWordEntry(entry);
    expect(parsed.ok).toBe(true);
    if (parsed.ok) expect(parsed.entry.frequency).toEqual({ zipf: null, band: null });
  });

  it("LIMITS 与 spec 的 maxItems / maxLength 一一对应", () => {
    const top = specSchema.properties;
    const sense = specSchema.$defs.sense.properties;
    const example = specSchema.$defs.example.properties;
    const confusable = top.confusables.items.properties;
    expect([LIMITS.pos, LIMITS.senses, LIMITS.confusables, LIMITS.notes]).toEqual([
      top.pos.maxItems,
      top.senses.maxItems,
      top.confusables.maxItems,
      top.notes.maxLength,
    ]);
    expect(LIMITS.pronunciation).toBe(top.pronunciation.properties.ipa.maxLength);
    expect([LIMITS.confusableWord, LIMITS.difference]).toEqual([
      confusable.word.maxLength,
      confusable.difference.maxLength,
    ]);
    expect([
      LIMITS.glossZh,
      LIMITS.glossEn,
      LIMITS.domains,
      LIMITS.domain,
      LIMITS.collocations,
      LIMITS.collocation,
      LIMITS.examples,
      LIMITS.pitfall,
      LIMITS.synonyms,
      LIMITS.synonym,
    ]).toEqual([
      sense.gloss_zh.maxLength,
      sense.gloss_en.maxLength,
      sense.domains.maxItems,
      sense.domains.items.maxLength,
      sense.collocations.maxItems,
      sense.collocations.items.maxLength,
      sense.examples.maxItems,
      sense.pitfall.maxLength,
      sense.synonyms.maxItems,
      sense.synonyms.items.maxLength,
    ]);
    expect([LIMITS.exampleText, LIMITS.translation, LIMITS.targetSpan]).toEqual([
      example.text.maxLength,
      example.translation.maxLength,
      example.target_span.maxLength,
    ]);
  });
});

describe("buildPreview", () => {
  it("流式预览里，多一个近义词的义项也照常显示", () => {
    const sense = { ...structuredClone(CHARGE.senses[0]), synonyms: ["a", "b", "c", "d"] };
    const preview = buildPreview({ word: "charge", senses: [sense] }, "charge");
    expect(preview?.senses).toHaveLength(1);
    expect(preview?.senses[0].synonyms).toHaveLength(3);
  });

  it("只保留结构完整的义项", () => {
    const preview = buildPreview(
      {
        word: "charge",
        pronunciation: { ipa: "/tʃɑːdʒ/" },
        senses: [CHARGE.senses[0], { rank: 2, gloss_zh: "半截" }],
      },
      "charge",
    );
    expect(preview?.senses).toHaveLength(1);
    expect(preview?.word).toBe("charge");
  });

  it("一个完整义项都没有时返回 null", () => {
    expect(buildPreview({ senses: [{ rank: 1 }] }, "x")).toBeNull();
  });
});

describe("detectLang", () => {
  it("按字符判断中英", () => {
    expect(detectLang("charge")).toBe("en");
    expect(detectLang("落实")).toBe("zh");
    expect(detectLang("AI 助手")).toBe("zh");
  });
});

describe("增量解析在整条流上都成立", () => {
  it("按任意切片喂进去都不抛，义项数单调不减", () => {
    const json = JSON.stringify(CHARGE);
    let last = 0;
    for (let cut = 1; cut <= json.length; cut += 3) {
      const partial = tryParsePartial(json.slice(0, cut));
      const count = partial?.senses.length ?? 0;
      expect(count).toBeGreaterThanOrEqual(last);
      last = count;
    }
    expect(last).toBeGreaterThanOrEqual(3);
  });

  it("最终结果能完整解析出来", () => {
    const parsed = parseFinal(JSON.stringify(CHARGE))?.value as typeof CHARGE;
    expect(parsed.senses).toHaveLength(3);
    expect(parsed.word).toBe("charge");
  });
});
