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
