import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, type GrammarIssue, type WordGloss } from "../../lib/ipc";
import { lastBlockIsOpen, markedCount, shouldCheckGrammar, sliceAround, splitMarked } from "./reading";

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

/**
 * 选区所在段落里、以选中词为中心的上下文。
 * 有语法标注时一个段落会被切成好几个文本节点，所以取最近的块级元素，再用 Range 算出准确偏移。
 */
function selectionContext(selection: Selection | null): string {
  if (!selection || selection.rangeCount === 0) return "";
  const range = selection.getRangeAt(0);
  const container = range.startContainer;
  // 起点落在元素节点上（比如从段落之间的空隙开始拖）时，startOffset 是子节点序号，不是字符偏移，
  // 拿它去切文本会取到文档开头。这种情况没有可靠的上下文，只给选中的内容本身
  if (container.nodeType === Node.ELEMENT_NODE && !(container as Element).closest("p, li, h3")) {
    return range.toString().trim();
  }
  const element = container.nodeType === Node.ELEMENT_NODE ? (container as Element) : container.parentElement;
  const block = element?.closest("p, li, h3");
  if (!block) return sliceAround(container.textContent ?? "", range.startOffset, range.toString().length);

  const before = document.createRange();
  before.selectNodeContents(block);
  before.setEnd(range.startContainer, range.startOffset);
  return sliceAround(block.textContent ?? "", before.toString().length, range.toString().length);
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

  // 真正画出来的标记数，不是模型给的问题条数
  const marks = useMemo(() => markedCount(source, issues), [source, issues]);

  const leftRef = useRef<HTMLDivElement>(null);
  const rightRef = useRef<HTMLDivElement>(null);
  const endRef = useRef<HTMLDivElement>(null);
  const checkedFor = useRef<string>("");

  // 流式时右栏自动跟到底；没写完时也滚到底，让「不完整」提示一眼可见
  useEffect(() => {
    if (busy || ((error || incomplete) && text)) endRef.current?.scrollIntoView({ block: "end" });
  }, [text, busy, error, incomplete]);

  // 原文换了（粘贴了另一篇）：上一篇的标记、还在路上的语法检查结果都作废。
  // 输入框在阅读视图里一直可用，这个组件不会因此重新挂载；不清的话，上一篇的波浪线和
  // 解释会画在新原文上。要排在下面那个检查 effect 之前
  const grammarSeq = useRef(0);
  useEffect(() => {
    grammarSeq.current += 1;
    // 「已经查过这一篇」的记号也要清：A → B（没写完 / 中文原文，没查）→ 再贴回 A 时，
    // 不清的话会认为 A 查过了，A 的标记就再也出不来；开发模式下组件重挂载也靠它重新触发检查
    checkedFor.current = "";
    setIssues([]);
    setChecking(false);
  }, [source]);

  // 语法检查放在译文之后（B 方案：先出译文，波浪线后到）
  useEffect(() => {
    // 没写完（出错、点了停止、服务端提前收工）不查，规则见 shouldCheckGrammar
    if (!shouldCheckGrammar({ busy, text, source, error, incomplete, checkedFor: checkedFor.current })) return;

    checkedFor.current = source;
    const my = grammarSeq.current;
    setChecking(true);
    void api
      .checkGrammar(source)
      .then((found) => {
        if (my === grammarSeq.current) setIssues(found);
      })
      .catch(() => {
        if (my === grammarSeq.current) setIssues([]);
      })
      .finally(() => {
        if (my === grammarSeq.current) setChecking(false);
      });
  }, [busy, text, source, error, incomplete]);

  // 划词请求的序号：连续选词时，先发的请求后返回，不能盖掉后选的那个词的浮层
  const glossSeq = useRef(0);
  // 同一个词 + 同一段上下文，再选一次直接用上次的答案，不再花一次请求
  const glossCache = useRef(new Map<string, WordGloss>());

  /** 查一个词：鼠标划选后调用，键盘快捷键也走这里 */
  const explain = useCallback(async (term: string, context: string, x: number, y: number) => {
    if (!term || term.length > 60) return;
    const my = ++glossSeq.current;
    const cacheKey = `${term}\u0000${context}`;
    const cached = glossCache.current.get(cacheKey);
    if (cached) {
      setGloss({ data: cached, error: null, x, y });
      return;
    }
    setGloss({ data: null, error: null, x, y });
    try {
      const data = await api.explainWord(term, context);
      // 先缓存再看有没有被顶掉：钱已经花了，下次选同一个词就不用再付
      if (glossCache.current.size >= 50) glossCache.current.clear();
      glossCache.current.set(cacheKey, data);
      if (my !== glossSeq.current) return;
      setGloss((current) => (current ? { ...current, data } : current));
    } catch (err) {
      if (my !== glossSeq.current) return;
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
      void explain(term, selectionContext(selection), event.clientX, event.clientY);
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
          glossSeq.current += 1;
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
      void explain(term, selectionContext(selection), rect.left, rect.bottom);
    };

    // 捕获阶段：保证排在 App 的冒泡监听之前，与挂载先后无关
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [explain]);

  // 点别处收起浮层（浮层内部的 mousedown 已 stopPropagation）
  useEffect(() => {
    const close = () => {
      glossSeq.current += 1;
      setGloss(null);
    };
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
        hint={checking ? "检查中…" : issues.length > 0 && marks > 0 ? `${marks} 处标记` : `${source.length} 字符`}
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
          <Prose text={text} tone="target" streaming={busy} />
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
function Prose({
  text,
  tone,
  issues = [],
  streaming = false,
}: {
  text: string;
  tone: "source" | "target";
  issues?: GrammarIssue[];
  /** 还在流式生成：最后一块没写完，不能先判成小标题 */
  streaming?: boolean;
}) {
  const color = tone === "source" ? "text-[#e6e1d8]" : "text-[#f6f3ed]";
  const blocks = text.split(/\n\s*\n/).filter((block) => block.trim().length > 0);
  const openLast = lastBlockIsOpen(text, streaming);
  // 每条语法问题只标一处：已经在前面的段落标过的，后面的段落不再标
  const used = new Set<number>();

  return (
    <div className="space-y-3.5">
      {blocks.map((block, index) => {
        const lines = block.split("\n").filter((line) => line.trim().length > 0);

        if (lines.length > 0 && lines.every((line) => /^\s*[-*•]\s+/.test(line))) {
          return (
            <ul key={index} className={`list-disc space-y-1 pl-5 text-[15px] leading-[1.75] ${color}`}>
              {lines.map((line, item) => (
                <li key={item}>{mark(line.replace(/^\s*[-*•]\s+/, ""), issues, used)}</li>
              ))}
            </ul>
          );
        }

        const trimmed = block.trim();
        const isHeading =
          lines.length === 1 &&
          trimmed.length <= 60 &&
          !/[.!?。！？:：;；]$/.test(trimmed) &&
          !(openLast && index === blocks.length - 1);

        if (isHeading) {
          return (
            <h3 key={index} className="pt-1 font-serif text-[19px] font-semibold leading-snug text-paper">
              {mark(trimmed, issues, used)}
            </h3>
          );
        }

        return (
          <p key={index} className={`whitespace-pre-wrap text-[15px] leading-[1.75] ${color}`}>
            {mark(block, issues, used)}
          </p>
        );
      })}
    </div>
  );
}

/** 把命中的错误片段包成虚线，其余原样输出。命中规则见 reading.ts 的 splitMarked。 */
function mark(text: string, issues: GrammarIssue[], used: Set<number>): React.ReactNode {
  if (issues.length === 0) return text;
  return splitMarked(text, issues, used).map((piece, key) =>
    typeof piece === "string" ? (
      piece
    ) : (
      <span
        key={`i${key}`}
        title={piece.issue.explanation + (piece.issue.replacement ? ` → ${piece.issue.replacement}` : "")}
        className="cursor-help underline decoration-[#ff6b4a] decoration-wavy decoration-2 underline-offset-[3px]"
      >
        {piece.text}
      </span>
    ),
  );
}
