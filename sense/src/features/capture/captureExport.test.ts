import { afterEach, describe, expect, it, vi } from "vitest";
import type { OverlayBlock } from "../../lib/ipc";
import type { Annotation } from "./captureAnnotations";
import { renderCapturePngBase64 } from "./captureExport";
import type { BlockPlan } from "./overlayLayout";

interface MockOp {
  op: string;
  args: unknown[];
}

function createFakeEnvironment(imgMode: "ok" | "error" | "zero" = "ok", naturalW = 600, naturalH = 300) {
  const ops: MockOp[] = [];
  const canvases: Array<{ width: number; height: number }> = [];

  class FakeImage {
    onload: (() => void) | null = null;
    onerror: (() => void) | null = null;
    naturalWidth = imgMode === "zero" ? 0 : naturalW;
    naturalHeight = imgMode === "zero" ? 0 : naturalH;
    private _src = "";
    set src(value: string) {
      this._src = value;
      queueMicrotask(() => {
        if (imgMode === "error") {
          this.onerror?.();
        } else {
          this.onload?.();
        }
      });
    }
    get src() {
      return this._src;
    }
  }

  const fakeCtx = {
    fillStyle: "",
    strokeStyle: "",
    lineWidth: 1,
    lineCap: "butt",
    lineJoin: "miter",
    font: "",
    textBaseline: "alphabetic",
    textAlign: "start",
    direction: "inherit",
    globalAlpha: 1,
    shadowColor: "",
    shadowBlur: 0,
    shadowOffsetY: 0,
    save: () => ops.push({ op: "save", args: [] }),
    restore: () => ops.push({ op: "restore", args: [] }),
    scale: (x: number, y: number) => ops.push({ op: "scale", args: [x, y] }),
    drawImage: (...args: unknown[]) => ops.push({ op: "drawImage", args }),
    fillRect: (x: number, y: number, w: number, h: number) =>
      ops.push({ op: "fillRect", args: [x, y, w, h, fakeCtx.fillStyle] }),
    strokeRect: (x: number, y: number, w: number, h: number) =>
      ops.push({ op: "strokeRect", args: [x, y, w, h, fakeCtx.strokeStyle, fakeCtx.lineWidth] }),
    beginPath: () => ops.push({ op: "beginPath", args: [] }),
    closePath: () => ops.push({ op: "closePath", args: [] }),
    moveTo: (x: number, y: number) => ops.push({ op: "moveTo", args: [x, y] }),
    lineTo: (x: number, y: number, ) => ops.push({ op: "lineTo", args: [x, y] }),
    ellipse: (...args: unknown[]) => ops.push({ op: "ellipse", args }),
    stroke: () => ops.push({ op: "stroke", args: [fakeCtx.strokeStyle, fakeCtx.lineWidth] }),
    fill: () => ops.push({ op: "fill", args: [fakeCtx.fillStyle] }),
    fillText: (text: string, x: number, y: number) =>
      ops.push({
        op: "fillText",
        args: [text, x, y, fakeCtx.textAlign, fakeCtx.direction, fakeCtx.textBaseline, fakeCtx.font],
      }),
  };

  (globalThis as Record<string, unknown>).Image = FakeImage;
  (globalThis as Record<string, unknown>).document = {
    createElement: (tag: string) => {
      expect(tag).toBe("canvas");
      const canvas = {
        width: 0,
        height: 0,
        getContext: (kind: string) => (kind === "2d" ? fakeCtx : null),
        toDataURL: (mime: string) => {
          expect(mime).toBe("image/png");
          return "data:image/png;base64,FAKE_EXPORT";
        },
      };
      canvases.push(canvas);
      return canvas;
    },
  };

  return { ops, canvases };
}

const samplePlan = (overrides: Partial<BlockPlan> = {}): BlockPlan => ({
  cover: { x: 10, y: 12, w: 90, h: 24 },
  bg: "#ffffff",
  fg: "#111111",
  fontSize: 14,
  lineHeight: 18,
  lines: ["翻译行1", "翻译行2"],
  x: 12,
  y: 14,
  anchor: "left",
  dir: "ltr",
  truncated: false,
  ...overrides,
});

const sampleBlock = (overrides: Partial<OverlayBlock> = {}): OverlayBlock => ({
  text: "Translated",
  x: 10,
  y: 12,
  w: 90,
  h: 24,
  line_h: 18,
  translated: true,
  ...overrides,
});

describe("captureExport 导出画布合成", () => {
  const prevImage = (globalThis as Record<string, unknown>).Image;
  const prevDoc = (globalThis as Record<string, unknown>).document;

  afterEach(() => {
    (globalThis as Record<string, unknown>).Image = prevImage;
    (globalThis as Record<string, unknown>).document = prevDoc;
    vi.restoreAllMocks();
  });

  it("[R3-P2-1] 原图未就绪（null/空串）或图片损坏/0x0 时直接报错，不再悄悄导出黑底", async () => {
    const { ops } = createFakeEnvironment("ok", 600, 300);
    await expect(
      renderCapturePngBase64({
        originalImageUrl: null,
        stageWidth: 300,
        stageHeight: 150,
        scale: 2,
        includeTranslation: true,
        blocks: [],
        plans: [],
        annotations: [],
      }),
    ).rejects.toThrow("当前截屏原图尚未就绪，无法导出图片");
    expect(ops).toHaveLength(0);

    createFakeEnvironment("error");
    await expect(
      renderCapturePngBase64({
        originalImageUrl: "data:image/png;base64,BAD",
        stageWidth: 300,
        stageHeight: 150,
        scale: 2,
        includeTranslation: false,
        blocks: [],
        plans: [],
        annotations: [],
      }),
    ).rejects.toThrow("无法加载原图以导出");

    createFakeEnvironment("zero", 0, 0);
    await expect(
      renderCapturePngBase64({
        originalImageUrl: "data:image/png;base64,ZERO",
        stageWidth: 300,
        stageHeight: 150,
        scale: 2,
        includeTranslation: false,
        blocks: [],
        plans: [],
        annotations: [],
      }),
    ).rejects.toThrow("截屏原图加载失败，无法导出图片");
  });

  it("Retina 缩放画布仅画已翻译且有变化的块，并按 LTR/RTL 正确绘制全部 5 类标注", async () => {
    const { ops, canvases } = createFakeEnvironment("ok", 600, 300);
    const blocks: OverlayBlock[] = [
      sampleBlock({ text: "译文1", translated: true }),
      sampleBlock({ text: "مترجم", translated: true }),
      sampleBlock({ text: "unchanged", translated: true, unchanged: true }),
      sampleBlock({ text: "未翻译原文", translated: false }),
    ];
    const plans: BlockPlan[] = [
      samplePlan({ anchor: "left", dir: "ltr", lines: ["译文1"] }),
      samplePlan({
        cover: { x: 120, y: 12, w: 90, h: 24 },
        anchor: "right",
        dir: "rtl",
        x: 205,
        y: 14,
        lines: ["مترجم"],
      }),
      samplePlan({ cover: { x: 20, y: 60, w: 40, h: 16 }, lines: ["unchanged"] }),
      samplePlan({ cover: { x: 80, y: 60, w: 40, h: 16 }, lines: ["未翻译原文"] }),
    ];
    const annotations: Annotation[] = [
      { id: "r", kind: "rect", x1: 20, y1: 30, x2: 50, y2: 70, color: "#ff3b30", strokeWidth: 3 },
      { id: "e", kind: "ellipse", x1: 60, y1: 30, x2: 100, y2: 70, color: "#ff9500", strokeWidth: 3 },
      { id: "a", kind: "arrow", x1: 10, y1: 80, x2: 100, y2: 120, color: "#007aff", strokeWidth: 4 },
      {
        id: "p",
        kind: "pen",
        points: [
          { x: 5, y: 5 },
          { x: 15, y: 12 },
          { x: 25, y: 20 },
        ],
        color: "#34c759",
        strokeWidth: 2,
      },
      { id: "t", kind: "text", x: 18, y: 28, text: "第一行\n第二行", color: "#ffffff", fontSize: 16 },
    ];

    const out = await renderCapturePngBase64({
      originalImageUrl: "data:image/png;base64,VALID",
      stageWidth: 300,
      stageHeight: 150,
      scale: 2,
      includeTranslation: true,
      blocks,
      plans,
      annotations,
    });

    expect(out).toBe("data:image/png;base64,FAKE_EXPORT");
    expect(canvases[0].width).toBe(600);
    expect(canvases[0].height).toBe(300);
    expect(ops.find((o) => o.op === "scale")?.args).toEqual([2, 2]);
    // 只有前两个真正 translated && !unchanged 的块画了遮盖与译文，不画 #16161b 黑底
    const fillRects = ops.filter((o) => o.op === "fillRect");
    expect(fillRects).toHaveLength(2);
    expect(fillRects.every((o) => o.args[4] !== "#16161b")).toBe(true);

    const texts = ops.filter((o) => o.op === "fillText");
    expect(texts.map((t) => t.args[0])).toEqual(["译文1", "مترجم", "第一行", "第二行"]);
    expect(texts[0].args[3]).toBe("left");
    expect(texts[0].args[4]).toBe("ltr");
    expect(texts[1].args[3]).toBe("right");
    expect(texts[1].args[4]).toBe("rtl");
    // 多行文本行高 round(16 * 1.25) = 20
    expect(texts[2].args[1]).toBe(18);
    expect(texts[2].args[2]).toBe(28);
    expect(texts[3].args[1]).toBe(18);
    expect(texts[3].args[2]).toBe(48);

    // 5 类标注都有对应 canvas 绘制
    expect(ops.some((o) => o.op === "strokeRect")).toBe(true);
    expect(ops.some((o) => o.op === "ellipse")).toBe(true);
    expect(ops.some((o) => o.op === "fill")).toBe(true); // 箭头三角形
    expect(ops.filter((o) => o.op === "stroke").length).toBeGreaterThanOrEqual(3); // 椭圆 + 箭头杆 + 涂鸦
  });

  it("includeTranslation=false（仅带标注原图）不绘制译文遮盖与译文文字", async () => {
    const { ops } = createFakeEnvironment("ok", 300, 150);
    await renderCapturePngBase64({
      originalImageUrl: "data:image/png;base64,VALID",
      stageWidth: 300,
      stageHeight: 150,
      scale: 1,
      includeTranslation: false,
      blocks: [sampleBlock()],
      plans: [samplePlan()],
      annotations: [{ id: "r", kind: "rect", x1: 10, y1: 10, x2: 60, y2: 60, color: "#ff3b30", strokeWidth: 3 }],
    });

    expect(ops.filter((o) => o.op === "fillRect")).toHaveLength(0);
    expect(ops.filter((o) => o.op === "fillText")).toHaveLength(0);
    expect(ops.filter((o) => o.op === "strokeRect")).toHaveLength(1);
  });
});
