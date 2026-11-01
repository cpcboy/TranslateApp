import { describe, expect, it } from "vitest";
import { isLongText, routeOf } from "./route";

describe("isLongText：查词还是翻译", () => {
  it("一个词、词组、成语都留在词典", () => {
    for (const word of [
      "charge",
      "落实",
      "take it for granted",
      "in the long run",
      "once in a blue moon",
      "the straw that broke the camel's back",
      "说曹操曹操到",
      "一寸光阴一寸金",
      "AI 助手",
      "Mr. Smith",
      "e.g.",
      "3.5mm 接口",
      "什么？",
      "Dr. John Smith",
      "Ph.D. in physics",
      "U.S. Department of State",
      "画蛇添足。",
      "一石二鸟！",
      "怎么回事？",
      "1,000,000 美元左右",
      "J. K. Rowling",
      "Harry S. Truman",
      "Smith and Co.",
      "Apples, oranges, etc.",
      "Johnson & Johnson Inc.",
      "a.m. to p.m.",
      "Sen. Elizabeth Warren",
      "Gov. John Doe",
      "Capt. Jack Sparrow",
      "Mt. Everest National Park",
      "Lt. Col. Smith",
      "Jan. 5 meeting",
      "说曹操，曹操到",
      "说曹操曹操到。",
      "一寸光阴一寸金。",
      "天无绝人之路。",
      "塞翁失马，焉知非福",
      "苹果、香蕉",
      "「画蛇添足。」",
      "“画蛇添足。”",
      "乔治・华盛顿",
    ]) {
      expect(isLongText(word), word).toBe(false);
    }
  });

  it("30 个字符的英文句子、中文句子是翻译，不是词典", () => {
    expect(isLongText("How are you doing today?")).toBe(true);
    expect(isLongText("He go to the office.")).toBe(true);
    expect(isLongText("I think we should leave. It is late")).toBe(true);
    expect(isLongText("我今天不太想去上班，感觉有点累")).toBe(true);
    expect(isLongText("今天天气很好，我们一起出去玩吧")).toBe(true); // 11 个字、带逗号
    expect(isLongText("这是一个完整的句子。")).toBe(true);
    expect(isLongText("请你明天上午九点前把这份材料交给我们")).toBe(true);
  });

  it("问号、感叹号、数字、小数、Node.js 后面一定是下一句，缩写规则只管句号", () => {
    for (const sentence of [
      "Did you say no? Then leave",
      "I said no! Go away now",
      "We met in 2020. It was great",
      "Take plan B. Then wait here",
      "Pi is 3.14. It is irrational",
      "I use Node.js. It is fast",
      "Wait... what is that",
      "Version 2.0. Release notes follow",
    ]) {
      expect(isLongText(sentence), sentence).toBe(true);
    }
  });

  it("中英混排也要按英文那部分判句子", () => {
    expect(isLongText("The word 苹果 means apple, right?")).toBe(true);
    expect(isLongText("I love 北京 very much and I will go there soon")).toBe(true);
    expect(isLongText("“I think we should leave.”")).toBe(true); // 弯引号收尾
  });

  it("多行粘贴、日文、韩文一律走翻译", () => {
    expect(isLongText("Hello,\nHow are you")).toBe(true);
    expect(isLongText("ありがとうございます")).toBe(true);
    expect(isLongText("안녕하세요")).toBe(true);
  });

  it("短语后面加了句末标点：当成句子（去掉标点才是查词）", () => {
    expect(isLongText("take it for granted.")).toBe(true);
    expect(isLongText("take it for granted")).toBe(false);
  });

  it("没有标点的长词组：英文超过 8 个单词才算整句", () => {
    expect(isLongText("one two three four five six seven eight")).toBe(false);
    expect(isLongText("one two three four five six seven eight nine")).toBe(true);
  });

  it("64 个字符的硬线按码点算，与 Rust 的 chars().count() 一致", () => {
    expect(isLongText("a".repeat(64))).toBe(false);
    expect(isLongText("a".repeat(65))).toBe(true);
    // 33 个 emoji = 66 个 UTF-16 单位，但只有 33 个码点：仍是「词」
    expect(isLongText("😀".repeat(33))).toBe(false);
    expect(isLongText("😀".repeat(65))).toBe(true);
  });

  it("空白不算内容", () => {
    expect(isLongText("   ")).toBe(false);
    expect(isLongText("  charge  ")).toBe(false);
  });
});

describe("routeOf：查词卡还是普通翻译（在 isLongText 之上再看目标语言）", () => {
  const dictionary = { dictionary: true };
  const translate = { dictionary: false };

  it("整句、长文一律普通翻译，不管 Rust 怎么说", () => {
    for (const plan of [dictionary, translate, null]) {
      expect(routeOf("This is a complete sentence that needs translating.", plan)).toBe("long");
      expect(routeOf("今天天气很好，我们一起去公园散步吧，好不好？", plan)).toBe("long");
      expect(routeOf("a\nb", plan)).toBe("long");
    }
  });

  it("短输入：Rust 说走查词卡就查词（中 ↔ 英，默认方向）", () => {
    expect(routeOf("charge", dictionary)).toBe("word");
    expect(routeOf("落实", dictionary)).toBe("word");
  });

  it("短输入：Rust 说不走（别的语言、或手选了别的目标语言）就普通翻译——一个词也是", () => {
    expect(routeOf("bonjour", translate)).toBe("long");
    expect(routeOf("charge", translate)).toBe("long");
    expect(routeOf("こんにちは", translate)).toBe("long");
  });

  it("拿不到 Rust 的结论（IPC 出错）：按以前的办法，短输入查词典", () => {
    expect(routeOf("charge", null)).toBe("word");
    expect(routeOf("落实", null)).toBe("word");
  });
});
