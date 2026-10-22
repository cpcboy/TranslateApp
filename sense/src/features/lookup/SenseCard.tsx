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
    <article className="flex flex-col rounded-2xl border border-line bg-panel px-7 pb-5 pt-6">
      <header className="flex items-start justify-between gap-6 border-b border-line pb-5">
        <div className="min-w-0">
          <h1 className="truncate font-serif text-[46px] font-semibold leading-[1.08] tracking-[-0.01em] text-paper">
            {entry.word}
          </h1>
          <p className="mt-2 font-mono text-[15px] tracking-[0.01em] text-muted">
            {ipa || "—"}
            {posLine ? <span className="text-subtle"> · {posLine}</span> : null}
          </p>
        </div>
        <div className="shrink-0 pt-1 text-right">
          <div className="inline-block rounded-full border border-line px-3.5 py-2 font-mono text-[14px] text-muted">
            {entry.frequency.zipf != null ? `词频 ${entry.frequency.zipf.toFixed(1)}` : "词频未知"}
            {band ? ` · ${band}` : ""}
          </div>
          <div className="mt-2.5 text-[13.5px] text-subtle">
            {entry.senses.length} 个义项 · 按常用度排序
          </div>
        </div>
      </header>

      <div className="mt-2">
        {visible.map((sense, index) => (
          <SenseRow
            key={`${sense.rank}-${sense.gloss_zh}`}
            sense={sense}
            lang={entry.lang}
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
          className="mt-2.5 flex items-center justify-between rounded-lg px-1 py-3 text-left text-[15px] text-muted hover:text-paper"
        >
          <span className="truncate pr-3">
            + {hidden} 个低频义项：{entry.senses.slice(3).map((sense) => sense.gloss_zh.split(/[；;，,]/)[0]).join(" · ")}
          </span>
          <span className="shrink-0 font-mono text-[14px]">展开 ⌄</span>
        </button>
      )}

      {expanded && entry.senses.length > 3 && (
        <button
          type="button"
          onClick={() => setExpanded(false)}
          className="mt-1 self-start px-1 py-1 font-mono text-[14px] text-subtle hover:text-paper"
        >
          收起 ⌃
        </button>
      )}

      {entry.confusables.length > 0 && (
        <div className="mt-3.5 border-t border-line pt-3.5">
          {entry.confusables.map((item) => (
            <p key={item.word} className="text-[15px] leading-relaxed text-muted">
              <span className="text-subtle">易混 </span>
              <b className="font-semibold text-paper">{item.word}</b>
              <span> —— {item.difference}</span>
            </p>
          ))}
        </div>
      )}

      {entry.notes && (
        <p className="mt-3.5 border-t border-line pt-3.5 text-[15px] leading-relaxed text-muted">
          {entry.notes}
        </p>
      )}

      {warnings.length > 0 && (
        <ul className="mt-3 space-y-1 border-t border-line pt-3">
          {warnings.map((warning) => (
            <li key={warning} className="text-[14px] leading-relaxed text-[#f0b45e]">
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
  lang,
  index,
  open,
  onToggle,
}: {
  sense: Sense;
  lang: "en" | "zh";
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
        "relative border-b border-[rgba(248,246,241,0.09)] py-4 pl-7 last:border-b-0",
        index === 0 && "pt-4",
      )}
    >
      <span
        aria-hidden
        className="absolute left-0 top-5 bottom-5 w-[4px] rounded-sm bg-acc-strong"
        style={{ opacity }}
      />

      <button type="button" onClick={onToggle} className="block w-full text-left">
        <div className="flex items-baseline gap-3">
          <span className="font-mono text-[14px] text-subtle">{String(index + 1).padStart(2, "0")}</span>
          <span className="text-[21px] font-semibold leading-[1.4] text-paper">{sense.gloss_zh}</span>
          <small className="text-[14px] font-normal text-subtle">
            {POS_LABEL[sense.pos] ?? sense.pos}
            {register ? ` · ${register}` : ""}
          </small>
          <span className="ml-auto shrink-0 font-mono text-[15px] text-muted">
            {Math.round(sense.share * 100)}%
          </span>
        </div>

        {example && (
          <>
            {/* 拉丁斜体只给英文例句：中文没有斜体字面，合成斜体会把字糊掉 */}
            <p
              className={cn(
                "mt-2.5 text-[17px] leading-[1.55] text-[#e6e1d8]",
                lang === "en" && "ex-en",
              )}
            >
              {example.text}
            </p>
            <p className="mt-2 flex flex-wrap items-center gap-2.5 text-[16px] leading-[1.6] text-muted">
              <span>{example.translation}</span>
              <span className="rounded bg-[rgba(248,246,241,0.1)] px-2.5 py-1 text-[13.5px] tracking-[0.02em] text-[#d8d3ca]">
                {example.scene}
                {sense.domains[0] ? ` · ${sense.domains[0]}` : ""}
              </span>
            </p>
          </>
        )}

        {/* 频次条：宽度 = 该义项的使用占比 */}
        <div className="mt-3 h-[4px] w-full overflow-hidden rounded-full bg-[rgba(248,246,241,0.12)]">
          <div
            className="h-full rounded-full bg-acc-strong"
            style={{ width: `${Math.max(3, Math.round(sense.share * 100))}%`, opacity: 0.9 }}
          />
        </div>
      </button>

      {open && (
        <div className="mt-2.5 space-y-2 rounded-lg bg-[rgba(248,246,241,0.06)] px-4 py-3">
          {sense.collocations.length > 0 && (
            <p className="text-[15px] leading-relaxed text-[#e6e1d8]">
              <span className="text-subtle">搭配 </span>
              {sense.collocations.join("  ·  ")}
            </p>
          )}
          {sense.gloss_en && (
            <p className="text-[15px] leading-relaxed text-muted">
              <span className="text-subtle">释义 </span>
              {sense.gloss_en}
            </p>
          )}
          {sense.synonyms.length > 0 && (
            <p className="text-[15px] leading-relaxed text-muted">
              <span className="text-subtle">近义 </span>
              {sense.synonyms.join(" / ")}
            </p>
          )}
          {sense.pitfall && (
            <p className="text-[15px] leading-relaxed text-[#f0b45e]">
              <span className="text-[#d1a35d]">易错 </span>
              {sense.pitfall}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
