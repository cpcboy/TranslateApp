import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { cn } from "./lib/cn";

/** 在浏览器里直接 `npm run dev` 时没有 Tauri 注入，避免 invoke 抛错。 */
const isTauri = "__TAURI_INTERNALS__" in window;

export default function App() {
  const inputRef = useRef<HTMLInputElement>(null);
  // 快捷键文案由 Rust 侧提供，界面不再硬编码，避免改键时两处漂移。
  const [hotkeyHint, setHotkeyHint] = useState("");

  useEffect(() => {
    if (!isTauri) return;
    void invoke<string>("hotkey_hint")
      .then(setHotkeyHint)
      .catch(() => {
        /* 取不到就不显示键帽，不影响使用 */
      });
  }, []);
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      // 中文输入法组字时按 Esc 是取消候选词，不该把窗口关掉。
      if (event.isComposing || event.keyCode === 229) return;
      if (event.key !== "Escape") return;
      event.preventDefault();
      if (isTauri) void invoke("hide_window");
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  useEffect(() => {
    if (!isTauri) return;
    let unlisten: (() => void) | undefined;
    void getCurrentWindow()
      .onFocusChanged(({ payload: focused }) => {
        if (focused) inputRef.current?.focus();
      })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {
        /* 拿不到窗口事件不影响使用，忽略 */
      });
    return () => unlisten?.();
  }, []);

  return (
    <div className="h-full w-full p-3">
      <div className="flex h-full flex-col overflow-hidden rounded-2xl border border-line bg-[#0d0d10]">
        <header className="flex items-center justify-between border-b border-line px-4 py-3">
          <div className="flex items-baseline gap-2">
            <span className="font-serif text-[15px] font-semibold tracking-[0.18em]">SENSE</span>
            <span className="text-[10px] tracking-[0.14em] text-muted">D1 骨架</span>
          </div>
          <span className="font-mono text-[10px] text-muted">
            {hotkeyHint ? `${hotkeyHint} 呼出 · Esc 隐藏` : "Esc 隐藏"}
          </span>
        </header>

        <main className="flex flex-1 flex-col items-center justify-center px-6">
          <div className="w-full max-w-[440px]">
            <div className="flex items-center gap-3 rounded-xl border border-line-strong bg-black/25 px-4 py-3">
              <span className="font-mono text-[11px] text-acc">›</span>
              <input
                ref={inputRef}
                autoFocus
                placeholder="查一个词，比如 charge"
                className={cn(
                  "w-full bg-transparent text-[15px] text-paper",
                  "placeholder:text-muted/70 focus:outline-none",
                )}
              />
            </div>

            <p className="mt-3 text-center text-[11px] leading-relaxed text-muted">
              窗口能呼出、能置顶、失焦会自己藏起来。
              <br />
              模型还没接，这一步只验证壳子。
            </p>
          </div>
        </main>

        <footer className="flex items-center justify-between border-t border-line px-4 py-2 text-[10px] text-muted">
          <span>无边框 · 置顶 · 失焦隐藏</span>
          <span className="font-mono">v0.1.0 · D1</span>
        </footer>
      </div>
    </div>
  );
}
