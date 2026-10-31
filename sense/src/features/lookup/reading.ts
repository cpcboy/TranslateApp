/**
 * 阅读视图里的几条纯逻辑，单独放出来是为了能写单测（组件本身没法在 node 下跑）。
 */
import type { GrammarIssue } from "../../lib/ipc";

export type Piece = string | { text: string; issue: GrammarIssue };

const WORD_CHAR = /[A-Za-z0-9_]/;

/**
 * 找 `fragment` 在 `text` 里第一处「整词」命中的位置，找不到返回 -1。
 *
 * 整词的意思：片段两端如果是字母数字，紧挨着它的字符就不能也是字母数字。
 * 以前是裸 indexOf，一条问题（fragment 为 go）会把 good、ago、going 都画上波浪线。
 */
export function findWhole(text: string, fragment: string): number {
  if (!fragment) return -1;
  const first = fragment[0];
  const last = fragment[fragment.length - 1];
  let at = text.indexOf(fragment);
  while (at >= 0) {
    const end = at + fragment.length;
    const leftOk = at === 0 || !(WORD_CHAR.test(text[at - 1]) && WORD_CHAR.test(first));
    const rightOk = end === text.length || !(WORD_CHAR.test(text[end]) && WORD_CHAR.test(last));
    if (leftOk && rightOk) return at;
    at = text.indexOf(fragment, at + 1);
  }
  return -1;
}

/**
 * 把一段文字按语法问题切成「普通文字 / 被标记的片段」。
 *
 * - 每条问题只标第一处整词命中；`used` 在各段之间共用，前面的段落标过的，后面的不再重复标。
 * - 两条问题的命中位置重叠时，先到先得。
 */
export function splitMarked(text: string, issues: GrammarIssue[], used: Set<number> = new Set()): Piece[] {
  const hits: { at: number; end: number; issue: GrammarIssue; index: number }[] = [];
  issues.forEach((issue, index) => {
    if (used.has(index)) return;
    const at = findWhole(text, issue.fragment);
    if (at >= 0) hits.push({ at, end: at + issue.fragment.length, issue, index });
  });
  hits.sort((a, b) => a.at - b.at);

  const pieces: Piece[] = [];
  let cursor = 0;
  for (const hit of hits) {
    if (hit.at < cursor) continue;
    if (hit.at > cursor) pieces.push(text.slice(cursor, hit.at));
    pieces.push({ text: hit.issue.fragment, issue: hit.issue });
    used.add(hit.index);
    cursor = hit.end;
  }
  if (cursor < text.length) pieces.push(text.slice(cursor));
  return pieces.length > 0 ? pieces : [text];
}

/**
 * 译文写完之后要不要对原文做语法检查。
 *
 * 没写完（出错、被停止、服务端提前收工）就不查：那是在白白花一次请求，
 * 而且用户马上会点「重新翻译」；译文完整了自然会再走到这里。
 */
export function shouldCheckGrammar(state: {
  busy: boolean;
  text: string;
  source: string;
  error: string | null;
  incomplete: string | null;
  checkedFor: string;
}): boolean {
  const { busy, text, source, error, incomplete, checkedFor } = state;
  if (busy || !text || !source || error || incomplete) return false;
  if (checkedFor === source) return false;
  const latin = (source.match(/[a-zA-Z]/g) ?? []).length;
  const cjk = (source.match(/[一-鿿]/g) ?? []).length;
  return latin >= 40 && cjk * 2 < latin; // 中文原文不查语法
}

/**
 * 以选中的词为中心，各取 `radius` 个字符作为上下文。
 *
 * 以前取的是文本节点开头 400 字：选中词在长段落后半部时，上下文里根本没有它，
 * 「这个词在这句话里是什么意思」就成了瞎猜。
 */
export function sliceAround(full: string, start: number, length: number, radius = 200): string {
  const from = Math.max(0, start - radius);
  const to = Math.min(full.length, start + length + radius);
  return full.slice(from, to).trim();
}

/**
 * 流式生成时，最后一块还没写完。判成小标题会让一句没写完的话先显示成标题、
 * 写完又变回正文，样式来回跳；所以这一块先按正文显示。
 * 文本以空行结尾，说明最后一块已经收尾了，可以放心判断。
 */
export function lastBlockIsOpen(text: string, streaming: boolean): boolean {
  return streaming && !/\n\s*\n\s*$/.test(text);
}

/** 把正文切成「会被拿去标注的片段」：与 LongTextView 里 Prose 的渲染顺序一致（列表逐项、小标题、段落）。 */
export function markableSegments(text: string): string[] {
  const segments: string[] = [];
  for (const block of text.split(/\n\s*\n/).filter((item) => item.trim().length > 0)) {
    const lines = block.split("\n").filter((line) => line.trim().length > 0);
    if (lines.length > 0 && lines.every((line) => /^\s*[-*•]\s+/.test(line))) {
      for (const line of lines) segments.push(line.replace(/^\s*[-*•]\s+/, ""));
    } else {
      segments.push(block);
    }
  }
  return segments;
}

/**
 * 真正画出了几处标记。标题栏的「N 处标记」要数这个，不能数模型给了几条问题：
 * 整词匹配之后，有的问题在原文里根本找不到位置，数问题条数会说多了。
 */
export function markedCount(text: string, issues: GrammarIssue[]): number {
  const used = new Set<number>();
  for (const segment of markableSegments(text)) splitMarked(segment, issues, used);
  return used.size;
}


/** 划词浮层的尺寸（CSS px）：宽与 LongTextView 里的 `w-[300px]` 一致，高是估计值；边距是浮层到窗口边缘的留白（设备像素） */
export const GLOSS_WIDTH = 300;
const GLOSS_HEIGHT = 150;
const GLOSS_MARGIN_X = 20;
const GLOSS_MARGIN_Y = 50;

/**
 * 划词浮层 `fixed` 定位时该写进 left / top 的值（CSS px）。
 *
 * 点击位置、窗口尺寸都是视口里的设备坐标；浮层在界面缩放（CSS zoom）的子树里，写进去的长度会被再乘一次 `scale`，
 * 所以夹紧在设备坐标里做，最后除以 `scale`；浮层自己的宽高也随 zoom 放大，夹紧时要算进去。
 * `scale` 为 1 时与原来的算法一致（再加一条：不会被夹到负数，窗口再窄也别把浮层推到屏幕左边之外）。
 */
export function glossPosition(
  point: { x: number; y: number },
  viewport: { width: number; height: number },
  scale: number,
): { left: number; top: number } {
  const maxLeft = viewport.width - GLOSS_WIDTH * scale - GLOSS_MARGIN_X;
  const maxTop = viewport.height - GLOSS_HEIGHT * scale - GLOSS_MARGIN_Y;
  return {
    left: Math.max(0, Math.min(point.x, maxLeft)) / scale,
    top: Math.max(0, Math.min(point.y + 14, maxTop)) / scale,
  };
}
