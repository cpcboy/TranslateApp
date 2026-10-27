import { beforeEach, describe, expect, it, vi } from "vitest";

// IPC 换成假的：store 的状态机是真代码，只有「Rust 那一侧」是模拟的
const mocks = vi.hoisted(() => ({
  handlers: new Map<string, (event: { payload: unknown }) => void>(),
  translateText: vi.fn(),
  lookup: vi.fn(),
  cancelTranslate: vi.fn(async () => {}),
  invalidateLookup: vi.fn(async () => {}),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    mocks.handlers.set(name, handler);
    return () => mocks.handlers.delete(name);
  }),
}));

vi.mock("./ipc", () => ({
  api: {
    translateText: mocks.translateText,
    lookup: mocks.lookup,
    cancelTranslate: mocks.cancelTranslate,
    invalidateLookup: mocks.invalidateLookup,
  },
  onDelta: vi.fn(async (handler: (payload: unknown) => void) => {
    mocks.handlers.set("sense://delta", (event) => handler(event.payload));
    return () => mocks.handlers.delete("sense://delta");
  }),
}));

import { describeFailure, useLookup } from "./store";

const emitLong = (requestId: string, text: string) =>
  mocks.handlers.get("sense://longtext-delta")?.({ payload: { request_id: requestId, text } });

beforeEach(() => {
  vi.clearAllMocks();
  mocks.handlers.clear();
  useLookup.setState({ longBusy: false });
  useLookup.getState().reset();
});

describe("长文：失败与不完整必须明说", () => {
  it("中途断开：还在合流缓冲里的最后一段也上屏，并进入错误态", async () => {
    mocks.translateText.mockImplementation(async (_text: string, requestId: string) => {
      emitLong(requestId, "第一段");
      emitLong(requestId, "第二段"); // 60ms 合流还没到，这两段都在缓冲里
      throw "传输中途断开，连接在回复写完前断了";
    });
    await useLookup.getState().runLong("x".repeat(80));
    const state = useLookup.getState();
    expect(state.longText).toBe("第一段第二段");
    expect(state.status).toBe("error");
    expect(state.error?.message).toContain("传输中途断开");
  });

  it("服务端提前收工（没报错）：原因带到界面", async () => {
    mocks.translateText.mockResolvedValue({ text: "半篇", truncated: false, incomplete: "模型服务以「内容策略」中止了输出" });
    await useLookup.getState().runLong("x".repeat(80));
    expect(useLookup.getState().longIncomplete).toContain("内容策略");
  });

  it("离开阅读视图时，把还在生成的长文停掉", () => {
    useLookup.setState({ longBusy: true, longSource: "x" });
    useLookup.getState().reset();
    expect(mocks.cancelTranslate).toHaveBeenCalledTimes(1);
  });

  it("没有在生成时不发多余的停止", () => {
    useLookup.getState().reset();
    expect(mocks.cancelTranslate).not.toHaveBeenCalled();
  });
});

describe("查词：重试后仍不合格", () => {
  it("错误文案走 describeFailure：说「不符合词典格式」，不说中断", async () => {
    mocks.lookup.mockResolvedValue({
      raw: "抱歉，不能回答。",
      source: "network",
      model: "m",
      first_delta_ms: 1,
      total_ms: 2,
      finish_reason: "stop",
    });
    await useLookup.getState().run("refuse");
    const state = useLookup.getState();
    expect(mocks.lookup).toHaveBeenCalledTimes(2); // 先 0.3，校验失败再 0.2 重试一次
    expect(state.status).toBe("error");
    expect(state.error?.message).toContain("不符合词典格式");
    expect(state.error?.message).not.toContain("中断");
  });

  it("两次都不合格：两次都把缓存标废，下次不会再命中坏数据", async () => {
    mocks.lookup.mockResolvedValue({
      raw: "抱歉，不能回答。",
      source: "network",
      model: "m",
      first_delta_ms: 1,
      total_ms: 2,
      finish_reason: "stop",
    });
    await useLookup.getState().run("refuse");
    // 一次在重试之前，一次在最终放弃之后（Rust 会把重试的结果也写进库）
    expect(mocks.invalidateLookup).toHaveBeenCalledTimes(2);
    expect(mocks.invalidateLookup).toHaveBeenLastCalledWith("refuse", "en");
  });

  it("重试换新的 requestId：第一轮迟到的增量混不进第二轮", async () => {
    const emitDelta = (request_id: string, text: string) =>
      mocks.handlers.get("sense://delta")?.({ payload: { request_id, text } });
    const ids: string[] = [];
    mocks.lookup.mockImplementation(async (args: { requestId: string; temperature: number }) => {
      ids.push(args.requestId);
      if (args.temperature !== 0.3) {
        emitDelta(ids[0], '{"word":"迟到的旧增量"'); // 第一轮的尾巴，现在才到
        await new Promise((resolve) => setTimeout(resolve, 100)); // 超过 66ms 合流窗口
      }
      return { raw: "坏的", source: "network", model: "m", first_delta_ms: 1, total_ms: 2, finish_reason: "stop" };
    });
    await useLookup.getState().run("refuse");
    // 断言放在 mock 外面：mock 里抛错会被 store 的 catch 吞掉，测试就成了摆设
    expect(ids).toHaveLength(2);
    expect(ids[1]).not.toBe(ids[0]);
    expect(useLookup.getState().buffer).not.toContain("迟到的旧增量");
  });
});

describe("describeFailure：重试后仍校验失败时的说明", () => {
  it("模型正常写完（stop）却不合格：说「不符合词典格式」并带上原因，不能说成中断", () => {
    const message = describeFailure(
      { raw: '{"word":"round","pos":["adj","n","prep","adv","v"]}', finish_reason: "stop" },
      "pos：超过 4 项",
    );
    expect(message).toContain("不符合词典格式");
    expect(message).toContain("pos：超过 4 项");
    expect(message).not.toContain("中断");
  });

  it("模型拒答、给了一段话：原文要露出来", () => {
    const message = describeFailure({ raw: "抱歉，我不能回答这个问题。", finish_reason: "stop" }, "不是合法的 JSON");
    expect(message).toContain("不是合法的 JSON");
    expect(message).toContain("「抱歉，我不能回答这个问题。」");
  });

  it("拿不到结束原因时才说可能在中途被截断", () => {
    const message = describeFailure({ raw: '{"word":"charge","senses":[', finish_reason: null }, "senses：少于 1 项");
    expect(message).toContain("可能在中途被截断");
  });

  it("服务端以别的原因收工：如实说出原因，不说成格式问题", () => {
    const message = describeFailure({ raw: '{"word":"x"', finish_reason: "insufficient_system_resource" }, "senses：缺少这个字段");
    expect(message).toContain("insufficient_system_resource");
    expect(message).not.toContain("不符合词典格式");
  });

  it("内容策略与长度上限各有说法", () => {
    expect(describeFailure({ raw: "{", finish_reason: "content_filter" }, "x")).toContain("内容策略");
    expect(describeFailure({ raw: "{", finish_reason: "length" }, "x")).toContain("长度上限");
  });

  it("空内容单独说明", () => {
    expect(describeFailure({ raw: "  ", finish_reason: "stop" }, "x")).toBe("模型返回了空内容，已自动重试一次。");
  });

  it("原文只在超过 180 字时才截断并加省略号", () => {
    expect(describeFailure({ raw: "短", finish_reason: "stop" }, "x")).toContain("「短」");
    const long = describeFailure({ raw: "长".repeat(300), finish_reason: "stop" }, "x");
    expect(long).toContain(`「${"长".repeat(180)}…」`);
  });
});
