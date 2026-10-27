import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { createMock, MARKERS, type MockServer } from "../../tools/mock-provider.mjs";
// 用 Vite 的 ?raw 读 Rust 源码：不引入 Node 类型依赖
import captureRs from "../../src-tauri/src/capture.rs?raw";
import longtextRs from "../../src-tauri/src/commands/longtext.rs?raw";
import readingRs from "../../src-tauri/src/commands/reading.rs?raw";

/**
 * mock 服务靠 system 提示词里的一句话认请求类型。Rust 里的提示词一改、这里没跟上，
 * mock 就悄悄回错数据，无 Key 的端到端验证形同虚设（截屏、长文两条链路就这样失效过）。
 * 所以先逐字对一遍 Rust 源码，再真的打一遍每种请求。
 */
describe("mock-provider 与 Rust 里的提示词保持同步", () => {
  it("每个标记都出现在对应的 Rust 源码里", () => {
    expect(captureRs).toContain(MARKERS.capture);
    expect(longtextRs).toContain(MARKERS.longText);
    expect(readingRs).toContain(MARKERS.grammar);
    expect(readingRs).toContain(MARKERS.explain);
  });
});

let server: MockServer;
let base = "";

beforeAll(async () => {
  server = createMock({ delay: 1, log: null });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  base = `http://127.0.0.1:${typeof address === "object" && address ? address.port : 0}/v1`;
});

afterAll(() => new Promise<void>((resolve) => server.close(() => resolve())));

/** 按真实请求的形状发一次，把 SSE 增量拼成完整文本 */
async function ask(system: string, user: string): Promise<string> {
  const response = await fetch(`${base}/chat/completions`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      model: "mock",
      stream: true,
      messages: [
        { role: "system", content: system },
        { role: "user", content: user },
      ],
    }),
  });
  const raw = await response.text();
  let text = "";
  for (const line of raw.split("\n")) {
    if (!line.startsWith("data: ") || line.includes("[DONE]")) continue;
    text += JSON.parse(line.slice(6)).choices?.[0]?.delta?.content ?? "";
  }
  return text;
}

describe("mock-provider 对每种请求都回对形状的数据", () => {
  it("截屏翻译：user 是字符串数组，回等长数组", async () => {
    const out = JSON.parse(await ask(`${MARKERS.capture}。input 是 JSON 数组`, JSON.stringify(["Hello", "World", "Again"])));
    expect(out).toHaveLength(3);
    expect(out[0]).toContain("译文1");
  });

  it("长文翻译：回纯文本并保留分段，不是 []", async () => {
    const source = "First paragraph.\n\nSecond paragraph.";
    const out = await ask(`${MARKERS.longText}中文。`, source);
    expect(out).not.toBe("[]");
    expect(out.split("\n")).toHaveLength(3);
    expect(out).toContain("【译】First paragraph.");
  });

  it("语法检查：有错的句子返回问题，没错返回 []", async () => {
    const issues = JSON.parse(await ask(MARKERS.grammar, "He go to the office every day."));
    expect(issues[0].fragment).toBe("He go");
    expect(JSON.parse(await ask(MARKERS.grammar, "Everything is fine here."))).toEqual([]);
  });

  it("划词解析：回带 gloss 的对象，词取自请求", async () => {
    const out = JSON.parse(await ask(MARKERS.explain, JSON.stringify({ term: "budget", context: "the budget was cut" })));
    expect(out.gloss).toContain("budget");
  });

  it("查词：卡片里的词就是请求的词", async () => {
    const out = JSON.parse(await ask("# 角色\n词典", JSON.stringify({ word: "round", lang: "en", level: "B2", context: null })));
    expect(out.word).toBe("round");
    expect(out.senses.length).toBeGreaterThan(0);
  });
});
