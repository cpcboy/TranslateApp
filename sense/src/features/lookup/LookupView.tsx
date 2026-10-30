import { useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isLongText } from "../../lib/route";
import { buildPreview } from "../../lib/schema";
import { useLookup } from "../../lib/store";
import type { WordEntry } from "../../lib/schema";
import { previewForDisplay, translatedFrom, untranslatedNotice } from "../../lib/translated";
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
    untranslated,
    error,
    meta,
    longSource,
    longText,
    longBusy,
    longTruncated,
    longIncomplete,
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
    () => previewForDisplay(query, partial && !entry ? buildPreview(partial, query) : null),
    [partial, entry, query],
  );

  const submit = () => {
    const value = text.trim();
    if (!value) return;
    // 一句话以上就当长文翻译：查词那套 JSON 契约放不下句子和文章（规则见 route.ts）
    if (isLongText(value)) {
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
              // Enter 翻译，Shift+Enter 换行；输入法组字时不拦。
              // WebKit 里组字结束的那一下 isComposing 已经是 false，keyCode 仍是 229（App.tsx 的 Esc 同理）
              if (
                event.key === "Enter" &&
                !event.shiftKey &&
                !event.nativeEvent.isComposing &&
                event.nativeEvent.keyCode !== 229
              ) {
                event.preventDefault();
                submit();
              }
            }}
            className="max-h-[200px] w-full resize-none overflow-y-auto bg-transparent text-[18px] leading-[1.6] text-paper focus:outline-none"
          />
          {/* 自绘占位符：WKWebView 里 input 的 placeholder 渲染不出中文 */}
          {text.length === 0 && (
            <span aria-hidden="true" className="pointer-events-none absolute inset-y-0 left-0 flex items-center text-[18px] text-subtle">
              查一个词（中文会先译成英文）
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
            incomplete={longIncomplete}
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
        ) : entry || preview ? (
          <ResultCard
            query={query}
            entry={entry}
            preview={preview}
            warnings={warnings}
            untranslated={untranslated}
          />
        ) : busy ? (
          // 还没有能画的卡（正在连接、第一个义项还没到、中文输入的预览被挡住）：骨架屏。
          // 以前只在 loading 时画骨架，streaming 却没有预览时会闪一下首页
          <Skeleton />
        ) : (
          <Empty onPick={(word) => { setText(word); void run(word); }} />
        )}
      </div>

      {meta && (
        <footer className="mt-1 flex shrink-0 items-center justify-between border-t border-line pt-3 font-mono text-[13.5px] text-subtle">
          <span>
            {meta.source === "cache"
              ? "本地缓存 · 0 ms"
              : `首义项 ${formatMs(meta.firstSenseMs)} · 总计 ${formatMs(meta.totalMs)}`}
          </span>
          <span className={cn(meta.source === "cache" && "text-[#8fb07a]")}>
            {meta.source === "cache" ? "缓存命中 · 0 请求" : "网络"} · {meta.model}
          </span>
        </footer>
      )}
    </div>
  );
}

/** 流式预览出现在最终卡片同一个位置，过渡时 SenseCard 必须是同一个实例——否则用户已经点开的义项会合上 */
const PREVIEW_WARNINGS = ["还在加载，先显示已经完整的义项"];

/**
 * 查词结果卡。没有 hook，纯按入参出树：预览与最终结果两种状态得到的树形（Fragment → [横幅位, SenseCard]）一致，
 * React 才会复用同一个 SenseCard；单测盯着这一点（LookupView.test.tsx）。
 */
export function ResultCard({
  query,
  entry,
  preview,
  warnings,
  untranslated,
}: {
  query: string;
  entry: WordEntry | null;
  preview: WordEntry | null;
  warnings: string[];
  untranslated: boolean;
}) {
  const card = entry ?? preview;
  if (!card) return null;
  return (
    <>
      {/* 中文输入没能译成英文：这张卡不是译好的结果，不能让人当成译好的，所以说在卡片上方 */}
      {entry && untranslated && (
        <p
          role="status"
          className="mb-2.5 rounded-lg border border-[rgba(209,163,93,0.45)] bg-[rgba(240,180,94,0.08)] px-4 py-3 text-[14.5px] leading-relaxed text-[#f0b45e]"
        >
          {untranslatedNotice(query)}
        </p>
      )}
      <SenseCard
        entry={card}
        warnings={entry ? warnings : PREVIEW_WARNINGS}
        from={translatedFrom(query, card)}
      />
    </>
  );
}

function Empty({ onPick }: { onPick: (word: string) => void }) {
  return (
    <div className="flex h-full flex-col items-center justify-center px-6 text-center">
      <p className="text-[19px] text-[#e6e1d8]">查一个词，给它一张完整的义项卡。</p>
      <p className="mt-3 text-[16px] leading-relaxed text-muted">
        音标、按常用度排序的义项、每个义项的例句和场景。
        <br />
        输入中文，会先译成英文，再解析那个英文词。
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
