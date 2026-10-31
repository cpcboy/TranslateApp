import { Component, useEffect, useMemo, useState, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, type OverlayBlock } from "../../lib/ipc";
import { OVERLAY_FONT, createCanvasMeasure, plainPlan, planOverlay, sanitizeBlocks, type BlockPlan } from "./overlayLayout";

/**
 * 译文覆盖层：窗口正好压在选区上，按 OCR 的位置逐块画出来。
 *
 * 用原文底下的底色盖住原文，译文写在原来的位置、用原文的字号和颜色——这就是「原位替换」。
 * 怎么排（字号怎么定、底色字色怎么取、放不下怎么办）都在 overlayLayout.ts。
 *
 * 结构分两层：外层 `TranslateOverlay` 只管「取数据 + Esc / 点击 / 失焦关闭」，自己几乎不算东西；
 * 里面的 `OverlayView`（排版、画层、提示条）包在错误边界里。渲染一旦抛出来，整棵树卸载，
 * 关闭用的处理器跟着没了，屏幕上就留下一个关不掉、看不见的透明置顶窗。
 * 所以画的部分出了错，最多退成一条带着译文的错误提示——关闭的处理器必须还在。
 */
export function TranslateOverlay() {
  const [blocks, setBlocks] = useState<OverlayBlock[] | null>(null);
  // 翻译失败的原因。有它时下面画的是识别出的原文，必须明说，不能让原文冒充译文
  const [error, setError] = useState<string | null>(null);
  // 翻译成功了，但有话要说（比如没能确定选区位置，译文不一定对准原文）
  const [notice, setNotice] = useState<string | null>(null);
  const [hint, setHint] = useState(true);

  useEffect(() => {
    void api
      .captureBlocks()
      .then((view) => {
        // IPC 边界上先清洗：后面就不用处处防 null。数据格式不对（不是数组）是错误，不能当成「没识别到文字」；
        // 只是丢了几条不合格的，当提示说一声
        const { blocks: clean, problem } = sanitizeBlocks(view?.blocks);
        const malformed = !Array.isArray(view?.blocks);
        setBlocks(clean);
        setError(view?.error ?? (malformed ? problem : null));
        setNotice([view?.notice, malformed ? null : problem].filter(Boolean).join("；") || null);
      })
      .catch((err) => {
        setBlocks([]);
        setError(String(err).replace(/^Error:\s*/, ""));
      });

    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") void api.closeOverlay();
    };
    window.addEventListener("keydown", onKey);
    const timer = window.setTimeout(() => setHint(false), 2200);

    // 点到别的窗口就收起来：否则这层一直浮在最上面，Esc 又发不到它
    const mountedAt = Date.now();
    let unlisten: (() => void) | undefined;
    void getCurrentWindow()
      .onFocusChanged(({ payload }) => {
        if (!payload && Date.now() - mountedAt > 200) void api.closeOverlay();
      })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {
        /* 拿不到焦点事件就退回只靠 Esc */
      });

    return () => {
      window.removeEventListener("keydown", onKey);
      window.clearTimeout(timer);
      unlisten?.();
    };
  }, []);

  return (
    <div
      className="relative h-full w-full overflow-hidden"
      style={{ fontFamily: OVERLAY_FONT }}
      onClick={() => void api.closeOverlay()}
    >
      <OverlayBoundary blocks={blocks}>
        <OverlayView blocks={blocks} error={error} notice={notice} hint={hint} />
      </OverlayBoundary>
    </div>
  );
}

/** 排版、画层和各种提示条：全部数据相关的计算都在这里，出错由外面的错误边界接住。 */
function OverlayView({
  blocks,
  error,
  notice,
  hint,
}: {
  blocks: OverlayBlock[] | null;
  error: string | null;
  notice: string | null;
  hint: boolean;
}) {
  // 窗口正好压在选区上，innerWidth / innerHeight 就是选区的逻辑尺寸
  const plans = useMemo(() => {
    if (!blocks) return [];
    const bounds = { w: window.innerWidth, h: window.innerHeight };
    try {
      return planOverlay(blocks, createCanvasMeasure(), bounds);
    } catch (err) {
      // planOverlay 自己已经逐块兜住了；这是最后一道，整屏退回最简排版
      console.error("[overlay] 排版出错，整屏按简单样式显示：", err);
      return blocks.map((block) => plainPlan(block, bounds));
    }
  }, [blocks]);
  const nothingToReplace = isNothingToReplace(blocks);

  return (
    <>
      {blocks && <OverlayLayers blocks={blocks} plans={plans} />}

      {nothingToReplace && !error && (
        <div
          role="status"
          className="absolute inset-x-1 top-1 z-10 rounded bg-[rgba(14,14,18,0.95)] px-2.5 py-1.5 text-[12.5px] leading-snug text-[#f6f3ed]"
        >
          译文和原文一样，没有要替换的内容。点击或 Esc 关闭。
        </div>
      )}

      {blocks !== null && blocks.length === 0 && !error && (
        <div className="absolute left-2 top-2 rounded bg-[rgba(14,14,18,0.95)] px-3 py-2 text-[13px] text-[#f6f3ed]">
          没识别到文字
        </div>
      )}

      {error && (
        <div
          role="alert"
          className="absolute inset-x-1 top-1 z-10 max-h-[calc(100%-8px)] overflow-y-auto rounded border border-[rgba(255,107,74,0.6)] bg-[rgba(48,14,10,0.97)] px-2.5 py-1.5 text-[12.5px] leading-snug text-[#ffd8cc]"
        >
          翻译失败：{error.replace(/[。.\s]+$/, "")}。
          {blocks && blocks.length > 0 ? "下面是识别出的原文，" : ""}点击或 Esc 关闭。
        </div>
      )}

      {notice && !error && (
        <div
          role="status"
          className="absolute inset-x-1 bottom-1 z-10 max-h-[calc(100%-8px)] overflow-y-auto rounded border border-[rgba(240,180,94,0.6)] bg-[rgba(44,32,10,0.97)] px-2.5 py-1.5 text-[12.5px] leading-snug text-[#f6e3bd]"
        >
          {notice}
        </div>
      )}

      {hint && !error && !notice && !nothingToReplace && blocks !== null && blocks.length > 0 && (
        <div className="pointer-events-none absolute right-2 top-2 rounded-full bg-[rgba(14,14,18,0.9)] px-3 py-1 text-[12px] text-[#b0aaa1]">
          Esc 关闭
        </div>
      )}
    </>
  );
}

/** 把抛出来的东西说成一句人话（可能是 Error、字符串、甚至 undefined）。 */
export function describeFailure(error: unknown): string {
  const text = error instanceof Error ? error.message : typeof error === "string" ? error : "未知错误";
  return text.trim().slice(0, 200) || "未知错误";
}

/**
 * 画的部分出错时的退路：一条错误提示 + 逐块列出译文原文本。
 * 越简单越好——它自己要是也抛，就没有下一层了。不做任何计算，只对数据做最保守的取用。
 */
export function OverlayFallback({ blocks, failure }: { blocks: OverlayBlock[] | null; failure: string }) {
  const texts = (Array.isArray(blocks) ? blocks : [])
    .map((block) => (block !== null && typeof block === "object" && typeof block.text === "string" ? block.text : ""))
    .filter((text) => text.trim() !== "");
  return (
    <div
      role="alert"
      className="absolute inset-x-1 top-1 z-10 max-h-[calc(100%-8px)] overflow-y-auto rounded border border-[rgba(255,107,74,0.6)] bg-[rgba(48,14,10,0.97)] px-2.5 py-1.5 text-[12.5px] leading-snug text-[#ffd8cc]"
    >
      没能原位显示译文：{failure}。{texts.length > 0 ? "下面是译文，" : ""}点击或 Esc 关闭。
      {texts.map((text, index) => (
        <div key={index} className="mt-1 whitespace-pre-wrap text-[#f6f3ed]">
          {text}
        </div>
      ))}
    </div>
  );
}

/** 错误边界：里面渲染抛出来，就换成 `OverlayFallback`，外面的关闭处理器不受影响。 */
export class OverlayBoundary extends Component<
  { blocks: OverlayBlock[] | null; children: ReactNode },
  { failure: string | null }
> {
  state: { failure: string | null } = { failure: null };

  static getDerivedStateFromError(error: unknown) {
    return { failure: describeFailure(error) };
  }

  componentDidCatch(error: unknown) {
    console.error("[overlay] 渲染出错，退成错误提示：", error);
  }

  render() {
    return this.state.failure === null ? (
      this.props.children
    ) : (
      <OverlayFallback blocks={this.props.blocks} failure={this.state.failure} />
    );
  }
}

/**
 * 所有块的译文都和原文一样（命令、版本号、专有名词这类会原样保留）：覆盖层里一块都不画，
 * 屏幕上看不出任何变化，得说一声，别让人以为没反应。
 */
export function isNothingToReplace(blocks: OverlayBlock[] | null): boolean {
  return !!blocks && blocks.length > 0 && blocks.every((block) => block?.unchanged === true);
}

/**
 * 画层：先把所有原文盖住，再画译文——相邻两块的遮盖矩形不能压在对方的译文上。
 * 译文和原文一样的块不画，原来的像素一点不动（比用另一种字体照着再画一遍更像原文，粗体标题也不会被换成常规字重）。
 */
export function OverlayLayers({ blocks, plans }: { blocks: OverlayBlock[]; plans: BlockPlan[] }) {
  return (
    <>
      {plans.map((plan, index) => (blocks[index]?.unchanged ? null : <CoverView key={`cover-${index}`} plan={plan} />))}
      {plans.map((plan, index) =>
        blocks[index]?.unchanged ? null : (
          <TextView key={`text-${index}`} plan={plan} translated={blocks[index]?.translated ?? false} />
        ),
      )}
    </>
  );
}

/** 盖住原文：用原文底下的底色纯色填满，没有圆角——圆角会在四个角上露出原文的毛边。 */
function CoverView({ plan }: { plan: BlockPlan }) {
  const { cover } = plan;
  return (
    <div
      aria-hidden
      data-role="cover"
      className="absolute"
      style={{ left: cover.x, top: cover.y, width: cover.w, height: cover.h, background: plan.bg }}
    />
  );
}

/**
 * 译文：一行一个元素，行已经折好了（见 `wrapText`），浏览器不再自己折，排出来才和预估的一致。
 * 没译成（失败时画的是识别出的原文）的稍微压暗一点，和顶部的失败提示一起，不让原文冒充译文。
 */
function TextView({ plan, translated }: { plan: BlockPlan; translated: boolean }) {
  return (
    <div
      data-role="text"
      data-size={plan.fontSize}
      data-truncated={plan.truncated ? "true" : undefined}
      className="absolute"
      style={{
        left: plan.x,
        top: plan.y,
        fontSize: plan.fontSize,
        lineHeight: `${plan.lineHeight}px`,
        color: plan.fg,
        whiteSpace: "pre",
        opacity: translated ? 1 : 0.82,
      }}
    >
      {plan.lines.map((line, index) => (
        <div key={index} style={{ height: plan.lineHeight }}>
          {line}
        </div>
      ))}
    </div>
  );
}
