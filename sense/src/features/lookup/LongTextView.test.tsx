import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { EchoNotice, Pane, Prose } from "./LongTextView";

describe("阅读视图的段落：文字方向", () => {
  it("每个段落、小标题、列表都带 dir=auto：阿拉伯语、希伯来语从右往左，中英文照旧从左往右", () => {
    const html = renderToStaticMarkup(
      <Prose text={"مرحبا بالعالم، هذه فقرة طويلة بما يكفي لتكون فقرة.\n\nשלום עולם\n\n- פריט ראשון\n- פריט שני"} tone="target" />,
    );
    // 一个段落（p）、一个小标题（h3）、一个列表（ul），各带一个
    expect(html.match(/dir="auto"/g)).toHaveLength(3);
    expect(html).toMatch(/<p[^>]*dir="auto"/);
    expect(html).toMatch(/<h3[^>]*dir="auto"/);
    expect(html).toMatch(/<ul[^>]*dir="auto"/);
  });

  it("列表的缩进用逻辑属性（ps-5）：从右往左的列表，缩进在右边", () => {
    const html = renderToStaticMarkup(<Prose text={"- one\n- two"} tone="source" />);
    expect(html).toContain("ps-5");
    expect(html).not.toContain("pl-5");
  });

  it("正文仍是按原样保留换行的段落（以前的行为不变）", () => {
    const html = renderToStaticMarkup(<Prose text={"First paragraph.\nSecond line."} tone="source" />);
    expect(html).toContain("whitespace-pre-wrap");
    expect(html).toContain("First paragraph.");
  });
});

describe("译文和原文一样时的说明", () => {
  it("原文语言没把握：原文可能本来就是目标语言，或者是不用翻译的内容；想查词典改回「自动」", () => {
    const html = renderToStaticMarkup(<EchoNotice target="英文" sourceKnown={false} />);
    expect(html).toContain("译文和原文一样");
    expect(html).toContain("原文可能本来就是英文");
    expect(html).toContain("代码、专有名词");
    expect(html).toContain("改回「自动」");
    expect(html).toContain('role="status"');
  });

  it("不知道目标语言时（事件没到）也能说：不写成 undefined / null", () => {
    const html = renderToStaticMarkup(<EchoNotice target={null} sourceKnown={false} />);
    expect(html).toContain("原文可能本来就是目标语言");
    expect(html).not.toMatch(/null|undefined/);
  });

  it("原文语言已经确认、和目标不同：不能说原文可能本来就是目标语言（标题上写着「原文 · 英文」），也不叫人去查词典", () => {
    const html = renderToStaticMarkup(<EchoNotice target="中文" sourceKnown />);
    expect(html).toContain("译文和原文一样");
    expect(html).toContain("代码、专有名词");
    expect(html).not.toContain("本来就是");
    expect(html).not.toContain("查词典");
    expect(html).toContain('role="status"');
  });
});

describe("阅读视图两栏的标题栏", () => {
  const render = (title: string) =>
    renderToStaticMarkup(
      <Pane title={title} hint="21 字符" action={<button type="button">停止</button>}>
        内容
      </Pane>,
    );

  it("窄窗口、放大字号时一行放不下：整块换行（flex-wrap），而不是把按钮挤成竖排", () => {
    const html = render("译文 · 印度尼西亚语");
    expect(html).toMatch(/<header[^>]*flex-wrap/);
    // 标题 + 提示不拆开
    expect(html).toMatch(/whitespace-nowrap[^>]*><span[^>]*>译文 · 印度尼西亚语/);
  });

  it("标题带语种：原样显示", () => {
    expect(render("原文 · 自动识别")).toContain("原文 · 自动识别");
  });
});
