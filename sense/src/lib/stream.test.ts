import { describe, expect, it } from "vitest";
import { closeOpen, extractJson, parseFinal, tryParsePartial } from "./stream";

const SAMPLE = `{
  "schema_version": 1,
  "word": "charge",
  "lang": "en",
  "senses": [
    {"rank": 1, "pos": "v", "gloss_zh": "收费", "share": 0.5, "examples": [{"text": "The hotel charges $200."}]},
    {"rank": 2, "pos": "v", "gloss_zh": "指控", "share": 0.3, "examples": [{"text": "He was charged with fraud."}]}
  ]
}`;

describe("extractJson", () => {
  it("剥掉 markdown 围栏", () => {
    const fenced = "```json\n" + SAMPLE + "\n```";
    expect(extractJson(fenced).startsWith("{")).toBe(true);
    expect(extractJson(fenced).endsWith("}")).toBe(true);
  });

  it("剥掉模型加的前言", () => {
    const noisy = "好的，以下是结果：\n" + SAMPLE;
    expect(extractJson(noisy).startsWith("{")).toBe(true);
  });

  it("空串不炸", () => {
    expect(extractJson("")).toBe("");
  });
});

describe("closeOpen", () => {
  it("补齐未闭合的字符串、数组和对象", () => {
    const broken = '{"a":[1,2,{"b":"半截';
    const fixed = closeOpen(broken);
    expect(() => JSON.parse(fixed)).not.toThrow();
  });

  it("去掉结尾多余的逗号", () => {
    const fixed = closeOpen('{"a":1,');
    expect(JSON.parse(fixed)).toEqual({ a: 1 });
  });

  it("已经是合法 JSON 时保持可用", () => {
    expect(JSON.parse(closeOpen('{"a":1}'))).toEqual({ a: 1 });
  });
});

describe("tryParsePartial", () => {
  it("完整 JSON 直接返回", () => {
    const parsed = tryParsePartial(SAMPLE);
    expect(parsed?.senses).toHaveLength(2);
  });

  it("只写到第一个义项时，只返回这一条", () => {
    const cut = SAMPLE.indexOf("},", SAMPLE.indexOf("收费")) + 1;
    const parsed = tryParsePartial(SAMPLE.slice(0, cut));
    expect(parsed?.senses).toHaveLength(1);
    expect((parsed?.senses[0] as { gloss_zh: string }).gloss_zh).toBe("收费");
  });

  it("半个字符串里的义项不算数", () => {
    const cut = SAMPLE.indexOf("He was charged") + 5;
    const parsed = tryParsePartial(SAMPLE.slice(0, cut));
    expect(parsed?.senses).toHaveLength(1);
  });

  it("什么都没有时返回 null 而不是抛异常", () => {
    expect(tryParsePartial("")).toBeNull();
    expect(tryParsePartial("{")).toBeNull();
    expect(tryParsePartial("好的")).toBeNull();
  });

  it("永远不抛异常（随机切片 200 次）", () => {
    for (let index = 0; index < 200; index += 1) {
      const cut = Math.floor(Math.random() * SAMPLE.length);
      expect(() => tryParsePartial(SAMPLE.slice(0, cut))).not.toThrow();
    }
  });
});

describe("parseFinal", () => {
  it("能解析带围栏的最终输出", () => {
    const parsed = parseFinal("```json\n" + SAMPLE + "\n```")?.value as { word: string };
    expect(parsed.word).toBe("charge");
  });

  it("坏到无法修复时返回 null", () => {
    expect(parseFinal("完全不是 JSON")).toBeNull();
  });

  it("被截断的输出要标记 repaired，不能当成完整结果", () => {
    const cut = SAMPLE.lastIndexOf("},");
    const parsed = parseFinal(SAMPLE.slice(0, cut));
    expect(parsed?.repaired).toBe(true);
  });

  it("完整输出不标记 repaired", () => {
    expect(parseFinal(SAMPLE)?.repaired).toBe(false);
  });
});
