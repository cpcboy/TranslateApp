export function Skeleton({ rows = 3 }: { rows?: number }) {
  return (
    <div className="rounded-2xl border border-line bg-[#0f0f12] px-4 pb-3 pt-3" aria-busy="true">
      <div className="flex items-start justify-between border-b border-line pb-2.5">
        <div>
          <div className="h-7 w-28 animate-pulse rounded bg-[rgba(242,239,233,0.08)]" />
          <div className="mt-2 h-3 w-36 animate-pulse rounded bg-[rgba(242,239,233,0.05)]" />
        </div>
        <div className="h-6 w-24 animate-pulse rounded-full bg-[rgba(242,239,233,0.05)]" />
      </div>
      <div className="mt-2 space-y-3">
        {Array.from({ length: rows }).map((_, index) => (
          <div key={index} className="space-y-1.5 pl-4">
            <div className="h-3.5 w-2/5 animate-pulse rounded bg-[rgba(242,239,233,0.09)]" />
            <div className="h-3 w-4/5 animate-pulse rounded bg-[rgba(242,239,233,0.05)]" />
            <div className="h-3 w-3/5 animate-pulse rounded bg-[rgba(242,239,233,0.04)]" />
          </div>
        ))}
      </div>
      <p className="mt-4 text-center font-mono text-[9.5px] tracking-[0.1em] text-[#5e5b57]">查询中…</p>
    </div>
  );
}
