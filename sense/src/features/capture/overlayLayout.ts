import type { OverlayBlock, OverlayRect, OverlayWord } from "../../lib/ipc";

/**
 * 译文覆盖层的排版：把译文「原位」放回原文的位置，字号、底色、字色都跟原文一致。
 *
 * 全是纯函数，文字宽度由调用方注入（`measure`）——真实界面用 canvas 量，单测用一个线性的假尺子。
 *
 * 为什么不用 Vision 的框高定字号：同一个 13px 的字号，Vision 给的框高在 10 到 19 之间乱跳
 * （只有小写字母的行偏矮，有 g / y 或被图标撑开的行偏高），按它算字号就是「忽大忽小」。
 * 框的**宽度**倒是稳的（比文字本身宽一点，宽出多少有规律，见 WORD_BOX_SCALE / WORD_BOX_PAD），所以改成：
 * 对每个词，找一个字号，让这个词在该字号下的宽度正好等于扣掉边距后的框宽，再按词长加权取中位数，
 * 最后把相近的字号吸附到一起。
 */

/** 量文字宽度（逻辑像素）。量不出来（没有 canvas）返回 0。 */
export type Measure = (text: string, size: number) => number;

/**
 * 覆盖层的字体栈：英文走系统界面字体（SF），中文回落到苹方——和 macOS 应用的常见渲染一致，
 * 反推原文字号用的尺子和画译文用的字体是同一套，量出来的宽度才对得上。
 */
export const OVERLAY_FONT = '-apple-system, "PingFang SC", "Helvetica Neue", Arial, sans-serif';

/**
 * Vision 逐词框的宽度 ≈ 词的真实宽度 × WORD_BOX_SCALE + WORD_BOX_PAD（逻辑像素）。
 * 用已知字号的夹具回归出来的（macOS 26 的 Vision，1 倍 / 2 倍图、深 / 浅底、侧栏 / 文章 / 大小标题共 213 个词，
 * 残差 2.3px）：总体是「宽 2.4% 再多 2.5px」；小字号（≤15px）的比例项更高（约 5%）、截距更小（约 1px），
 * 大字号相反，一组系数对两头的误差都在 1% 以内。
 * 一开始按「固定多 5px」标定，对 30–70px 宽的小词等于少估了 8%，字号整体偏小 1–2px。
 * 只在系统界面字体（SF）上标定过：Arial 约 −3%、Verdana 约 +7%、衬线体约 −11%（见 CHANGELOG 的已知限制）。
 */
export const WORD_BOX_SCALE = 1.024;
export const WORD_BOX_PAD = 2.5;
/** 字号的上下限 */
export const MIN_SIZE = 9;
export const MAX_SIZE = 72;
/** 译文放不下时最多缩到 max(MIN_SIZE, ⌊原文字号 × FLOOR_RATIO⌋) */
export const FLOOR_RATIO = 0.75;
/** 相差不到这个比例的字号当成同一档 */
export const SNAP_TOLERANCE = 0.1;
/** 取不到样式信息（旧版辅助程序、像素分析失败）时的固定样式：和之前一样的深色底、米白字 */
export const FALLBACK_BG = "rgba(14,14,18,0.96)";
export const FALLBACK_FG = "#f6f3ed";
const DARK_FG = "#1a1a1a";
/** 原文的笔画中心比「行盒中心」低多少（字号的倍数）。真机量过：1–2.5px，随字号走，不随字形走 */
const INK_CENTER_DROP = 0.1;

const clamp = (value: number, low: number, high: number) => Math.min(high, Math.max(low, value));
const finiteOr = (value: unknown, fallback: number) => (typeof value === "number" && Number.isFinite(value) ? value : fallback);

// ───────────────────────── 反推原文字号 ─────────────────────────

/** 按词长加权的中位数。 */
export function weightedMedian(samples: { value: number; weight: number }[]): number {
  const sorted = [...samples].sort((a, b) => a.value - b.value);
  const total = sorted.reduce((sum, sample) => sum + sample.weight, 0);
  let seen = 0;
  for (const sample of sorted) {
    seen += sample.weight;
    if (seen >= total / 2) return sample.value;
  }
  return sorted[sorted.length - 1]?.value ?? 0;
}

/**
 * 找一个字号，让 `text` 在这个字号下恰好 `width` 宽（measure 对字号单调，二分）。
 * 量不出来、或者落在搜索范围的边缘（说明宽度和文字对不上）返回 null。
 */
export function fitSize(text: string, width: number, measure: Measure): number | null {
  const LOW = 4;
  const HIGH = 160;
  if (!(width > 0) || !(measure(text, HIGH) > 0)) return null;
  let low = LOW;
  let high = HIGH;
  for (let step = 0; step < 24; step += 1) {
    const mid = (low + high) / 2;
    if (measure(text, mid) < width) low = mid;
    else high = mid;
  }
  const size = (low + high) / 2;
  return size <= LOW + 0.5 || size >= HIGH - 0.5 ? null : size;
}

const hasLetterOrDigit = (token: string) => /[\p{L}\p{N}]/u.test(token);
const codePoints = (text: string) => [...text].length;

export type SizeEstimate = { size: number; weight: number };

/**
 * 从原文的逐词宽度反推原文字号。
 *
 * 只用两个字符以上、含字母数字或汉字的词：行首的图标会被 Vision 读成一个孤零零的字符（©、•、C……），
 * 它的框把图标和间距都框了进去，量出来的「字号」是错的，按词长加权后它本来也没有分量。
 * 没有可用的词（整行只有单个字符）返回 null，调用方退回别的办法。
 */
export function estimateFontSize(words: OverlayWord[] | undefined, measure: Measure): SizeEstimate | null {
  const samples: { value: number; weight: number }[] = [];
  for (const word of words ?? []) {
    if (!word || typeof word.t !== "string") continue; // 后端的数据不可全信
    const weight = codePoints(word.t);
    if (weight < 2 || !hasLetterOrDigit(word.t)) continue;
    const value = fitSize(word.t, (word.w - WORD_BOX_PAD) / WORD_BOX_SCALE, measure);
    if (value !== null) samples.push({ value, weight });
  }
  if (samples.length === 0) return null;
  return { size: weightedMedian(samples), weight: samples.reduce((sum, sample) => sum + sample.weight, 0) };
}

/**
 * 把相近的字号吸附到一起：界面里的字号就那么几档，同一档的各行估出来的数有零点几像素的出入，
 * 画成译文就是「有的大一点有的小一点」。从小到大贪心分簇（簇内最大不超过簇首的 1 + tolerance 倍），
 * 簇内取加权中位数，再取到 0.5px。没有估计值的（null）原样返回 null。
 */
export function snapSizes(estimates: (SizeEstimate | null)[], tolerance = SNAP_TOLERANCE): (number | null)[] {
  const result: (number | null)[] = estimates.map(() => null);
  const order = estimates
    .map((estimate, index) => ({ estimate, index }))
    .filter((item): item is { estimate: SizeEstimate; index: number } => item.estimate !== null)
    .sort((a, b) => a.estimate.size - b.estimate.size);

  let cluster: typeof order = [];
  const flush = () => {
    if (cluster.length === 0) return;
    const median = weightedMedian(cluster.map(({ estimate }) => ({ value: estimate.size, weight: estimate.weight })));
    const snapped = Math.round(median * 2) / 2;
    for (const { index } of cluster) result[index] = snapped;
    cluster = [];
  };
  for (const item of order) {
    if (cluster.length > 0 && item.estimate.size > cluster[0].estimate.size * (1 + tolerance)) flush();
    cluster.push(item);
  }
  flush();
  return result;
}

// ───────────────────────── 折行与适配 ─────────────────────────

const NO_LINE_START = new Set([..."，。、；：！？）】」』》”’,.;:!?)]}%…"]);
const NO_LINE_END = new Set([..."（【「『《“‘([{"]);
const isCjk = (ch: string) => /[\u2e80-\u9fff\uf900-\ufaff\uff00-\uffef\u3000-\u303f]/.test(ch);

/** 切成「可以在它后面换行」的单元：拉丁词（连同后面的空格）、单个汉字；标点按禁则挂在前一个 / 后一个单元上。 */
function toUnits(paragraph: string): string[] {
  const chars = [...paragraph];
  const units: string[] = [];
  let opening = "";
  let index = 0;
  while (index < chars.length) {
    const ch = chars[index];
    if (NO_LINE_END.has(ch)) {
      opening += ch;
      index += 1;
      continue;
    }
    // 全角标点接在拉丁词后面（「abc，def」）：挂到前一个单元上，不能自己起一个单元落到行首
    if (opening === "" && isCjk(ch) && NO_LINE_START.has(ch) && units.length > 0) {
      units[units.length - 1] += ch;
      index += 1;
      continue;
    }
    let unit = opening;
    opening = "";
    if (isCjk(ch)) {
      unit += ch;
      index += 1;
      while (index < chars.length && NO_LINE_START.has(chars[index])) {
        unit += chars[index];
        index += 1;
      }
    } else {
      while (index < chars.length && !isCjk(chars[index]) && chars[index] !== " ") {
        unit += chars[index];
        index += 1;
      }
      while (index < chars.length && chars[index] === " ") {
        unit += " ";
        index += 1;
      }
    }
    if (unit) units.push(unit);
  }
  if (opening) units.push(opening);
  return units;
}

/** 单个单元就比整行还宽（一长串没有空格的网址之类）：逐字硬切。 */
function splitToWidth(unit: string, size: number, width: number, measure: Measure): string[] {
  const pieces: string[] = [];
  let current = "";
  for (const ch of unit) {
    // 标点宁可让这一行多出一个字，也不能自己落到下一行的行首——但只悬挂一个：当前这一行本身还没超宽时才行。
    // 一长串标点（目录点线、Loading......）不让一路挂下去，否则整串成一行、实宽远超可用宽度，还不会被标成截断
    const overhang = NO_LINE_START.has(ch) && measure(current, size) <= width;
    if (current && measure(current + ch, size) > width && !overhang) {
      pieces.push(current);
      current = ch;
    } else {
      current += ch;
    }
  }
  if (current) pieces.push(current);
  return pieces;
}

/** 贪心折行：汉字之间随处可断，拉丁词在空格处断，标点遵守行首 / 行尾禁则，译文里的 `\n` 当作硬换行。 */
export function wrapText(text: string, size: number, width: number, measure: Measure): string[] {
  const lines: string[] = [];
  // 收一行：本身就比整行宽（一长串没有空格的网址之类）的逐字硬切
  const push = (line: string) => {
    const trimmed = line.trimEnd();
    if (measure(trimmed, size) > width && codePoints(trimmed) > 1) lines.push(...splitToWidth(trimmed, size, width, measure));
    else lines.push(trimmed);
  };
  for (const paragraph of text.split("\n")) {
    const before = lines.length;
    let line = "";
    for (const unit of toUnits(paragraph)) {
      const candidate = line + unit;
      if (line === "" || measure(candidate.trimEnd(), size) <= width) {
        line = candidate;
        continue;
      }
      // 手里只有段首的空白（后面紧跟一个超宽的单元）：丢掉，不单独成一个空行
      if (line.trim() !== "") push(line);
      line = unit;
    }
    // 空段落留一个空行（保留译文里的空行），有内容的段落收尾那一行
    if (line.trim() !== "" || lines.length === before) push(line);
  }
  return lines;
}

/** 末尾加省略号，直到放得下。 */
export function ellipsize(line: string, size: number, width: number, measure: Measure): string {
  const chars = [...line.trimEnd()];
  while (chars.length > 0 && measure(`${chars.join("")}…`, size) > width) chars.pop();
  return `${chars.join("")}…`;
}

export type Fit = { size: number; lines: string[]; truncated: boolean };

/**
 * 在 `width` 宽、最多 `maxLines` 行里放下 `text`：先用原文字号；放不下按 0.5px 一档往下缩，最多缩到 `minSize`；
 * 缩到底还放不下就截断并加省略号——并如实标出来，不让一半的译文冒充完整的。
 */
export function fitText(
  text: string,
  size: number,
  width: number,
  maxLines: number,
  minSize: number,
  measure: Measure,
): Fit {
  // 字号 / 下限不是有限数、或大到 `current -= 0.5` 在浮点里原地踏步（1e300）时，按「步长递减」写的循环会死循环。
  // 所以字号先夹进 [0, 1000]，循环按步数走（最多 (1000 − 0) / 0.5 步），不看 current 减没减下去
  const start = Number.isFinite(size) ? clamp(size, 0, 1000) : MIN_SIZE;
  const floor = Math.min(start, Number.isFinite(minSize) ? minSize : MIN_SIZE);
  const steps = Math.ceil((start - floor) / 0.5);
  for (let step = 0; step < steps; step += 1) {
    const current = start - step * 0.5;
    const lines = wrapText(text, current, width, measure);
    if (lines.length <= maxLines) return { size: current, lines, truncated: false };
  }
  const lines = wrapText(text, floor, width, measure);
  if (lines.length <= maxLines) return { size: floor, lines, truncated: false };
  const kept = lines.slice(0, maxLines);
  kept[kept.length - 1] = ellipsize(kept[kept.length - 1], floor, width, measure);
  return { size: floor, lines: kept, truncated: true };
}

// ───────────────────────── 颜色 ─────────────────────────

function parseHex(color: string | null | undefined): [number, number, number] | null {
  const match = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(color ?? "");
  return match ? [parseInt(match[1], 16), parseInt(match[2], 16), parseInt(match[3], 16)] : null;
}

/** WCAG 相对亮度 */
function luminance([r, g, b]: [number, number, number]): number {
  const channel = (value: number) => {
    const s = value / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

/** WCAG 对比度，1–21 */
export function contrastRatio(a: string, b: string): number {
  const first = parseHex(a);
  const second = parseHex(b);
  if (!first || !second) return 1;
  const [high, low] = [luminance(first), luminance(second)].sort((x, y) => y - x);
  return (high + 0.05) / (low + 0.05);
}

/**
 * 译文的底色与字色。优先用原图里采到的；没采到就退回固定的深色样式。
 * 采到的字色和底色反差不够（采样落在了抗锯齿的淡边上、或是半透明的图标）、或者压根没采到字色时，
 * 在米白和近黑里挑和底色对比度更高的——译文必须看得清，这比「和原文颜色一模一样」重要。
 * （别用固定的亮度阈值：中灰底上白字只有 2.4:1、黑字有 6.7:1，分界在亮度 0.19 附近，不是 0.4。）
 */
export function pickColors(block: Pick<OverlayBlock, "bg" | "fg">): { bg: string; fg: string } {
  const bg = parseHex(block.bg);
  if (!bg) return { bg: FALLBACK_BG, fg: FALLBACK_FG };
  const bgHex = block.bg as string;
  if (block.fg && parseHex(block.fg) && contrastRatio(block.fg, bgHex) >= 3) return { bg: bgHex, fg: block.fg };
  return { bg: bgHex, fg: contrastRatio(FALLBACK_FG, bgHex) >= contrastRatio(DARK_FG, bgHex) ? FALLBACK_FG : DARK_FG };
}

// ───────────────────────── 单块与整屏 ─────────────────────────

export type BlockPlan = {
  /** 盖住原文的矩形（用 bg 填满） */
  cover: OverlayRect;
  bg: string;
  fg: string;
  fontSize: number;
  lineHeight: number;
  /** 已经折好的行，一行一个元素 */
  lines: string[];
  /** 第一行文字框的左上角 */
  x: number;
  y: number;
  /** 译文被截断了（缩到底也放不下） */
  truncated: boolean;
};

const finite = (value: number) => Number.isFinite(value);

function validRect(rect: OverlayRect | null | undefined): rect is OverlayRect {
  return !!rect && finite(rect.x) && finite(rect.y) && finite(rect.w) && finite(rect.h) && rect.w > 0 && rect.h > 0;
}

/** 外扩 `pad`，再夹进 [0, w] × [0, h]。 */
function padAndClip(rect: OverlayRect, pad: number, bounds: { w: number; h: number }): OverlayRect {
  const left = Math.max(0, rect.x - pad);
  const top = Math.max(0, rect.y - pad);
  const right = Math.min(bounds.w, rect.x + rect.w + pad);
  const bottom = Math.min(bounds.h, rect.y + rect.h + pad);
  return { x: left, y: top, w: Math.max(0, right - left), h: Math.max(0, bottom - top) };
}

/** 没法从逐词宽度反推时的兜底：旧办法，按单行框高估。 */
export function fallbackSize(block: Pick<OverlayBlock, "line_h">): number {
  const lineH = Number.isFinite(block.line_h) ? block.line_h : 14;
  return clamp(Math.round(lineH * 0.95), 12, 26);
}

/**
 * 译文里的硬换行并成一行。源文是一行（或已经合并成一段）交给模型的，模型加的换行没有版面意义；
 * 留着的话，单行的块会因为多出一行而必然缩到底、被截断。汉字和汉字之间不补空格，其余补一个。
 */
export function joinLines(text: string): string {
  return text
    .replace(/[ \t]*\r?\n[ \t\r\n]*/g, (match, offset: number, whole: string) => {
      const before = whole[offset - 1];
      const after = whole[offset + match.length];
      return before !== undefined && after !== undefined && isCjk(before) && isCjk(after) ? "" : " ";
    })
    .trim();
}

/** 这一块原文有几行。后端没给（旧版）或给了没法用的值时，按块高 / 单行高估，不能一律当一行。 */
function inferLineCount(blockHeight: number, lineHeight: number, lines: unknown): number {
  if (typeof lines === "number" && Number.isFinite(lines) && lines >= 1) return Math.min(200, Math.round(lines));
  const guess = Math.round(blockHeight / Math.max(lineHeight, 1));
  return Number.isFinite(guess) && guess >= 1 ? Math.min(200, guess) : 1;
}

/** 窗口尺寸不是正的有限数（窗口刚建好、还没有尺寸）时当成「不限」，别把所有遮盖块都夹成 0 宽。 */
function normalizeBounds(bounds: { w: number; h: number }): { w: number; h: number } {
  return {
    w: Number.isFinite(bounds.w) && bounds.w > 0 ? bounds.w : Infinity,
    h: Number.isFinite(bounds.h) && bounds.h > 0 ? bounds.h : Infinity,
  };
}

/**
 * 量不了字（没有 canvas）时的替身：汉字一个字号宽、空格 0.3、其余 0.55。
 * 估得粗，但折行、缩号、截断这些逻辑还能照常起作用——量出来恒为 0 的话，什么都「放得下」，
 * 长译文会被窗口悄悄裁掉，而且不标截断。
 */
function heuristicMeasure(text: string, size: number): number {
  let width = 0;
  for (const ch of text) width += size * (isCjk(ch) ? 1 : ch === " " ? 0.3 : 0.55);
  return width;
}

/**
 * 排一块译文。`size` 是原文字号（已吸附）。
 *
 * - 盖住原文：用辅助程序算好的「允许遮盖的范围」（没有就用笔画的紧包围盒、再没有就用 Vision 的框，外扩一点），纯色填底色。
 * - 起笔：笔画的左边、竖直方向与原文的笔画中心对齐（行盒中心比笔画中心高 0.1 个字号）。
 * - 字号：和原文一样；译文比原文长时，先往右边那片空白里延伸（`room`），不够再缩，最多缩到 0.75 倍。
 */
export function planBlock(
  block: OverlayBlock,
  size: number,
  measure: Measure,
  rawBounds: { w: number; h: number },
): BlockPlan {
  const bounds = normalizeBounds(rawBounds);
  // 几何字段逐个清洗：`Math.max(NaN, 8)` 仍是 NaN，一个坏字段会让整块的坐标全变成 NaN
  const box: OverlayRect = {
    x: finiteOr(block.x, 0),
    y: finiteOr(block.y, 0),
    w: Math.max(8, finiteOr(block.w, 8)),
    h: Math.max(8, finiteOr(block.h, 8)),
  };
  const lineCount = inferLineCount(box.h, finiteOr(block.line_h, box.h), block.lines);
  const body = validRect(block.ink) ? block.ink : box;
  const fontSize = clamp(size, MIN_SIZE, MAX_SIZE);
  const minSize = Math.max(MIN_SIZE, Math.floor(fontSize * FLOOR_RATIO));
  const text = joinLines(typeof block.text === "string" ? block.text : "");

  // 多行的段落按段宽折行；单行的可以往右边的空白里延伸（room 是笔画右边一直到下一个非底色像素的距离）
  const room = lineCount === 1 ? Math.max(0, finiteOr(block.room, 0) - 6) : 0;
  const available = Math.max(body.w, Math.min(bounds.w - 2, body.x + body.w + room) - body.x);
  const fit = fitText(text, fontSize, available, lineCount, minSize, measure);

  // 竖直对齐：单行取笔画框的中心；多行时第一行的笔画从框顶起，行距 = (块高 − 一行笔画高) / (行数 − 1)
  const inkLine = fontSize * 0.95; // 一行笔画（升部到降部）大约这么高
  const pitch = lineCount === 1 ? 0 : clamp((body.h - inkLine) / (lineCount - 1), fit.size * 1.15, fit.size * 2.2);
  const lineHeight = lineCount === 1 ? Math.round(fit.size * 1.3) : Math.round(pitch);
  const firstInkCenter = lineCount === 1 ? body.y + body.h / 2 : body.y + inkLine / 2;
  const firstCenter = firstInkCenter - fit.size * INK_CENTER_DROP;
  const { bg, fg } = pickColors(block);

  // 辅助程序给了「允许遮盖的范围」就照它盖：它是看着像素算的，每一侧只扩到下一个非底色像素为止，
  // 不会压到隔壁的色块、分隔线上。没给才自己把笔画（或 Vision 的框）外扩一圈
  const cover = validRect(block.cover)
    ? padAndClip(block.cover, 0, bounds)
    : padAndClip(body, Math.max(2, Math.round(fontSize * 0.12)), bounds);

  return {
    cover,
    bg,
    fg,
    fontSize: fit.size,
    lineHeight,
    lines: fit.lines,
    x: body.x,
    y: firstCenter - lineHeight / 2,
    truncated: fit.truncated,
  };
}

/**
 * 最简单的兜底排版：不量字、不折行，固定的深色底、米白字，字号按框高估。**不可能抛异常**。
 * 后端给的数据不可全信（字段缺失、类型不对、NaN）：排版出错时退回它，至少能把译文显示出来，
 * 而不是让整个覆盖层的渲染崩掉——渲染一崩，Esc / 点击 / 失焦关闭这些处理器跟着卸载，
 * 屏幕上就留下一个关不掉、看不见的透明置顶窗。
 */
export function plainPlan(raw: OverlayBlock | null | undefined, rawBounds: { w: number; h: number }): BlockPlan {
  // 数组里混进 null / 数字也不抛
  const block = (raw !== null && typeof raw === "object" ? raw : {}) as Partial<OverlayBlock>;
  const bounds = normalizeBounds(rawBounds);
  const num = (value: unknown, fallback: number) => (typeof value === "number" && Number.isFinite(value) ? value : fallback);
  const x = Math.max(0, num(block.x, 0));
  const y = Math.max(0, num(block.y, 0));
  const w = Math.max(8, num(block.w, 8));
  const h = Math.max(8, num(block.h, 8));
  const fontSize = fallbackSize({ line_h: num(block.line_h, h) });
  const lineHeight = Math.round(fontSize * 1.3);
  return {
    cover: padAndClip({ x, y, w, h }, 2, bounds),
    bg: FALLBACK_BG,
    fg: FALLBACK_FG,
    fontSize,
    lineHeight,
    lines: [typeof block.text === "string" ? block.text : ""],
    x,
    y: y + (h - lineHeight) / 2,
    truncated: false,
  };
}

/** 估不出字号的块（整行只有单个字符、纯符号）：靠拢最近的已吸附字号（差在 25% 以内），否则用按框高估的兜底值。 */
function nearestSnapped(fallback: number, snapped: (number | null)[]): number {
  let best = fallback;
  let gap = Infinity;
  for (const size of snapped) {
    if (size === null) continue;
    const distance = Math.abs(size - fallback);
    if (distance < gap) {
      gap = distance;
      best = size;
    }
  }
  return gap <= fallback * 0.25 ? best : fallback;
}

/**
 * IPC 边界上的数据清洗：不是数组是格式错误（不能当成「没识别到文字」）；数组里不是对象的元素（null、数字……）丢掉，
 * 并把丢了几条告诉使用方。后端（Rust）保证不会这样，但这一层挡住了之后，下游就不用处处防 null。
 */
export function sanitizeBlocks(raw: unknown): { blocks: OverlayBlock[]; problem: string | null } {
  if (!Array.isArray(raw)) return { blocks: [], problem: "识别结果的格式不对" };
  const blocks = raw.filter((item): item is OverlayBlock => item !== null && typeof item === "object" && !Array.isArray(item));
  const dropped = raw.length - blocks.length;
  return { blocks, problem: dropped === 0 ? null : `有 ${dropped} 条识别结果的格式不对，已跳过` };
}

/** 整屏：先估各块的原文字号、吸附，再逐块排版。任何一块出错都不影响别的块，也不往外抛。 */
export function planOverlay(blocks: OverlayBlock[], measure: Measure, bounds: { w: number; h: number }): BlockPlan[] {
  // 量不了字（没有 canvas）：字号只能按框高估，排版用字符宽度的估算
  const canMeasure = measure("M", 100) > 0;
  const ruler = canMeasure ? measure : heuristicMeasure;
  const estimates = blocks.map((block) => {
    if (!canMeasure) return null;
    try {
      return estimateFontSize(block.words, measure);
    } catch (error) {
      console.error("[overlay] 反推字号出错，这一块按框高估：", error);
      return null;
    }
  });
  const snapped = snapSizes(estimates);
  return blocks.map((block, index) => {
    try {
      const size = snapped[index] ?? nearestSnapped(fallbackSize(block), snapped);
      return planBlock(block, size, ruler, bounds);
    } catch (error) {
      console.error("[overlay] 排版出错，这一块按简单样式显示：", error);
      return plainPlan(block, bounds);
    }
  });
}

/** 用 canvas 量文字宽度。拿不到 canvas 时恒返回 0，`planOverlay` 会据此改用字符宽度的估算。 */
export function createCanvasMeasure(family: string = OVERLAY_FONT): Measure {
  const context = typeof document === "undefined" ? null : document.createElement("canvas").getContext("2d");
  return (text, size) => {
    if (!context) return 0;
    context.font = `${size}px ${family}`;
    return context.measureText(text).width;
  };
}
