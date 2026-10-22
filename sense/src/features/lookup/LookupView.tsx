import { useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { buildPreview } from "../../lib/schema";
import { useLookup } from "../../lib/store";
import { cn } from "../../lib/cn";
import { ErrorState } from "./ErrorState";
import { SenseCard } from "./SenseCard";
import { Skeleton } from "./Skeleton";

const EXAMPLES = ["charge", "落实", "run", "交代"];

export function LookupView({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { status, query, entry, partial, warnings, error, meta, run } = useLookup();
  const [text, setText] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  // 窗口一显示就聚焦输入框：用户按完快捷键应该能直接打字。
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void getCurrentWindow()
      .onFocusChanged(({ payload }) => {
        if (payload) inputRef.current?.focus();
      })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {
        /* 拿不到窗口事件不影响输入 */
      });
    return () => unlisten?.();
  }, []);

  const preview = useMemo(
    () => (partial && !entry ? buildPreview(partial, query) : null),
    [partial, entry, query],
  );

  const submit = () => {
    const value = text.trim();
    if (!value) return;
    void run(value);
  };

  const busy = status === "loading" || status === "streaming";

  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      <div className="flex items-center gap-2.5 rounded-xl border border-line-strong bg-black/25 px-3.5 py-2.5">
        <span className="font-mono text-[11px] text-acc">›</span>
        <input
          ref={inputRef}
          value={text}
          onChange={(event) => setText(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.nativeEvent.isComposing) submit();
          }}
          placeholder="查一个词，比如 charge"
          className="w-full bg-transparent text-[14px] text-paper placeholder:text-muted/70 focus:outline-none"
        />
        {busy && <span className="font-mono text-[9px] text-muted">查询中</span>}
        <kbd className="rounded border border-line px-1.5 py-0.5 font-mono text-[9px] text-muted">↵</kbd>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto pr-0.5">
        {error ? (
          <ErrorState
            error={error}
            onRetry={() => void run(query)}
            onOpenSettings={onOpenSettings}
          />
        ) : entry ? (
          <SenseCard entry={entry} warnings={warnings} />
        ) : preview ? (
          <SenseCard entry={preview} warnings={["还在加载，先显示已经完整的义项"]} />
        ) : status === "loading" ? (
          <Skeleton />
        ) : (
          <Empty onPick={(word) => { setText(word); void run(word); }} />
        )}
      </div>

      {meta && (
        <footer className="flex shrink-0 items-center justify-between font-mono text-[9.5px] text-[#5e5b57]">
          <span>
            首义项 {formatMs(meta.firstDeltaMs)} · 总计 {formatMs(meta.totalMs)}
          </span>
          <span className={cn(meta.source === "cache" && "text-[#6c8a5a]")}>
            {meta.source === "cache" ? "缓存命中 · 0 请求" : "网络"} · {meta.model}
          </span>
        </footer>
      )}
    </div>
  );
}

function Empty({ onPick }: { onPick: (word: string) => void }) {
  return (
    <div className="flex h-full flex-col items-center justify-center px-6 text-center">
      <p className="text-[13px] text-[#beb9b1]">查一个词，给它一张完整的义项卡。</p>
      <p className="mt-1.5 text-[11px] leading-relaxed text-muted">
        音标、按常用度排序的义项、每个义项的例句和场景。
        <br />
        查过的词会存本地，第二次不再走网络。
      </p>
      <div className="mt-4 flex flex-wrap justify-center gap-1.5">
        {EXAMPLES.map((word) => (
          <button
            key={word}
            type="button"
            onClick={() => onPick(word)}
            className="rounded-full border border-line px-2.5 py-1 font-mono text-[10.5px] text-muted hover:border-line-strong hover:text-paper"
          >
            {word}
          </button>
        ))}
      </div>
    </div>
  );
}

function formatMs(ms: number): string {
  if (!ms) return "—";
  return ms >= 1000 ? `${(ms / 1000).toFixed(2)}s` : `${Math.round(ms)}ms`;
}
