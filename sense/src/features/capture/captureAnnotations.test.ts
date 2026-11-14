import { describe, expect, it } from "vitest";
import {
  ANNOTATION_COLORS,
  ANNOTATION_SIZES,
  TEXT_DRAFT_PAD_X,
  TEXT_DRAFT_PAD_Y,
  clampPoint,
  computeArrowHeadPoints,
  computeTextBoxLayout,
  isMeaningfulAnnotation,
  normalizeRect,
  type Annotation,
} from "./captureAnnotations";

describe("captureAnnotations 工具函数", () => {
  it("提供钉钉风格 6 色与 3 档线宽字号预设", () => {
    expect(ANNOTATION_COLORS).toHaveLength(6);
    expect(ANNOTATION_SIZES).toHaveLength(3);
    expect(ANNOTATION_SIZES.map((s) => s.id)).toEqual(["sm", "md", "lg"]);
  });

  it("clampPoint 将坐标限制在舞台 [0, width] x [0, height] 内", () => {
    expect(clampPoint({ x: -12, y: 150 }, 100, 80)).toEqual({ x: 0, y: 80 });
    expect(clampPoint({ x: 42, y: 36 }, 100, 80)).toEqual({ x: 42, y: 36 });
    expect(clampPoint({ x: 10, y: 10 }, 0, -5)).toEqual({ x: 0, y: 0 });
  });

  it("normalizeRect 兼容任意拖拽方向（右下到左上）", () => {
    expect(normalizeRect(120, 90, 20, 30)).toEqual({
      x: 20,
      y: 30,
      w: 100,
      h: 60,
    });
  });

  it("computeArrowHeadPoints 返回尖端与左右两翼三个顶点", () => {
    const [tip, left, right] = computeArrowHeadPoints(0, 0, 40, 0, 4);
    expect(tip).toEqual({ x: 40, y: 0 });
    expect(left.x).toBeLessThan(tip.x);
    expect(right.x).toBeLessThan(tip.x);
    expect(left.y).toBeGreaterThan(0);
    expect(right.y).toBeLessThan(0);

    const [zeroTip] = computeArrowHeadPoints(10, 10, 10, 10, 4);
    expect(zeroTip).toEqual({ x: 10, y: 10 });
  });

  it("isMeaningfulAnnotation 过滤极小误触图形和空白文字", () => {
    const tinyRect: Annotation = {
      id: "r1",
      kind: "rect",
      color: "#ff3b30",
      strokeWidth: 4,
      x1: 10,
      y1: 10,
      x2: 12,
      y2: 12,
    };
    const validRect: Annotation = {
      ...tinyRect,
      id: "r2",
      x2: 40,
      y2: 32,
    };
    const emptyText: Annotation = {
      id: "t1",
      kind: "text",
      color: "#ff3b30",
      fontSize: 18,
      x: 10,
      y: 10,
      text: "   ",
    };
    const validText: Annotation = {
      ...emptyText,
      id: "t2",
      text: "重点修改",
    };
    const singlePointPen: Annotation = {
      id: "p1",
      kind: "pen",
      color: "#007aff",
      strokeWidth: 4,
      points: [{ x: 10, y: 10 }],
    };
    const multiPointPen: Annotation = {
      ...singlePointPen,
      id: "p2",
      points: [
        { x: 10, y: 10 },
        { x: 18, y: 14 },
      ],
    };

    expect(isMeaningfulAnnotation(tinyRect)).toBe(false);
    expect(isMeaningfulAnnotation(validRect)).toBe(true);
    expect(isMeaningfulAnnotation(emptyText)).toBe(false);
    expect(isMeaningfulAnnotation(validText)).toBe(true);
    expect(isMeaningfulAnnotation(singlePointPen)).toBe(false);
    expect(isMeaningfulAnnotation(multiPointPen)).toBe(true);
  });

  it("[R3-P1-3] computeTextBoxLayout 边缘夹逼且与文本偏移一致，避免提交后跳变或溢出", () => {
    // 1) 画布内部点击：输入框在点击点，落盘 textX/textY 精确加 CSS 边衬 (6, 3)
    const interior = computeTextBoxLayout({
      x: 50,
      y: 40,
      value: "重点",
      fontSize: 16,
      stageWidth: 500,
      stageHeight: 300,
    });
    expect(interior.left).toBe(50);
    expect(interior.top).toBe(40);
    expect(interior.textX).toBe(50 + TEXT_DRAFT_PAD_X);
    expect(interior.textY).toBe(40 + TEXT_DRAFT_PAD_Y);
    expect(interior.lineHeight).toBe(20);

    // 2) 贴近右下角点击：先夹逼 left/top 再按同一规则推算 textX/textY，提交后不向外跳变
    const rightBottom = computeTextBoxLayout({
      x: 492,
      y: 294,
      value: "行1\n行2\n行3",
      fontSize: 16,
      stageWidth: 500,
      stageHeight: 300,
    });
    expect(rightBottom.rows).toBe(3);
    expect(rightBottom.left).toBe(390);
    expect(rightBottom.top).toBe(234);
    expect(rightBottom.textX).toBe(390 + TEXT_DRAFT_PAD_X);
    expect(rightBottom.textY).toBe(234 + TEXT_DRAFT_PAD_Y);
    expect(rightBottom.left + rightBottom.maxWidth).toBeLessThanOrEqual(500);
    expect(rightBottom.top + rightBottom.maxHeight).toBeLessThanOrEqual(300);

    // 3) 极窄画布不产出负位置
    const tiny = computeTextBoxLayout({
      x: 70,
      y: 35,
      value: "短",
      fontSize: 16,
      stageWidth: 80,
      stageHeight: 40,
    });
    expect(tiny.left).toBe(0);
    expect(tiny.top).toBeGreaterThanOrEqual(0);
    expect(tiny.textX).toBe(TEXT_DRAFT_PAD_X);
  });
});
