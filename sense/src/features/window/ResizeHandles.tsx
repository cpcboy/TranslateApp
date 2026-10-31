import type { PointerEvent as ReactPointerEvent } from "react";
import { api } from "../../lib/ipc";
import { cn } from "../../lib/cn";
import { createDragSession, type Edge } from "../../lib/windowFrame";

/**
 * 窗口四条边和四个角的缩放把手，藏在外圈 12px 的透明边距里（再往面板里探 4px，好抓）。
 *
 * 为什么自己做：系统对无边框窗口自带缩放，但那个缩放区只有窗口最外缘约 3px，而可见的面板比窗口缩进 12px，
 * 用户抓可见的边框什么也不会发生；Tauri 的 `startResizeDragging` 在 tao 0.37 的 macOS 上又直接返回
 * NotSupported（静默无效）。这里自己跟踪指针，算出新外框（windowFrame.ts），逐帧交给 Rust 的 `set_window_frame`。
 * 把手最外 3px 与系统缩放区重叠：AppKit 先处理那里的按下，网页收不到，两套机制不会同时触发；往里的部分归这里。
 *
 * 右下角那个有可见的小标记，其余七个是隐形的，只靠鼠标指针的形状提示。
 */
const HANDLES: { edge: Edge; className: string }[] = [
  { edge: "n", className: "left-4 right-4 top-0 h-4 cursor-ns-resize" },
  { edge: "s", className: "bottom-0 left-4 right-4 h-4 cursor-ns-resize" },
  { edge: "w", className: "bottom-4 left-0 top-4 w-4 cursor-ew-resize" },
  { edge: "e", className: "bottom-4 right-0 top-4 w-4 cursor-ew-resize" },
  { edge: "nw", className: "left-0 top-0 h-4 w-4 cursor-nwse-resize" },
  { edge: "ne", className: "right-0 top-0 h-4 w-4 cursor-nesw-resize" },
  { edge: "sw", className: "bottom-0 left-0 h-4 w-4 cursor-nesw-resize" },
  { edge: "se", className: "group bottom-0 right-0 flex h-6 w-6 cursor-nwse-resize items-end justify-end p-[3px]" },
];

export function ResizeHandles() {
  return (
    <>
      {HANDLES.map(({ edge, className }) => (
        <div
          key={edge}
          role="presentation"
          aria-hidden="true"
          title={edge === "se" ? "拖动调整窗口大小" : undefined}
          onPointerDown={(event) => startResize(edge, event)}
          className={cn("absolute z-10", className)}
        >
          {edge === "se" && (
            <svg width="12" height="12" viewBox="0 0 12 12" className="text-subtle group-hover:text-paper">
              <path d="M11 4 L4 11 M11 8 L8 11" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
            </svg>
          )}
        </div>
      ))}
    </>
  );
}

/**
 * 按下把手：开一次拖拽会话（时序见 `createDragSession`），把 DOM 事件接上去。
 * 监听器要在量起始外框（异步）之前就挂上，并且有多条收尾的路——松手、取消、窗口失焦（被隐藏）、指针捕获丢失。
 */
function startResize(edge: Edge, event: ReactPointerEvent<HTMLDivElement>) {
  if (event.button !== 0) return;
  event.preventDefault();
  const handle = event.currentTarget;
  const pointerId = event.pointerId;
  handle.setPointerCapture(pointerId);

  // 用 screenX / screenY 算偏移：拖左边 / 上边时窗口自己在动，相对窗口的坐标会跟着变，屏幕坐标不会
  const onMove = (moveEvent: PointerEvent) => session.move(moveEvent.screenX, moveEvent.screenY, moveEvent.buttons);
  const onEnd = () => session.end();

  const cleanup = () => {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onEnd);
    window.removeEventListener("pointercancel", onEnd);
    window.removeEventListener("blur", onEnd);
    handle.removeEventListener("lostpointercapture", onEnd);
    if (handle.hasPointerCapture(pointerId)) handle.releasePointerCapture(pointerId);
  };

  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onEnd);
  window.addEventListener("pointercancel", onEnd);
  window.addEventListener("blur", onEnd);
  handle.addEventListener("lostpointercapture", onEnd);

  const session = createDragSession({
    edge,
    startX: event.screenX,
    startY: event.screenY,
    buttonsAtStart: event.buttons,
    measure: api.windowFrame,
    send: api.setWindowFrame,
    onEnd: cleanup,
  });
}
