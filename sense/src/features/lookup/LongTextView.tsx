import { useEffect, useRef, useState } from "react";

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
}: {
  source: string;
  text: string;
  busy: boolean;
}) {
  const [copied, setCopied] = useState<"source" | "target" | null>(null);
  const endRef = useRef<HTMLDivElement>(null);

  // 边出边滚到底，省得用户自己追
  useEffect(() => {
    if (busy) endRef.current?.scrollIntoView({ block: "end" });
  }, [text, busy]);

  const copy = async (which: "source" | "target", value: string) => {
    if (!value) return;
    const ok = await copyText(value);
    if (ok) {
      setCopied(which);
      window.setTimeout(() => setCopied(null), 1400);
    }
  };

  return (
    <div className="flex h-full min-h-0 gap-3">
      <Pane
        title="原文"
        hint={`${source.length} 字符`}
        action={
          <CopyButton label={copied === "source" ? "已复制" : "复制原文"} onClick={() => void copy("source", source)} />
        }
      >
        <pre className="whitespace-pre-wrap font-sans text-[15px] leading-[1.75] text-[#e6e1d8]">
          {source}
        </pre>
      </Pane>

      <Pane
        title="译文"
        hint={busy ? "生成中…" : text ? `${text.length} 字符` : ""}
        action={
          <CopyButton
            label={copied === "target" ? "已复制" : "复制译文"}
            disabled={!text || busy}
            onClick={() => void copy("target", text)}
          />
        }
      >
        <pre className="whitespace-pre-wrap font-sans text-[15px] leading-[1.75] text-[#f6f3ed]">
          {text}
          {busy && <span className="ml-[1px] inline-block h-[1.1em] w-[2px] animate-pulse bg-acc align-middle" />}
        </pre>
        <div ref={endRef} />
      </Pane>
    </div>
  );
}

function Pane({
  title,
  hint,
  action,
  children,
}: {
  title: string;
  hint?: string;
  action: React.ReactNode;
  children: React.ReactNode;
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
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">{children}</div>
    </section>
  );
}

function CopyButton({
  label,
  onClick,
  disabled,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
}) {
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
