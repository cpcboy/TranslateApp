import { beforeEach, describe, expect, it, vi } from "vitest";

// IPC 换成假的：偏好的状态机是真代码，只有「Rust 那一侧」是模拟的
const mocks = vi.hoisted(() => ({
  translateLanguages: vi.fn(),
  setTranslateTarget: vi.fn(),
}));
vi.mock("./ipc", () => ({ api: mocks }));

import { AUTO, AUTO_HINT, confirmedSourceCode, paneTitles, parsePair, pillLabel, useTarget } from "./languages";

const ZH = { code: "zh", name: "中文", native: "简体中文" };
const EN = { code: "en", name: "英文", native: "English" };
const JA = { code: "ja", name: "日语", native: "日本語" };
const LANGUAGES = [ZH, EN, JA];

describe("parsePair：Rust 发来的两端语言", () => {
  it("完整的负载原样整形（多余的字段如 request_id、text 不带进来）", () => {
    expect(
      parsePair({ request_id: "r1", text: "x", source: JA, source_confident: true, target: ZH }),
    ).toEqual({ source: JA, source_confident: true, target: ZH });
  });

  it("认不出原文：source 为 null，没把握的标志也一定是 false", () => {
    expect(parsePair({ source: null, source_confident: true, target: ZH })).toEqual({
      source: null,
      source_confident: false,
      target: ZH,
    });
  });

  it("没把握：保留默认的语言，但 source_confident 是 false", () => {
    expect(parsePair({ source: EN, source_confident: false, target: ZH })?.source_confident).toBe(false);
    // 字段缺了按没把握算，不能默认成"有把握"
    expect(parsePair({ source: EN, target: ZH })?.source_confident).toBe(false);
    expect(parsePair({ source: EN, source_confident: "true", target: ZH })?.source_confident).toBe(false);
  });

  it("畸形的负载整个丢掉（界面退回只写「原文」「译文」），不抛错", () => {
    for (const bad of [
      null,
      undefined,
      "ja",
      42,
      {},
      { source: JA },
      { target: "zh" },
      { target: { code: "zh" } },
      { target: { code: 1, name: "中文", native: "简体中文" } },
    ]) {
      expect(parsePair(bad), JSON.stringify(bad)).toBeNull();
    }
    // 目标好的、原文畸形：只丢原文
    expect(parsePair({ source: { code: "ja" }, source_confident: true, target: ZH })).toEqual({
      source: null,
      source_confident: false,
      target: ZH,
    });
  });

  it("旧版本的长文结果里没有语言字段：整形成 null，store 据此保留事件里已经拿到的", () => {
    expect(parsePair({ text: "译文", truncated: false, incomplete: null })).toBeNull();
  });
});

describe("两栏标题、按钮文字", () => {
  it("有把握就报语种；没把握写「自动识别」，不报一个可能是错的", () => {
    expect(paneTitles({ source: JA, source_confident: true, target: ZH })).toEqual({
      source: "原文 · 日语",
      target: "译文 · 中文",
    });
    expect(paneTitles({ source: EN, source_confident: false, target: ZH }).source).toBe("原文 · 自动识别");
    expect(paneTitles({ source: null, source_confident: false, target: EN })).toEqual({
      source: "原文 · 自动识别",
      target: "译文 · 英文",
    });
  });

  it("还没拿到语言：只写原文、译文", () => {
    expect(paneTitles(null)).toEqual({ source: "原文", target: "译文" });
  });

  it("confirmedSourceCode：只有有把握的才给代码（语法检查靠它）", () => {
    expect(confirmedSourceCode({ source: EN, source_confident: true, target: ZH })).toBe("en");
    expect(confirmedSourceCode({ source: EN, source_confident: false, target: ZH })).toBeNull();
    expect(confirmedSourceCode({ source: null, source_confident: false, target: ZH })).toBeNull();
    expect(confirmedSourceCode(null)).toBeNull();
  });

  it("pillLabel：auto 写「自动」，选了的写语言名，认不得的退回「自动」", () => {
    expect(pillLabel(AUTO, LANGUAGES)).toBe("自动");
    expect(pillLabel("ja", LANGUAGES)).toBe("日语");
    expect(pillLabel("zh", LANGUAGES)).toBe("中文");
    expect(pillLabel("klingon", LANGUAGES)).toBe("自动");
    expect(pillLabel("ja", [])).toBe("自动");
  });

  it("「自动」的说明写明默认方向", () => {
    expect(AUTO_HINT).toContain("中文 → 英文");
    expect(AUTO_HINT).toContain("其它语言 → 中文");
  });
});

describe("目标语言偏好 store", () => {
  /** 一次存盘：返回 settle，由测试决定它什么时候成功、失败 */
  const pendingSaves = () => {
    const saves: Array<{ code: string; ok: () => void; fail: (reason: unknown) => void }> = [];
    mocks.setTranslateTarget.mockImplementation(
      (code: string) =>
        new Promise<string>((resolve, reject) => saves.push({ code, ok: () => resolve(code), fail: reject })),
    );
    return saves;
  };

  beforeEach(() => {
    vi.clearAllMocks();
    useTarget.setState({ pref: AUTO, saved: AUTO, inflight: null, languages: [], error: null });
  });

  it("load：拿到语言表和已存的偏好", async () => {
    mocks.translateLanguages.mockResolvedValue({ target: "ja", languages: LANGUAGES });
    await useTarget.getState().load();
    expect(useTarget.getState().pref).toBe("ja");
    expect(useTarget.getState().saved).toBe("ja");
    expect(useTarget.getState().languages).toEqual(LANGUAGES);
  });

  it("load：偏好不在语言表里（配置被改坏）退回 auto", async () => {
    mocks.translateLanguages.mockResolvedValue({ target: "klingon", languages: LANGUAGES });
    await useTarget.getState().load();
    expect(useTarget.getState().pref).toBe(AUTO);
  });

  it("load 失败（比如不在 Tauri 里）：语言表留空、不抛错——选择器就不出现", async () => {
    mocks.translateLanguages.mockRejectedValue(new Error("no tauri"));
    await expect(useTarget.getState().load()).resolves.toBeUndefined();
    expect(useTarget.getState().languages).toEqual([]);
    expect(useTarget.getState().pref).toBe(AUTO);
  });

  it("load 恰好在存盘进行中返回：取到的是存盘之前的旧值，不能让界面闪回去", async () => {
    const saves = pendingSaves();
    const choosing = useTarget.getState().choose("ja");
    mocks.translateLanguages.mockResolvedValue({ target: "auto", languages: LANGUAGES }); // 旧值
    await useTarget.getState().load();
    expect(useTarget.getState().pref).toBe("ja"); // 仍是刚选的
    expect(useTarget.getState().languages).toEqual(LANGUAGES); // 语言表照常更新
    saves[0].ok();
    expect(await choosing).toBe(true);
    expect(useTarget.getState().pref).toBe("ja");
    expect(useTarget.getState().saved).toBe("ja");
  });

  it("choose：先改界面再存，存下的值以 Rust 返回的为准", async () => {
    const saves = pendingSaves();
    const choosing = useTarget.getState().choose("ja");
    expect(useTarget.getState().pref).toBe("ja"); // 还没存完，界面已经是新的
    saves[0].ok();
    expect(await choosing).toBe(true); // 存成功了：界面可以收起选择器
    expect(mocks.setTranslateTarget).toHaveBeenCalledWith("ja");
    expect(useTarget.getState().pref).toBe("ja");
    expect(useTarget.getState().saved).toBe("ja");
    expect(useTarget.getState().inflight).toBeNull();
    expect(useTarget.getState().error).toBeNull();
  });

  it("choose 同一个（没有存盘在进行）：什么也不做，不白发一次 IPC", async () => {
    useTarget.setState({ pref: "ja", saved: "ja" });
    expect(await useTarget.getState().choose("ja")).toBe(true); // 已经是它了，算选好
    expect(mocks.setTranslateTarget).not.toHaveBeenCalled();
  });

  it("存盘进行中再点同一个语言：等同一次存盘的结果，不抢着说「选好了」，也不再发一次", async () => {
    const saves = pendingSaves();
    const first = useTarget.getState().choose("ja");
    const again = useTarget.getState().choose("ja"); // 双击
    expect(mocks.setTranslateTarget).toHaveBeenCalledTimes(1);
    saves[0].fail("Error: 写不进去");
    // 两次都得到同一个结果：失败。第二次不能因为"界面上已经是 ja"就先返回 true、把选择器收起来
    expect(await first).toBe(false);
    expect(await again).toBe(false);
    expect(useTarget.getState().pref).toBe(AUTO);
    expect(useTarget.getState().error).toBe("写不进去");
  });

  it("choose 存失败：退回最近一次确认存下的值，并把原因留给界面说出来（不假装选上了）", async () => {
    useTarget.setState({ pref: "en", saved: "en" });
    mocks.setTranslateTarget.mockRejectedValue("Error: 写 settings.json 失败");
    expect(await useTarget.getState().choose("ja")).toBe(false); // 没存上：选择器要留着，把原因给人看
    expect(useTarget.getState().pref).toBe("en");
    expect(useTarget.getState().error).toBe("写 settings.json 失败");
    // 下一次成功的选择把错误清掉
    mocks.setTranslateTarget.mockResolvedValue("fr");
    await useTarget.getState().choose("fr");
    expect(useTarget.getState().error).toBeNull();
  });

  it("连选两个、存盘都失败：退回的是最近一次真正存下的（auto），不是中间点过的 ja", async () => {
    const saves = pendingSaves();
    const first = useTarget.getState().choose("ja");
    const second = useTarget.getState().choose("fr");
    saves[0].fail("第一次失败");
    saves[1].fail("第二次失败");
    expect(await first).toBe(false);
    expect(await second).toBe(false);
    expect(useTarget.getState().pref).toBe(AUTO); // ja 从没存上，界面不能停在 ja
    expect(useTarget.getState().error).toBe("第二次失败");
  });

  it("连选两个：先发的存成功了、后发的失败：退回先发的那个（盘上实际是它）", async () => {
    const saves = pendingSaves();
    const first = useTarget.getState().choose("ja");
    const second = useTarget.getState().choose("fr");
    saves[0].ok();
    expect(await first).toBe(false); // 被后来的取代了，收起选择器不归它
    saves[1].fail("后发的失败");
    expect(await second).toBe(false);
    expect(useTarget.getState().saved).toBe("ja");
    expect(useTarget.getState().pref).toBe("ja");
  });

  it("连选两个：先发的那次成功得晚，不能把界面改回它，也不去收起选择器（收起归后选的那次）", async () => {
    const saves = pendingSaves();
    const first = useTarget.getState().choose("ja");
    const second = useTarget.getState().choose("fr");
    saves[1].ok();
    expect(await second).toBe(true);
    saves[0].ok(); // 先发的 ja 现在才存完：界面已经是 fr，不能被改回 ja
    expect(await first).toBe(false);
    expect(useTarget.getState().pref).toBe("fr");
    expect(useTarget.getState().saved).toBe("fr"); // 序号小的后到，不能把序号大的已经存下的值盖掉
    expect(useTarget.getState().error).toBeNull();
  });

  it("连选两个：先发的那次失败，不能把后选的冲掉", async () => {
    const saves = pendingSaves();
    const first = useTarget.getState().choose("ja");
    const second = useTarget.getState().choose("fr");
    expect(useTarget.getState().pref).toBe("fr");
    saves[1].ok();
    expect(await second).toBe(true);
    saves[0].fail("慢了一步的失败");
    expect(await first).toBe(false); // 被后来的取代了：收起选择器归后来那次
    expect(useTarget.getState().pref).toBe("fr");
    expect(useTarget.getState().error).toBeNull();
  });

  it("存盘调用同步抛错（不是返回一个失败的 promise）也按失败处理，不留下「正在存」的状态", async () => {
    mocks.setTranslateTarget.mockImplementation(() => {
      throw new Error("同步抛错");
    });
    expect(await useTarget.getState().choose("ja")).toBe(false);
    expect(useTarget.getState().inflight).toBeNull();
    expect(useTarget.getState().pref).toBe(AUTO);
    expect(useTarget.getState().error).toBe("同步抛错");
  });

  it("clearError：打开选择器时把上一回的失败提示清掉", async () => {
    mocks.setTranslateTarget.mockRejectedValue("存不上");
    await useTarget.getState().choose("ja");
    expect(useTarget.getState().error).toBe("存不上");
    useTarget.getState().clearError();
    expect(useTarget.getState().error).toBeNull();
  });
});
