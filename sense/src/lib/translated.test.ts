import { describe, expect, it } from "vitest";
import { detectLang } from "./schema";
import { makeEntry as entry } from "./testing";
import {
  asEnglishEntry,
  previewForDisplay,
  translatedFrom,
  translationProblem,
  untranslatedNotice,
} from "./translated";
// 用 Vite 的 ?raw 读 Rust 源码：与 Rust 的汉字判断对拍，不引入 Node 类型依赖
import lookupRs from "../../src-tauri/src/commands/lookup.rs?raw";

describe("中文输入有没有被译成英文（只看词本身）", () => {
  it("中文输入拿回英文词条：没问题", () => {
    expect(translationProblem("充电", entry("recharge"))).toBeNull();
    expect(translationProblem("说曹操曹操到", entry("speak of the devil"))).toBeNull();
  });

  it("中英混排的输入也按中文输入算：译成英文才算数", () => {
    expect(translationProblem("AI 助手", entry("AI assistant"))).toBeNull();
    expect(translationProblem("AI 助手", entry("AI 助手", "zh"))).not.toBeNull();
  });

  it("模型没按提示词来、还是给了中文词条：必须被认出来，不能当正常结果", () => {
    expect(translationProblem("充电", entry("充电", "zh"))).toContain("还是中文");
  });

  it("lang 写了 en、词本身却还是汉字：同样算没译成", () => {
    expect(translationProblem("充电", entry("充电", "en"))).toContain("还是中文");
  });

  it("词里带着中文（WeChat (微信)）：算没译成——提示词明说了不要保留汉字", () => {
    expect(translationProblem("微信", entry("WeChat (微信)", "en"))).not.toBeNull();
  });

  it("词是英文、lang 却照抄成了 zh：词是对的，不算没译成（spec 里 lang 的字面含义本来就是查询语言）", () => {
    expect(translationProblem("充电", entry("recharge", "zh"))).toBeNull();
  });

  it("英文输入不查这一项：英文词条怎么回都不报", () => {
    expect(translationProblem("charge", entry("charge"))).toBeNull();
    expect(translationProblem("charge", entry("charge", "zh"))).toBeNull();
    expect(translationProblem("charge", entry("充电", "zh"))).toBeNull();
  });
});

describe("asEnglishEntry：词是英文、lang 照抄成 zh 时归一成 en", () => {
  it("中文输入 + 英文词 + lang zh：lang 变 en，其余原样，也不改动原对象", () => {
    const original = entry("recharge", "zh");
    const fixed = asEnglishEntry("充电", original);
    expect(fixed.lang).toBe("en");
    expect(fixed.word).toBe("recharge");
    expect(fixed.senses).toEqual(original.senses);
    expect(original.lang).toBe("zh");
  });

  it("lang 本来就是 en：原样（同一个对象）", () => {
    const original = entry("recharge", "en");
    expect(asEnglishEntry("充电", original)).toBe(original);
  });

  it("没译成（词还是中文）：原样，不假装成英文词条", () => {
    const original = entry("充电", "zh");
    expect(asEnglishEntry("充电", original)).toBe(original);
    expect(asEnglishEntry("充电", original).lang).toBe("zh");
  });

  it("英文输入：原样", () => {
    const original = entry("charge", "zh");
    expect(asEnglishEntry("charge", original)).toBe(original);
  });
});

describe("previewForDisplay：流式预览要不要画", () => {
  it("没有预览：null", () => {
    expect(previewForDisplay("充电", null)).toBeNull();
  });

  it("中文输入、预览里的词还是中文（词没流出来、拿输入顶着，或模型给的就是中文词）：先不画", () => {
    expect(previewForDisplay("充电", entry("充电", "en"))).toBeNull();
    expect(previewForDisplay("充电", entry("充电", "zh"))).toBeNull();
  });

  it("中文输入、预览里的词已是英文：画；lang 照抄成 zh 的按 en 画", () => {
    expect(previewForDisplay("充电", entry("recharge"))?.word).toBe("recharge");
    expect(previewForDisplay("充电", entry("recharge", "zh"))?.lang).toBe("en");
  });

  it("英文输入：预览原样", () => {
    const preview = entry("charge");
    expect(previewForDisplay("charge", preview)).toBe(preview);
  });
});

describe("卡片上「中文 → 英文」那一行", () => {
  it("中译英成功：给出中文原词", () => {
    expect(translatedFrom("充电", entry("recharge"))).toBe("充电");
    expect(translatedFrom("充电", entry("recharge", "zh"))).toBe("充电");
  });

  it("输入两头的空白不带进去", () => {
    expect(translatedFrom("  落实 ", entry("implement"))).toBe("落实");
  });

  it("英文输入：没有这一行", () => {
    expect(translatedFrom("charge", entry("charge"))).toBeNull();
  });

  it("中文输入却没译成英文：也不显示这一行（不能宣称译过）", () => {
    expect(translatedFrom("充电", entry("充电", "zh"))).toBeNull();
  });

  it("流式预览里词还没到、先用输入顶着：不显示", () => {
    // buildPreview 在 word 还没流出来时用查询词顶替，此时 word 就是中文
    expect(translatedFrom("充电", entry("充电", "en"))).toBeNull();
  });
});

describe("没译成英文时卡片上方的说明", () => {
  it("点名是哪个词，并说清下面是什么——「中文词」与「词里带着中文」两种情况都成立", () => {
    expect(untranslatedNotice("充电")).toBe("没能把「充电」译成英文：模型给的词条里仍是中文词，下面按原样展示");
  });

  it("输入两头的空白不带进去", () => {
    expect(untranslatedNotice("  充电 ")).toBe("没能把「充电」译成英文：模型给的词条里仍是中文词，下面按原样展示");
  });
});

describe("汉字判断与 Rust 的 detect_lang 同一把尺", () => {
  it("Rust 源码里的范围就是 U+4E00..=U+9FFF（改了这里要同步改 schema.ts 的 detectLang）", () => {
    expect(lookupRs).toContain("('\\u{4e00}'..='\\u{9fff}').contains(&ch)");
  });

  it("TS 一侧的边界：范围内是中文，范围外（含扩展 A 区、彝文）不是", () => {
    expect(detectLang("一")).toBe("zh");
    expect(detectLang("鿿")).toBe("zh");
    expect(detectLang("䷿")).toBe("en");
    expect(detectLang("ꀀ")).toBe("en");
    expect(detectLang("AI 助手")).toBe("zh");
    expect(detectLang("charge")).toBe("en");
  });
});
