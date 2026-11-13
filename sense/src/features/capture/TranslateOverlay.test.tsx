import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { OverlayBlock } from "../../lib/ipc";
import { CaptureToolbar } from "./CaptureToolbar";
import { OverlayBoundary, OverlayFallback, OverlayLayers, OverlayView, describeFailure, isNothingToReplace } from "./TranslateOverlay";
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
  anchor: "left",
  dir: "ltr",
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

/** 每个译文元素按顺序：有没有 `data-untranslated`，里面写的是什么字（一行一行拼起来）。 */
const textElements = (html: string) =>
  [...html.matchAll(/<div[^>]*data-role="text"([^>]*)>((?:<div[^>]*>[^<]*<\/div>)+)<\/div>/g)].map((element) => ({
    flagged: element[1].includes('data-untranslated="true"'),
    text: [...element[2].matchAll(/<div[^>]*>([^<]*)<\/div>/g)].map((line) => line[1]).join("\n"),
  }));

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
    expect(count(html, '<div dir="ltr" style="height:20px">')).toBe(3); // 每行一个固定行高的元素（外层的 line-height 不算）
    for (const text of ["第一行", "第二行", "第三行"]) expect(html).toContain(text);
  });

  it("从右往左的原文：文字框的右边缘贴着 x（translateX(-100%)）、各行靠右；其余照旧从左边起笔", () => {
    const right = renderToStaticMarkup(<OverlayLayers blocks={[block()]} plans={[plan({ anchor: "right", x: 296 })]} />);
    expect(right).toContain('data-anchor="right"');
    expect(right).toContain("left:296px");
    expect(right).toContain("transform:translateX(-100%)");
    expect(right).toContain("text-align:right");
    const left = renderToStaticMarkup(<OverlayLayers blocks={[block()]} plans={[plan()]} />);
    expect(left).toContain('data-anchor="left"');
    expect(left).not.toContain("translateX");
    expect(left).not.toContain("text-align");
  });

  it("每一行的 dir 由排版结果定（目标语言），不再用 dir=auto 去猜首个强字符", () => {
    const lines = ["Wi-Fi متصل (2)", "iCloud: تم الحفظ."]; // 以拉丁品牌名开头的阿拉伯文：dir=auto 会判成从左往右，括号、句点跑到错的一边
    const rtl = renderToStaticMarkup(<OverlayLayers blocks={[block()]} plans={[plan({ lines, dir: "rtl" })]} />);
    expect(count(rtl, 'dir="rtl"')).toBe(2);
    expect(rtl).not.toContain('dir="auto"');
    const ltr = renderToStaticMarkup(<OverlayLayers blocks={[block()]} plans={[plan({ lines })]} />);
    expect(count(ltr, 'dir="ltr"')).toBe(2);
    expect(ltr).not.toContain('dir="auto"');
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

  it("一部分译成、一部分没译成：没译成的加虚线下划线（data-untranslated），译成的不加", () => {
    const blocks = [block({ translated: true }), block({ translated: false, text: "Docs" }), block({ translated: true })];
    const html = renderToStaticMarkup(<OverlayLayers blocks={blocks} plans={[plan(), plan({ lines: ["Docs"] }), plan()]} />);
    // 带标记的正是第二块（没译成、画的是原文 Docs），别的两块没有：标到前一块 / 后一块都会红
    expect(textElements(html)).toEqual([
      { flagged: false, text: "新建任务" },
      { flagged: true, text: "Docs" },
      { flagged: false, text: "新建任务" },
    ]);
    expect(count(html, "underline dotted")).toBe(1);
    expect(count(html, 'data-untranslated="true"')).toBe(1);
    expect(count(html, "opacity:0.82")).toBe(1);
  });

  it("没译成的块排在哪都加虚线（开头、中间、结尾，不止一块），译成的都不加", () => {
    const blocks = [block({ translated: false, text: "A" }), block({ translated: true }), block({ translated: false, text: "B" }), block({ translated: true }), block({ translated: false, text: "C" })];
    const plans = [plan({ lines: ["A"] }), plan(), plan({ lines: ["B"] }), plan(), plan({ lines: ["C"] })];
    const html = renderToStaticMarkup(<OverlayLayers blocks={blocks} plans={plans} />);
    expect(textElements(html).map((element) => element.flagged)).toEqual([true, false, true, false, true]);
  });

  it("译成的块里有「译文和原文一样」的：也算译成了，没译成的照样加虚线", () => {
    const blocks = [block({ translated: true, unchanged: true, text: "V5.9.3" }), block({ translated: false, text: "Docs" })];
    const html = renderToStaticMarkup(<OverlayLayers blocks={blocks} plans={[plan({ lines: ["V5.9.3"] }), plan({ lines: ["Docs"] })]} />);
    expect(count(html, 'data-untranslated="true"')).toBe(1);
    expect(html).not.toContain("V5.9.3");
  });

  it("全都没译成（失败提示已经说明了）：不加虚线，整屏的虚线只是噪声", () => {
    const blocks = [block({ translated: false }), block({ translated: false })];
    const html = renderToStaticMarkup(<OverlayLayers blocks={blocks} plans={[plan(), plan()]} />);
    expect(html).not.toContain("underline");
    expect(html).not.toContain("data-untranslated");
    expect(count(html, "opacity:0.82")).toBe(2);
  });

  it("全都译成了：什么标记都没有", () => {
    const html = renderToStaticMarkup(<OverlayLayers blocks={[block(), block()]} plans={[plan(), plan()]} />);
    expect(html).not.toContain("underline");
    expect(html).not.toContain("data-untranslated");
  });

  it("没有块 / 没有排版结果：什么都不画，不出错", () => {
    expect(renderToStaticMarkup(<OverlayLayers blocks={[]} plans={[]} />)).toBe("");
    expect(renderToStaticMarkup(<OverlayLayers blocks={[block()]} plans={[]} />)).toBe("");
  });

  it("流式翻译尚未全部完成时（pending=true）：已译完的块立刻原位覆盖，未译完的块保留屏幕原文、不误画虚线下划线", () => {
    const blocks = [block({ translated: true, text: "新建任务" }), block({ translated: false, text: "Open" })];
    const plans = [plan({ lines: ["新建任务"] }), plan({ lines: ["Open"] })];
    const html = renderToStaticMarkup(<OverlayLayers blocks={blocks} plans={plans} pending />);
    expect(count(html, 'data-role="cover"')).toBe(1);
    expect(count(html, 'data-role="text"')).toBe(1);
    expect(html).toContain("新建任务");
    expect(html).not.toContain("Open");
    expect(html).not.toContain("data-untranslated");
    expect(html).not.toContain("underline");
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
    expect(html).toContain("点击关闭按钮或 Esc 退出");
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

function withMockBrowser<T>(fn: () => T, dims: { w: number; h: number } = { w: 800, h: 600 }): T {
  const prevWindow = (globalThis as Record<string, unknown>).window;
  const prevDocument = (globalThis as Record<string, unknown>).document;
  (globalThis as Record<string, unknown>).window = { innerWidth: dims.w, innerHeight: dims.h };
  (globalThis as Record<string, unknown>).document = { createElement: () => ({ getContext: () => null }) };
  try {
    return fn();
  } finally {
    if (prevWindow === undefined) delete (globalThis as Record<string, unknown>).window;
    else (globalThis as Record<string, unknown>).window = prevWindow;
    if (prevDocument === undefined) delete (globalThis as Record<string, unknown>).document;
    else (globalThis as Record<string, unknown>).document = prevDocument;
  }
}

describe("OverlayView 两阶段状态渲染", () => {
  it("ocr 阶段：显示「识别中… · Esc 取消」，即使 blocks=[] 也不误报「没识别到文字」", () => {
    const htmlNull = renderToStaticMarkup(
      <OverlayView blocks={null} error={null} notice={null} stage="ocr" hint />,
    );
    expect(htmlNull).toContain("识别中… · Esc 取消");
    expect(htmlNull).not.toContain("没识别到文字");

    const htmlEmpty = withMockBrowser(() =>
      renderToStaticMarkup(<OverlayView blocks={[]} error={null} notice={null} stage="ocr" hint />),
    );
    expect(htmlEmpty).toContain("识别中… · Esc 取消");
    expect(htmlEmpty).not.toContain("没识别到文字");
  });

  it("translating 阶段：显示「翻译中… · Esc 取消」，不误报「译文和原文一样」，先译完的块提前显示", () => {
    const blocks = [
      block({ text: "新建任务", translated: true, unchanged: false }),
      block({ text: "Open", translated: false, unchanged: true, y: 50 }),
    ];
    const html = withMockBrowser(() =>
      renderToStaticMarkup(<OverlayView blocks={blocks} error={null} notice={null} stage="translating" hint />),
    );
    expect(html).toContain("翻译中… · Esc 取消");
    expect(html).toContain("新建任务");
    expect(html).not.toContain("Open");
    expect(html).not.toContain("译文和原文一样");
    expect(html).not.toContain("Esc 关闭");
  });

  it("done 阶段：空块显示「没识别到文字」，有正常译文显示「Esc 关闭」；工具条接管时可隐藏译文与右上角提示", () => {
    const emptyHtml = withMockBrowser(() =>
      renderToStaticMarkup(<OverlayView blocks={[]} error={null} notice={null} stage="done" hint />),
    );
    expect(emptyHtml).toContain("没识别到文字");
    expect(emptyHtml).toContain("Esc 关闭");

    const doneHtml = withMockBrowser(() =>
      renderToStaticMarkup(<OverlayView blocks={[block({ text: "新建任务" })]} error={null} notice={null} stage="done" hint />),
    );
    expect(doneHtml).toContain("新建任务");
    expect(doneHtml).toContain("Esc 关闭");
    expect(doneHtml).not.toContain("识别中");
    expect(doneHtml).not.toContain("翻译中");

    const rawOnlyHtml = withMockBrowser(() =>
      renderToStaticMarkup(
        <OverlayView
          blocks={[block({ text: "新建任务" })]}
          error={null}
          notice={null}
          stage="done"
          hint
          hideTranslation
          hideFloatingEsc
        />,
      ),
    );
    expect(rawOnlyHtml).not.toContain("新建任务");
    expect(rawOnlyHtml).not.toContain("Esc 关闭");
  });

  it("CaptureToolbar 渲染钉钉风格工具条、语言下拉、标注二级栏与存为图片菜单", () => {
    const html = renderToStaticMarkup(
      <CaptureToolbar
        targetLang="zh"
        retranslating={false}
        showOriginal={false}
        onToggleOriginal={() => {}}
        onSelectLang={() => {}}
        activeTool="arrow"
        onSelectTool={() => {}}
        activeColor="#ff3b30"
        onSelectColor={() => {}}
        activeSize="md"
        onSelectSize={() => {}}
        annotationsCount={1}
        onUndo={() => {}}
        onSave={() => {}}
        saving={false}
        hasImage
        openPopover="save"
        onTogglePopover={() => {}}
        popoverUp={false}
        subbarAbove={false}
        onClose={() => {}}
      />,
    );
    for (const label of [
      "显示原文",
      "显示结果",
      "中文",
      "矩形",
      "椭圆",
      "箭头",
      "画笔",
      "文字",
      "撤销",
      "存为图片",
      "关闭",
      "保存未翻译原图",
      "保存带标注原图",
      "保存翻译后图片",
    ]) {
      expect(html).toContain(label);
    }
    expect(html).toContain('data-testid="capture-subbar"');
  });

  it("携带 bounds 时不因 window.innerWidth 滞后而把右下角平移后的遮盖块裁成 0x0", () => {
    const shifted = block({
      text: "Hello",
      x: 222,
      y: 64,
      w: 60,
      h: 16,
      cover: { x: 220, y: 62, w: 64, h: 20 },
      ink: { x: 222, y: 64, w: 58, h: 14 },
    });
    const html = withMockBrowser(
      () =>
        renderToStaticMarkup(
          <OverlayView
            blocks={[shifted]}
            error="超时"
            notice={null}
            stage="done"
            hint={false}
            bounds={{ w: 300, h: 84 }}
          />,
        ),
      { w: 80, h: 22 },
    );
    const coverStyle =
      html.match(/<div[^>]*data-role="cover"[^>]*>/)?.[0]?.match(/style="([^"]+)"/)?.[1] ?? "";
    expect(coverStyle).not.toBe("");
    const widthPx = Number(coverStyle.match(/width:([\d.]+)px/)?.[1] ?? 0);
    const heightPx = Number(coverStyle.match(/height:([\d.]+)px/)?.[1] ?? 0);
    expect(widthPx).toBeGreaterThan(0);
    expect(heightPx).toBeGreaterThan(0);
  });
});
