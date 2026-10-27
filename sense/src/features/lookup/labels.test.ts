import { describe, expect, it } from "vitest";
import { KNOWN_POS, KNOWN_REGISTER, parseWordEntry } from "../../lib/schema";
import { posLabel, REGISTER_LABEL, registerLabel, sceneLine, WARN_REGISTER } from "./labels";

const ENTRY = {
  schema_version: 1,
  word: "shit",
  lang: "en",
  pronunciation: { ipa: "ʃɪt", pinyin: null },
  pos: ["interj", "n"],
  frequency: { zipf: 4.5, band: "high" },
  senses: [
    {
      rank: 1,
      pos: "modal",
      gloss_zh: "可恶",
      gloss_en: "damn",
      share: 1,
      register: "vulgar",
      domains: ["口语"],
      collocations: [],
      examples: [{ text: "Oh shit.", translation: "糟了。", target_span: "shit", scene: "口语" }],
      pitfall: null,
      synonyms: [],
    },
  ],
  confusables: [],
  notes: null,
};

describe("标签", () => {
  it("名单里的词性、语域都有标签（新增一个值忘了配标签，这里会红）", () => {
    for (const pos of KNOWN_POS) expect(posLabel(pos), pos).not.toBe(pos);
    for (const register of KNOWN_REGISTER) expect(REGISTER_LABEL, register).toHaveProperty(register);
  });

  it("粗俗义项要有标签，并且走醒目样式", () => {
    expect(REGISTER_LABEL.vulgar).toBe("粗俗");
    expect(WARN_REGISTER.has("vulgar")).toBe(true);
    expect(WARN_REGISTER.has("slang")).toBe(false);
  });

  it("名单外的词性原样显示，不替它改成 n.", () => {
    expect(posLabel("modal")).toBe("modal");
    const parsed = parseWordEntry(ENTRY);
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(parsed.entry.senses[0].pos).toBe("modal");
      expect(posLabel(parsed.entry.senses[0].pos)).toBe("modal");
    }
  });

  it("词性压根不是字符串时记成「其他」", () => {
    const parsed = parseWordEntry({ ...ENTRY, senses: [{ ...ENTRY.senses[0], pos: 7 }] });
    expect(parsed.ok).toBe(true);
    if (parsed.ok) expect(posLabel(parsed.entry.senses[0].pos)).toBe("其他");
  });

  it("场景与领域同名时只显示一个", () => {
    expect(sceneLine("法律", "法律")).toBe("法律");
    expect(sceneLine("口语", "口语")).toBe("口语");
    expect(sceneLine("职场", "法律")).toBe("职场 · 法律");
    expect(sceneLine("日常", undefined)).toBe("日常");
  });

  it("模型给 constructor / __proto__ 这类词时不读原型上的东西", () => {
    expect(posLabel("constructor")).toBe("constructor");
    expect(posLabel("__proto__")).toBe("__proto__");
    expect(registerLabel("toString")).toBe("toString");
  });

  it("词性大小写、尾点、全称写法归一，不会和「n.」并排出现一份原样的「N」", () => {
    for (const [given, shown] of [["N", "n."], ["Adj.", "adj."], ["noun", "n."], [" Verb ", "v."], ["Interjection", "interj."]] as const) {
      const parsed = parseWordEntry({ ...ENTRY, senses: [{ ...ENTRY.senses[0], pos: given }] });
      expect(parsed.ok).toBe(true);
      if (parsed.ok) expect(posLabel(parsed.entry.senses[0].pos), given).toBe(shown);
    }
  });

  it("语域：模型写 offensive / Vulgar / TABOO 一样要画出「粗俗」徽标；名单外的原样显示", () => {
    for (const given of ["offensive", "Vulgar", "VULGAR", "taboo", "rude", "obscene", "derogatory", "profane", "pejorative"]) {
      const parsed = parseWordEntry({ ...ENTRY, senses: [{ ...ENTRY.senses[0], register: given }] });
      expect(parsed.ok).toBe(true);
      if (parsed.ok) {
        const register = parsed.entry.senses[0].register ?? "";
        expect(WARN_REGISTER.has(register), given).toBe(true);
      }
    }
    const archaic = parseWordEntry({ ...ENTRY, senses: [{ ...ENTRY.senses[0], register: "archaic" }] });
    expect(archaic.ok && registerLabel(archaic.entry.senses[0].register ?? "")).toBe("archaic");
    const nothing = parseWordEntry({ ...ENTRY, senses: [{ ...ENTRY.senses[0], register: "" }] });
    expect(nothing.ok && nothing.entry.senses[0].register).toBe("neutral");
    const keptNull = parseWordEntry({ ...ENTRY, senses: [{ ...ENTRY.senses[0], register: null }] });
    expect(keptNull.ok && keptNull.entry.senses[0].register).toBe(null);
  });

  it("语域：各种「粗俗」的写法都画徽标；各种「没有特别语域」的写法不画标签", () => {
    for (const given of ["profanity", "swear word", "coarse", "impolite", "slur", "explicit", "nsfw", "very rude", "vulgar slang", "offensive slur", "extremely offensive", "offensive/derogatory"]) {
      const parsed = parseWordEntry({ ...ENTRY, senses: [{ ...ENTRY.senses[0], register: given }] });
      expect(parsed.ok).toBe(true);
      if (parsed.ok) expect(WARN_REGISTER.has(parsed.entry.senses[0].register ?? ""), given).toBe(true);
    }
    for (const given of ["none", "Standard", "general", "normal", "n/a", "unmarked", "unknown"]) {
      const parsed = parseWordEntry({ ...ENTRY, senses: [{ ...ENTRY.senses[0], register: given }] });
      expect(parsed.ok && parsed.entry.senses[0].register, given).toBe("neutral");
    }
    // 整词匹配：prudent 里含有 rude 的字母，不能被当成粗俗
    const prudent = parseWordEntry({ ...ENTRY, senses: [{ ...ENTRY.senses[0], register: "prudent" }] });
    expect(prudent.ok && prudent.entry.senses[0].register).toBe("prudent");
  });

  it("复合词性（n./v.）不被削掉尾点，单个词性的尾点照常去掉", () => {
    const pos = (given: string) => {
      const parsed = parseWordEntry({ ...ENTRY, senses: [{ ...ENTRY.senses[0], pos: given }] });
      return parsed.ok ? parsed.entry.senses[0].pos : null;
    };
    expect(pos("n./v.")).toBe("n./v.");
    expect(pos("v.t.")).toBe("v.t.");
    expect(pos("adj.")).toBe("adj");
    expect(pos("Noun.")).toBe("n");
  });
});
