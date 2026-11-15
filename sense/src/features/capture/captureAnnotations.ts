/**
 * 钉钉风格截屏标注的数据结构与纯几何辅助函数。
 * 所有坐标均为截屏选区局部逻辑像素（`0..stageW`, `0..stageH`），与 SVG 预览层及 Retina Canvas 导出共享。
 */

export type AnnotationTool = "none" | "rect" | "ellipse" | "arrow" | "pen" | "text";

export type AnnotationPoint = { x: number; y: number };

export type AnnotationColor = {
  id: string;
  label: string;
  hex: string;
};

export type AnnotationSize = {
  id: "sm" | "md" | "lg";
  label: string;
  strokeWidth: number;
  fontSize: number;
};

export const ANNOTATION_COLORS: AnnotationColor[] = [
  { id: "red", label: "红", hex: "#ff3b30" },
  { id: "orange", label: "橙", hex: "#ff9500" },
  { id: "yellow", label: "黄", hex: "#ffcc00" },
  { id: "green", label: "绿", hex: "#34c759" },
  { id: "blue", label: "蓝", hex: "#007aff" },
  { id: "white", label: "白", hex: "#ffffff" },
];

export const ANNOTATION_SIZES: AnnotationSize[] = [
  { id: "sm", label: "细", strokeWidth: 2, fontSize: 14 },
  { id: "md", label: "中", strokeWidth: 4, fontSize: 18 },
  { id: "lg", label: "粗", strokeWidth: 6, fontSize: 24 },
];

export type Annotation =
  | {
      id: string;
      kind: "rect" | "ellipse";
      x1: number;
      y1: number;
      x2: number;
      y2: number;
      color: string;
      strokeWidth: number;
    }
  | {
      id: string;
      kind: "arrow";
      x1: number;
      y1: number;
      x2: number;
      y2: number;
      color: string;
      strokeWidth: number;
    }
  | {
      id: string;
      kind: "pen";
      points: AnnotationPoint[];
      color: string;
      strokeWidth: number;
    }
  | {
      id: string;
      kind: "text";
      x: number;
      y: number;
      text: string;
      color: string;
      fontSize: number;
    };

/** 将坐标夹回选区画布内部，防止拖出边缘导致标注坐标越界。 */
export function clampPoint(point: AnnotationPoint, width: number, height: number): AnnotationPoint {
  return {
    x: Math.min(Math.max(0, point.x), Math.max(0, width)),
    y: Math.min(Math.max(0, point.y), Math.max(0, height)),
  };
}

/** 无论从哪个方向拖框，都规范成左上角 + 非负宽高。 */
export function normalizeRect(
  x1: number,
  y1: number,
  x2: number,
  y2: number,
): { x: number; y: number; w: number; h: number } {
  const x = Math.min(x1, x2);
  const y = Math.min(y1, y2);
  return { x, y, w: Math.abs(x2 - x1), h: Math.abs(y2 - y1) };
}

/** 计算钉钉风格的实心三角形箭头坐标，返回三个顶点（尖端 + 左翼 + 右翼）。 */
export function computeArrowHeadPoints(
  x1: number,
  y1: number,
  x2: number,
  y2: number,
  strokeWidth: number,
): [AnnotationPoint, AnnotationPoint, AnnotationPoint] {
  const dx = x2 - x1;
  const dy = y2 - y1;
  const len = Math.hypot(dx, dy);
  const ux = len > 1e-6 ? dx / len : 1;
  const uy = len > 1e-6 ? dy / len : 0;
  const px = -uy;
  const py = ux;
  const headLen = Math.min(Math.max(strokeWidth * 3.4, 11), len > 0 ? len * 0.7 : strokeWidth * 3.4);
  const headHalfW = Math.max(strokeWidth * 1.9, 6);
  const baseX = x2 - ux * headLen;
  const baseY = y2 - uy * headLen;
  return [
    { x: x2, y: y2 },
    { x: baseX + px * headHalfW, y: baseY + py * headHalfW },
    { x: baseX - px * headHalfW, y: baseY - py * headHalfW },
  ];
}

/** 判断一条刚结束的交互标注是否有效（避免单击误落 0px 空图形）。 */
export function isMeaningfulAnnotation(item: Annotation): boolean {
  if (item.kind === "text") {
    return item.text.trim().length > 0;
  }
  if (item.kind === "pen") {
    return item.points.length >= 2;
  }
  const dx = Math.abs(item.x2 - item.x1);
  const dy = Math.abs(item.y2 - item.y1);
  return Math.hypot(dx, dy) >= 4;
}

export const TEXT_DRAFT_PAD_X = 6;
export const TEXT_DRAFT_PAD_Y = 3;
export const MIN_TEXT_FONT_SIZE = 12;
export const MAX_TEXT_FONT_SIZE = 72;
export const TEXT_FONT_SIZE_STEP = 2;
export const MIN_RESIZE_SHAPE_SIZE = 8;

export type ResizeHandle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w";
export type TextResizeHandle = "nw" | "ne" | "se" | "sw";
export type EditableAnnotation = Extract<Annotation, { kind: "rect" | "ellipse" | "text" }>;

export interface HandleDescriptor<H extends string = ResizeHandle> {
  handle: H;
  x: number;
  y: number;
  cursor: string;
}

export interface TextAnnotationBounds {
  x: number;
  y: number;
  w: number;
  h: number;
  lines: string[];
  lineHeight: number;
}

export interface TextBoxLayout {
  left: number;
  top: number;
  width: number;
  minWidth: number;
  maxWidth: number;
  maxHeight: number;
  maxLineWidth: number;
  lineHeight: number;
  rows: number;
  textX: number;
  textY: number;
  wrappedLines: string[];
}

export type MeasureAnnotationTextFn = (line: string, fontSize: number) => number;

export function estimateTextLineWidth(line: string, fontSize: number): number {
  let width = 0;
  for (const ch of line) {
    const code = ch.codePointAt(0) ?? 0;
    if (code > 0xff) {
      width += fontSize * 1.02;
    } else if (ch === " " || ch === "\t") {
      width += fontSize * 0.42;
    } else if ("MWGQO@#%".includes(ch)) {
      width += fontSize * 0.92;
    } else if ((ch >= "A" && ch <= "Z") || (ch >= "0" && ch <= "9")) {
      width += fontSize * 0.72;
    } else if (".,:;!'|()[]{}`\"\\/".includes(ch)) {
      width += fontSize * 0.48;
    } else {
      width += fontSize * 0.64;
    }
  }
  return width;
}

function splitWrapUnits(line: string): string[] {
  const units: string[] = [];
  let asciiWord = "";
  const isAsciiWordChar = (ch: string) =>
    (ch >= "A" && ch <= "Z") ||
    (ch >= "a" && ch <= "z") ||
    (ch >= "0" && ch <= "9") ||
    ch === "_" ||
    ch === "-" ||
    ch === "'";

  for (const ch of Array.from(line)) {
    if (isAsciiWordChar(ch)) {
      asciiWord += ch;
    } else {
      if (asciiWord) {
        units.push(asciiWord);
        asciiWord = "";
      }
      units.push(ch);
    }
  }
  if (asciiWord) {
    units.push(asciiWord);
  }
  return units;
}

/**
 * 将文字标注按可用行宽 `maxLineWidth` 自动折行，供 `<textarea>` 行高估算、SVG `<text>` 与 Canvas 导出共用，
 * 防止在画布右侧边缘输入单行长文本时越出 `stageWidth` 被硬裁剪。
 */
export function wrapAnnotationText(
  text: string,
  fontSize: number,
  maxLineWidth: number,
  measureFn: MeasureAnnotationTextFn = estimateTextLineWidth,
): string[] {
  const rawLines = text.split("\n");
  if (!Number.isFinite(maxLineWidth) || maxLineWidth <= 0) {
    return rawLines;
  }

  const out: string[] = [];
  const splitOversizedToken = (token: string): string[] => {
    const chars = Array.from(token);
    const parts: string[] = [];
    let chunk = "";
    for (const ch of chars) {
      const candidate = chunk + ch;
      if (chunk.length > 0 && measureFn(candidate, fontSize) > maxLineWidth) {
        parts.push(chunk);
        chunk = ch;
      } else {
        chunk = candidate;
      }
    }
    if (chunk.length > 0) {
      parts.push(chunk);
    }
    return parts;
  };

  for (const rawLine of rawLines) {
    if (rawLine.length === 0 || measureFn(rawLine, fontSize) <= maxLineWidth) {
      out.push(rawLine);
      continue;
    }

    let current = "";
    for (const unit of splitWrapUnits(rawLine)) {
      if (current.length === 0) {
        if (measureFn(unit, fontSize) <= maxLineWidth) {
          current = unit;
        } else {
          const parts = splitOversizedToken(unit);
          for (let i = 0; i < parts.length - 1; i += 1) {
            out.push(parts[i]);
          }
          current = parts[parts.length - 1] ?? "";
        }
        continue;
      }

      const candidate = current + unit;
      if (measureFn(candidate, fontSize) <= maxLineWidth) {
        current = candidate;
      } else {
        const trimmedCurrent = current.replace(/ +$/, "");
        out.push(trimmedCurrent.length > 0 ? trimmedCurrent : current);
        if (unit === " ") {
          current = "";
        } else if (measureFn(unit, fontSize) <= maxLineWidth) {
          current = unit;
        } else {
          const parts = splitOversizedToken(unit);
          for (let i = 0; i < parts.length - 1; i += 1) {
            out.push(parts[i]);
          }
          current = parts[parts.length - 1] ?? "";
        }
      }
    }
    if (current.length > 0) {
      out.push(current);
    }
  }

  return out;
}

/**
 * 计算文字标注输入框与 SVG/Canvas 落盘共用位置，避免边缘夹逼后确认跳变或被画布裁剪。
 */
export function computeTextBoxLayout(options: {
  x: number;
  y: number;
  value: string;
  fontSize: number;
  stageWidth: number;
  stageHeight: number;
  measureFn?: MeasureAnnotationTextFn;
}): TextBoxLayout {
  const { x, y, value, fontSize, stageWidth, stageHeight, measureFn = estimateTextLineWidth } = options;
  const safeW = Math.max(0, stageWidth);
  const safeH = Math.max(0, stageHeight);
  const lineHeight = Math.round(fontSize * 1.25);
  const rawLines = value.split("\n");
  const naturalFourCharBoxW =
    fontSize > 24
      ? Math.ceil(measureFn("输入文字", fontSize)) + TEXT_DRAFT_PAD_X * 2 + 12
      : 110;
  const maxBoxW = Math.min(Math.max(110, naturalFourCharBoxW), safeW);
  const left = Math.max(0, Math.min(x, Math.max(0, safeW - maxBoxW)));
  const availableW = Math.max(0, safeW - left);
  const minFloor = safeW > 0 ? Math.min(40, safeW) : 0;
  const maxWidth = safeW > 0 ? Math.max(minFloor, availableW) : 0;
  const minWidth =
    safeW > 0
      ? Math.min(Math.max(96, Math.ceil(fontSize * 2) + TEXT_DRAFT_PAD_X * 2), maxWidth)
      : 0;
  const maxLineWidth = safeW > 0 ? Math.max(1, maxWidth - TEXT_DRAFT_PAD_X * 2) : 0;
  const wrappedLines =
    safeW > 0 ? wrapAnnotationText(value, fontSize, maxLineWidth, measureFn) : rawLines;
  const didSoftWrap = wrappedLines.length > rawLines.length;
  const maxVisibleRows =
    safeH > 0
      ? Math.min(12, Math.max(1, Math.floor((safeH - TEXT_DRAFT_PAD_Y * 2) / lineHeight)))
      : 8;
  const rows = Math.min(maxVisibleRows, Math.max(1, wrappedLines.length));
  const fullHeight = wrappedLines.length * lineHeight + TEXT_DRAFT_PAD_Y * 2;
  const clampHeight =
    safeH > 0 ? Math.min(safeH, fullHeight) : rows * lineHeight + TEXT_DRAFT_PAD_Y * 2;
  const top = Math.max(0, Math.min(y, Math.max(0, safeH - clampHeight)));
  const contentLineWidth = Math.max(
    measureFn("输入文字", fontSize),
    ...wrappedLines.map((line) => measureFn(line, fontSize)),
  );
  const desiredWidth = Math.ceil(contentLineWidth + TEXT_DRAFT_PAD_X * 2 + 12);
  const width =
    safeW > 0
      ? didSoftWrap
        ? maxWidth
        : Math.max(minWidth, Math.min(maxWidth, desiredWidth))
      : 0;
  const maxHeight = Math.max(lineHeight + TEXT_DRAFT_PAD_Y * 2, safeH - top);
  return {
    left,
    top,
    width,
    minWidth,
    maxWidth,
    maxHeight,
    maxLineWidth,
    lineHeight,
    rows,
    textX: left + TEXT_DRAFT_PAD_X,
    textY: top + TEXT_DRAFT_PAD_Y,
    wrappedLines,
  };
}

function roundCoord(value: number): number {
  return Math.round(value * 100) / 100;
}

/** 将文字标注字号钳制在安全可读区间 `[12, 72]` 整数像素内。 */
export function clampTextFontSize(fontSize: number): number {
  if (!Number.isFinite(fontSize)) return 18;
  return Math.min(MAX_TEXT_FONT_SIZE, Math.max(MIN_TEXT_FONT_SIZE, Math.round(fontSize)));
}

/** 计算已提交文字标注在当前画布宽度下的折行与外包围盒，供命中框、四角缩放柄及移动边界共用。 */
export function computeTextAnnotationBounds(
  item: Extract<Annotation, { kind: "text" }>,
  stageWidth: number,
  measureFn: MeasureAnnotationTextFn = estimateTextLineWidth,
): TextAnnotationBounds {
  const fontSize = clampTextFontSize(item.fontSize);
  const lineHeight = Math.round(fontSize * 1.25);
  const maxLineW = stageWidth > 0 ? Math.max(1, stageWidth - item.x - TEXT_DRAFT_PAD_X) : Infinity;
  const lines = Number.isFinite(maxLineW)
    ? wrapAnnotationText(item.text, fontSize, maxLineW, measureFn)
    : item.text.split("\n");
  const measuredMax = lines.reduce((acc, line) => Math.max(acc, measureFn(line, fontSize)), 0);
  const w = Math.max(Math.ceil(fontSize * 0.8), Math.ceil(measuredMax));
  const h = Math.max(lineHeight, lines.length * lineHeight);
  return { x: item.x, y: item.y, w, h, lines, lineHeight };
}

/** 文字标注选中态虚线外框（外扩微量内边距避免压住笔画边缘）。 */
export function getTextSelectionFrame(bounds: { x: number; y: number; w: number; h: number }): {
  x: number;
  y: number;
  w: number;
  h: number;
} {
  return {
    x: bounds.x - 4,
    y: bounds.y - 2,
    w: bounds.w + 8,
    h: bounds.h + 4,
  };
}

/** 返回矩形 / 椭圆外框的 8 个缩放控制柄坐标与对应鼠标光标。 */
export function getShapeResizeHandles(rect: {
  x: number;
  y: number;
  w: number;
  h: number;
}): HandleDescriptor<ResizeHandle>[] {
  const { x, y, w, h } = rect;
  const midX = x + w / 2;
  const midY = y + h / 2;
  const right = x + w;
  const bottom = y + h;
  return [
    { handle: "nw", x, y, cursor: "nwse-resize" },
    { handle: "n", x: midX, y, cursor: "ns-resize" },
    { handle: "ne", x: right, y, cursor: "nesw-resize" },
    { handle: "e", x: right, y: midY, cursor: "ew-resize" },
    { handle: "se", x: right, y: bottom, cursor: "nwse-resize" },
    { handle: "s", x: midX, y: bottom, cursor: "ns-resize" },
    { handle: "sw", x, y: bottom, cursor: "nesw-resize" },
    { handle: "w", x, y: midY, cursor: "ew-resize" },
  ];
}

/** 返回已提交文字标注虚线选中框四角的缩放控制柄（拖拽可连续缩放字号）。 */
export function getTextResizeHandles(bounds: {
  x: number;
  y: number;
  w: number;
  h: number;
}): HandleDescriptor<TextResizeHandle>[] {
  const f = getTextSelectionFrame(bounds);
  const right = f.x + f.w;
  const bottom = f.y + f.h;
  return [
    { handle: "nw", x: f.x, y: f.y, cursor: "nwse-resize" },
    { handle: "ne", x: right, y: f.y, cursor: "nesw-resize" },
    { handle: "se", x: right, y: bottom, cursor: "nwse-resize" },
    { handle: "sw", x: f.x, y: bottom, cursor: "nesw-resize" },
  ];
}

/**
 * 在画布 `[0..stageW] × [0..stageH]` 内平移矩形、椭圆或文字标注。
 * 文字标注向右拖动时以当前行宽贴靠右边缘，避免把短标签挤成单字竖排；向左拖动时允许原有的右缘折行自然展开。
 */
export function moveAnnotationInStage<T extends EditableAnnotation>(
  item: T,
  dx: number,
  dy: number,
  stageWidth: number,
  stageHeight: number,
  measureFn: MeasureAnnotationTextFn = estimateTextLineWidth,
): T {
  const safeW = Math.max(0, stageWidth);
  const safeH = Math.max(0, stageHeight);

  if (item.kind !== "text") {
    const r = normalizeRect(item.x1, item.y1, item.x2, item.y2);
    const w = safeW > 0 ? Math.min(r.w, safeW) : r.w;
    const h = safeH > 0 ? Math.min(r.h, safeH) : r.h;
    const nextX = safeW > 0 ? Math.min(Math.max(0, r.x + dx), Math.max(0, safeW - w)) : r.x + dx;
    const nextY = safeH > 0 ? Math.min(Math.max(0, r.y + dy), Math.max(0, safeH - h)) : r.y + dy;
    return {
      ...item,
      x1: roundCoord(nextX),
      y1: roundCoord(nextY),
      x2: roundCoord(nextX + w),
      y2: roundCoord(nextY + h),
    };
  }

  const textItem: Extract<Annotation, { kind: "text" }> = item;
  const startBounds = computeTextAnnotationBounds(textItem, safeW, measureFn);
  let nextX = textItem.x + dx;
  if (safeW > 0) {
    const rigidW = Math.min(startBounds.w, Math.max(0, safeW - TEXT_DRAFT_PAD_X * 2));
    const minX = Math.min(TEXT_DRAFT_PAD_X, Math.max(0, safeW - rigidW));
    const maxX = Math.max(minX, safeW - rigidW - TEXT_DRAFT_PAD_X);
    nextX = Math.min(Math.max(minX, nextX), maxX);
  }
  const movedXItem: Extract<Annotation, { kind: "text" }> = {
    ...textItem,
    x: roundCoord(nextX),
  };
  const nextBounds = computeTextAnnotationBounds(movedXItem, safeW, measureFn);
  let nextY = textItem.y + dy;
  if (safeH > 0) {
    const minY = Math.min(TEXT_DRAFT_PAD_Y, Math.max(0, safeH - nextBounds.h));
    const maxY = Math.max(minY, safeH - nextBounds.h - TEXT_DRAFT_PAD_Y);
    nextY = Math.min(Math.max(minY, nextY), maxY);
  }
  return { ...movedXItem, y: roundCoord(nextY) } as T;
}

/**
 * 按 8 个方向控制柄缩放矩形或椭圆边框尺寸，保证最小宽高不坍缩且始终落在画布内。
 */
export function resizeShapeAnnotationInStage<
  T extends Extract<Annotation, { kind: "rect" | "ellipse" }>,
>(
  item: T,
  handle: ResizeHandle,
  dx: number,
  dy: number,
  stageWidth: number,
  stageHeight: number,
): T {
  const r = normalizeRect(item.x1, item.y1, item.x2, item.y2);
  const safeW = Math.max(0, stageWidth);
  const safeH = Math.max(0, stageHeight);
  const minW = safeW > 0 ? Math.min(MIN_RESIZE_SHAPE_SIZE, safeW) : MIN_RESIZE_SHAPE_SIZE;
  const minH = safeH > 0 ? Math.min(MIN_RESIZE_SHAPE_SIZE, safeH) : MIN_RESIZE_SHAPE_SIZE;

  let nextLeft = r.x;
  let nextRight = r.x + r.w;
  let nextTop = r.y;
  let nextBottom = r.y + r.h;

  if (handle === "nw" || handle === "w" || handle === "sw") {
    const maxLeft = Math.max(0, nextRight - minW);
    nextLeft = Math.min(Math.max(0, nextLeft + dx), maxLeft);
  } else if (handle === "ne" || handle === "e" || handle === "se") {
    const upperRight = safeW > 0 ? safeW : Infinity;
    const minRight = Math.min(upperRight, nextLeft + minW);
    nextRight = Math.min(upperRight, Math.max(minRight, nextRight + dx));
  }

  if (handle === "nw" || handle === "n" || handle === "ne") {
    const maxTop = Math.max(0, nextBottom - minH);
    nextTop = Math.min(Math.max(0, nextTop + dy), maxTop);
  } else if (handle === "sw" || handle === "s" || handle === "se") {
    const upperBottom = safeH > 0 ? safeH : Infinity;
    const minBottom = Math.min(upperBottom, nextTop + minH);
    nextBottom = Math.min(upperBottom, Math.max(minBottom, nextBottom + dy));
  }

  return {
    ...item,
    x1: roundCoord(nextLeft),
    y1: roundCoord(nextTop),
    x2: roundCoord(nextRight),
    y2: roundCoord(nextBottom),
  };
}

/**
 * 为已提交文字标注设置新字号，并在贴近右缘或底边放大时自动微调坐标，防止放大后越界裁剪或无谓断字。
 */
export function applyTextAnnotationFontSize(
  item: Extract<Annotation, { kind: "text" }>,
  rawFontSize: number,
  stageWidth: number,
  stageHeight: number,
  measureFn: MeasureAnnotationTextFn = estimateTextLineWidth,
): Extract<Annotation, { kind: "text" }> {
  const currentFontSize = clampTextFontSize(item.fontSize);
  const requestedFontSize = clampTextFontSize(rawFontSize);
  const safeW = Math.max(0, stageWidth);
  const safeH = Math.max(0, stageHeight);

  const rawLines = item.text.split("\n");
  const prevBounds = computeTextAnnotationBounds(item, safeW, measureFn);
  const wasSoftWrapped = prevBounds.lines.length > rawLines.length;

  const buildAtFontSize = (fs: number) => {
    let nextX = item.x;
    if (safeW > 0) {
      const minCharW = Math.min(
        Math.ceil(fs * 1.05),
        Math.max(0, safeW - TEXT_DRAFT_PAD_X * 2),
      );
      const desiredW = wasSoftWrapped
        ? minCharW
        : Math.min(
            Math.ceil(rawLines.reduce((acc, line) => Math.max(acc, measureFn(line, fs)), 0)),
            Math.max(0, safeW - TEXT_DRAFT_PAD_X * 2),
          );
      const minX = Math.min(TEXT_DRAFT_PAD_X, Math.max(0, safeW - desiredW));
      const maxAllowedX = Math.max(minX, safeW - desiredW - TEXT_DRAFT_PAD_X);
      if (nextX > maxAllowedX) {
        nextX = maxAllowedX;
      }
    }
    const candidate = {
      ...item,
      x: roundCoord(nextX),
      fontSize: fs,
    };
    const nextBounds = computeTextAnnotationBounds(candidate, safeW, measureFn);
    return { candidate, nextBounds };
  };

  let chosen = buildAtFontSize(requestedFontSize);
  if (safeH > 0 && requestedFontSize > currentFontSize && chosen.nextBounds.h > safeH) {
    let fitted: ReturnType<typeof buildAtFontSize> | null = null;
    for (let fs = requestedFontSize - 1; fs >= currentFontSize; fs -= 1) {
      const trial = buildAtFontSize(fs);
      if (trial.nextBounds.h <= safeH) {
        fitted = trial;
        break;
      }
    }
    chosen = fitted ?? buildAtFontSize(currentFontSize);
  }

  const { candidate, nextBounds } = chosen;
  let nextY = candidate.y;
  if (safeH > 0) {
    const minY = Math.min(TEXT_DRAFT_PAD_Y, Math.max(0, safeH - nextBounds.h));
    const maxY = Math.max(minY, safeH - nextBounds.h - TEXT_DRAFT_PAD_Y);
    if (nextY > maxY) {
      nextY = maxY;
    } else if (nextY < minY) {
      nextY = minY;
    }
  }
  return { ...candidate, y: roundCoord(nextY) };
}

/**
 * 拖拽文字标注四角控制柄连续调节字号，保持对角锚点稳定且不越出画布边界。
 */
export function resizeTextAnnotationFontSize(
  item: Extract<Annotation, { kind: "text" }>,
  handle: TextResizeHandle,
  dx: number,
  dy: number,
  stageWidth: number,
  stageHeight: number,
  measureFn: MeasureAnnotationTextFn = estimateTextLineWidth,
): Extract<Annotation, { kind: "text" }> {
  const safeW = Math.max(0, stageWidth);
  const safeH = Math.max(0, stageHeight);
  const currentFontSize = clampTextFontSize(item.fontSize);
  const startBounds = computeTextAnnotationBounds(item, safeW, measureFn);
  const signX = handle === "ne" || handle === "se" ? 1 : -1;
  const signY = handle === "sw" || handle === "se" ? 1 : -1;
  const outX = dx * signX;
  const outY = dy * signY;
  const effW = Math.max(40, startBounds.w);
  const effH = Math.max(24, startBounds.h);
  const scale = Math.max(0.2, 1 + (outX / effW + outY / effH) * 0.55);
  const nextFontSize = clampTextFontSize(item.fontSize * scale);

  if (handle === "se") {
    return applyTextAnnotationFontSize(item, nextFontSize, safeW, safeH, measureFn);
  }

  const rawLines = item.text.split("\n");
  const anchorRight = startBounds.x + startBounds.w;
  const anchorBottom = startBounds.y + startBounds.h;

  const buildAnchoredAt = (fs: number) => {
    const unanchored = applyTextAnnotationFontSize(
      { ...item, x: TEXT_DRAFT_PAD_X, y: TEXT_DRAFT_PAD_Y },
      fs,
      safeW,
      safeH,
      measureFn,
    );
    const effFs = unanchored.fontSize;
    const unanchoredW =
      startBounds.lines.length > rawLines.length
        ? Math.round(startBounds.w * (effFs / Math.max(1, currentFontSize)))
        : effFs > 0
          ? computeTextAnnotationBounds(unanchored, safeW, measureFn).w
          : startBounds.w;

    let targetX = handle === "nw" || handle === "sw" ? anchorRight - unanchoredW : item.x;
    if (safeW > 0) {
      const rigidW = Math.min(unanchoredW, Math.max(0, safeW - TEXT_DRAFT_PAD_X * 2));
      const minX = Math.min(TEXT_DRAFT_PAD_X, Math.max(0, safeW - rigidW));
      const maxX = Math.max(minX, safeW - rigidW - TEXT_DRAFT_PAD_X);
      targetX = Math.min(Math.max(minX, targetX), maxX);
    }

    const withX = {
      ...item,
      x: roundCoord(targetX),
      fontSize: effFs,
    };
    const nextBounds = computeTextAnnotationBounds(withX, safeW, measureFn);
    let targetY = handle === "nw" || handle === "ne" ? anchorBottom - nextBounds.h : item.y;
    if (safeH > 0) {
      const minY = Math.min(TEXT_DRAFT_PAD_Y, Math.max(0, safeH - nextBounds.h));
      const maxY = Math.max(minY, safeH - nextBounds.h - TEXT_DRAFT_PAD_Y);
      targetY = Math.min(Math.max(minY, targetY), maxY);
    }
    return { result: { ...withX, y: roundCoord(targetY) }, nextBounds };
  };

  let chosen = buildAnchoredAt(nextFontSize);
  if (safeH > 0 && nextFontSize > currentFontSize && chosen.nextBounds.h > safeH) {
    let fitted: ReturnType<typeof buildAnchoredAt> | null = null;
    for (let fs = Math.min(nextFontSize, chosen.result.fontSize) - 1; fs >= currentFontSize; fs -= 1) {
      const trial = buildAnchoredAt(fs);
      if (trial.nextBounds.h <= safeH) {
        fitted = trial;
        break;
      }
    }
    chosen = fitted ?? buildAnchoredAt(currentFontSize);
  }
  return chosen.result;
}
