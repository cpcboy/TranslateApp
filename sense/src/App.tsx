import { useEffect, useRef, useState } from "react";
import { LookupView } from "./features/lookup/LookupView";
import { SettingsView } from "./features/settings/SettingsView";
import { api } from "./lib/ipc";
import { useLookup } from "./lib/store";

const isTauri = "__TAURI_INTERNALS__" in window;

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
    <div className="h-full w-full p-3">
      <div className="flex h-full flex-col overflow-hidden rounded-2xl border border-line bg-[#121217]">
        <header className="flex shrink-0 items-center justify-between border-b border-line px-5 py-3">
          <div className="flex items-baseline gap-3">
            <span className="font-serif text-[17px] font-semibold tracking-[0.16em] text-paper">
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
          <span className="font-mono text-[11.5px] text-subtle">
            {hint ? `${hint} 呼出 · Esc 隐藏` : "Esc 隐藏"}
          </span>
        </header>

        <main className="min-h-0 flex-1 px-3 py-2.5">
          {view === "lookup" ? (
            <LookupView onOpenSettings={() => setView("settings")} />
          ) : (
            <SettingsView onBack={() => setView("lookup")} />
          )}
        </main>
      </div>
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
          ? "rounded-md bg-[rgba(246,243,237,0.1)] px-2.5 py-1 text-[12.5px] text-paper"
          : "rounded-md px-2.5 py-1 text-[12.5px] text-muted hover:text-paper"
      }
    >
      {children}
    </button>
  );
}
