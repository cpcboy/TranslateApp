import { describe, expect, it } from "vitest";
import {
  ANNOTATION_COLORS,
  ANNOTATION_SIZES,
  MAX_TEXT_FONT_SIZE,
  MIN_RESIZE_SHAPE_SIZE,
  MIN_TEXT_FONT_SIZE,
  TEXT_DRAFT_PAD_X,
  TEXT_DRAFT_PAD_Y,
  applyTextAnnotationFontSize,
  clampPoint,
  clampTextFontSize,
  computeArrowHeadPoints,
  computeTextAnnotationBounds,
  computeTextBoxLayout,
  estimateTextLineWidth,
  getShapeResizeHandles,
  getTextResizeHandles,
  isMeaningfulAnnotation,
  moveAnnotationInStage,
  normalizeRect,
  resizeShapeAnnotationInStage,
  resizeTextAnnotationFontSize,
  wrapAnnotationText,
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

    // 3) 极窄画布不产出负位置且宽度收紧在画布内
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
    expect(tiny.minWidth).toBeLessThanOrEqual(80);
    expect(tiny.width).toBeLessThanOrEqual(80);

    // 4) 宽画布输入较长纯中文、引号与多空格文本时，输入框宽度随内容动态放宽且不越出画布
    const emptyWide = computeTextBoxLayout({
      x: 40,
      y: 50,
      value: "",
      fontSize: 18,
      stageWidth: 800,
      stageHeight: 600,
    });
    const wideChinese = computeTextBoxLayout({
      x: 40,
      y: 50,
      value: '重要提示："多空格   测试中文长句不要被过窄裁剪"',
      fontSize: 18,
      stageWidth: 800,
      stageHeight: 600,
    });
    expect(wideChinese.width).toBeGreaterThan(emptyWide.width);
    expect(wideChinese.width).toBeGreaterThan(260);
    expect(wideChinese.left + wideChinese.width).toBeLessThanOrEqual(800);

    // 5) 零宽画布无边界跳变
    const zeroW = computeTextBoxLayout({
      x: 10,
      y: 10,
      value: "测试",
      fontSize: 18,
      stageWidth: 0,
      stageHeight: 40,
    });
    expect(zeroW.left).toBe(0);
    expect(zeroW.minWidth).toBe(0);
    expect(zeroW.width).toBe(0);
    expect(zeroW.maxWidth).toBe(0);

    // 6) 极扁画布 (stageHeight < boxHeight) 贴顶保底可读高
    const flatStrip = computeTextBoxLayout({
      x: 20,
      y: 10,
      value: "扁条测试",
      fontSize: 18,
      stageWidth: 300,
      stageHeight: 16,
    });
    expect(flatStrip.top).toBe(0);
    expect(flatStrip.maxHeight).toBeGreaterThanOrEqual(flatStrip.lineHeight + TEXT_DRAFT_PAD_Y * 2);

    // 7) 舞台高度充足时多行文本按实际行数展开（上限 12 行），超出舞台可容纳行数时按舞台容量封顶且与提交态零跳变
    const sixLines = computeTextBoxLayout({
      x: 10,
      y: 10,
      value: "L1\nL2\nL3\nL4\nL5\nL6",
      fontSize: 18,
      stageWidth: 500,
      stageHeight: 400,
    });
    expect(sixLines.rows).toBe(6);

    const fifteenLines = computeTextBoxLayout({
      x: 10,
      y: 320,
      value: Array.from({ length: 15 }, (_, idx) => `L${idx + 1}`).join("\n"),
      fontSize: 18,
      stageWidth: 500,
      stageHeight: 400,
    });
    expect(fifteenLines.rows).toBe(12);
    const fullFifteenTextH = 15 * fifteenLines.lineHeight;
    const expectedCommittedFifteenY = Math.max(
      TEXT_DRAFT_PAD_Y,
      Math.min(fifteenLines.textY, Math.max(TEXT_DRAFT_PAD_Y, 400 - fullFifteenTextH - TEXT_DRAFT_PAD_Y)),
    );
    expect(fifteenLines.textY).toBe(expectedCommittedFifteenY);

    // 8) 360×190 画布底边偏下 (x=245, y=66) 输入折成 7 行的中文批注：输入态直接显示 7 行并实时上推 top，与回车提交态 Y 零跳变
    const sevenLineBottom = computeTextBoxLayout({
      x: 245,
      y: 66,
      value: "底边右角七行中文批注测试看看输入态和提交态会不会发生纵向跳移呢",
      fontSize: 18,
      stageWidth: 360,
      stageHeight: 190,
    });
    expect(sevenLineBottom.wrappedLines.length).toBeGreaterThanOrEqual(6);
    expect(sevenLineBottom.rows).toBe(sevenLineBottom.wrappedLines.length);
    const totalBottomTextH = sevenLineBottom.wrappedLines.length * sevenLineBottom.lineHeight;
    const committedBottomY = Math.max(
      TEXT_DRAFT_PAD_Y,
      Math.min(
        sevenLineBottom.textY,
        Math.max(TEXT_DRAFT_PAD_Y, 190 - totalBottomTextH - TEXT_DRAFT_PAD_Y),
      ),
    );
    expect(sevenLineBottom.textY).toBe(committedBottomY);
    expect(sevenLineBottom.top + sevenLineBottom.rows * sevenLineBottom.lineHeight + TEXT_DRAFT_PAD_Y * 2).toBeLessThanOrEqual(190);

    const wideCaps = computeTextBoxLayout({
      x: 20,
      y: 20,
      value: "WWWWMMMM@#WWWWMMMM",
      fontSize: 18,
      stageWidth: 400,
      stageHeight: 200,
    });
    expect(wideCaps.width).toBeGreaterThan(240);
    expect(wideCaps.left + wideCaps.width).toBeLessThanOrEqual(400);
  });

  it("[FIX-RIGHT-EDGE-WRAP] 画布右侧边缘单行超长中文/英文批注自动折行，零字符丢失且不越出 stageWidth", () => {
    // 复现外部真机 QC 场景：360px 宽舞台在 x=245 处输入 17 字超长中文单行
    const longZh = "右侧边缘超长中文批注测试会不会被裁剪";
    const rightLong = computeTextBoxLayout({
      x: 245,
      y: 18,
      value: longZh,
      fontSize: 18,
      stageWidth: 360,
      stageHeight: 190,
    });

    expect(rightLong.left).toBe(245);
    expect(rightLong.width).toBe(115);
    expect(rightLong.wrappedLines.length).toBeGreaterThanOrEqual(3);
    expect(rightLong.rows).toBe(Math.min(5, rightLong.wrappedLines.length));
    // 所有折行拼接回原文 0 字符丢失，且每一行右边缘都严格落在 360px 舞台内
    expect(rightLong.wrappedLines.join("")).toBe(longZh);
    for (const line of rightLong.wrappedLines) {
      expect(rightLong.textX + estimateTextLineWidth(line, 18)).toBeLessThanOrEqual(360);
    }

    // 英文带空格优先按词折行，超长连续单词再按字符兜底切分，极小 maxLineWidth 也不死循环
    const enLines = wrapAnnotationText(
      "Hello world Supercalifragilisticexpialidocious",
      16,
      95,
    );
    expect(enLines.length).toBeGreaterThanOrEqual(4);
    expect(enLines[0]).toBe("Hello");
    expect(enLines[1]).toBe("world");
    for (const line of enLines) {
      expect(estimateTextLineWidth(line, 16)).toBeLessThanOrEqual(95);
    }

    const tinyWrap = wrapAnnotationText("测A😊", 18, 4);
    expect(tinyWrap).toEqual(["测", "A", "😊"]);
  });

  it("[FEAT-SHAPE-MOVE-RESIZE] 矩形与椭圆支持 8 控制柄缩放与画布内平移，保持最小尺寸不翻转不越界", () => {
    const invertedRect: Extract<Annotation, { kind: "rect" | "ellipse" }> = {
      id: "r-inv",
      kind: "rect",
      x1: 160,
      y1: 120,
      x2: 60,
      y2: 40,
      color: "#ff3b30",
      strokeWidth: 4,
    };
    const norm = normalizeRect(invertedRect.x1, invertedRect.y1, invertedRect.x2, invertedRect.y2);
    const handles = getShapeResizeHandles(norm);
    expect(handles.map((h) => h.handle)).toEqual(["nw", "n", "ne", "e", "se", "s", "sw", "w"]);
    expect(handles.find((h) => h.handle === "n")).toMatchObject({ x: 110, y: 40, cursor: "ns-resize" });
    expect(handles.find((h) => h.handle === "e")).toMatchObject({ x: 160, y: 80, cursor: "ew-resize" });

    // 右下角扩大并越过画布右/下边界时，自动夹紧到 300×200
    const grownSe = resizeShapeAnnotationInStage(invertedRect, "se", 200, 150, 300, 200);
    expect(grownSe).toMatchObject({ x1: 60, y1: 40, x2: 300, y2: 200 });

    // 左上角向内过度推挤时，保持最小尺寸 MIN_RESIZE_SHAPE_SIZE (8px)，绝不翻转或塌缩为 0
    const shrunkNw = resizeShapeAnnotationInStage(invertedRect, "nw", 500, 500, 300, 200);
    expect(shrunkNw).toMatchObject({
      x1: 160 - MIN_RESIZE_SHAPE_SIZE,
      y1: 120 - MIN_RESIZE_SHAPE_SIZE,
      x2: 160,
      y2: 120,
    });

    // 单边控制柄（n/s/e/w）只改对应边，对边与正交方向不变
    const ellipse: Extract<Annotation, { kind: "rect" | "ellipse" }> = {
      id: "e1",
      kind: "ellipse",
      x1: 50,
      y1: 40,
      x2: 150,
      y2: 100,
      color: "#34c759",
      strokeWidth: 4,
    };
    expect(resizeShapeAnnotationInStage(ellipse, "w", -30, 25, 300, 200)).toMatchObject({
      x1: 20,
      y1: 40,
      x2: 150,
      y2: 100,
    });
    expect(resizeShapeAnnotationInStage(ellipse, "n", 40, -25, 300, 200)).toMatchObject({
      x1: 50,
      y1: 15,
      x2: 150,
      y2: 100,
    });
    expect(resizeShapeAnnotationInStage(ellipse, "ne", 40, -100, 300, 200)).toMatchObject({
      x1: 50,
      y1: 0,
      x2: 190,
      y2: 100,
    });
    expect(resizeShapeAnnotationInStage(ellipse, "sw", -100, 45, 300, 200)).toMatchObject({
      x1: 0,
      y1: 40,
      x2: 150,
      y2: 145,
    });

    // 平移矩形/椭圆在四壁处保持原宽高夹紧
    expect(moveAnnotationInStage(ellipse, 400, 300, 300, 200)).toMatchObject({
      x1: 200,
      y1: 140,
      x2: 300,
      y2: 200,
    });
    expect(moveAnnotationInStage(ellipse, -200, -200, 300, 200)).toMatchObject({
      x1: 0,
      y1: 0,
      x2: 100,
      y2: 60,
    });
  });

  it("[FEAT-TEXT-MOVE-AND-FONTSIZE] 已提交文字支持画布内平移（右壁防挤竖排、左移自动展开）与四角/步进字号缩放", () => {
    expect(clampTextFontSize(4)).toBe(MIN_TEXT_FONT_SIZE);
    expect(clampTextFontSize(120)).toBe(MAX_TEXT_FONT_SIZE);
    expect(clampTextFontSize(23.6)).toBe(24);
    expect(clampTextFontSize(Number.NaN)).toBe(18);

    const shortLabel: Extract<Annotation, { kind: "text" }> = {
      id: "t-short",
      kind: "text",
      x: 40,
      y: 30,
      text: "重点核对",
      color: "#ffcc00",
      fontSize: 18,
    };
    const initialBounds = computeTextAnnotationBounds(shortLabel, 360);
    expect(initialBounds.lines).toEqual(["重点核对"]);
    expect(getTextResizeHandles(initialBounds).map((h) => h.handle)).toEqual(["nw", "ne", "se", "sw"]);

    // 向右拖到底：保持单行宽度贴靠右壁，不会把 4 个字挤成单字竖排
    const movedRight = moveAnnotationInStage(shortLabel, 500, 400, 360, 190);
    const movedRightBounds = computeTextAnnotationBounds(movedRight, 360);
    expect(movedRightBounds.lines).toEqual(["重点核对"]);
    expect(movedRight.x + movedRightBounds.w).toBeLessThanOrEqual(360 - TEXT_DRAFT_PAD_X + 1);
    expect(movedRight.y + movedRightBounds.h).toBeLessThanOrEqual(190 - TEXT_DRAFT_PAD_Y + 1);

    // 原先在右缘自动折成多行的长文本，向左拖到宽敞区域后自然展开为单行
    const rightWrapped: Extract<Annotation, { kind: "text" }> = {
      id: "t-wrap",
      kind: "text",
      x: 260,
      y: 25,
      text: "右缘折行左移展开",
      color: "#ffffff",
      fontSize: 18,
    };
    expect(computeTextAnnotationBounds(rightWrapped, 360).lines.length).toBeGreaterThanOrEqual(2);
    const movedLeft = moveAnnotationInStage(rightWrapped, -230, 10, 360, 190);
    expect(movedLeft.x).toBe(30);
    expect(computeTextAnnotationBounds(movedLeft, 360).lines).toEqual(["右缘折行左移展开"]);

    // 贴近右下角放大字号时自动左移/上移防越界裁剪
    const enlargedEdge = applyTextAnnotationFontSize(movedRight, 36, 360, 190);
    const enlargedBounds = computeTextAnnotationBounds(enlargedEdge, 360);
    expect(enlargedEdge.fontSize).toBe(36);
    expect(enlargedBounds.lines).toEqual(["重点核对"]);
    expect(enlargedEdge.x + enlargedBounds.w).toBeLessThanOrEqual(360);
    expect(enlargedEdge.y + enlargedBounds.h).toBeLessThanOrEqual(190);

    // 拖拽右下角（se）向外放大、向内缩小；拖拽左上角（nw）向外放大时右底锚点保持稳定
    const midText: Extract<Annotation, { kind: "text" }> = {
      id: "t-mid",
      kind: "text",
      x: 120,
      y: 80,
      text: "字号缩放",
      color: "#ff3b30",
      fontSize: 20,
    };
    const grownSe = resizeTextAnnotationFontSize(midText, "se", 60, 30, 360, 220);
    expect(grownSe.fontSize).toBeGreaterThan(20);
    expect(grownSe.x).toBe(120);
    expect(grownSe.y).toBe(80);

    const shrunkSe = resizeTextAnnotationFontSize(midText, "se", -80, -40, 360, 220);
    expect(shrunkSe.fontSize).toBeLessThan(20);
    expect(shrunkSe.fontSize).toBeGreaterThanOrEqual(MIN_TEXT_FONT_SIZE);

    const grownNw = resizeTextAnnotationFontSize(midText, "nw", -50, -25, 360, 220);
    expect(grownNw.fontSize).toBeGreaterThan(20);
    expect(grownNw.x).toBeLessThan(midText.x);
    expect(grownNw.y).toBeLessThan(midText.y);
  });

  it("[FIX-QC-TEXT-BOUNDARY] 多行文字放大受画布高度封顶防顶底同溢，且大字号在右侧起笔自适应向左扩宽防单字竖列", () => {
    // [P1-3] 190px 高画布中 3 行文字拖大至 72px 时自动封顶在可完整容纳的最大字号（50px -> 3 * 63 = 189 <= 190）
    const threeLineText: Extract<Annotation, { kind: "text" }> = {
      id: "t-3lines",
      kind: "text",
      x: 24,
      y: 20,
      text: "第一行重要说明\n第二行补充细节\n第三行底部不溢出",
      color: "#ff3b30",
      fontSize: 18,
    };
    const cappedByApply = applyTextAnnotationFontSize(threeLineText, 72, 480, 190);
    const cappedBounds = computeTextAnnotationBounds(cappedByApply, 480);
    expect(cappedByApply.fontSize).toBe(50);
    expect(cappedBounds.h).toBeLessThanOrEqual(190);
    expect(cappedByApply.y).toBeGreaterThanOrEqual(0);
    expect(cappedByApply.y + cappedBounds.h).toBeLessThanOrEqual(190);

    // 四角缩放（含 nw / ne / sw / se）同样严格遵守画布高度封顶，且缩小字号始终放行
    const cappedBySe = resizeTextAnnotationFontSize(threeLineText, "se", 600, 400, 480, 190);
    expect(cappedBySe.fontSize).toBe(50);
    expect(computeTextAnnotationBounds(cappedBySe, 480).h).toBeLessThanOrEqual(190);

    const cappedByNw = resizeTextAnnotationFontSize(
      { ...threeLineText, x: 160, y: 80 },
      "nw",
      -600,
      -400,
      480,
      190,
    );
    expect(cappedByNw.fontSize).toBe(50);
    expect(computeTextAnnotationBounds(cappedByNw, 480).h).toBeLessThanOrEqual(190);

    // 若初始多行在 18px 已超超高（如 10 行 230px > 190px），点 A+ 不会反常缩小，点 A- 仍可正常缩小到 12px
    const tenLines: Extract<Annotation, { kind: "text" }> = {
      id: "t-10lines",
      kind: "text",
      x: 12,
      y: 3,
      text: "1\n2\n3\n4\n5\n6\n7\n8\n9\n10",
      color: "#ffffff",
      fontSize: 18,
    };
    expect(applyTextAnnotationFontSize(tenLines, 22, 480, 190).fontSize).toBe(18);
    expect(applyTextAnnotationFontSize(tenLines, 14, 480, 190).fontSize).toBe(14);

    // [P2-1] 62px 大字号在 480px 画布右侧 x=400 起笔输入「重点关注」，自动向左借宽保持单行横排，绝不挤成 4 行单字竖列
    const largeRightDraft = computeTextBoxLayout({
      x: 400,
      y: 30,
      value: "重点关注",
      fontSize: 62,
      stageWidth: 480,
      stageHeight: 190,
    });
    expect(largeRightDraft.wrappedLines).toEqual(["重点关注"]);
    expect(largeRightDraft.rows).toBe(1);
    expect(largeRightDraft.left + largeRightDraft.width).toBeLessThanOrEqual(480);
  });
});
