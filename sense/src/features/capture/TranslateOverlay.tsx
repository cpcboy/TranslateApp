import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, type OverlayBlock } from "../../lib/ipc";

/**
 * 译文覆盖层：窗口正好压在选区上，按 OCR 的位置逐块画出来。
 *
 * 深色底盖住原文，中文写进同一块区域——这就是「原位替换」。
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
        setBlocks(view.blocks);
        setError(view.error);
        setNotice(view.notice);
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
      onClick={() => void api.closeOverlay()}
    >
      {blocks?.map((block, index) => (
        <BlockView key={`${block.x}-${block.y}-${index}`} block={block} />
      ))}

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

      {hint && !error && !notice && blocks !== null && blocks.length > 0 && (
        <div className="pointer-events-none absolute right-2 top-2 rounded-full bg-[rgba(14,14,18,0.9)] px-3 py-1 text-[12px] text-[#b0aaa1]">
          Esc 关闭
        </div>
      )}
    </div>
  );
}

/**
 * 单块译文。
 *
 * 字号不是拍脑袋定的：Vision 给的框高≈原文字高，先按 0.95 倍起步，
 * 再实测是否溢出，逐级缩小到装得下为止——中文比英文短，通常不用缩。
 */
function BlockView({ block }: { block: OverlayBlock }) {
  const ref = useRef<HTMLDivElement>(null);
  // 按「一行」的高度算字号：合并成两行的块，h 也是两行，直接用会变大两倍
  const base = Math.min(26, Math.max(12, Math.round(block.line_h * 0.95)));

  useLayoutEffect(() => {
    const node = ref.current;
    if (!node) return;
    let size = base;
    node.style.fontSize = `${size}px`;
    for (let step = 0; step < 16; step += 1) {
      const overflow =
        node.scrollHeight > node.clientHeight + 1 || node.scrollWidth > node.clientWidth + 1;
      if (!overflow || size <= 10) break;
      size -= 1;
      node.style.fontSize = `${size}px`;
    }
  }, [block.text, block.w, block.h, block.line_h, base]);

  return (
    <div
      ref={ref}
      className="absolute overflow-hidden rounded-[3px] bg-[rgba(14,14,18,0.96)] px-[3px] text-[#f6f3ed]"
      style={{
        left: block.x,
        top: block.y,
        width: Math.max(block.w, 8),
        height: Math.max(block.h, 8),
        lineHeight: 1.18,
        display: "flex",
        alignItems: "center",
        opacity: block.translated ? 1 : 0.82,
      }}
    >
      {block.text}
    </div>
  );
}
