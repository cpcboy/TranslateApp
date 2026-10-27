import { describe, expect, it } from "vitest";
import type { GrammarIssue } from "../../lib/ipc";
import { findWhole, lastBlockIsOpen, markableSegments, markedCount, shouldCheckGrammar, sliceAround, splitMarked } from "./reading";

const issue = (fragment: string): GrammarIssue => ({ fragment, replacement: "", explanation: "" });
const marked = (pieces: ReturnType<typeof splitMarked>) =>
  pieces.filter((piece) => typeof piece !== "string").map((piece) => (piece as { text: string }).text);

describe("语法标注：只标整词、每条问题只标一处", () => {
  it("fragment 为 go 时，good / ago / going 不能被标上", () => {
    // 前面的 good / ago / going 里都含有 "go"，裸 indexOf 会先命中它们
    const text = "It was a good idea, long ago, and going well. He go to the office.";
    const pieces = splitMarked(text, [issue("go")]);
    expect(marked(pieces)).toEqual(["go"]);
    // 标的是 He 后面那个 go
    const before = pieces.slice(0, pieces.findIndex((piece) => typeof piece !== "string")).join("");
    expect(before.endsWith("He ")).toBe(true);
  });

  it("同一个片段在文中出现多次：只标第一处", () => {
    const pieces = splitMarked("I has a pen. You has a book.", [issue("has")]);
    expect(marked(pieces)).toEqual(["has"]);
  });

  it("前面的段落已经标过的问题，后面的段落不再重复标", () => {
    const used = new Set<number>();
    const issues = [issue("has")];
    expect(marked(splitMarked("I has a pen.", issues, used))).toEqual(["has"]);
    expect(marked(splitMarked("You has a book.", issues, used))).toEqual([]);
  });

  it("多词片段、带标点的片段照常命中", () => {
    expect(marked(splitMarked("He go to school every day.", [issue("He go")]))).toEqual(["He go"]);
    expect(marked(splitMarked("Its a dog, isnt it?", [issue("isnt it?")]))).toEqual(["isnt it?"]);
  });

  it("找不到、空片段都不标，原文原样返回", () => {
    expect(splitMarked("hello world", [issue("xyz"), issue("")])).toEqual(["hello world"]);
  });

  it("两条问题位置重叠：先到先得，不会把文字切乱", () => {
    const pieces = splitMarked("He go to school", [issue("He go"), issue("go to")]);
    expect(marked(pieces)).toEqual(["He go"]);
    expect(pieces.map((piece) => (typeof piece === "string" ? piece : piece.text)).join("")).toBe("He go to school");
  });

  it("findWhole：词边界", () => {
    expect(findWhole("good go", "go")).toBe(5);
    expect(findWhole("ago", "go")).toBe(-1);
    expect(findWhole("go.", "go")).toBe(0);
    expect(findWhole("a-go-go", "go")).toBe(2);
  });
});

describe("语法检查什么时候该发", () => {
  const base = {
    busy: false,
    text: "译文",
    source: "This is a fairly long English paragraph that clearly has more than forty letters in it.",
    error: null,
    incomplete: null,
    checkedFor: "",
  };

  it("译文完整写完才查", () => {
    expect(shouldCheckGrammar(base)).toBe(true);
  });

  it("点了停止（译文不完整）不查：别在放弃的译文上再花一次请求", () => {
    expect(shouldCheckGrammar({ ...base, incomplete: "你点了停止" })).toBe(false);
  });

  it("出错、还在生成、没有译文、同一份原文已经查过都不查", () => {
    expect(shouldCheckGrammar({ ...base, error: "断了" })).toBe(false);
    expect(shouldCheckGrammar({ ...base, busy: true })).toBe(false);
    expect(shouldCheckGrammar({ ...base, text: "" })).toBe(false);
    expect(shouldCheckGrammar({ ...base, checkedFor: base.source })).toBe(false);
  });

  it("中文原文和太短的原文不查", () => {
    expect(shouldCheckGrammar({ ...base, source: "这是一段很长的中文原文，".repeat(10) })).toBe(false);
    expect(shouldCheckGrammar({ ...base, source: "short text" })).toBe(false);
  });
});

describe("划词上下文：以选中的词为中心", () => {
  it("选中词在长段落后半部时，上下文里必须有它", () => {
    const paragraph = `${"filler ".repeat(200)}the budget was cut.`;
    const start = paragraph.indexOf("budget");
    const context = sliceAround(paragraph, start, "budget".length);
    expect(context).toContain("budget");
    expect(context.length).toBeLessThanOrEqual(2 * 200 + "budget".length);
    // 旧做法：取开头 400 字，里面没有这个词
    expect(paragraph.slice(0, 400)).not.toContain("budget");
  });

  it("靠近开头或结尾时不越界", () => {
    expect(sliceAround("short text", 0, 5)).toBe("short text");
    expect(sliceAround("short text", 6, 4)).toBe("short text");
  });
});

describe("流式生成时最后一块先按正文显示", () => {
  it("还在生成、最后一块没收尾：算开着", () => {
    expect(lastBlockIsOpen("标题\n\n正在写的一句", true)).toBe(true);
    expect(lastBlockIsOpen("正在写的一句", true)).toBe(true);
  });

  it("文本以空行结尾（这一块已收尾），或者不是流式：不算开着", () => {
    expect(lastBlockIsOpen("标题\n\n", true)).toBe(false);
    expect(lastBlockIsOpen("标题", false)).toBe(false);
  });
});

describe("标题栏的「N 处标记」数画出来的，不数模型给的条数", () => {
  it("问题在原文里找不到整词位置时不计入", () => {
    // 「go」只出现在 good / ago 里，整词匹配找不到；只有 has 画得出来
    expect(markedCount("A good idea long ago. He has a pen.", [issue("go"), issue("has")])).toBe(1);
  });

  it("多段共用「已标过」：同一条问题只算一处", () => {
    expect(markedCount("I has a pen.\n\nYou has a book.", [issue("has")])).toBe(1);
  });

  it("单行短原文会被渲染成小标题，仍然要能标注", () => {
    expect(markedCount("He go to the school every morning", [issue("He go")])).toBe(1);
  });

  it("列表逐项切分，去掉项目符号", () => {
    expect(markableSegments("- one item\n- another item\n\nPlain paragraph.")).toEqual([
      "one item",
      "another item",
      "Plain paragraph.",
    ]);
  });
});
