import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "../../lib/ipc";

type Rect = { x: number; y: number; w: number; h: number };

/**
 * 全屏选区遮罩。
 *
 * 只做一件事：让用户拖一个框，然后把**全局逻辑坐标**交给 Rust。
 * 截图、识别、翻译都在后端做，这里不碰图片。
 */
export function CaptureOverlay() {
  const [rect, setRect] = useState<Rect | null>(null);
  const [busy, setBusy] = useState(false);
  const start = useRef<{ x: number; y: number } | null>(null);
  const current = useRef<Rect | null>(null);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") void api.cancelCapture();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const begin = (event: React.PointerEvent<HTMLDivElement>) => {
    if (busy) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    start.current = { x: event.clientX, y: event.clientY };
    const next = { x: event.clientX, y: event.clientY, w: 0, h: 0 };
    current.current = next;
    setRect(next);
  };

  const drag = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!start.current || busy) return;
    const from = start.current;
    const next = {
      x: Math.min(from.x, event.clientX),
      y: Math.min(from.y, event.clientY),
      w: Math.abs(event.clientX - from.x),
      h: Math.abs(event.clientY - from.y),
    };
    current.current = next;
    setRect(next);
  };

  const end = async () => {
    const area = current.current;
    start.current = null;
    if (!area) return;
    if (area.w < 10 || area.h < 10) {
      // 误点一下不算选区，继续等
      current.current = null;
      setRect(null);
      return;
    }

    setBusy(true);
    try {
      // 窗口原点是全局逻辑坐标；CSS 像素与逻辑点在这里是一比一
      const origin = await getCurrentWindow().outerPosition();
      const dpr = window.devicePixelRatio || 1;
      await api.finishCapture(
        origin.x / dpr + area.x,
        origin.y / dpr + area.y,
        area.w,
        area.h,
      );
    } catch (err) {
      // 失败时把遮罩撤掉，把原因留给主窗口的日志
      console.error(err);
      void api.cancelCapture();
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      className="relative h-full w-full cursor-crosshair select-none"
      onPointerDown={begin}
      onPointerMove={drag}
      onPointerUp={() => void end()}
      onPointerCancel={() => void end()}
    >
      {/* 半透明压暗：让选框更清楚；截图前这层会被关掉，不会进图 */}
      <div className="absolute inset-0 bg-[rgba(6,6,8,0.35)]" />

      {rect && (
        <>
          <div
            className="absolute border-2 border-[#ff6b4a] shadow-[0_0_0_9999px_rgba(6,6,8,0.45)]"
            style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }}
          />
          <div
            className="absolute rounded bg-[rgba(12,12,16,0.9)] px-2 py-1 font-mono text-[12px] text-[#f6f3ed]"
            style={{ left: rect.x, top: Math.max(0, rect.y - 26) }}
          >
            {Math.round(rect.w)} × {Math.round(rect.h)}
          </div>
        </>
      )}

      <div className="pointer-events-none absolute bottom-6 left-1/2 -translate-x-1/2 rounded-full bg-[rgba(12,12,16,0.86)] px-4 py-2 text-[13px] text-[#f6f3ed]">
        {busy ? "识别中…" : "拖动选择区域 · Esc 取消"}
      </div>
    </div>
  );
}
