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

  const visible = useMemo(() => (expanded ? entry.senses : entry.senses.slice(0, 3)), [entry.senses, expanded]);
  const hidden = entry.senses.length - visible.length;

  const ipa = entry.pronunciation.ipa ?? entry.pronunciation.pinyin ?? "";
  const posLine = entry.pos.map((item) => POS_LABEL[item] ?? item).join(" / ");
  const band = entry.frequency.band ? BAND_LABEL[entry.frequency.band] ?? "" : "";

  return (
    <article className="flex flex-col gap-0 rounded-2xl border border-line bg-[#0f0f12] px-4 pb-3 pt-3">
      <header className="flex items-start justify-between gap-3 border-b border-line pb-2.5">
        <div className="min-w-0">
          <h1 className="truncate font-serif text-[28px] font-semibold leading-tight tracking-[-0.01em] text-paper">
            {entry.word}
          </h1>
          <p className="mt-1 font-mono text-[11px] tracking-[0.02em] text-[#a8a49d]">
            {ipa || "—"}
            {posLine ? <span className="text-muted"> · {posLine}</span> : null}
          </p>
        </div>
        <div className="shrink-0 text-right">
          <div className="inline-block rounded-full border border-line px-2 py-[3px] font-mono text-[10px] text-[#82807b]">
            {entry.frequency.zipf != null ? `词频 ${entry.frequency.zipf.toFixed(1)}` : "词频未知"}
            {band ? ` · ${band}` : ""}
          </div>
          <div className="mt-1.5 text-[9px] uppercase tracking-[0.12em] text-[#5e5b57]">
            {entry.senses.length} 个义项 · 按常用度排序
          </div>
        </div>
      </header>

      <div className="mt-1">
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
          className="mt-1 flex items-center justify-between rounded-md px-1 py-1.5 text-[10.5px] text-muted hover:text-paper"
        >
          <span>
            + {hidden} 个低频义项：{entry.senses.slice(3).map((sense) => sense.gloss_zh.split(/[；;，,]/)[0]).join(" · ")}
          </span>
          <span className="font-mono text-[9.5px]">展开 ⌄</span>
        </button>
      )}

      {expanded && entry.senses.length > 3 && (
        <button
          type="button"
          onClick={() => setExpanded(false)}
          className="mt-1 self-start px-1 py-1 font-mono text-[9.5px] text-muted hover:text-paper"
        >
          收起 ⌃
        </button>
      )}

      {entry.confusables.length > 0 && (
        <div className="mt-2 border-t border-line pt-2">
          <span className="text-[10px] text-[#7e7b76]">易混 </span>
          {entry.confusables.map((item) => (
            <span key={item.word} className="text-[10.5px] text-[#beb9b1]">
              <b className="font-semibold text-paper">{item.word}</b>
              <span className="text-muted"> —— {item.difference}</span>
            </span>
          ))}
        </div>
      )}

      {entry.notes && (
        <p className="mt-2 border-t border-line pt-2 text-[10.5px] leading-relaxed text-muted">{entry.notes}</p>
      )}

      {warnings.length > 0 && (
        <ul className="mt-2 space-y-0.5 border-t border-line pt-2">
          {warnings.map((warning) => (
            <li key={warning} className="text-[10px] leading-relaxed text-[#d8a24a]">
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
  const opacity = [1, 0.55, 0.3, 0.16][index] ?? 0.16;
  const register = REGISTER_LABEL[sense.register] ?? "";

  return (
    <div
      className={cn(
        "relative border-b border-[rgba(242,239,233,0.065)] py-[7px] pl-4 pr-0 last:border-b-0",
        index === 0 && "pt-2",
      )}
    >
      <span
        aria-hidden
        className="absolute left-0 top-[9px] bottom-[9px] w-[2px] rounded-sm bg-acc"
        style={{ opacity }}
      />

      <button type="button" onClick={onToggle} className="block w-full text-left">
        <div className="flex items-baseline gap-1.5">
          <span className="font-mono text-[9.5px] text-[#6b6863]">
            {String(index + 1).padStart(2, "0")}
          </span>
          <span className="text-[13.5px] font-semibold leading-[1.35] text-paper">
            {sense.gloss_zh}
          </span>
          <small className="text-[10px] font-normal text-[#7e7b76]">
            {POS_LABEL[sense.pos] ?? sense.pos}
            {register ? ` · ${register}` : ""}
          </small>
          <span className="ml-auto shrink-0 font-mono text-[10.5px] text-[#9a968f]">
            {Math.round(sense.share * 100)}%
          </span>
        </div>

        {example && (
          <>
            <p className="mt-1 text-[11.5px] italic leading-[1.5] text-[#beb9b1]">{example.text}</p>
            <p className="mt-[1px] flex flex-wrap items-center gap-2 text-[11px] leading-[1.55] text-[#827e78]">
              <span>{example.translation}</span>
              <span className="rounded bg-[rgba(242,239,233,0.05)] px-[5px] py-[1px] text-[9.5px] tracking-[0.04em] text-[#8b8781]">
                {example.scene}
                {sense.domains[0] ? ` · ${sense.domains[0]}` : ""}
              </span>
            </p>
          </>
        )}

        {/* 频次条：宽度 = 该义项的使用占比 */}
        <div className="mt-1.5 h-[2px] w-full overflow-hidden rounded-full bg-[rgba(242,239,233,0.07)]">
          <div
            className="h-full rounded-full bg-acc"
            style={{ width: `${Math.max(2, Math.round(sense.share * 100))}%`, opacity: 0.75 }}
          />
        </div>
      </button>

      {open && (
        <div className="mt-1.5 space-y-1 rounded-md bg-[rgba(242,239,233,0.03)] px-2 py-1.5">
          {sense.collocations.length > 0 && (
            <p className="text-[10.5px] text-[#beb9b1]">
              <span className="text-muted">搭配 </span>
              {sense.collocations.join("  ·  ")}
            </p>
          )}
          {sense.gloss_en && (
            <p className="text-[10.5px] text-[#8b8781]">
              <span className="text-muted">释义 </span>
              {sense.gloss_en}
            </p>
          )}
          {sense.synonyms.length > 0 && (
            <p className="text-[10.5px] text-[#8b8781]">
              <span className="text-muted">近义 </span>
              {sense.synonyms.join(" / ")}
            </p>
          )}
          {sense.pitfall && (
            <p className="text-[10.5px] leading-relaxed text-[#d8a24a]">
              <span className="text-[#a8863f]">易错 </span>
              {sense.pitfall}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
