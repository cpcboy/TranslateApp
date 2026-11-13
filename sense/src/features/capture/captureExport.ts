import type { OverlayBlock } from "../../lib/ipc";
import { OVERLAY_FONT, type BlockPlan } from "./overlayLayout";
import { computeArrowHeadPoints, normalizeRect, type Annotation } from "./captureAnnotations";

function loadImageElement(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error("无法加载原图以导出"));
    img.src = src;
  });
}

function drawBlocksToContext(
  ctx: CanvasRenderingContext2D,
  blocks: OverlayBlock[],
  plans: BlockPlan[],
  hasOriginalImage: boolean,
) {
  const shouldDraw = (b: OverlayBlock | undefined) =>
    !!b && !b.unchanged && (!hasOriginalImage || b.translated === true);

  for (let i = 0; i < plans.length; i += 1) {
    if (!shouldDraw(blocks[i])) continue;
    const { cover, bg } = plans[i];
    ctx.fillStyle = bg;
    ctx.fillRect(cover.x, cover.y, cover.w, cover.h);
  }

  for (let i = 0; i < plans.length; i += 1) {
    const block = blocks[i];
    if (!shouldDraw(block)) continue;
    const plan = plans[i];
    const translated = block?.translated ?? false;
    ctx.save();
    ctx.font = `${plan.fontSize}px ${OVERLAY_FONT}`;
    ctx.fillStyle = plan.fg;
    ctx.textBaseline = "top";
    ctx.textAlign = plan.anchor === "right" ? "right" : "left";
    if ("direction" in ctx) {
      (ctx as CanvasRenderingContext2D & { direction: string }).direction = plan.dir;
    }
    ctx.globalAlpha = translated ? 1 : 0.82;
    for (let li = 0; li < plan.lines.length; li += 1) {
      const lineY = plan.y + li * plan.lineHeight;
      ctx.fillText(plan.lines[li], plan.x, lineY);
    }
    ctx.restore();
  }
}

function drawAnnotationsToContext(ctx: CanvasRenderingContext2D, annotations: Annotation[]) {
  for (const item of annotations) {
    ctx.save();
    ctx.strokeStyle = item.color;
    ctx.fillStyle = item.color;

    if (item.kind === "rect") {
      const r = normalizeRect(item.x1, item.y1, item.x2, item.y2);
      ctx.lineWidth = item.strokeWidth;
      ctx.lineJoin = "miter";
      ctx.strokeRect(r.x, r.y, r.w, r.h);
    } else if (item.kind === "ellipse") {
      const r = normalizeRect(item.x1, item.y1, item.x2, item.y2);
      ctx.lineWidth = item.strokeWidth;
      ctx.beginPath();
      ctx.ellipse(r.x + r.w / 2, r.y + r.h / 2, r.w / 2, r.h / 2, 0, 0, Math.PI * 2);
      ctx.stroke();
    } else if (item.kind === "arrow") {
      const [tip, left, right] = computeArrowHeadPoints(
        item.x1,
        item.y1,
        item.x2,
        item.y2,
        item.strokeWidth,
      );
      const baseMidX = (left.x + right.x) / 2;
      const baseMidY = (left.y + right.y) / 2;
      ctx.lineWidth = item.strokeWidth;
      ctx.lineCap = "round";
      ctx.beginPath();
      ctx.moveTo(item.x1, item.y1);
      ctx.lineTo(baseMidX, baseMidY);
      ctx.stroke();

      ctx.beginPath();
      ctx.moveTo(tip.x, tip.y);
      ctx.lineTo(left.x, left.y);
      ctx.lineTo(right.x, right.y);
      ctx.closePath();
      ctx.fill();
    } else if (item.kind === "pen" && item.points.length >= 2) {
      ctx.lineWidth = item.strokeWidth;
      ctx.lineCap = "round";
      ctx.lineJoin = "round";
      ctx.beginPath();
      ctx.moveTo(item.points[0].x, item.points[0].y);
      for (let i = 1; i < item.points.length; i += 1) {
        ctx.lineTo(item.points[i].x, item.points[i].y);
      }
      ctx.stroke();
    } else if (item.kind === "text" && item.text.trim()) {
      ctx.font = `600 ${item.fontSize}px ${OVERLAY_FONT}`;
      ctx.textBaseline = "top";
      ctx.textAlign = "left";
      ctx.shadowColor = "rgba(0, 0, 0, 0.45)";
      ctx.shadowBlur = 2;
      ctx.shadowOffsetY = 1;
      const lineH = Math.round(item.fontSize * 1.25);
      const lines = item.text.split("\n");
      for (let i = 0; i < lines.length; i += 1) {
        ctx.fillText(lines[i], item.x, item.y + i * lineH);
      }
    }

    ctx.restore();
  }
}

export async function renderCapturePngBase64(options: {
  originalImageUrl: string | null;
  stageWidth: number;
  stageHeight: number;
  scale: number;
  includeTranslation: boolean;
  blocks: OverlayBlock[];
  plans: BlockPlan[];
  annotations: Annotation[];
}): Promise<string> {
  const {
    originalImageUrl,
    stageWidth,
    stageHeight,
    scale,
    includeTranslation,
    blocks,
    plans,
    annotations,
  } = options;

  const img = originalImageUrl ? await loadImageElement(originalImageUrl) : null;
  const targetW = img && img.naturalWidth > 0 ? img.naturalWidth : Math.max(1, Math.round(stageWidth * scale));
  const targetH = img && img.naturalHeight > 0 ? img.naturalHeight : Math.max(1, Math.round(stageHeight * scale));

  const canvas = document.createElement("canvas");
  canvas.width = targetW;
  canvas.height = targetH;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("无法创建图片导出画布");

  const ratioX = stageWidth > 0 ? targetW / stageWidth : scale;
  const ratioY = stageHeight > 0 ? targetH / stageHeight : scale;
  ctx.scale(ratioX, ratioY);

  if (img) {
    ctx.drawImage(img, 0, 0, stageWidth, stageHeight);
  } else {
    ctx.fillStyle = "#16161b";
    ctx.fillRect(0, 0, stageWidth, stageHeight);
  }

  if (includeTranslation && blocks.length > 0 && plans.length > 0) {
    drawBlocksToContext(ctx, blocks, plans, Boolean(img));
  }
  if (annotations.length > 0) {
    drawAnnotationsToContext(ctx, annotations);
  }

  return canvas.toDataURL("image/png");
}
