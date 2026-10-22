import { useMemo, useState } from "react";
import type { Sense, WordEntry } from "../../lib/schema";
import { cn } from "../../lib/cn";

const POS_LABEL: Record<string, string> = {
  n: "n.",
  v: "v.",
  adj: "adj.",
  adv: "adv.",
  prep: "prep.",
  conj: "conj.",
  pron: "pron.",
  num: "num.",
  phr: "phr.",
};

const REGISTER_LABEL: Record<string, string> = {
  neutral: "",
  formal: "书面",
  informal: "口语",
  technical: "技术",
  legal: "法律",
  slang: "俚语",
};

const BAND_LABEL: Record<string, string> = {
  high: "高频",
  mid: "中频",
  low: "低频",
};

export function SenseCard({
  entry,
  warnings = [],
}: {
  entry: WordEntry;
  warnings?: string[];
}) {
  const [expanded, setExpanded] = useState(false);
  const [openSense, setOpenSense] = useState<number | null>(null);

  const visible = useMemo(
    () => (expanded ? entry.senses : entry.senses.slice(0, 3)),
    [entry.senses, expanded],
  );
  const hidden = entry.senses.length - visible.length;

  const ipa = entry.pronunciation.ipa ?? entry.pronunciation.pinyin ?? "";
  const posLine = entry.pos.map((item) => POS_LABEL[item] ?? item).join(" / ");
  const band = entry.frequency.band ? BAND_LABEL[entry.frequency.band] ?? "" : "";

  return (
    <article className="flex flex-col rounded-2xl border border-line bg-panel px-5 pb-3.5 pt-4">
      <header className="flex items-start justify-between gap-4 border-b border-line pb-3">
        <div className="min-w-0">
          <h1 className="truncate font-serif text-[34px] font-semibold leading-[1.12] tracking-[-0.01em] text-paper">
            {entry.word}
          </h1>
          <p className="mt-1.5 font-mono text-[13px] tracking-[0.01em] text-muted">
            {ipa || "—"}
            {posLine ? <span className="text-subtle"> · {posLine}</span> : null}
          </p>
        </div>
        <div className="shrink-0 pt-1 text-right">
          <div className="inline-block rounded-full border border-line px-2.5 py-1 font-mono text-[12px] text-muted">
            {entry.frequency.zipf != null ? `词频 ${entry.frequency.zipf.toFixed(1)}` : "词频未知"}
            {band ? ` · ${band}` : ""}
          </div>
          <div className="mt-2 text-[11px] tracking-[0.1em] text-subtle">
            {entry.senses.length} 个义项 · 按常用度排序
          </div>
        </div>
      </header>

      <div className="mt-1.5">
        {visible.map((sense, index) => (
          <SenseRow
            key={`${sense.rank}-${sense.gloss_zh}`}
            sense={sense}
            index={index}
            open={openSense === sense.rank}
            onToggle={() => setOpenSense((current) => (current === sense.rank ? null : sense.rank))}
          />
        ))}
      </div>

      {hidden > 0 && (
        <button
          type="button"
          onClick={() => setExpanded(true)}
          className="mt-1.5 flex items-center justify-between rounded-lg px-1 py-2 text-left text-[13px] text-muted hover:text-paper"
        >
          <span className="truncate pr-3">
            + {hidden} 个低频义项：{entry.senses.slice(3).map((sense) => sense.gloss_zh.split(/[；;，,]/)[0]).join(" · ")}
          </span>
          <span className="shrink-0 font-mono text-[11.5px]">展开 ⌄</span>
        </button>
      )}

      {expanded && entry.senses.length > 3 && (
        <button
          type="button"
          onClick={() => setExpanded(false)}
          className="mt-1 self-start px-1 py-1 font-mono text-[11.5px] text-subtle hover:text-paper"
        >
          收起 ⌃
        </button>
      )}

      {entry.confusables.length > 0 && (
        <div className="mt-2.5 border-t border-line pt-2.5">
          {entry.confusables.map((item) => (
            <p key={item.word} className="text-[12.5px] leading-relaxed text-muted">
              <span className="text-subtle">易混 </span>
              <b className="font-semibold text-paper">{item.word}</b>
              <span> —— {item.difference}</span>
            </p>
          ))}
        </div>
      )}

      {entry.notes && (
        <p className="mt-2.5 border-t border-line pt-2.5 text-[12.5px] leading-relaxed text-muted">
          {entry.notes}
        </p>
      )}

      {warnings.length > 0 && (
        <ul className="mt-2.5 space-y-1 border-t border-line pt-2.5">
          {warnings.map((warning) => (
            <li key={warning} className="text-[12px] leading-relaxed text-[#e0a94f]">
              · {warning}
            </li>
          ))}
        </ul>
      )}
    </article>
  );
}

function SenseRow({
  sense,
  index,
  open,
  onToggle,
}: {
  sense: Sense;
  index: number;
  open: boolean;
  onToggle: () => void;
}) {
  const example = sense.examples[0];
  const opacity = [1, 0.62, 0.38, 0.2][index] ?? 0.2;
  const register = REGISTER_LABEL[sense.register] ?? "";

  return (
    <div
      className={cn(
        "relative border-b border-[rgba(246,243,237,0.08)] py-2.5 pl-5 last:border-b-0",
        index === 0 && "pt-3",
      )}
    >
      <span
        aria-hidden
        className="absolute left-0 top-3 bottom-3 w-[3px] rounded-sm bg-acc-strong"
        style={{ opacity }}
      />

      <button type="button" onClick={onToggle} className="block w-full text-left">
        <div className="flex items-baseline gap-2">
          <span className="font-mono text-[12px] text-subtle">{String(index + 1).padStart(2, "0")}</span>
          <span className="text-[16.5px] font-semibold leading-[1.4] text-paper">{sense.gloss_zh}</span>
          <small className="text-[12px] font-normal text-subtle">
            {POS_LABEL[sense.pos] ?? sense.pos}
            {register ? ` · ${register}` : ""}
          </small>
          <span className="ml-auto shrink-0 font-mono text-[12.5px] text-muted">
            {Math.round(sense.share * 100)}%
          </span>
        </div>

        {example && (
          <>
            <p className="mt-1.5 text-[14px] italic leading-[1.55] text-[#d7d2ca]">{example.text}</p>
            <p className="mt-1 flex flex-wrap items-center gap-2 text-[13px] leading-[1.6] text-muted">
              <span>{example.translation}</span>
              <span className="rounded bg-[rgba(246,243,237,0.08)] px-1.5 py-0.5 text-[11.5px] tracking-[0.02em] text-[#c3beb6]">
                {example.scene}
                {sense.domains[0] ? ` · ${sense.domains[0]}` : ""}
              </span>
            </p>
          </>
        )}

        {/* 频次条：宽度 = 该义项的使用占比 */}
        <div className="mt-2 h-[3px] w-full overflow-hidden rounded-full bg-[rgba(246,243,237,0.1)]">
          <div
            className="h-full rounded-full bg-acc-strong"
            style={{ width: `${Math.max(3, Math.round(sense.share * 100))}%`, opacity: 0.85 }}
          />
        </div>
      </button>

      {open && (
        <div className="mt-2 space-y-1.5 rounded-lg bg-[rgba(246,243,237,0.05)] px-3 py-2">
          {sense.collocations.length > 0 && (
            <p className="text-[12.5px] leading-relaxed text-[#d7d2ca]">
              <span className="text-subtle">搭配 </span>
              {sense.collocations.join("  ·  ")}
            </p>
          )}
          {sense.gloss_en && (
            <p className="text-[12.5px] leading-relaxed text-muted">
              <span className="text-subtle">释义 </span>
              {sense.gloss_en}
            </p>
          )}
          {sense.synonyms.length > 0 && (
            <p className="text-[12.5px] leading-relaxed text-muted">
              <span className="text-subtle">近义 </span>
              {sense.synonyms.join(" / ")}
            </p>
          )}
          {sense.pitfall && (
            <p className="text-[12.5px] leading-relaxed text-[#e0a94f]">
              <span className="text-[#b08a44]">易错 </span>
              {sense.pitfall}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
