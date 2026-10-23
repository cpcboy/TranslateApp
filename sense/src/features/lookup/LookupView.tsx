import { useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { buildPreview } from "../../lib/schema";
import { useLookup } from "../../lib/store";
import { cn } from "../../lib/cn";
import { ErrorState } from "./ErrorState";
import { LongTextView } from "./LongTextView";
import { SenseCard } from "./SenseCard";
import { Skeleton } from "./Skeleton";

const EXAMPLES = ["charge", "落实", "run", "交代"];

export function LookupView({ onOpenSettings }: { onOpenSettings: () => void }) {
  const {
    status,
    query,
    entry,
    partial,
    warnings,
    error,
    meta,
    longSource,
    longText,
    longBusy,
    longTruncated,
    run,
    runLong,
    cancelLong,
    reset,
  } = useLookup();
  const [text, setText] = useState("");
  const inputRef = useRef<HTMLTextAreaElement>(null);

  // 窗口一显示就聚焦输入框：用户按完快捷键应该能直接打字。
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // 输入框跟着内容长高：粘贴长文时不能只露出一行
  useEffect(() => {
    const node = inputRef.current;
    if (!node) return;
    node.style.height = "auto";
    node.style.height = `${Math.min(node.scrollHeight, 200)}px`;
  }, [text]);

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
    // 超过一个词的长度就当长文翻译：查词那套 JSON 契约放不下整篇文章
    if (value.length > 64) {
      void runLong(value);
      setText("");
      return;
    }
    void run(value);
  };

  const busy = status === "loading" || status === "streaming";

  return (
    <div className="flex h-full min-h-0 flex-col gap-2.5">
      <div className="flex items-center gap-3 rounded-xl border border-line-strong bg-black/25 px-5 py-4">
        <span className="font-mono text-[16px] text-acc">›</span>
        <div className="relative min-w-0 flex-1">
          <textarea
            ref={inputRef}
            value={text}
            rows={1}
            aria-label="查一个词，或粘贴一段长文"
            onChange={(event) => setText(event.currentTarget.value)}
            onKeyDown={(event) => {
              // Enter 翻译，Shift+Enter 换行；输入法组字时不拦
              if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
                event.preventDefault();
                submit();
              }
            }}
            className="max-h-[200px] w-full resize-none overflow-y-auto bg-transparent text-[18px] leading-[1.6] text-paper focus:outline-none"
          />
          {/* 自绘占位符：WKWebView 里 input 的 placeholder 渲染不出中文 */}
          {text.length === 0 && (
            <span aria-hidden="true" className="pointer-events-none absolute inset-y-0 left-0 flex items-center text-[18px] text-subtle">
              查一个词，比如 charge
            </span>
          )}
        </div>
        {busy && <span className="font-mono text-[13.5px] text-muted">查询中</span>}
        <kbd className="rounded border border-line px-2 py-1 font-mono text-[13.5px] text-muted">↵</kbd>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto pr-0.5">
        {longSource ? (
          <LongTextView
            source={longSource}
            text={longText}
            busy={longBusy}
            truncated={longTruncated}
            error={error ? error.message : null}
            onStop={() => void cancelLong()}
            onRetry={() => void runLong(longSource)}
            onBack={() => reset()}
          />
        ) : error ? (
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
        <footer className="mt-1 flex shrink-0 items-center justify-between border-t border-line pt-3 font-mono text-[13.5px] text-subtle">
          <span>
            首义项 {formatMs(meta.firstSenseMs)} · 总计 {formatMs(meta.totalMs)}
          </span>
          <span className={cn(meta.source === "cache" && "text-[#8fb07a]")}>
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
      <p className="text-[19px] text-[#e6e1d8]">查一个词，给它一张完整的义项卡。</p>
      <p className="mt-3 text-[16px] leading-relaxed text-muted">
        音标、按常用度排序的义项、每个义项的例句和场景。
        <br />
        查过的词会存本地，第二次不再走网络。
      </p>
      <div className="mt-5 flex flex-wrap justify-center gap-2">
        {EXAMPLES.map((word) => (
          <button
            key={word}
            type="button"
            onClick={() => onPick(word)}
            className="rounded-full border border-line px-4 py-2.5 font-mono text-[15px] text-muted hover:border-line-strong hover:text-paper"
          >
            {word}
          </button>
        ))}
      </div>
    </div>
  );
}

function formatMs(ms: number | null | undefined): string {
  if (!ms) return "—";
  return ms >= 1000 ? `${(ms / 1000).toFixed(2)}s` : `${Math.round(ms)}ms`;
}
