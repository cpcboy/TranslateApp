import Ajv2020 from "ajv/dist/2020";
import { describe, expect, it } from "vitest";
import { buildPreview, detectLang, parseWordEntry, wordEntrySchema } from "./schema";
import { parseFinal, tryParsePartial } from "./stream";
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

describe("buildPreview", () => {
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
