import { describe, expect, it, vi } from "vitest";
import type { OverlayBlock } from "../../lib/ipc";
import {
  FALLBACK_BG,
  FALLBACK_FG,
  WORD_BOX_PAD,
  WORD_BOX_SCALE,
  contrastRatio,
  ellipsize,
  estimateFontSize,
  fallbackSize,
  fitSize,
  fitText,
  joinLines,
  pickColors,
  plainPlan,
  planBlock,
  planOverlay,
  sanitizeBlocks,
  snapSizes,
  weightedMedian,
  wrapText,
  type Measure,
} from "./overlayLayout";

/** 线性的假尺子：汉字一个字号宽，空格 0.3，其余 0.5。宽度与字号成正比，足够验证排版逻辑。 */
const CJK = /[\u2e80-\u9fff\uf900-\ufaff\uff00-\uffef\u3000-\u303f]/;
const ruler: Measure = (text, size) => [...text].reduce((sum, ch) => sum + size * (CJK.test(ch) ? 1 : ch === " " ? 0.3 : 0.5), 0);
/** Vision 给的逐词框：比词本身宽一点（按回归出来的线性关系造） */
const word = (t: string, size: number) => ({ t, w: ruler(t, size) * WORD_BOX_SCALE + WORD_BOX_PAD });
const BOUNDS = { w: 400, h: 300 };

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

describe("校准锁：Vision 逐词框宽 ≈ 真实宽度 × 1.024 + 2.5", () => {
  it("用字面量造数据：改了 WORD_BOX_SCALE / WORD_BOX_PAD 就必须同时重新标定、改这里", () => {
    // 这两个常量是用真 Vision 在 213 个词上回归出来的（见 overlayLayout.ts 里的注释和 CHANGELOG）。
    // 上面其它测试都是「用常量造数据、再用常量反推」，常量怎么改它们都会过；只有这一条用写死的数字钉住了标定结果
    const literal = (text: string, size: number) => ({ t: text, w: ruler(text, size) * 1.024 + 2.5 });
    for (const size of [11, 13, 16, 24]) {
      expect(estimateFontSize([literal("Experts", size)], ruler)?.size, `${size}px`).toBeCloseTo(size, 1);
    }
    expect(WORD_BOX_SCALE).toBe(1.024);
    expect(WORD_BOX_PAD).toBe(2.5);
  });
});

describe("从逐词宽度反推原文字号", () => {
  it("按宽度反推出字号；行首被读成单个字符的图标不参与", () => {
    const estimate = estimateFontSize([{ t: "©", w: 23 }, word("New", 13), word("task", 13)], ruler);
    expect(estimate?.size).toBeCloseTo(13, 1);
    expect(estimate?.weight).toBe(7); // 只有 New + task 的词长，图标那个字符没有分量
  });

  it("中文词同样能反推", () => {
    expect(estimateFontSize([word("新建任务", 14)], ruler)?.size).toBeCloseTo(14, 1);
  });

  it("一个量歪的词拉不偏结果：按词长加权取中位数", () => {
    // 「Al」是 AI 被读错、框还特别宽（40）；另外两个词都说是 13
    const estimate = estimateFontSize([word("Assistants", 13), word("(BETA)", 13), { t: "Al", w: 40 }], ruler);
    expect(estimate?.size).toBeCloseTo(13, 1);
  });

  it("没有可用的词（只有单个字符 / 纯符号 / 没有逐词框）返回 null，交给调用方兜底", () => {
    expect(estimateFontSize([{ t: "©", w: 20 }, { t: "•", w: 10 }, { t: "A", w: 12 }], ruler)).toBeNull();
    expect(estimateFontSize([{ t: "··", w: 20 }], ruler)).toBeNull();
    expect(estimateFontSize([], ruler)).toBeNull();
    expect(estimateFontSize(undefined, ruler)).toBeNull();
  });

  it("尺子量不出来（没有 canvas）或框比边距还窄：null，不瞎猜", () => {
    expect(estimateFontSize([word("Library", 13)], () => 0)).toBeNull();
    expect(estimateFontSize([{ t: "Library", w: WORD_BOX_PAD - 1 }], ruler)).toBeNull();
  });

  it("fitSize 对单调的尺子精确；落在搜索范围边缘算无解", () => {
    expect(fitSize("abcdef", ruler("abcdef", 17.25), ruler)).toBeCloseTo(17.25, 2);
    expect(fitSize("abcdef", 1, ruler)).toBeNull(); // 太窄：解在下限
    expect(fitSize("abcdef", 100000, ruler)).toBeNull(); // 太宽：解在上限
    expect(fitSize("abcdef", 0, ruler)).toBeNull();
  });

  it("加权中位数", () => {
    expect(weightedMedian([{ value: 1, weight: 1 }, { value: 2, weight: 1 }, { value: 100, weight: 1 }])).toBe(2);
    expect(weightedMedian([{ value: 10, weight: 1 }, { value: 20, weight: 9 }])).toBe(20);
  });
});

describe("字号吸附：同一档的字号合成一个值", () => {
  it("相近的合到簇内加权中位数，取到 0.5px", () => {
    const snapped = snapSizes([
      { size: 13.4, weight: 8 },
      { size: 12.9, weight: 10 },
      { size: 13.8, weight: 6 },
    ]);
    expect(snapped).toEqual([13.5, 13.5, 13.5]);
  });

  it("差得多的不合并（标题和正文是两档）", () => {
    expect(snapSizes([{ size: 13.5, weight: 5 }, { size: 16.5, weight: 5 }, { size: 24, weight: 5 }])).toEqual([13.5, 16.5, 24]);
  });

  it("没有估计值的保持 null，顺序不变", () => {
    expect(snapSizes([null, { size: 13.2, weight: 4 }, null, { size: 13, weight: 4 }])).toEqual([null, 13, null, 13]);
  });

  it("以簇首为界，不链式合并：12、13 同档，14.2 另起一档", () => {
    expect(snapSizes([{ size: 12, weight: 1 }, { size: 13, weight: 1 }, { size: 14.2, weight: 1 }])).toEqual([12, 12, 14]);
  });

  it("全是 null / 空数组不出错", () => {
    expect(snapSizes([null, null])).toEqual([null, null]);
    expect(snapSizes([])).toEqual([]);
  });
});

describe("折行", () => {
  it("拉丁词在空格处断开", () => {
    expect(wrapText("alpha beta gamma delta", 10, 56, ruler)).toEqual(["alpha beta", "gamma delta"]);
  });

  it("汉字之间随处可断", () => {
    expect(wrapText("一二三四五六", 10, 30, ruler)).toEqual(["一二三", "四五六"]);
  });

  it("逗号句号不落在行首，开括号不落在行尾", () => {
    expect(wrapText("一二三，四五", 10, 30, ruler)).toEqual(["一二", "三，四", "五"]);
    expect(wrapText("一二（三四", 10, 30, ruler)).toEqual(["一二", "（三四"]);
  });

  it("单个词比整行还宽：逐字硬切——行首、行中、行尾的都要切", () => {
    const long = "a".repeat(20);
    expect(wrapText(long, 10, 40, ruler)).toEqual(["aaaaaaaa", "aaaaaaaa", "aaaa"]);
    expect(wrapText(`ab ${long} cd`, 10, 40, ruler)).toEqual(["ab", "aaaaaaaa", "aaaaaaaa", "aaaa", "cd"]);
  });

  it("译文里的换行是硬换行；空串是一个空行", () => {
    expect(wrapText("一\n二", 10, 100, ruler)).toEqual(["一", "二"]);
    expect(wrapText("", 10, 100, ruler)).toEqual([""]);
  });

  it("省略号：一直去掉末尾的字符，直到连省略号一起放得下", () => {
    expect(ellipsize("abcdefghij", 10, 30, ruler)).toBe("abcde…");
    expect(ellipsize("abc", 10, 3, ruler)).toBe("…"); // 窄到连一个字都放不下，也不出错
  });
});

describe("适配：同字号放得下就不缩；放不下先缩、缩到 0.75 倍还不行才截断", () => {
  it("放得下：原字号原样", () => {
    expect(fitText("新建任务", 13, 60, 1, 9, ruler)).toEqual({ size: 13, lines: ["新建任务"], truncated: false });
  });

  it("单行放不下：按 0.5px 一档缩小，缩到刚好放得下就停", () => {
    const fit = fitText("新建任务任务", 13, 60, 1, 9, ruler); // 6 字：13px 宽 78，10px 宽 60
    expect(fit).toEqual({ size: 10, lines: ["新建任务任务"], truncated: false });
  });

  it("缩到底还放不下：截断加省略号，并如实标出 truncated", () => {
    const fit = fitText("一".repeat(20), 13, 60, 1, 9, ruler);
    expect(fit.size).toBe(9);
    expect(fit.truncated).toBe(true);
    expect(fit.lines).toHaveLength(1);
    expect(fit.lines[0].endsWith("…")).toBe(true);
    expect(ruler(fit.lines[0], 9)).toBeLessThanOrEqual(60);
  });

  it("多行的块：先折行，不缩字号", () => {
    const fit = fitText("一".repeat(12), 13, 60, 3, 9, ruler);
    expect(fit.size).toBe(13);
    expect(fit.lines).toEqual(["一一一一", "一一一一", "一一一一"]);
  });

  it("字号本来就不大于下限：不再往下缩", () => {
    expect(fitText("一".repeat(10), 9, 40, 1, 9, ruler).size).toBe(9);
  });

  it("缩号的步长踩不到下限时（字号带 .25）也不能把放得下的判成截断", () => {
    // 13.25 → 12.75 → … → 10.25 → 下限 10：循环里没有踩到 10，收尾必须再按下限排一次
    const fit = fitText("一".repeat(6), 13.25, 60, 1, 10, ruler);
    expect(fit).toEqual({ size: 10, lines: ["一一一一一一"], truncated: false });
  });
});

describe("颜色", () => {
  it("采不到底色：退回固定的深色样式", () => {
    expect(pickColors({})).toEqual({ bg: FALLBACK_BG, fg: FALLBACK_FG });
    expect(pickColors({ bg: null, fg: "#ffffff" })).toEqual({ bg: FALLBACK_BG, fg: FALLBACK_FG });
    expect(pickColors({ bg: "not-a-color", fg: "#ffffff" })).toEqual({ bg: FALLBACK_BG, fg: FALLBACK_FG });
  });

  it("采到的底色与字色反差够：原样用，译文和原文同色", () => {
    expect(pickColors({ bg: "#1e1e1e", fg: "#c9c9c9" })).toEqual({ bg: "#1e1e1e", fg: "#c9c9c9" });
    expect(pickColors({ bg: "#2d5fd1", fg: "#ffffff" })).toEqual({ bg: "#2d5fd1", fg: "#ffffff" });
  });

  it("字色和底色反差在 1.2 到 3 之间（有点区别但看不清）也要换：门槛是 3:1", () => {
    expect(contrastRatio("#4a4a4a", "#1e1e1e")).toBeGreaterThan(1.2);
    expect(contrastRatio("#4a4a4a", "#1e1e1e")).toBeLessThan(3);
    expect(pickColors({ bg: "#1e1e1e", fg: "#4a4a4a" }).fg).toBe(FALLBACK_FG);
    expect(pickColors({ bg: "#1e1e1e", fg: "#767676" }).fg).toBe("#767676"); // 反差 ≥ 3：原样保留
  });

  it("字色反差不够（采样落在淡边上）：按底色的明暗换成白 / 黑字——译文必须看得清", () => {
    expect(pickColors({ bg: "#1e1e1e", fg: "#2a2a2a" }).fg).toBe(FALLBACK_FG);
    expect(pickColors({ bg: "#ffffff", fg: "#eeeeee" }).fg).toBe("#1a1a1a");
  });

  it("没采到字色：同样按底色的明暗选", () => {
    expect(pickColors({ bg: "#101010", fg: null }).fg).toBe(FALLBACK_FG);
    expect(pickColors({ bg: "#f0f0f0", fg: null }).fg).toBe("#1a1a1a");
    expect(pickColors({ bg: "#101010", fg: "garbage" }).fg).toBe(FALLBACK_FG);
  });

  it("对比度", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 1);
    expect(contrastRatio("#777777", "#777777")).toBe(1);
    expect(contrastRatio("nope", "#ffffff")).toBe(1);
  });
});

describe("排一块译文", () => {
  const ink = { x: 12, y: 22, w: 96, h: 12 };
  const base = block({ ink, bg: "#1e1e1e", fg: "#c9c9c9", room: 0, lines: 1 });

  it("起笔在原文笔画的左边，字号就是原文字号，底色字色取自原图", () => {
    const plan = planBlock(base, 13, ruler, BOUNDS);
    expect(plan.x).toBe(12);
    expect(plan.fontSize).toBe(13);
    expect(plan.lines).toEqual(["新建任务"]);
    expect(plan.truncated).toBe(false);
    expect([plan.bg, plan.fg]).toEqual(["#1e1e1e", "#c9c9c9"]);
  });

  it("辅助程序给了遮盖范围就照它盖（它保证不压到隔壁的东西），不再自己外扩；仍夹在窗口之内", () => {
    const given = block({ ...base, cover: { x: 12, y: 22, w: 96, h: 12 } }); // 四周都紧贴着别的东西：一点都不能扩
    expect(planBlock(given, 13, ruler, BOUNDS).cover).toEqual({ x: 12, y: 22, w: 96, h: 12 });
    const outside = block({ ...base, cover: { x: -6, y: 290, w: 500, h: 40 } });
    expect(planBlock(outside, 13, ruler, BOUNDS).cover).toEqual({ x: 0, y: 290, w: 400, h: 10 });
    const broken = block({ ...base, cover: { x: 1, y: 1, w: 0, h: 5 } }); // 无效：退回笔画外扩
    expect(planBlock(broken, 13, ruler, BOUNDS).cover).toEqual({ x: 10, y: 20, w: 100, h: 16 });
  });

  it("没有遮盖范围时：笔画包围盒外扩一点（盖住抗锯齿的毛边），夹在窗口之内", () => {
    expect(planBlock(base, 13, ruler, BOUNDS).cover).toEqual({ x: 10, y: 20, w: 100, h: 16 });
    const atEdge = block({ ink: { x: 0, y: 0, w: 396, h: 12 }, bg: "#1e1e1e", fg: "#c9c9c9" });
    expect(planBlock(atEdge, 13, ruler, BOUNDS).cover, "不能伸出窗口").toEqual({ x: 0, y: 0, w: 398, h: 14 });
  });

  it("竖直方向：行盒中心比原文的笔画中心高 0.1 个字号", () => {
    const plan = planBlock(base, 13, ruler, BOUNDS);
    expect(plan.y + plan.lineHeight / 2).toBeCloseTo(22 + 12 / 2 - 1.3, 5);
  });

  it("译文比原文长：先往右边的空白里延伸，字号不变", () => {
    const long = block({ ...base, text: "ABCDEFGHIJKLMNOPQRSTUVWXYZ", room: 200 }); // 26 个字母，13px 下宽 169，笔画只有 96
    const plan = planBlock(long, 13, ruler, BOUNDS);
    expect(plan.fontSize).toBe(13);
    expect(plan.lines).toEqual(["ABCDEFGHIJKLMNOPQRSTUVWXYZ"]);
  });

  it("右边没有空白：缩小字号；缩到底还放不下就截断并标出来", () => {
    const long = block({ ...base, text: "ABCDEFGHIJKLMNOPQRSTUVWXYZ", room: 0 });
    const plan = planBlock(long, 13, ruler, BOUNDS);
    expect(plan.fontSize).toBeLessThan(13);
    expect(plan.fontSize).toBeGreaterThanOrEqual(9);
    expect(plan.truncated).toBe(true);
    expect(plan.lines[0].endsWith("…")).toBe(true);
  });

  it("空白再大，也不会伸出窗口", () => {
    const edge = block({ ...base, ink: { x: 300, y: 22, w: 90, h: 12 }, text: "ABCDEFGHIJKLMNOPQRSTUVWXYZ", room: 5000 });
    const plan = planBlock(edge, 13, ruler, BOUNDS);
    const right = plan.x + Math.max(...plan.lines.map((line) => ruler(line, plan.fontSize)));
    expect(right).toBeLessThanOrEqual(BOUNDS.w - 2 + 1e-6);
  });

  it("多行的段落：按段宽折行，行距取自原文，起笔从第一行的位置开始", () => {
    const paragraph = block({
      text: "一".repeat(12),
      ink: { x: 10, y: 20, w: 52, h: 52.35 }, // 3 行，行距 20，一行笔画 12.35
      lines: 3,
      bg: "#ffffff",
      fg: "#222222",
      room: 500, // 段落不往右延伸
    });
    const plan = planBlock(paragraph, 13, ruler, BOUNDS);
    expect(plan.lines).toEqual(["一一一一", "一一一一", "一一一一"]);
    expect(plan.lineHeight).toBe(20);
    expect(plan.y + plan.lineHeight / 2).toBeCloseTo(20 + (13 * 0.95) / 2 - 1.3, 5);
  });

  it("没有笔画框（辅助程序没给）：按 Vision 的框盖，颜色退回固定样式", () => {
    const plan = planBlock(block(), 13, ruler, BOUNDS);
    expect(plan.x).toBe(10);
    expect(plan.cover).toEqual({ x: 8, y: 18, w: 104, h: 20 });
    expect([plan.bg, plan.fg]).toEqual([FALLBACK_BG, FALLBACK_FG]);
  });

  it("数据里有 NaN / 0 行 / 负的空白：当没有，不出错", () => {
    const messy = block({ ink: { x: Number.NaN, y: 1, w: 5, h: 5 }, lines: 0, room: -20 });
    const plan = planBlock(messy, 13, ruler, BOUNDS);
    expect(plan.x).toBe(10);
    expect(plan.lines).toEqual(["新建任务"]);
    expect(Number.isFinite(plan.y)).toBe(true);
  });

  it("字号被夹在 9–72 之间", () => {
    expect(planBlock(base, 2, ruler, BOUNDS).fontSize).toBe(9);
    expect(planBlock(base, 500, ruler, { w: 4000, h: 3000 }).fontSize).toBeLessThanOrEqual(72);
  });
});

describe("整屏：同一档的块用同一个字号", () => {
  const row = (text: string, size: number, y: number, extra: Partial<OverlayBlock> = {}) =>
    block({
      text,
      y,
      words: [{ t: "© ", w: 23 }, word("Library", size)],
      ink: { x: 10, y, w: 90, h: 12 },
      bg: "#1e1e1e",
      fg: "#c9c9c9",
      ...extra,
    });

  it("估出来差零点几像素的几行，画出来是同一个字号", () => {
    const plans = planOverlay([row("库", 12.8, 20), row("更多", 13.3, 50), row("已计划", 13.6, 80)], ruler, BOUNDS);
    const sizes = plans.map((plan) => plan.fontSize);
    expect(new Set(sizes).size).toBe(1);
    expect(sizes[0]).toBeGreaterThanOrEqual(13);
    expect(sizes[0]).toBeLessThanOrEqual(13.5);
  });

  it("标题和正文不会被吸到一起", () => {
    const plans = planOverlay([row("库", 13, 20), row("更多", 13, 50), row("标题", 20, 80)], ruler, BOUNDS);
    expect(plans[0].fontSize).toBe(plans[1].fontSize);
    expect(plans[2].fontSize).toBeGreaterThan(plans[0].fontSize + 5);
  });

  it("兜底字号：按单行框高，夹在 12–26", () => {
    expect(fallbackSize({ line_h: 16 })).toBe(15);
    expect(fallbackSize({ line_h: 5 })).toBe(12);
    expect(fallbackSize({ line_h: 100 })).toBe(26);
  });

  it("空的块列表", () => {
    expect(planOverlay([], ruler, BOUNDS)).toEqual([]);
  });
});

describe("没采到字色时在米白和近黑里挑对比度高的（评审：中灰底上白字只有 2.4:1）", () => {
  it("整条灰阶（0–255）都不会选到对比度差的那一边", () => {
    for (let level = 0; level < 256; level += 1) {
      const hex = `#${level.toString(16).padStart(2, "0").repeat(3)}`;
      const { fg } = pickColors({ bg: hex, fg: null });
      const other = fg === FALLBACK_FG ? "#1a1a1a" : FALLBACK_FG;
      expect(contrastRatio(fg, hex), hex).toBeGreaterThanOrEqual(contrastRatio(other, hex));
    }
  });

  it("中灰底用近黑字（黑字 6.7:1、白字 2.4:1），深灰底用米白字", () => {
    expect(pickColors({ bg: "#a0a0a0", fg: null }).fg).toBe("#1a1a1a");
    expect(pickColors({ bg: "#808080", fg: null }).fg).toBe("#1a1a1a");
    expect(pickColors({ bg: "#606060", fg: null }).fg).toBe(FALLBACK_FG);
  });

  it("彩色底（绿 / 橙 / 蓝按钮）同样按对比度挑", () => {
    for (const bg of ["#2d5fd1", "#e8710a", "#1e8e3e", "#f9ab00"]) {
      const { fg } = pickColors({ bg, fg: null });
      const other = fg === FALLBACK_FG ? "#1a1a1a" : FALLBACK_FG;
      expect(contrastRatio(fg, bg), bg).toBeGreaterThanOrEqual(contrastRatio(other, bg));
    }
  });
});

describe("译文里的硬换行与折行的边角", () => {
  it("硬换行并成一行：汉字间不补空格，其余补一个；首尾的去掉", () => {
    expect(joinLines("第一行\n第二行")).toBe("第一行第二行");
    expect(joinLines("Hello\nworld")).toBe("Hello world");
    expect(joinLines("中文\nEnglish")).toBe("中文 English");
    expect(joinLines("a  \r\n  b\n\n\nc")).toBe("a b c");
    expect(joinLines("\n abc \n")).toBe("abc");
  });

  it("单行的块：译文里多出来的换行不会让它必然缩到底、被截断", () => {
    const single = block({ text: "第一行\n第二行", ink: { x: 12, y: 22, w: 96, h: 12 }, room: 0, lines: 1 });
    const plan = planBlock(single, 13, ruler, BOUNDS);
    expect(plan.lines).toEqual(["第一行第二行"]);
    expect(plan.truncated).toBe(false);
  });

  it("全角标点接在拉丁词后面不落行首，硬切时也一样", () => {
    const lines = wrapText("abcdefgh，ij", 10, 40, ruler); // 「abcdefgh」恰好 40 宽，「，」放不下
    expect(lines.every((line) => !line.startsWith("，"))).toBe(true);
    expect(lines.join("")).toBe("abcdefgh，ij");
  });

  it("段首的空白后面紧跟一个超宽的词：不产生空行", () => {
    expect(wrapText(`  ${"a".repeat(20)}`, 10, 40, ruler)).toEqual(["aaaaaaaa", "aaaaaaaa", "aaaa"]);
  });

  it("空段落仍然留一个空行；有内容的段落不多出空行", () => {
    expect(wrapText("a\n\nb", 10, 100, ruler)).toEqual(["a", "", "b"]);
    expect(wrapText("   ", 10, 100, ruler)).toEqual([""]);
  });

  it("字号 / 下限不是有限数、或大到 -= 0.5 原地踏步（1e300）时不死循环，结果是合理的字号", () => {
    for (const size of [Number.POSITIVE_INFINITY, Number.NaN, 1e300]) {
      const fit = fitText("abc", size, 100, 1, 9, ruler);
      expect(Number.isFinite(fit.size), String(size)).toBe(true);
      expect(fit.size, String(size)).toBeLessThanOrEqual(1000);
    }
    expect(Number.isFinite(fitText("abc", 13, 100, 1, Number.NaN, ruler).size)).toBe(true);
  });
});

describe("后端的数据不可全信：排版不抛异常，覆盖层不能因此崩掉", () => {
  it("逐词框缺 t / 文本缺失 / 几何是 NaN / words 不是数组：planOverlay 不抛，每块都给出有限的数", () => {
    const bad = [
      { ...block(), words: [{ t: undefined, w: 10 }, null, { t: "ok", w: Number.NaN }] },
      { ...block(), text: undefined },
      { ...block(), x: Number.NaN, y: Number.NaN, w: Number.NaN, h: Number.NaN, line_h: Number.NaN, ink: { x: Number.NaN, y: 0, w: 5, h: 5 } },
      { ...block(), words: "not-an-array" },
      { ...block(), lines: Number.POSITIVE_INFINITY, room: Number.NaN, cover: { x: Number.NaN, y: 0, w: 1, h: 1 } },
    ] as unknown as OverlayBlock[];
    const plans = planOverlay(bad, ruler, BOUNDS);
    expect(plans).toHaveLength(bad.length);
    for (const [index, plan] of plans.entries()) {
      for (const value of [plan.fontSize, plan.lineHeight, plan.x, plan.y, plan.cover.x, plan.cover.y, plan.cover.w, plan.cover.h]) {
        expect(Number.isFinite(value), `第 ${index} 块`).toBe(true);
      }
      expect(plan.lines.every((line) => typeof line === "string")).toBe(true);
    }
  });

  it("窗口尺寸是 0 / NaN（刚建好还没有尺寸）：当成不限，遮盖块不会被夹成 0 宽", () => {
    const plan = planBlock(block({ ink: { x: 12, y: 22, w: 96, h: 12 }, bg: "#1e1e1e", fg: "#c9c9c9" }), 13, ruler, { w: 0, h: Number.NaN });
    expect(plan.cover.w).toBeGreaterThan(0);
    expect(plan.cover.h).toBeGreaterThan(0);
  });

  it("某一块排版真的抛了异常：只有它退回最简排版，别的块不受影响，也不往外抛", () => {
    const boom = new Proxy(block(), {
      get(target, key) {
        if (key === "ink") throw new Error("boom");
        return Reflect.get(target, key);
      },
    });
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      const plans = planOverlay([block({ text: "正常" }), boom, block({ text: "也正常" })], ruler, BOUNDS);
      expect(plans.map((plan) => plan.lines[0])).toEqual(["正常", "新建任务", "也正常"]);
      expect(plans[1].bg).toBe(FALLBACK_BG); // 最简排版：固定深色底
      expect(spy).toHaveBeenCalled();
    } finally {
      spy.mockRestore();
    }
  });

  it("plainPlan 永远给得出有限的结果", () => {
    const plan = plainPlan({ text: 42, x: "a", y: null, w: undefined, h: Number.NaN } as unknown as OverlayBlock, BOUNDS);
    expect(plan.lines).toEqual([""]);
    for (const value of [plan.fontSize, plan.lineHeight, plan.x, plan.y, plan.cover.w, plan.cover.h]) expect(Number.isFinite(value)).toBe(true);
  });

  it("量不了字（measure 恒为 0）：改用字符宽度估算，长译文照样缩小 / 截断并标出来，不是静默裁掉", () => {
    const long = block({
      text: "ABCDEFGHIJKLMNOPQRSTUVWXYZ".repeat(3),
      ink: { x: 12, y: 22, w: 96, h: 12 },
      room: 0,
      lines: 1,
      words: [word("Library", 13)],
    });
    const [plan] = planOverlay([long], () => 0, BOUNDS);
    expect(plan.truncated).toBe(true);
    expect(plan.lines[0].endsWith("…")).toBe(true);
  });
});

describe("旧版后端（没有任何新字段）：前端退回旧行为", () => {
  it("没有 lines：按块高 / 单行高推行数，多行的块照样折行，不被压成一行", () => {
    const old = block({ text: "一".repeat(35), x: 10, y: 20, w: 300, h: 50, line_h: 16 });
    const plan = planBlock(old, 13, ruler, BOUNDS);
    expect(plan.lines.length).toBeGreaterThan(1);
    expect(plan.truncated).toBe(false);
    expect(plan.fontSize).toBe(13);
    expect(planBlock({ ...old, lines: 0 }, 13, ruler, BOUNDS).lines.length).toBeGreaterThan(1);
  });

  it("什么新字段都没有：固定深色底、米白字、按框高估字号（和改动前一致）", () => {
    const [plan] = planOverlay([block({ text: "新建任务", line_h: 16 })], ruler, BOUNDS);
    expect([plan.bg, plan.fg, plan.fontSize]).toEqual([FALLBACK_BG, FALLBACK_FG, fallbackSize({ line_h: 16 })]);
  });
});

describe("估不出字号的块靠拢邻居（「忽大忽小」的残余）", () => {
  const row = (text: string, y: number) =>
    block({ text, y, words: [word("Library", 13)], ink: { x: 10, y, w: 90, h: 12 }, bg: "#1e1e1e", fg: "#c9c9c9" });

  it("单字符 / 纯符号的块：差在 25% 以内就用最近的已吸附字号（13），而不是按框高估出来的 14", () => {
    const [, , lone] = planOverlay([row("库", 20), row("更多", 50), block({ line_h: 15, text: "×", y: 80 })], ruler, BOUNDS);
    expect(fallbackSize({ line_h: 15 })).toBe(14);
    expect(lone.fontSize).toBe(13);
  });

  it("差得远的（一个大号标题字母）保持按框高估的值，不被拉成正文大小", () => {
    const [, , big] = planOverlay([row("库", 20), row("更多", 50), block({ line_h: 40, text: "A", y: 80 })], ruler, BOUNDS);
    expect(big.fontSize).toBe(fallbackSize({ line_h: 40 }));
  });

  it("一个估计值都没有时用兜底值", () => {
    const [plan] = planOverlay([block({ line_h: 16, text: "×" })], ruler, BOUNDS);
    expect(plan.fontSize).toBe(fallbackSize({ line_h: 16 }));
  });
});

describe("排版几何的边界（评审里变异存活的几处）", () => {
  const ink = { x: 12, y: 22, w: 96, h: 12 };
  const base = block({ ink, bg: "#1e1e1e", fg: "#c9c9c9", room: 0, lines: 1 });

  it("右边的空白留 6px 余量：恰好放得下的不缩，多一点就缩", () => {
    // 可用宽度 = 笔画宽 96 + (room 60 − 6) = 150。13px 下每个拉丁字母 6.5px：23 个 = 149.5，24 个 = 156
    expect(planBlock({ ...base, text: "A".repeat(23), room: 60 }, 13, ruler, BOUNDS).fontSize).toBe(13);
    expect(planBlock({ ...base, text: "A".repeat(24), room: 60 }, 13, ruler, BOUNDS).fontSize).toBeLessThan(13);
  });

  it("笔画贴着窗口右边：可用宽度不会比笔画本身还窄", () => {
    const edge = { ...base, ink: { x: 390, y: 22, w: 20, h: 12 }, room: 500, text: "AAA" }; // 窗口 400 宽，右边只剩 8，但笔画就有 20
    expect(planBlock(edge, 13, ruler, BOUNDS).fontSize).toBe(13); // 3 个字母 19.5px，放得进笔画自己的 20
  });

  it("缩小的步长是 0.5px：宽度刚好卡在 12.5 与 13 之间的，落在 12.5", () => {
    const text = "新建任务任务"; // 6 个汉字：13px 宽 78，12.5px 宽 75
    expect(planBlock({ ...base, text, ink: { ...ink, w: 75 } }, 13, ruler, BOUNDS).fontSize).toBe(12.5);
  });

  it("缩到的下限是 max(9, ⌊0.75 × 字号⌋)：字号 20 缩到 15，字号 10 缩到 9", () => {
    const never = "一".repeat(60);
    const big = planBlock({ ...base, text: never }, 20, ruler, BOUNDS);
    expect([big.fontSize, big.truncated]).toEqual([15, true]);
    const small = planBlock({ ...base, text: never }, 10, ruler, BOUNDS);
    expect([small.fontSize, small.truncated]).toEqual([9, true]);
  });

  it("单行的行高是字号 × 1.3 取整", () => {
    expect(planBlock({ ...base, text: "新建" }, 13, ruler, BOUNDS).lineHeight).toBe(17);
    expect(planBlock({ ...base, text: "新建" }, 20, ruler, BOUNDS).lineHeight).toBe(26);
  });

  it("多行的行距：取自原文，但夹在 1.15–2.2 倍字号之间", () => {
    const tight = block({ text: "一".repeat(9), ink: { x: 10, y: 20, w: 52, h: 20 }, lines: 3, bg: "#fff", fg: "#222" });
    expect(planBlock(tight, 13, ruler, BOUNDS).lineHeight).toBe(15); // (20 − 12.35) / 2 = 3.8 → 夹到 1.15 × 13 = 14.95
    const loose = block({ text: "一".repeat(9), ink: { x: 10, y: 20, w: 52, h: 200 }, lines: 3, bg: "#fff", fg: "#222" });
    expect(planBlock(loose, 13, ruler, BOUNDS).lineHeight).toBe(29); // (200 − 12.35) / 2 = 93.8 → 夹到 2.2 × 13 = 28.6
  });
});

describe("第二轮评审后补的用例", () => {
  it("一长串标点（目录点线、Loading......）不能一路「挂」在行尾：硬切后每一行都不超宽（最多挂一个字符）", () => {
    for (const text of [".".repeat(60), "。".repeat(30), `Chapter${".".repeat(40)}5`]) {
      const lines = wrapText(text, 13, 100, ruler);
      expect(lines.join(""), text).toBe(text);
      for (const line of lines) expect(ruler(line, 13), `${text.slice(0, 12)}… 的一行`).toBeLessThanOrEqual(100 + 13);
    }
  });

  it("点线放不下时：缩到底还放不下就截断并标出来，不是一整行 390px 被窗口静默裁掉", () => {
    const fit = fitText(".".repeat(60), 13, 100, 1, 9, ruler);
    expect(fit.truncated).toBe(true);
    expect(fit.lines).toHaveLength(1);
    expect(ruler(fit.lines[0], fit.size)).toBeLessThanOrEqual(100);
  });

  it("标点挂在行尾的好处还在：恰好填满一行的词后面跟一个全角标点，标点跟着前面一起", () => {
    expect(wrapText("abcdefgh，ij", 10, 40, ruler).every((line) => !line.startsWith("，"))).toBe(true);
  });

  it("blocks 里混进 null / 数字 / 数组：planOverlay 与 plainPlan 都不抛，每个元素仍给出一个有限的排版", () => {
    const messy = [null, undefined, 42, [], block({ text: "正常" })] as unknown as OverlayBlock[];
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      const plans = planOverlay(messy, ruler, BOUNDS);
      expect(plans).toHaveLength(messy.length);
      for (const plan of plans) {
        for (const value of [plan.fontSize, plan.lineHeight, plan.x, plan.y, plan.cover.x, plan.cover.y, plan.cover.w, plan.cover.h]) {
          expect(Number.isFinite(value)).toBe(true);
        }
      }
      expect(plans[4].lines).toEqual(["正常"]);
    } finally {
      spy.mockRestore();
    }
  });

  it("plainPlan 也清洗窗口尺寸：宽 0 / NaN 时遮盖块不会被夹成 0×0", () => {
    const plan = plainPlan(block(), { w: 0, h: Number.NaN });
    expect(plan.cover.w).toBeGreaterThan(0);
    expect(plan.cover.h).toBeGreaterThan(0);
  });

  it("sanitizeBlocks：IPC 边界上的清洗——非数组是格式错误（不是「没识别到文字」），数组里的非对象元素丢掉并说一声", () => {
    expect(sanitizeBlocks(undefined)).toEqual({ blocks: [], problem: "识别结果的格式不对" });
    expect(sanitizeBlocks("nope").problem).not.toBeNull();
    const clean = block();
    expect(sanitizeBlocks([clean])).toEqual({ blocks: [clean], problem: null });
    const mixed = sanitizeBlocks([clean, null, 3, [], clean]);
    expect(mixed.blocks).toEqual([clean, clean]);
    expect(mixed.problem).toContain("3");
    expect(sanitizeBlocks([])).toEqual({ blocks: [], problem: null });
  });
});
