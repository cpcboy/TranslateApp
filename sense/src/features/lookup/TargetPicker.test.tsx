import { isValidElement, type ReactElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { AUTO_HINT } from "../../lib/languages";
import { TargetButton, TargetPanel } from "./TargetPicker";

const LANGUAGES = [
  { code: "zh", name: "中文", native: "简体中文" },
  { code: "zh-Hant", name: "繁体中文", native: "繁體中文" },
  { code: "en", name: "英文", native: "English" },
  { code: "ja", name: "日语", native: "日本語" },
];

type Clickable = ReactElement<{ onClick: () => void; children?: ReactNode }>;

/** 元素树里所有带 onClick 的元素（函数组件不展开：Chip 的 props 里就有 onClick），按出现顺序 */
function clickables(node: ReactNode): Clickable[] {
  if (Array.isArray(node)) return node.flatMap(clickables);
  if (!isValidElement(node)) return [];
  const props = node.props as { onClick?: unknown; children?: ReactNode };
  const own = typeof props.onClick === "function" ? [node as Clickable] : [];
  return [...own, ...clickables(props.children)];
}

function textOf(node: ReactNode): string {
  if (node === null || node === undefined || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  return isValidElement(node) ? textOf((node.props as { children?: ReactNode }).children) : "";
}

describe("目标语言面板", () => {
  const render = (pref: string, error: string | null = null) =>
    renderToStaticMarkup(<TargetPanel pref={pref} languages={LANGUAGES} error={error} onChoose={() => {}} />);

  it("先是「自动」，再按语言表的顺序列出每一种；说明里写明默认方向", () => {
    const html = render("auto");
    const positions = ["自动", "中文", "繁体中文", "英文", "日语"].map((label) => html.indexOf(`>${label}`));
    expect(positions.every((at) => at >= 0)).toBe(true);
    expect([...positions].sort((a, b) => a - b)).toEqual(positions);
    expect(html).toContain(AUTO_HINT);
  });

  it("当前选中的那一个标 aria-pressed，且只有一个", () => {
    for (const pref of ["auto", "zh", "ja"]) {
      const html = render(pref);
      expect(html.match(/aria-pressed="true"/g)).toHaveLength(1);
    }
    // 选 ja 时按下的是日语那颗
    expect(render("ja")).toMatch(/aria-pressed="true"[^>]*>日语/);
    expect(render("auto")).toMatch(/aria-pressed="true"[^>]*>自动/);
  });

  it("语言自己的写法并排放在中文名后面，两者相同就不重复", () => {
    const html = render("auto");
    expect(html).toContain("日本語");
    expect(html).toContain("English");
    const same = renderToStaticMarkup(
      <TargetPanel pref="auto" languages={[{ code: "xx", name: "同名", native: "同名" }]} error={null} onChoose={() => {}} />,
    );
    expect(same.match(/同名/g)).toHaveLength(1);
  });

  it("点哪个就把哪个的代码交出去：auto 先，然后是各语言", () => {
    const onChoose = vi.fn();
    const tree = TargetPanel({ pref: "auto", languages: LANGUAGES, error: null, onChoose });
    const chips = clickables(tree);
    expect(chips.map((chip) => textOf(chip.props.children))).toEqual([
      "自动",
      "中文简体中文",
      "繁体中文繁體中文",
      "英文English",
      "日语日本語",
    ]);
    chips.forEach((chip) => chip.props.onClick());
    expect(onChoose.mock.calls.map((call) => call[0])).toEqual(["auto", "zh", "zh-Hant", "en", "ja"]);
  });

  it("放不下时自己滚动：面板限高，语言那一块可滚，标题与错误提示在滚动区外面", () => {
    const html = render("ja", "写 settings.json 失败");
    // 限高 + 垂直滚动：22 种语言在 760×720 小窗口、放大字号时放不下，输入框不能被挤出去
    expect(html).toMatch(/<section[^>]*max-h-\[60%\]/);
    expect(html).toContain("overflow-y-auto");
    // 错误提示在滚动区之前（外面），不会被滚到看不见的地方
    expect(html.indexOf('role="alert"')).toBeGreaterThan(0);
    expect(html.indexOf('role="alert"')).toBeLessThan(html.indexOf("overflow-y-auto"));
    // 语言的芯片不换行（「印度尼西亚语 Bahasa Indonesia」不能被拦腰折断）
    expect(html).toContain("whitespace-nowrap");
    // macOS 的叠加式滚动条平时隐藏：这块滚动区的滚动条要一直显示，用户才看得出下面还有语言
    expect(html).toContain("[&amp;::-webkit-scrollbar]:w-2.5");
    expect(html).toContain("[&amp;::-webkit-scrollbar-thumb]:bg-[rgba(248,246,241,0.4)]");
  });

  it("存偏好失败时把原因说出来（role=alert），没出错就没有", () => {
    expect(render("ja")).not.toContain('role="alert"');
    const html = render("en", "写 settings.json 失败");
    expect(html).toContain('role="alert"');
    expect(html).toContain("没能保存这个选择：写 settings.json 失败");
  });

  it("把会让人意外的两条规则写明：查词卡只做中英；截屏翻译共用这个设置", () => {
    const html = render("auto");
    expect(html).toContain("查词卡只做中 ↔ 英");
    expect(html).toContain("单个词也按普通翻译处理");
    expect(html).toContain("截屏翻译用同一个设置");
    expect(html).toContain("识别有把握、而选的又恰好是原文语言时");
  });
});

describe("输入框旁的按钮", () => {
  const render = (pref: string, open = false) =>
    renderToStaticMarkup(<TargetButton pref={pref} languages={LANGUAGES} open={open} onClick={() => {}} />);

  it("auto 写「自动」，选了语言写语言名；展开 / 收起的箭头跟着变", () => {
    expect(render("auto")).toContain("→ 自动 ▾");
    expect(render("ja")).toContain("→ 日语 ▾");
    expect(render("ja", true)).toContain("→ 日语 ▴");
    expect(render("auto", true)).toContain('aria-expanded="true"');
    expect(render("auto")).toContain('aria-expanded="false"');
  });

  it("选了非默认语言时高亮——它会改变查词的行为，不能让人忘了它开着", () => {
    expect(render("auto")).not.toContain("border-acc/60");
    expect(render("ja")).toContain("border-acc/60");
  });

  it("读屏能读出现在翻译成什么", () => {
    expect(render("ja")).toContain('aria-label="翻译成：日语。点击修改"');
    expect(render("auto")).toContain('aria-label="翻译成：自动。点击修改"');
  });

  it("点击交给调用方", () => {
    const onClick = vi.fn();
    const tree = TargetButton({ pref: "auto", languages: LANGUAGES, open: false, onClick });
    (tree as Clickable).props.onClick();
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
