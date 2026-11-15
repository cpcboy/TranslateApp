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
  const maxBoxW = Math.min(110, safeW);
  const left = Math.max(0, Math.min(x, Math.max(0, safeW - maxBoxW)));
  const availableW = Math.max(0, safeW - left);
  const minFloor = safeW > 0 ? Math.min(40, safeW) : 0;
  const maxWidth = safeW > 0 ? Math.max(minFloor, availableW) : 0;
  const minWidth = safeW > 0 ? Math.min(96, maxWidth) : 0;
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
