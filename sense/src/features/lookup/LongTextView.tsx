import { useCallback, useEffect, useRef, useState } from "react";
import { api, type GrammarIssue, type WordGloss } from "../../lib/ipc";

/** 复制到剪贴板：优先 Clipboard API，不行就退回 textarea + execCommand。 */
async function copyText(value: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(value);
    return true;
  } catch {
    try {
      const area = document.createElement("textarea");
      area.value = value;
      area.style.position = "fixed";
      area.style.opacity = "0";
      document.body.appendChild(area);
      area.select();
      const ok = document.execCommand("copy");
      document.body.removeChild(area);
      return ok;
    } catch {
      return false;
    }
  }
}

type GlossState = { data: WordGloss | null; error: string | null; x: number; y: number } | null;

export function LongTextView({
  source,
  text,
  busy,
  truncated,
  incomplete,
  error,
  onStop,
  onRetry,
  onBack,
}: {
  source: string;
  text: string;
  busy: boolean;
  truncated: boolean;
  /** 没有正常写完的原因（被停止、内容策略、没收到结束标记……）；完整时为 null */
  incomplete: string | null;
  error: string | null;
  onStop: () => void;
  onRetry: () => void;
  onBack: () => void;
}) {
  const [copied, setCopied] = useState<"source" | "target" | null>(null);
  const [issues, setIssues] = useState<GrammarIssue[]>([]);
  const [checking, setChecking] = useState(false);
  const [gloss, setGloss] = useState<GlossState>(null);

  const leftRef = useRef<HTMLDivElement>(null);
  const rightRef = useRef<HTMLDivElement>(null);
  const endRef = useRef<HTMLDivElement>(null);
  const checkedFor = useRef<string>("");

  // 流式时右栏自动跟到底；没写完时也滚到底，让「不完整」提示一眼可见
  useEffect(() => {
    if (busy || ((error || incomplete) && text)) endRef.current?.scrollIntoView({ block: "end" });
  }, [text, busy, error, incomplete]);

  // 语法检查放在译文之后（B 方案：先出译文，波浪线后到）
  useEffect(() => {
    if (busy || !text || !source || error) return;
    if (checkedFor.current === source) return;

    const latin = (source.match(/[a-zA-Z]/g) ?? []).length;
    const cjk = (source.match(/[\u4e00-\u9fff]/g) ?? []).length;
    if (latin < 40 || cjk * 2 >= latin) return; // 中文原文不查语法

    checkedFor.current = source;
    setChecking(true);
    void api
      .checkGrammar(source)
      .then(setIssues)
      .catch(() => setIssues([]))
      .finally(() => setChecking(false));
  }, [busy, text, source, error]);

  /** 查一个词：鼠标划选后调用，键盘快捷键也走这里 */
  const explain = useCallback(async (term: string, context: string, x: number, y: number) => {
    if (!term || term.length > 60) return;
    setGloss({ data: null, error: null, x, y });
    try {
      const data = await api.explainWord(term, context);
      setGloss((current) => (current ? { ...current, data } : current));
    } catch (err) {
      setGloss((current) =>
        current ? { ...current, error: String(err).replace(/^Error:\s*/, "") } : current,
      );
    }
  }, []);

  const onMouseUp = useCallback(
    (event: React.MouseEvent) => {
      const selection = window.getSelection();
      const term = selection?.toString().trim() ?? "";
      if (!term) return;
      const context = selection?.anchorNode?.textContent?.slice(0, 400) ?? "";
      void explain(term, context, event.clientX, event.clientY);
    },
    [explain],
  );

  // Esc 处理器要知道浮层开没开，但不想每次开关都重新挂监听
  const glossOpen = useRef(false);
  useEffect(() => {
    glossOpen.current = gloss !== null;
  }, [gloss]);

  // 键盘可达：选中后 ⌘⇧D 查词（鼠标不是唯一入口）
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        // 浮层开着时这个 Esc 只关浮层：不能再传到 App，让它把整篇译文一起退掉。
        // 输入法组字时的 Esc 是取消候选词，不拦。
        if (glossOpen.current && !event.isComposing && event.keyCode !== 229) {
          event.preventDefault();
          event.stopImmediatePropagation();
          setGloss(null);
        }
        return;
      }
      if (!(event.metaKey && event.shiftKey) || event.key.toLowerCase() !== "d") return;

      const selection = window.getSelection();
      const term = selection?.toString().trim() ?? "";
      if (!term) return;
      event.preventDefault();
      const rect = selection?.rangeCount
        ? selection.getRangeAt(0).getBoundingClientRect()
        : ({ left: 240, bottom: 240 } as DOMRect);
      const context = selection?.anchorNode?.textContent?.slice(0, 400) ?? "";
      void explain(term, context, rect.left, rect.bottom);
    };

    // 捕获阶段：保证排在 App 的冒泡监听之前，与挂载先后无关
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [explain]);

  // 点别处收起浮层（浮层内部的 mousedown 已 stopPropagation）
  useEffect(() => {
    const close = () => setGloss(null);
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, []);

  // 两栏按比例同步滚动
  const syncing = useRef<"left" | "right" | null>(null);
  const syncScroll = (from: "left" | "right") => () => {
    if (syncing.current && syncing.current !== from) return;
    syncing.current = from;
    const a = from === "left" ? leftRef.current : rightRef.current;
    const b = from === "left" ? rightRef.current : leftRef.current;
    if (a && b) {
      const ratio = a.scrollTop / Math.max(1, a.scrollHeight - a.clientHeight);
      b.scrollTop = ratio * Math.max(0, b.scrollHeight - b.clientHeight);
    }
    requestAnimationFrame(() => {
      syncing.current = null;
    });
  };

  const copy = async (which: "source" | "target", value: string) => {
    if (!value) return;
    if (await copyText(value)) {
      setCopied(which);
      window.setTimeout(() => setCopied(null), 1400);
    }
  };

  return (
    <div className="flex h-full min-h-0 gap-3">
      <Pane
        title="原文"
        hint={checking ? "检查中…" : issues.length > 0 ? `${issues.length} 处标记` : `${source.length} 字符`}
        action={
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={onBack}
              className="rounded-md px-2 py-1 text-[12.5px] text-subtle hover:text-paper"
            >
              ← 返回查词
            </button>
            <CopyButton
              label={copied === "source" ? "已复制" : "复制原文"}
              onClick={() => void copy("source", source)}
            />
          </div>
        }
        scrollRef={leftRef}
        onScroll={syncScroll("left")}
      >
        <div onMouseUp={onMouseUp}>
          <Prose text={source} tone="source" issues={issues} />
        </div>
      </Pane>

      <Pane
        title="译文"
        hint={busy ? "生成中…" : text ? `${error || incomplete ? "不完整 · " : ""}${text.length} 字符` : ""}
        action={
          <div className="flex items-center gap-2">
            <CopyButton
              label={copied === "target" ? "已复制" : "复制译文"}
              disabled={!text || busy}
              onClick={() => void copy("target", text)}
            />
            {busy && (
              <button
                type="button"
                onClick={onStop}
                className="rounded-md border border-[rgba(255,107,74,0.5)] px-2.5 py-1 text-[12.5px] text-acc hover:bg-[rgba(255,107,74,0.12)]"
              >
                停止
              </button>
            )}
          </div>
        }
        scrollRef={rightRef}
        onScroll={syncScroll("right")}
      >
        <div onMouseUp={onMouseUp}>
          <Prose text={text} tone="target" />
        </div>

        {busy && <span className="ml-[1px] inline-block h-[1.1em] w-[2px] animate-pulse bg-acc align-middle" />}

        {/* 有半篇译文时也必须报错：不许把中途断掉的结果当成完整译文 */}
        {!busy && error && (
          <div
            role="alert"
            className={`rounded-lg border border-[rgba(255,82,48,0.35)] bg-[rgba(255,82,48,0.08)] px-4 py-3 ${text ? "mt-3" : ""}`}
          >
            <p className="text-[13.5px] leading-relaxed text-[#f6b09b]">
              {text ? `译文不完整：${error.replace(/[。.\s]+$/, "")}。上面只是已经收到的部分。` : error}
            </p>
            <button
              type="button"
              onClick={onRetry}
              className="mt-2 rounded-md border border-line-strong px-3 py-1.5 text-[13px] text-paper hover:bg-[rgba(248,246,241,0.06)]"
            >
              {text ? "重新翻译" : "重试"}
            </button>
          </div>
        )}

        {/* 没报错、但也没正常写完（停止、内容策略、连接提前关闭）：同样要明说 */}
        {!busy && !error && incomplete && (
          <div
            role="status"
            className={`rounded-lg border border-[rgba(240,180,94,0.35)] bg-[rgba(240,180,94,0.08)] px-4 py-3 ${text ? "mt-3" : ""}`}
          >
            <p className="text-[13.5px] leading-relaxed text-[#f0b45e]">
              {text ? `译文不完整：${incomplete}。上面只是已经收到的部分。` : `没有译文：${incomplete}。`}
            </p>
            <button
              type="button"
              onClick={onRetry}
              className="mt-2 rounded-md border border-line-strong px-3 py-1.5 text-[13px] text-paper hover:bg-[rgba(248,246,241,0.06)]"
            >
              重新翻译
            </button>
          </div>
        )}

        {truncated && (
          <p className="mt-3 rounded-lg border border-[rgba(240,180,94,0.35)] bg-[rgba(240,180,94,0.08)] px-3 py-2 text-[13px] leading-relaxed text-[#f0b45e]">
            译文触到输出上限，最后一段可能不完整。可以选中剩余部分再翻一次。
          </p>
        )}

        <div ref={endRef} />
      </Pane>

      {gloss && (
        <div
          className="fixed z-50 w-[300px] rounded-xl border border-line-strong bg-[#16161b] px-4 py-3 shadow-[0_18px_50px_-20px_rgba(0,0,0,0.9)]"
          style={{
            left: Math.min(gloss.x, window.innerWidth - 320),
            top: Math.min(gloss.y + 14, window.innerHeight - 200),
          }}
          onMouseDown={(event) => event.stopPropagation()}
        >
          {gloss.error ? (
            <p className="text-[13px] text-[#f6b09b]">{gloss.error}</p>
          ) : !gloss.data ? (
            <p className="text-[13px] text-muted">查询中…</p>
          ) : (
            <>
              <div className="flex items-baseline gap-2">
                <span className="text-[16px] font-semibold text-paper">{gloss.data.term}</span>
                {gloss.data.pos && <span className="text-[12px] text-subtle">{gloss.data.pos}</span>}
              </div>
              <p className="mt-1.5 text-[14px] leading-relaxed text-[#e6e1d8]">{gloss.data.gloss}</p>
              {gloss.data.note && (
                <p className="mt-1 text-[13px] leading-relaxed text-subtle">{gloss.data.note}</p>
              )}
              <p className="mt-2 font-mono text-[11px] text-subtle">Esc 关闭 · 选中词后 ⌘⇧D</p>
            </>
          )}
        </div>
      )}
    </div>
  );
}

function Pane({
  title,
  hint,
  action,
  children,
  scrollRef,
  onScroll,
}: {
  title: string;
  hint?: string;
  action: React.ReactNode;
  children: React.ReactNode;
  scrollRef?: React.RefObject<HTMLDivElement | null>;
  onScroll?: () => void;
}) {
  return (
    <section className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-2xl border border-line bg-panel">
      <header className="flex shrink-0 items-center justify-between border-b border-line px-4 py-2.5">
        <div className="flex items-baseline gap-2">
          <span className="text-[14px] font-medium text-paper">{title}</span>
          {hint && <span className="font-mono text-[12px] text-subtle">{hint}</span>}
        </div>
        {action}
      </header>
      <div ref={scrollRef} onScroll={onScroll} className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        {children}
      </div>
    </section>
  );
}

function CopyButton({ label, onClick, disabled }: { label: string; onClick: () => void; disabled?: boolean }) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className="rounded-md border border-line px-2.5 py-1 text-[12.5px] text-muted hover:border-line-strong hover:text-paper disabled:opacity-50"
    >
      {label}
    </button>
  );
}

/**
 * 按原文的结构渲染：空行 = 段落间隔，短行且无句末标点 = 小标题，`- `/`• ` = 列表。
 * 语法错误用虚线下划线 + 颜色双编码（不靠颜色单独表意），悬停看解释。
 */
function Prose({ text, tone, issues = [] }: { text: string; tone: "source" | "target"; issues?: GrammarIssue[] }) {
  const color = tone === "source" ? "text-[#e6e1d8]" : "text-[#f6f3ed]";
  const blocks = text.split(/\n\s*\n/).filter((block) => block.trim().length > 0);

  return (
    <div className="space-y-3.5">
      {blocks.map((block, index) => {
        const lines = block.split("\n").filter((line) => line.trim().length > 0);

        if (lines.length > 0 && lines.every((line) => /^\s*[-*•]\s+/.test(line))) {
          return (
            <ul key={index} className={`list-disc space-y-1 pl-5 text-[15px] leading-[1.75] ${color}`}>
              {lines.map((line, item) => (
                <li key={item}>{mark(line.replace(/^\s*[-*•]\s+/, ""), issues)}</li>
              ))}
            </ul>
          );
        }

        const trimmed = block.trim();
        const isHeading = lines.length === 1 && trimmed.length <= 60 && !/[.!?。！？:：;；]$/.test(trimmed);

        if (isHeading) {
          return (
            <h3 key={index} className="pt-1 font-serif text-[19px] font-semibold leading-snug text-paper">
              {trimmed}
            </h3>
          );
        }

        return (
          <p key={index} className={`whitespace-pre-wrap text-[15px] leading-[1.75] ${color}`}>
            {mark(block, issues)}
          </p>
        );
      })}
    </div>
  );
}

/** 把命中的错误片段包成虚线，其余原样输出。 */
function mark(text: string, issues: GrammarIssue[]): React.ReactNode {
  const hits = issues.filter((issue) => issue.fragment && text.includes(issue.fragment));
  if (hits.length === 0) return text;

  const nodes: React.ReactNode[] = [];
  let rest = text;
  let key = 0;

  while (rest.length > 0) {
    let at = -1;
    let hit: GrammarIssue | null = null;
    for (const issue of hits) {
      const index = rest.indexOf(issue.fragment);
      if (index >= 0 && (at < 0 || index < at)) {
        at = index;
        hit = issue;
      }
    }
    if (!hit || at < 0) {
      nodes.push(rest);
      break;
    }
    if (at > 0) nodes.push(rest.slice(0, at));
    nodes.push(
      <span
        key={`i${key++}`}
        title={hit.explanation + (hit.replacement ? ` → ${hit.replacement}` : "")}
        className="cursor-help underline decoration-[#ff6b4a] decoration-wavy decoration-2 underline-offset-[3px]"
      >
        {hit.fragment}
      </span>,
    );
    rest = rest.slice(at + hit.fragment.length);
  }

  return nodes;
}
