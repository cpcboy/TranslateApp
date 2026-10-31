import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { OverlayBlock } from "../../lib/ipc";
import { OverlayBoundary, OverlayFallback, OverlayLayers, describeFailure, isNothingToReplace } from "./TranslateOverlay";
import type { BlockPlan } from "./overlayLayout";

const plan = (overrides: Partial<BlockPlan> = {}): BlockPlan => ({
  cover: { x: 10, y: 20, w: 100, h: 16 },
  bg: "#1f1f1f",
  fg: "#dcdcdc",
  fontSize: 14,
  lineHeight: 18,
  lines: ["新建任务"],
  x: 12,
  y: 19,
  truncated: false,
  ...overrides,
});

const block = (overrides: Partial<OverlayBlock> = {}): OverlayBlock => ({
  text: "新建任务",
  x: 10,
  y: 20,
  w: 100,
  h: 16,
  line_h: 16,
  translated: true,
  ...overrides,
});

const count = (html: string, needle: string) => html.split(needle).length - 1;

describe("OverlayLayers：覆盖层的画层", () => {
  it("每块一个遮盖矩形（用原文的底色填满、不带圆角）+ 一个译文元素（原文的字号和颜色）", () => {
    const html = renderToStaticMarkup(<OverlayLayers blocks={[block()]} plans={[plan()]} />);
    expect(count(html, 'data-role="cover"')).toBe(1);
    expect(count(html, 'data-role="text"')).toBe(1);
    expect(html).toContain("background:#1f1f1f");
    expect(html).toContain("left:10px;top:20px;width:100px;height:16px");
    expect(html).toContain("font-size:14px");
    expect(html).toContain("color:#dcdcdc");
    expect(html).not.toContain("border-radius");
    expect(html).toContain("新建任务");
  });

  it("所有遮盖矩形都画在所有译文之前：相邻两块的遮盖不能压在对方的译文上", () => {
    const blocks = [block(), block({ y: 40 }), block({ y: 60 })];
    const plans = [plan(), plan({ cover: { x: 10, y: 40, w: 100, h: 16 } }), plan({ cover: { x: 10, y: 60, w: 100, h: 16 } })];
    const html = renderToStaticMarkup(<OverlayLayers blocks={blocks} plans={plans} />);
    expect(html.lastIndexOf('data-role="cover"')).toBeLessThan(html.indexOf('data-role="text"'));
    expect(count(html, 'data-role="cover"')).toBe(3);
    expect(count(html, 'data-role="text"')).toBe(3);
  });

  it("译文和原文一样的块不画（遮盖和译文都不画），别的块照常画", () => {
    const blocks = [block({ text: "WorkBuddy", unchanged: true }), block({ text: "新建任务" })];
    const plans = [plan({ lines: ["WorkBuddy"] }), plan({ lines: ["新建任务"] })];
    const html = renderToStaticMarkup(<OverlayLayers blocks={blocks} plans={plans} />);
    expect(count(html, 'data-role="cover"')).toBe(1);
    expect(count(html, 'data-role="text"')).toBe(1);
    expect(html).not.toContain("WorkBuddy");
    expect(html).toContain("新建任务");
  });

  it("多行译文一行一个元素，浏览器不再自己折行", () => {
    const html = renderToStaticMarkup(<OverlayLayers blocks={[block()]} plans={[plan({ lines: ["第一行", "第二行", "第三行"], lineHeight: 20 })]} />);
    expect(html).toContain("white-space:pre");
    expect(count(html, '<div style="height:20px">')).toBe(3); // 每行一个固定行高的元素（外层的 line-height 不算）
    for (const text of ["第一行", "第二行", "第三行"]) expect(html).toContain(text);
  });

  it("被截断的译文如实标出来（data-truncated），不让半截译文冒充完整的", () => {
    const html = renderToStaticMarkup(<OverlayLayers blocks={[block()]} plans={[plan({ lines: ["译文被截…"], truncated: true })]} />);
    expect(html).toContain('data-truncated="true"');
    expect(renderToStaticMarkup(<OverlayLayers blocks={[block()]} plans={[plan()]} />)).not.toContain("data-truncated");
  });

  it("没译成的（失败时画的是识别出的原文）压暗一点，和顶部的失败提示一起，不让原文冒充译文", () => {
    const failed = renderToStaticMarkup(<OverlayLayers blocks={[block({ translated: false })]} plans={[plan()]} />);
    expect(failed).toContain("opacity:0.82");
    const ok = renderToStaticMarkup(<OverlayLayers blocks={[block({ translated: true })]} plans={[plan()]} />);
    expect(ok).toContain("opacity:1");
  });

  it("没有块 / 没有排版结果：什么都不画，不出错", () => {
    expect(renderToStaticMarkup(<OverlayLayers blocks={[]} plans={[]} />)).toBe("");
    expect(renderToStaticMarkup(<OverlayLayers blocks={[block()]} plans={[]} />)).toBe("");
  });
});

describe("isNothingToReplace：全部原样保留时要给一条说明", () => {
  it("每一块都是 unchanged 才算", () => {
    expect(isNothingToReplace([block({ unchanged: true }), block({ unchanged: true })])).toBe(true);
    expect(isNothingToReplace([block({ unchanged: true }), block()])).toBe(false);
    expect(isNothingToReplace([block()])).toBe(false);
  });

  it("没有块（没识别到文字）、还没拿到数据（null）都不算——那是别的提示", () => {
    expect(isNothingToReplace([])).toBe(false);
    expect(isNothingToReplace(null)).toBe(false);
  });
});

describe("isNothingToReplace 对脏数据也不抛", () => {
  it("blocks 里混进 null / undefined：当作「不是全部原样保留」，不抛异常", () => {
    expect(isNothingToReplace([null, block({ unchanged: true })] as unknown as OverlayBlock[])).toBe(false);
    expect(isNothingToReplace([undefined] as unknown as OverlayBlock[])).toBe(false);
  });
});

describe("错误边界：画的部分出错时退成一条带着译文的提示，关闭的处理器不受影响", () => {
  it("getDerivedStateFromError：把抛出来的东西说成一句人话（Error / 字符串 / 其它）", () => {
    expect(OverlayBoundary.getDerivedStateFromError(new Error("boom"))).toEqual({ failure: "boom" });
    expect(OverlayBoundary.getDerivedStateFromError("字符串错误")).toEqual({ failure: "字符串错误" });
    expect(OverlayBoundary.getDerivedStateFromError(undefined)).toEqual({ failure: "未知错误" });
    expect(OverlayBoundary.getDerivedStateFromError({ weird: true })).toEqual({ failure: "未知错误" });
    expect(describeFailure(new Error("x".repeat(500))).length).toBe(200);
    expect(describeFailure(new Error("   "))).toBe("未知错误");
  });

  it("退路：说明原因，并逐块列出译文（译文不能因为排版出错就没了）", () => {
    const html = renderToStaticMarkup(
      <OverlayFallback blocks={[block({ text: "新建任务" }), block({ text: "已计划" })]} failure="排版炸了" />,
    );
    expect(html).toContain('role="alert"');
    expect(html).toContain("没能原位显示译文：排版炸了");
    expect(html).toContain("下面是译文");
    expect(html).toContain("新建任务");
    expect(html).toContain("已计划");
    expect(html).toContain("点击或 Esc 关闭");
  });

  it("退路本身不会抛：blocks 是 null / 混着 null、数字、缺 text 的元素时也只是少列几条", () => {
    for (const blocks of [null, [], [null, 42, {}, { text: 7 }, { text: "  " }, block({ text: "好的" })]] as unknown as (OverlayBlock[] | null)[]) {
      const html = renderToStaticMarkup(<OverlayFallback blocks={blocks} failure="x" />);
      expect(html).toContain("没能原位显示译文：x");
    }
    expect(renderToStaticMarkup(<OverlayFallback blocks={[null, 42, block({ text: "好的" })] as unknown as OverlayBlock[]} failure="x" />)).toContain("好的");
    expect(renderToStaticMarkup(<OverlayFallback blocks={null} failure="x" />)).not.toContain("下面是译文");
  });

  it("没出错时边界原样渲染子元素", () => {
    const html = renderToStaticMarkup(
      <OverlayBoundary blocks={null}>
        <p>子元素</p>
      </OverlayBoundary>,
    );
    expect(html).toBe("<p>子元素</p>");
  });
});
