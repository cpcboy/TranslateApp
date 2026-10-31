import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { LookupView } from "./features/lookup/LookupView";
import { SettingsView } from "./features/settings/SettingsView";
import { ResizeHandles } from "./features/window/ResizeHandles";
import { WindowControls } from "./features/window/WindowControls";
import { api, onHotkeyChanged } from "./lib/ipc";
import { useLookup } from "./lib/store";
import { UiScaleContext } from "./lib/uiScaleContext";
import { useUiScale } from "./lib/useUiScale";

const isTauri = "__TAURI_INTERNALS__" in window;

type View = "lookup" | "settings";

export default function App() {
  const [view, setView] = useState<View>("lookup");
  const [hint, setHint] = useState("");
  const [notice, setNotice] = useState("");
  const run = useLookup((state) => state.run);
  const longSource = useLookup((state) => state.longSource);
  const resetLookup = useLookup((state) => state.reset);
  const { scale, badge } = useUiScale();

  useEffect(() => {
    if (!isTauri) return;
    const refresh = () =>
      void api
        .hotkeyHint()
        .then(setHint)
        .catch(() => {
          /* 取不到就不显示键帽 */
        });
    refresh();
    // 在设置里改了查词键，标题栏的键帽要跟着变，不用等重启
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void onHotkeyChanged(refresh)
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
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

  // 截屏失败（尤其没授权）必须让人看见，否则就是「按了没反应」
  useEffect(() => {
    if (!isTauri) return;
    let unlisten: (() => void) | undefined;
    void listen<string>("sense://capture-error", (event) => {
      setNotice(event.payload);
      setView("lookup");
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, []);

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
      // 长文阅读中按 Esc 先退出阅读视图，不要直接把整个窗口收起来
      if (longSource) {
        resetLookup();
        return;
      }
      if (isTauri) void api.hideWindow();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [view, longSource, resetLookup]);

  return (
    <div className="relative h-full w-full p-3">
      <div className="h-full overflow-hidden rounded-2xl border border-line bg-[#121217]">
        {/* ⌘+ / ⌘- / ⌘0 缩放的是这一层里面的内容（见 lib/uiScale.ts）；
            面板外框、外圈透明边距和缩放把手不跟着缩放 */}
        <ZoomLayer scale={scale}>
          {/* 无边框窗口没有标题栏。deep 让整条 header 的空白都能拖动，
              但按钮/输入框仍优先响应点击（Tauri 会在 clickable 元素上短路）。 */}
          <header
            data-tauri-drag-region="deep"
            className="flex shrink-0 items-center justify-between gap-4 border-b border-line py-4 pl-5 pr-7"
          >
            <div className="flex shrink-0 items-center gap-5">
              {isTauri && <WindowControls />}
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
            </div>
            <span className="min-w-0 truncate font-mono text-[13.5px] text-subtle">
              {hint ? `${hint} 呼出 · Esc 隐藏 · ⌘± 字号` : "Esc 隐藏 · ⌘± 字号"}
            </span>
          </header>

          {notice && (
            <div className="mx-4 mt-3 shrink-0 rounded-lg border border-[rgba(255,82,48,0.35)] bg-[rgba(255,82,48,0.1)] px-4 py-3 text-[13.5px] leading-relaxed text-[#f6b09b]">
              <div className="flex items-start justify-between gap-3">
                <span>{notice}</span>
                <button
                  type="button"
                  onClick={() => setNotice("")}
                  className="shrink-0 rounded px-2 py-0.5 text-[13px] text-[#f6b09b] hover:text-paper"
                >
                  知道了
                </button>
              </div>
            </div>
          )}

          <main className="min-h-0 flex-1 px-4 py-3.5">
            {view === "lookup" ? (
              <LookupView onOpenSettings={() => setView("settings")} />
            ) : (
              <SettingsView onBack={() => setView("lookup")} />
            )}
          </main>
        </ZoomLayer>
      </div>

      {badge && (
        <div
          role="status"
          aria-live="polite"
          className="pointer-events-none absolute bottom-7 left-1/2 z-20 -translate-x-1/2 rounded-full border border-line-strong bg-black/80 px-4 py-1.5 font-mono text-[13.5px] text-paper"
        >
          {badge}
        </div>
      )}

      {isTauri && <ResizeHandles />}
    </div>
  );
}

/**
 * 面板内容套一层 CSS zoom（界面缩放），并把倍数交给 context：
 * 这一层里 `position: fixed` 的东西（划词浮层）要靠它把视口坐标换算回 zoom 里的长度。
 */
function ZoomLayer({ scale, children }: { scale: number; children: React.ReactNode }) {
  return (
    <UiScaleContext.Provider value={scale}>
      <div style={{ zoom: scale }} className="flex h-full flex-col">
        {children}
      </div>
    </UiScaleContext.Provider>
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
          ? "whitespace-nowrap rounded-md bg-[rgba(248,246,241,0.12)] px-3.5 py-2 text-[15px] font-medium text-paper"
          : "whitespace-nowrap rounded-md px-3.5 py-2 text-[15px] text-muted hover:text-paper"
      }
    >
      {children}
    </button>
  );
}
