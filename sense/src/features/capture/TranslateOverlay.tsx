import { useEffect, useState } from "react";
import { api, type OverlayBlock } from "../../lib/ipc";

/**
 * 译文覆盖层：窗口正好压在选区上，按 OCR 的位置逐块画出来。
 *
 * 深色底盖住原文，中文写进同一块区域——这就是「原位替换」。
 */
export function TranslateOverlay() {
  const [blocks, setBlocks] = useState<OverlayBlock[] | null>(null);
  const [hint, setHint] = useState(true);

  useEffect(() => {
    void api
      .captureBlocks()
      .then(setBlocks)
      .catch(() => setBlocks([]));

    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") void api.closeOverlay();
    };
    window.addEventListener("keydown", onKey);
    const timer = window.setTimeout(() => setHint(false), 2200);

    return () => {
      window.removeEventListener("keydown", onKey);
      window.clearTimeout(timer);
    };
  }, []);

  return (
    <div
      className="relative h-full w-full overflow-hidden"
      onClick={() => void api.closeOverlay()}
    >
      {blocks?.map((block, index) => (
        <div
          key={`${block.x}-${block.y}-${index}`}
          className="absolute overflow-hidden rounded-[3px] bg-[rgba(14,14,18,0.95)] px-[2px] text-[#f6f3ed]"
          style={{
            left: block.x,
            top: block.y,
            width: Math.max(block.w, 8),
            height: Math.max(block.h, 8),
            fontSize: Math.max(10, Math.min(17, Math.round(block.h * 0.62))),
            lineHeight: 1.22,
            display: "flex",
            alignItems: "center",
            opacity: block.translated ? 1 : 0.85,
          }}
        >
          <span className="line-clamp-2 w-full">{block.text}</span>
        </div>
      ))}

      {blocks !== null && blocks.length === 0 && (
        <div className="absolute left-2 top-2 rounded bg-[rgba(14,14,18,0.95)] px-3 py-2 text-[13px] text-[#f6f3ed]">
          没识别到文字
        </div>
      )}

      {hint && blocks !== null && blocks.length > 0 && (
        <div className="pointer-events-none absolute right-2 top-2 rounded-full bg-[rgba(14,14,18,0.9)] px-3 py-1 text-[12px] text-[#b0aaa1]">
          Esc 关闭
        </div>
      )}
    </div>
  );
}
