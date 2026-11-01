import { beforeEach, describe, expect, it, vi } from "vitest";

// IPC 换成假的：store 的状态机是真代码，只有「Rust 那一侧」是模拟的
const mocks = vi.hoisted(() => ({
  handlers: new Map<string, (event: { payload: unknown }) => void>(),
  /** 非 null 时，订阅语言事件那一下要等它：用来模拟「订阅期间用户已经走开了」 */
  holdLangListen: null as Promise<void> | null,
  /** 非 null 时，订阅查词增量那一下要等它 */
  holdDeltaListen: null as Promise<void> | null,
  /** 为 true 时，订阅语言事件会失败（模拟事件系统出问题） */
  rejectLangListen: false,
  translateText: vi.fn(),
  planTranslation: vi.fn(),
  lookup: vi.fn(),
  cancelTranslate: vi.fn(async () => {}),
  invalidateLookup: vi.fn(async () => {}),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    if (name === "sense://longtext-lang" && mocks.holdLangListen) await mocks.holdLangListen;
    if (name === "sense://longtext-lang" && mocks.rejectLangListen) throw new Error("事件系统不可用");
    mocks.handlers.set(name, handler);
    return () => mocks.handlers.delete(name);
  }),
}));

vi.mock("./ipc", () => ({
  api: {
    translateText: mocks.translateText,
    planTranslation: mocks.planTranslation,
    lookup: mocks.lookup,
    cancelTranslate: mocks.cancelTranslate,
    invalidateLookup: mocks.invalidateLookup,
  },
  onDelta: vi.fn(async (handler: (payload: unknown) => void) => {
    if (mocks.holdDeltaListen) await mocks.holdDeltaListen;
    mocks.handlers.set("sense://delta", (event) => handler(event.payload));
    return () => mocks.handlers.delete("sense://delta");
  }),
}));

import { describeFailure, routeInput, useLookup } from "./store";
import { makeEntry } from "./testing";

const emitLong = (requestId: string, text: string) =>
  mocks.handlers.get("sense://longtext-delta")?.({ payload: { request_id: requestId, text } });
const emitLang = (payload: unknown) => mocks.handlers.get("sense://longtext-lang")?.({ payload });

beforeEach(() => {
  vi.clearAllMocks();
  mocks.handlers.clear();
  mocks.holdLangListen = null;
  mocks.holdDeltaListen = null;
  mocks.rejectLangListen = false;
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

  it("点停止：后端还没返回时就已经是「不完整」，语法检查据此不发", async () => {
    let finish: (value: { text: string; truncated: boolean; incomplete: string | null }) => void = () => {};
    mocks.translateText.mockImplementation(
      () => new Promise((resolve) => (finish = resolve)),
    );
    const running = useLookup.getState().runLong("x".repeat(80));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useLookup.getState().longBusy).toBe(true);

    await useLookup.getState().cancelLong();
    // 后端的 translate_text 还没返回：longBusy 已经 false，此刻必须已经标了「不完整」
    expect(useLookup.getState().longBusy).toBe(false);
    expect(useLookup.getState().longIncomplete).toBe("你点了停止");
    expect(mocks.cancelTranslate).toHaveBeenCalledTimes(1);

    finish({ text: "半篇", truncated: false, incomplete: "你点了停止" });
    await running;
    expect(useLookup.getState().longIncomplete).toBe("你点了停止");
    expect(useLookup.getState().longText).toBe("半篇");
  });

  it("已经结束的译文：迟到的「停止」不能把它标成不完整", async () => {
    mocks.translateText.mockResolvedValue({ text: "完整的译文", truncated: false, incomplete: null });
    await useLookup.getState().runLong("x".repeat(80));
    expect(useLookup.getState().longBusy).toBe(false);

    await useLookup.getState().cancelLong(); // 按钮还没来得及消失就被点中
    expect(useLookup.getState().longIncomplete).toBeNull();
    expect(mocks.cancelTranslate).not.toHaveBeenCalled();
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

/** 最小的合法词条 JSON：只关心 word / lang */
const rawEntry = (word: string, lang: "en" | "zh" = "en") => JSON.stringify(makeEntry(word, lang));

const reply = (raw: string, source: "cache" | "network" = "network") => ({
  raw,
  source,
  model: "m",
  first_delta_ms: 1,
  total_ms: 2,
  finish_reason: "stop",
});

describe("查词：中文输入先译成英文，再解析那个英文词", () => {
  it("拿回英文词条：一次就成，卡片是英文词，没有警告", async () => {
    mocks.lookup.mockResolvedValue(reply(rawEntry("recharge")));
    await useLookup.getState().run("充电");
    const state = useLookup.getState();
    expect(mocks.lookup).toHaveBeenCalledTimes(1);
    expect(state.status).toBe("done");
    expect(state.entry?.word).toBe("recharge");
    expect(state.entry?.lang).toBe("en");
    expect(state.query).toBe("充电"); // 界面据此显示「充电 → 英文」
    expect(state.warnings).toEqual([]);
    expect(state.untranslated).toBe(false);
    expect(mocks.invalidateLookup).not.toHaveBeenCalled();
  });

  it("命中缓存的英文词条同样直接用，不重试", async () => {
    mocks.lookup.mockResolvedValue(reply(rawEntry("implement"), "cache"));
    await useLookup.getState().run("落实");
    expect(mocks.lookup).toHaveBeenCalledTimes(1);
    expect(useLookup.getState().meta?.source).toBe("cache");
  });

  it("第一次模型还是给了中文词条：当校验失败，0.2 重试并跳过缓存，拿到英文就用英文", async () => {
    mocks.lookup
      .mockResolvedValueOnce(reply(rawEntry("充电", "zh")))
      .mockResolvedValueOnce(reply(rawEntry("recharge")));
    await useLookup.getState().run("充电");
    const state = useLookup.getState();

    expect(mocks.lookup).toHaveBeenCalledTimes(2);
    expect(mocks.lookup.mock.calls[0][0]).toMatchObject({ word: "充电", temperature: 0.3 });
    expect(mocks.lookup.mock.calls[1][0]).toMatchObject({ word: "充电", temperature: 0.2, bypassCache: true });
    // 第一次那个中文词条已经被 Rust 写进缓存了：必须标废
    expect(mocks.invalidateLookup).toHaveBeenCalledWith("充电", "zh");
    expect(state.status).toBe("done");
    expect(state.entry?.word).toBe("recharge");
    expect(state.warnings).toEqual([]);
    expect(state.untranslated).toBe(false);
  });

  it("重试还是中文词条：降级展示，明说没译成英文，不当成正常结果，也不留在缓存里", async () => {
    mocks.lookup.mockResolvedValue(reply(rawEntry("充电", "zh")));
    await useLookup.getState().run("充电");
    const state = useLookup.getState();

    expect(mocks.lookup).toHaveBeenCalledTimes(2);
    expect(state.status).toBe("done"); // 有内容可看，不是错误页
    expect(state.entry?.word).toBe("充电");
    expect(state.untranslated).toBe(true); // 界面据此在卡片上方明说没译成
    // 重试之前一次，接受降级结果之后一次：下次再查要重新问模型，而不是直接拿到这张降级卡
    expect(mocks.invalidateLookup).toHaveBeenCalledTimes(2);
    expect(mocks.invalidateLookup).toHaveBeenLastCalledWith("充电", "zh");
  });

  it("词的语言写对了、词本身还是汉字（lang: en, word: 充电）：同样算没译成", async () => {
    mocks.lookup.mockResolvedValue(reply(rawEntry("充电", "en")));
    await useLookup.getState().run("充电");
    expect(mocks.lookup).toHaveBeenCalledTimes(2);
    expect(useLookup.getState().untranslated).toBe(true);
  });

  it("词是英文、lang 却照抄成了 zh：一次就成，不重试，lang 归一成 en，没有降级标志", async () => {
    mocks.lookup.mockResolvedValue(reply(rawEntry("recharge", "zh")));
    await useLookup.getState().run("充电");
    const state = useLookup.getState();
    expect(mocks.lookup).toHaveBeenCalledTimes(1);
    expect(state.status).toBe("done");
    expect(state.entry?.word).toBe("recharge");
    expect(state.entry?.lang).toBe("en");
    expect(state.untranslated).toBe(false);
    expect(mocks.invalidateLookup).not.toHaveBeenCalled();
  });

  it("词里带着中文（WeChat (微信)）：当没译成，重试后降级", async () => {
    mocks.lookup.mockResolvedValue(reply(rawEntry("WeChat (微信)", "en")));
    await useLookup.getState().run("微信");
    expect(mocks.lookup).toHaveBeenCalledTimes(2);
    expect(useLookup.getState().untranslated).toBe(true);
  });

  it("降级路径里等「标废」的时候用户改查了别的词：旧结果不能覆盖新查询", async () => {
    mocks.lookup.mockImplementation(async (args: { word: string }) =>
      args.word === "充电" ? reply(rawEntry("充电", "zh")) : reply(rawEntry("charge")),
    );
    let calls = 0;
    let release: () => void = () => {};
    mocks.invalidateLookup.mockImplementation(async () => {
      calls += 1;
      if (calls === 2) await new Promise<void>((resolve) => (release = resolve)); // 降级后的那次标废卡住
    });

    const first = useLookup.getState().run("充电");
    await vi.waitFor(() => expect(calls).toBe(2));
    await useLookup.getState().run("charge"); // 用户改查别的词
    release();
    await first;

    const state = useLookup.getState();
    expect(state.query).toBe("charge");
    expect(state.entry?.word).toBe("charge");
    expect(state.untranslated).toBe(false);
  });

  it("降级之后再查英文词、再查一个会失败的词：untranslated 都不残留", async () => {
    mocks.lookup.mockResolvedValue(reply(rawEntry("充电", "zh")));
    await useLookup.getState().run("充电");
    expect(useLookup.getState().untranslated).toBe(true);

    mocks.lookup.mockResolvedValue(reply(rawEntry("charge")));
    await useLookup.getState().run("charge");
    expect(useLookup.getState().untranslated).toBe(false);

    mocks.lookup.mockResolvedValue(reply("抱歉"));
    await useLookup.getState().run("refuse");
    expect(useLookup.getState().status).toBe("error");
    expect(useLookup.getState().untranslated).toBe(false);
  });

  it("缓存里留着没标废的中文词条（上一轮中途被打断）：照样重试，不当正常结果", async () => {
    mocks.lookup
      .mockResolvedValueOnce(reply(rawEntry("充电", "zh"), "cache"))
      .mockResolvedValueOnce(reply(rawEntry("recharge")));
    await useLookup.getState().run("充电");
    const state = useLookup.getState();
    expect(mocks.lookup).toHaveBeenCalledTimes(2);
    expect(state.entry?.word).toBe("recharge");
    expect(state.meta?.source).toBe("network");
    expect(state.untranslated).toBe(false);
  });

  it("重试那次请求失败（网络断了）：进错误态，不展示第一次的中文词条，也不带降级标志", async () => {
    mocks.lookup
      .mockResolvedValueOnce(reply(rawEntry("充电", "zh")))
      .mockRejectedValueOnce({ code: "network", message: "连不上" });
    await useLookup.getState().run("充电");
    const state = useLookup.getState();
    expect(state.status).toBe("error");
    expect(state.entry).toBeNull();
    expect(state.untranslated).toBe(false);
    expect(state.error?.code).toBe("network");
  });

  it("降级之后重置：untranslated 立刻清掉，不留到下一次", async () => {
    mocks.lookup.mockResolvedValue(reply(rawEntry("充电", "zh")));
    await useLookup.getState().run("充电");
    expect(useLookup.getState().untranslated).toBe(true);

    useLookup.getState().reset();
    expect(useLookup.getState().untranslated).toBe(false);
    expect(useLookup.getState().entry).toBeNull();
  });

  it("降级之后再查别的词：新查询一开始（还在加载）就清掉降级标志", async () => {
    mocks.lookup.mockResolvedValue(reply(rawEntry("充电", "zh")));
    await useLookup.getState().run("充电");
    expect(useLookup.getState().untranslated).toBe(true);

    let release: (value: ReturnType<typeof reply>) => void = () => {};
    mocks.lookup.mockImplementation(() => new Promise((resolve) => (release = resolve)));
    const next = useLookup.getState().run("charge");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useLookup.getState().status).toBe("loading");
    expect(useLookup.getState().untranslated).toBe(false);

    release(reply(rawEntry("charge")));
    await next;
    expect(useLookup.getState().untranslated).toBe(false);
    expect(useLookup.getState().entry?.word).toBe("charge");
  });

  it("英文输入不受影响：词条怎么回都不触发中译英的重试", async () => {
    mocks.lookup.mockResolvedValue(reply(rawEntry("charge")));
    await useLookup.getState().run("charge");
    expect(mocks.lookup).toHaveBeenCalledTimes(1);
    expect(useLookup.getState().warnings).toEqual([]);
    expect(useLookup.getState().untranslated).toBe(false);
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

const ZH = { code: "zh", name: "中文", native: "简体中文" };
const EN = { code: "en", name: "英文", native: "English" };
const JA = { code: "ja", name: "日语", native: "日本語" };

describe("长文：原文语言与目标语言", () => {
  it("翻译一开始 Rust 发来的语言事件就上屏——不用等译文写完", async () => {
    let seen: unknown = null;
    mocks.translateText.mockImplementation(async (_text: string, requestId: string) => {
      emitLang({ request_id: requestId, source: JA, source_confident: true, target: ZH });
      seen = useLookup.getState().longLang; // 此刻译文还没返回
      return { text: "你好", truncated: false, incomplete: null };
    });
    await useLookup.getState().runLong("こんにちは".repeat(20));
    expect(seen).toEqual({ source: JA, source_confident: true, target: ZH });
  });

  it("结果里带的语言也认（事件丢了也不影响），两边一致", async () => {
    mocks.translateText.mockResolvedValue({
      text: "Hello",
      truncated: false,
      incomplete: null,
      source: JA,
      source_confident: true,
      target: EN,
    });
    await useLookup.getState().runLong("こんにちは".repeat(20));
    expect(useLookup.getState().longLang).toEqual({ source: JA, source_confident: true, target: EN });
  });

  it("旧版返回里没有语言字段：保留事件里已经拿到的，不被清掉", async () => {
    mocks.translateText.mockImplementation(async (_text: string, requestId: string) => {
      emitLang({ request_id: requestId, source: JA, source_confident: true, target: ZH });
      return { text: "你好", truncated: false, incomplete: null };
    });
    await useLookup.getState().runLong("こんにちは".repeat(20));
    expect(useLookup.getState().longLang?.target).toEqual(ZH);
  });

  it("翻译失败（断流）：两栏标题的语言仍在，因为事件在请求发出之前就到了", async () => {
    mocks.translateText.mockImplementation(async (_text: string, requestId: string) => {
      emitLang({ request_id: requestId, source: EN, source_confident: false, target: ZH });
      throw "没填 API Key";
    });
    await useLookup.getState().runLong("x".repeat(80));
    const state = useLookup.getState();
    expect(state.status).toBe("error");
    expect(state.longLang).toEqual({ source: EN, source_confident: false, target: ZH });
  });

  it("别的请求的语言事件、畸形的事件一律不理", async () => {
    mocks.translateText.mockImplementation(async (_text: string, requestId: string) => {
      emitLang({ request_id: "别的请求", source: JA, source_confident: true, target: EN });
      emitLang({ request_id: requestId, target: "不是语言对象" });
      emitLang(null);
      return { text: "x", truncated: false, incomplete: null };
    });
    await useLookup.getState().runLong("x".repeat(80));
    expect(useLookup.getState().longLang).toBeNull();
  });

  it("翻译结束（成功、失败都算）后，两个事件订阅都取消了——不留监听", async () => {
    mocks.translateText.mockResolvedValue({ text: "x", truncated: false, incomplete: null });
    await useLookup.getState().runLong("a".repeat(80));
    expect(mocks.handlers.has("sense://longtext-delta")).toBe(false);
    expect(mocks.handlers.has("sense://longtext-lang")).toBe(false);

    mocks.translateText.mockRejectedValue("断了");
    await useLookup.getState().runLong("b".repeat(80));
    expect(mocks.handlers.has("sense://longtext-delta")).toBe(false);
    expect(mocks.handlers.has("sense://longtext-lang")).toBe(false);
  });

  it("开始新的一篇或离开阅读视图：上一篇的语言清掉，不会标在新原文上", async () => {
    mocks.translateText.mockImplementation(async (_text: string, requestId: string) => {
      emitLang({ request_id: requestId, source: JA, source_confident: true, target: ZH });
      return { text: "x", truncated: false, incomplete: null };
    });
    await useLookup.getState().runLong("a".repeat(80));
    expect(useLookup.getState().longLang).not.toBeNull();
    useLookup.getState().reset();
    expect(useLookup.getState().longLang).toBeNull();
  });

  it("迟到的语言事件（上一篇的请求）不会写到新一篇上", async () => {
    let firstId = "";
    mocks.translateText.mockImplementationOnce(async (_text: string, requestId: string) => {
      firstId = requestId;
      return { text: "x", truncated: false, incomplete: null };
    });
    await useLookup.getState().runLong("a".repeat(80));
    mocks.translateText.mockImplementationOnce(async () => {
      emitLang({ request_id: firstId, source: JA, source_confident: true, target: EN }); // 旧请求的 id
      return { text: "y", truncated: false, incomplete: null };
    });
    await useLookup.getState().runLong("b".repeat(80));
    expect(useLookup.getState().longLang).toBeNull();
  });
});

describe("长文：订阅事件期间被取代", () => {
  it("订阅语言事件的 await 期间用户离开了阅读视图：不再发翻译请求，两个订阅也都取消了", async () => {
    let release: () => void = () => {};
    mocks.holdLangListen = new Promise<void>((resolve) => (release = resolve));
    const running = useLookup.getState().runLong("x".repeat(80)); // 卡在订阅语言事件上
    await new Promise((resolve) => setTimeout(resolve, 0));
    useLookup.getState().reset(); // 用户按了 Esc / 返回
    release();
    await running;
    expect(mocks.translateText).not.toHaveBeenCalled(); // 发出去的译文没人要，还白计费
    expect(mocks.handlers.has("sense://longtext-delta")).toBe(false);
    expect(mocks.handlers.has("sense://longtext-lang")).toBe(false);
    expect(useLookup.getState().longSource).toBeNull(); // 状态也没被旧请求写回去
  });

  it("订阅期间又提交了别的：只有新的那次发请求", async () => {
    let release: () => void = () => {};
    mocks.holdLangListen = new Promise<void>((resolve) => (release = resolve));
    mocks.translateText.mockResolvedValue({ text: "新的", truncated: false, incomplete: null });
    const first = useLookup.getState().runLong("a".repeat(80));
    await new Promise((resolve) => setTimeout(resolve, 0));
    mocks.holdLangListen = null; // 新的一次的订阅不卡
    const second = useLookup.getState().runLong("b".repeat(80));
    release();
    await Promise.all([first, second]);
    expect(mocks.translateText).toHaveBeenCalledTimes(1);
    expect(mocks.translateText.mock.calls[0][0]).toBe("b".repeat(80));
  });
});

describe("长文：语言事件订阅不上", () => {
  it("订阅语言事件失败：不拖垮翻译，译文照常出来（标题只是锦上添花），查词那个订阅也取消了", async () => {
    mocks.rejectLangListen = true;
    mocks.translateText.mockResolvedValue({ text: "译文", truncated: false, incomplete: null });
    await useLookup.getState().runLong("x".repeat(80));
    const state = useLookup.getState();
    expect(mocks.translateText).toHaveBeenCalledTimes(1);
    expect(state.status).toBe("done");
    expect(state.longText).toBe("译文");
    expect(mocks.handlers.has("sense://longtext-delta")).toBe(false);
  });
});

describe("查词：订阅增量期间被取代", () => {
  it("订阅的 await 期间离开了：不再发 lookup，订阅也取消了，状态没被旧请求写回去", async () => {
    let release: () => void = () => {};
    mocks.holdDeltaListen = new Promise<void>((resolve) => (release = resolve));
    const running = useLookup.getState().run("charge");
    await new Promise((resolve) => setTimeout(resolve, 0));
    useLookup.getState().reset(); // 用户按了 Esc / 返回
    release();
    await running;
    expect(mocks.lookup).not.toHaveBeenCalled(); // 发出去的结果没人要，还白计费
    expect(mocks.handlers.has("sense://delta")).toBe(false);
    expect(useLookup.getState().status).toBe("idle");
  });

  it("订阅期间又查了别的词：只有新的那个词发 lookup", async () => {
    let release: () => void = () => {};
    mocks.holdDeltaListen = new Promise<void>((resolve) => (release = resolve));
    mocks.lookup.mockResolvedValue(reply(rawEntry("round")));
    const first = useLookup.getState().run("charge");
    await new Promise((resolve) => setTimeout(resolve, 0));
    mocks.holdDeltaListen = null;
    const second = useLookup.getState().run("round");
    release();
    await Promise.all([first, second]);
    expect(mocks.lookup).toHaveBeenCalledTimes(1);
    expect(mocks.lookup.mock.calls[0][0].word).toBe("round");
  });
});

describe("长文：失败时补上语言", () => {
  const plan = { source: JA, source_confident: true, target: ZH, dictionary: false };

  it("语言事件没到就失败了：问 Rust 补上，标题不退回只写「原文」「译文」", async () => {
    mocks.translateText.mockRejectedValue("没填 API Key");
    mocks.planTranslation.mockResolvedValue(plan);
    await useLookup.getState().runLong("こんにちは".repeat(20));
    const state = useLookup.getState();
    expect(state.status).toBe("error");
    expect(state.longLang).toEqual({ source: JA, source_confident: true, target: ZH });
    expect(mocks.planTranslation).toHaveBeenCalledWith("こんにちは".repeat(20));
  });

  it("事件已经到了：不再多问一次", async () => {
    mocks.translateText.mockImplementation(async (_text: string, requestId: string) => {
      emitLang({ request_id: requestId, source: EN, source_confident: true, target: ZH });
      throw "断了";
    });
    await useLookup.getState().runLong("x".repeat(80));
    expect(mocks.planTranslation).not.toHaveBeenCalled();
  });

  it("补不上（Rust 也失败）：不抛错，标题照旧只写「原文」「译文」", async () => {
    mocks.translateText.mockRejectedValue("断了");
    mocks.planTranslation.mockRejectedValue("没有这个命令");
    await expect(useLookup.getState().runLong("x".repeat(80))).resolves.toBeUndefined();
    expect(useLookup.getState().status).toBe("error");
    expect(useLookup.getState().longLang).toBeNull();
  });

  it("补语言的那一问还没回来、用户已经换了一篇：旧的答案不能写到新的一篇上", async () => {
    let answer: (value: typeof plan) => void = () => {};
    mocks.translateText.mockRejectedValueOnce("断了");
    mocks.planTranslation.mockImplementationOnce(() => new Promise((resolve) => (answer = resolve)));
    const first = useLookup.getState().runLong("a".repeat(80));
    await new Promise((resolve) => setTimeout(resolve, 0));
    mocks.translateText.mockResolvedValue({ text: "新", truncated: false, incomplete: null });
    await useLookup.getState().runLong("b".repeat(80));
    answer(plan);
    await first;
    expect(useLookup.getState().longSource).toBe("b".repeat(80));
    expect(useLookup.getState().longLang).toBeNull();
  });
});

describe("routeInput：查词卡还是普通翻译", () => {
  const plan = (dictionary: boolean) => ({ source: null, source_confident: false, target: ZH, dictionary });

  it("短输入问 Rust：说走查词卡就是 word", async () => {
    mocks.planTranslation.mockResolvedValue(plan(true));
    expect(await routeInput("charge")).toBe("word");
    expect(mocks.planTranslation).toHaveBeenCalledWith("charge");
  });

  it("短输入问 Rust：别的语言、或手选了别的目标语言，一个词也走普通翻译", async () => {
    mocks.planTranslation.mockResolvedValue(plan(false));
    expect(await routeInput("bonjour")).toBe("long");
  });

  it("整句和长文不用问 Rust，直接普通翻译", async () => {
    expect(await routeInput("This is a complete sentence that needs translating.")).toBe("long");
    expect(mocks.planTranslation).not.toHaveBeenCalled();
  });

  it("问 Rust 失败：按以前的办法，短输入查词典", async () => {
    mocks.planTranslation.mockRejectedValue("没有这个命令");
    expect(await routeInput("charge")).toBe("word");
  });

  it("空输入什么也不做", async () => {
    expect(await routeInput("   ")).toBeNull();
    expect(mocks.planTranslation).not.toHaveBeenCalled();
  });

  it("连按两次回车：慢的那次被新的一次取代，返回 null，不会抢在后面开跑", async () => {
    const answers: Array<(value: ReturnType<typeof plan>) => void> = [];
    mocks.planTranslation.mockImplementation(() => new Promise((resolve) => answers.push(resolve)));
    const older = routeInput("charge");
    const newer = routeInput("round");
    answers[1](plan(true)); // 新的先回来
    answers[0](plan(true)); // 老的后回来
    expect(await newer).toBe("word");
    expect(await older).toBeNull();
  });

  it("先提交一个短词、紧接着提交长文：长文是最新的，短词那次作废", async () => {
    let answer: (value: ReturnType<typeof plan>) => void = () => {};
    mocks.planTranslation.mockImplementation(() => new Promise((resolve) => (answer = resolve)));
    const word = routeInput("charge");
    const long = routeInput("This is a complete sentence that needs translating.");
    answer(plan(true));
    expect(await long).toBe("long");
    expect(await word).toBeNull();
  });
});
