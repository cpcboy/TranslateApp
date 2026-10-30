import { Fragment } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { makeEntry } from "../../lib/testing";

// store 换成「想让它是什么就是什么」的假 hook：LookupView / ResultCard 本体是真代码。
// 用 react-dom/server 渲染成 HTML 字符串来看画了什么——node 环境不需要 DOM；effect 不会执行，也用不到窗口 API。
const store = vi.hoisted(() => ({ current: {} as Record<string, unknown> }));
vi.mock("../../lib/store", () => ({ useLookup: () => store.current }));

import { LookupView, ResultCard } from "./LookupView";
import { SenseCard } from "./SenseCard";

const idle = () => ({
  status: "idle",
  query: "",
  buffer: "",
  partial: null,
  entry: null,
  warnings: [] as string[],
  untranslated: false,
  error: null,
  meta: null,
  longSource: null,
  longText: "",
  longBusy: false,
  longTruncated: false,
  longIncomplete: null,
  run: vi.fn(),
  runLong: vi.fn(),
  cancelLong: vi.fn(),
  reset: vi.fn(),
});

const render = (over: Record<string, unknown>) => {
  store.current = { ...idle(), ...over };
  return renderToStaticMarkup(<LookupView onOpenSettings={() => {}} />);
};

const BANNER = "没能把「充电」译成英文";

beforeEach(() => {
  store.current = idle();
});

describe("结果卡的树形：预览 → 最终结果必须复用同一个 SenseCard", () => {
  // React 用「同一位置、同一类型」判断要不要复用。预览和最终结果树形不一致，流式一结束 SenseCard 就被卸载重建，
  // 用户已经点开的义项会自己合上（真实出过这个回归）
  const shape = (tree: ReturnType<typeof ResultCard>) => {
    expect(tree?.type).toBe(Fragment);
    const children = (tree?.props as { children: Array<{ type?: unknown } | false | null> }).children;
    expect(children).toHaveLength(2);
    expect(children[1] && (children[1] as { type?: unknown }).type).toBe(SenseCard);
    return children;
  };

  it("预览、最终结果、降级结果：都是 Fragment → [横幅位, SenseCard]", () => {
    shape(ResultCard({ query: "充电", entry: null, preview: makeEntry("recharge"), warnings: [], untranslated: false }));
    shape(ResultCard({ query: "充电", entry: makeEntry("recharge"), preview: null, warnings: [], untranslated: false }));
    shape(ResultCard({ query: "充电", entry: makeEntry("充电", "zh"), preview: null, warnings: [], untranslated: true }));
  });

  it("横幅只在「最终结果 + 没译成」时占那个位置；预览阶段就算 untranslated 也不画", () => {
    const withBanner = shape(
      ResultCard({ query: "充电", entry: makeEntry("充电", "zh"), preview: null, warnings: [], untranslated: true }),
    );
    expect(withBanner[0]).toBeTruthy();
    const preview = shape(
      ResultCard({ query: "充电", entry: null, preview: makeEntry("recharge"), warnings: [], untranslated: true }),
    );
    expect(preview[0]).toBeFalsy();
  });

  it("什么都没有就不画", () => {
    expect(ResultCard({ query: "充电", entry: null, preview: null, warnings: [], untranslated: false })).toBeNull();
  });
});

describe("不许假装成功：没译成英文时，页面上看得见", () => {
  it("降级：横幅在卡片上方，卡片是中文词条，没有「充电 → 英文」那一行", () => {
    const html = render({ status: "done", query: "充电", entry: makeEntry("充电", "zh"), untranslated: true });
    expect(html).toContain(BANNER);
    expect(html).toContain('role="status"');
    expect(html.indexOf(BANNER)).toBeLessThan(html.indexOf("<article"));
    expect(html).not.toContain("译成</span>");
  });

  it("译成了：有「充电 → 英文」那一行，没有横幅", () => {
    const html = render({ status: "done", query: "充电", entry: makeEntry("recharge"), untranslated: false });
    expect(html).not.toContain(BANNER);
    expect(html).toContain("recharge");
    expect(html).toContain("充电");
    expect(html).toContain("译成</span>");
  });

  it("英文输入：既没有横幅，也没有那一行", () => {
    const html = render({ status: "done", query: "charge", entry: makeEntry("charge"), untranslated: false });
    expect(html).not.toContain(BANNER);
    expect(html).not.toContain("译成</span>");
  });

  it("词条标题带完整的词作 title：4 个单词的译词在窄窗口 / 大字号下被截断时，悬停还能看全", () => {
    const html = render({ status: "done", query: "塞翁失马", entry: makeEntry("a blessing in disguise") });
    expect(html).toContain('title="a blessing in disguise"');
  });
});

describe("流式过程中画什么", () => {
  const senses = makeEntry("recharge").senses;

  it("中文输入、预览里的词还没流出来（拿输入顶着）：不画中文卡，显示骨架屏，不闪首页", () => {
    const html = render({ status: "streaming", query: "充电", partial: { senses } });
    expect(html).toContain('aria-busy="true"');
    expect(html).not.toContain("<article");
    expect(html).not.toContain("给它一张完整的义项卡");
  });

  it("中文输入、预览里的词已经是英文：画出来，标「还在加载」，带「充电 → 英文」", () => {
    const html = render({ status: "streaming", query: "充电", partial: { word: "recharge", lang: "en", senses } });
    expect(html).toContain("<article");
    expect(html).toContain("recharge");
    expect(html).toContain("还在加载");
    expect(html).toContain("译成</span>");
    expect(html).not.toContain(BANNER);
  });

  it("中文输入、模型给的预览就是中文词：同样不画", () => {
    const html = render({ status: "streaming", query: "充电", partial: { word: "充电", lang: "zh", senses } });
    expect(html).toContain('aria-busy="true"');
    expect(html).not.toContain("<article");
  });

  it("英文输入：预览照常画", () => {
    const html = render({ status: "streaming", query: "charge", partial: { word: "charge", lang: "en", senses } });
    expect(html).toContain("<article");
    expect(html).toContain("还在加载");
  });

  it("正在连接（loading，还没有任何增量）：骨架屏", () => {
    const html = render({ status: "loading", query: "充电" });
    expect(html).toContain('aria-busy="true"');
  });

  it("空闲：首页，写明中文会先译成英文", () => {
    const html = render({});
    expect(html).toContain("输入中文，会先译成英文，再解析那个英文词");
    expect(html).not.toContain('aria-busy="true"');
  });
});
