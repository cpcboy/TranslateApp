import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { LookupView } from "./features/lookup/LookupView";
import { SettingsView } from "./features/settings/SettingsView";
import { api } from "./lib/ipc";
import { useLookup } from "./lib/store";

const isTauri = "__TAURI_INTERNALS__" in window;

/** 与 tauri.conf.json 的 minWidth / minHeight 保持一致，拖拽时先夹一次。 */
const MIN_WIDTH = 760;
const MIN_HEIGHT = 720;

type View = "lookup" | "settings";

export default function App() {
  const [view, setView] = useState<View>("lookup");
  const [hint, setHint] = useState("");
  const run = useLookup((state) => state.run);

  useEffect(() => {
    if (!isTauri) return;
    void api
      .hotkeyHint()
      .then(setHint)
      .catch(() => {
        /* 取不到就不显示键帽 */
      });
  }, []);

  // 仅 debug 构建：自动化验证时用环境变量触发一次查询。
  const devRunOnce = useRef(false);
  useEffect(() => {
    if (!isTauri || devRunOnce.current) return;
    void api
      .devFlags()
      .then((flags) => {
        if (!flags.dev_lookup || devRunOnce.current) return;
        devRunOnce.current = true; // StrictMode 会双跑 effect，别重复发请求
        void run(flags.dev_lookup);
      })
      .catch(() => {
        /* 拿不到就正常空跑 */
      });
  }, [run]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      // 中文输入法组字时按 Esc 是取消候选词，不该关窗口。
      if (event.isComposing || event.keyCode === 229) return;
      if (event.key !== "Escape") return;
      event.preventDefault();
      if (view === "settings") {
        setView("lookup");
        return;
      }
      if (isTauri) void api.hideWindow();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [view]);

  return (
    <div className="relative h-full w-full p-3">
      <div className="flex h-full flex-col overflow-hidden rounded-2xl border border-line bg-[#121217]">
        {/* 无边框窗口没有标题栏。deep 让整条 header 的空白都能拖动，
            但按钮/输入框仍优先响应点击（Tauri 会在 clickable 元素上短路）。 */}
        <header
          data-tauri-drag-region="deep"
          className="flex shrink-0 items-center justify-between border-b border-line px-7 py-4"
        >
          <div className="flex items-baseline gap-4">
            <span className="font-serif text-[22px] font-semibold tracking-[0.16em] text-paper">
              SENSE
            </span>
            <nav className="flex items-center gap-1">
              <Tab active={view === "lookup"} onClick={() => setView("lookup")}>
                查词
              </Tab>
              <Tab active={view === "settings"} onClick={() => setView("settings")}>
                设置
              </Tab>
            </nav>
          </div>
          <span className="font-mono text-[13.5px] text-subtle">
            {hint ? `${hint} 呼出 · Esc 隐藏` : "Esc 隐藏"}
          </span>
        </header>

        <main className="min-h-0 flex-1 px-4 py-3.5">
          {view === "lookup" ? (
            <LookupView onOpenSettings={() => setView("settings")} />
          ) : (
            <SettingsView onBack={() => setView("lookup")} />
          )}
        </main>
      </div>

      <ResizeGrip />
    </div>
  );
}

/**
 * 右下角的缩放把手。
 *
 * 注意：tao 0.37 在 macOS 上把 `drag_resize_window` 实现成直接返回
 * NotSupported，所以 Tauri 的 `startResizeDragging` 在这台机器上是静默无效的
 * （错误被 Rust 侧吞掉，前端 Promise 还会 resolve）。这里改成自己跟踪
 * pointermove，用 `setSize` 逐帧设置窗口尺寸——这是当前唯一真实可用的路径。
 */
function ResizeGrip() {
  if (!isTauri) return null;

  const start = async (event: React.PointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    const handle = event.currentTarget;
    handle.setPointerCapture(event.pointerId);

    const win = getCurrentWindow();
    const [size, scale] = await Promise.all([win.innerSize(), win.scaleFactor()]);
    const startWidth = size.width / scale;
    const startHeight = size.height / scale;
    const startX = event.screenX;
    const startY = event.screenY;

    let frame = 0;
    let nextWidth = startWidth;
    let nextHeight = startHeight;

    const onMove = (moveEvent: PointerEvent) => {
      nextWidth = Math.max(MIN_WIDTH, Math.round(startWidth + moveEvent.screenX - startX));
      nextHeight = Math.max(MIN_HEIGHT, Math.round(startHeight + moveEvent.screenY - startY));
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        void win.setSize(new LogicalSize(nextWidth, nextHeight)).catch(() => {
          /* 窗口被关掉时会失败，忽略 */
        });
      });
    };

    const onEnd = () => {
      if (frame) cancelAnimationFrame(frame);
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onEnd);
      window.removeEventListener("pointercancel", onEnd);
      if (handle.hasPointerCapture(event.pointerId)) {
        handle.releasePointerCapture(event.pointerId);
      }
    };

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onEnd);
    window.addEventListener("pointercancel", onEnd);
  };

  return (
    <div
      role="presentation"
      aria-hidden="true"
      title="拖动调整窗口大小"
      onPointerDown={start}
      className="group absolute bottom-0 right-0 z-10 flex h-6 w-6 cursor-nwse-resize items-end justify-end p-[3px]"
    >
      <svg width="12" height="12" viewBox="0 0 12 12" className="text-subtle group-hover:text-paper">
        <path d="M11 4 L4 11 M11 8 L8 11" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
      </svg>
    </div>
  );
}

function Tab({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={
        active
          ? "rounded-md bg-[rgba(248,246,241,0.12)] px-3.5 py-2 text-[15px] font-medium text-paper"
          : "rounded-md px-3.5 py-2 text-[15px] text-muted hover:text-paper"
      }
    >
      {children}
    </button>
  );
}
