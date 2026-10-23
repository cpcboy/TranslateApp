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

export function LongTextView({
  source,
  text,
  busy,
  onStop,
}: {
  source: string;
  text: string;
  busy: boolean;
  onStop: () => void;
}) {
  const [copied, setCopied] = useState<"source" | "target" | null>(null);
  const [issues, setIssues] = useState<GrammarIssue[]>([]);
  const [checking, setChecking] = useState(false);
  const [gloss, setGloss] = useState<{ data: WordGloss | null; error: string | null; x: number; y: number } | null>(null);
  const endRef = useRef<HTMLDivElement>(null);
  const checkedFor = useRef<string>("");

  useEffect(() => {
    if (busy) endRef.current?.scrollIntoView({ block: "end" });
  }, [text, busy]);

  // 语法检查放在译文之后（用户选的是 B 方案：先出译文，波浪线后到）
  useEffect(() => {
    if (busy || !text || !source) return;
    if (checkedFor.current === source) return;
    // 只检查英文；中文原文没有「语法错误」这回事
    const latin = (source.match(/[a-zA-Z]/g) ?? []).length;
    const cjk = (source.match(/[\u4e00-\u9fff]/g) ?? []).length;
    if (latin < 40 || cjk * 2 >= latin) return;

    checkedFor.current = source;
    setChecking(true);
    void api
      .checkGrammar(source)
      .then(setIssues)
      .catch(() => setIssues([]))
      .finally(() => setChecking(false));
  }, [busy, text, source]);

  const onSelect = useCallback(
    async (event: React.MouseEvent) => {
      const selection = window.getSelection();
      const term = selection?.toString().trim() ?? "";
      if (!term || term.length > 60) return;

      // 取选中文本所在的句子当上下文，避免给「词典第一个义项」
      const anchorNode = selection?.anchorNode?.textContent ?? "";
      const context = anchorNode.slice(0, 400);

      setGloss({ data: null, error: null, x: event.clientX, y: event.clientY });
      try {
        const data = await api.explainWord(term, context);
        setGloss((current) => (current ? { ...current, data } : current));
      } catch (err) {
        setGloss((current) =>
          current ? { ...current, error: String(err).replace(/^Error:\s*/, "") } : current,
        );
      }
    },
    [],
  );

  useEffect(() => {
    const close = () => setGloss(null);
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setGloss(null);
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", onKey);
    };
  }, []);

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
          <CopyButton label={copied === "source" ? "已复制" : "复制原文"} onClick={() => void copy("source", source)} />
        }
      >
        <div onMouseUp={onSelect}>
          <Prose text={source} tone="source" issues={issues} />
        </div>
      </Pane>

      <Pane
        title="译文"
        hint={busy ? "生成中…" : text ? `${text.length} 字符` : ""}
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
      >
        <div onMouseUp={onSelect}>
          <Prose text={text} tone="target" />
        </div>
        {busy && <span className="ml-[1px] inline-block h-[1.1em] w-[2px] animate-pulse bg-acc align-middle" />}
        <div ref={endRef} />
      </Pane>

      {gloss && (
        <div
          className="fixed z-50 w-[300px] rounded-xl border border-line-strong bg-[#16161b] px-4 py-3 shadow-[0_18px_50px_-20px_rgba(0,0,0,0.9)]"
          style={{ left: Math.min(gloss.x, window.innerWidth - 320), top: Math.min(gloss.y + 14, window.innerHeight - 180) }}
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
            </>
          )}
        </div>
      )}
    </div>
  );
}

function Pane({ title, hint, action, children }: { title: string; hint?: string; action: React.ReactNode; children: React.ReactNode }) {
  return (
    <section className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-2xl border border-line bg-panel">
      <header className="flex shrink-0 items-center justify-between border-b border-line px-4 py-2.5">
        <div className="flex items-baseline gap-2">
          <span className="text-[14px] font-medium text-paper">{title}</span>
          {hint && <span className="font-mono text-[12px] text-subtle">{hint}</span>}
        </div>
        {action}
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">{children}</div>
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
