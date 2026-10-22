export function Skeleton({ rows = 3 }: { rows?: number }) {
  return (
    <div className="rounded-2xl border border-line bg-panel px-6 pb-4 pt-5" aria-busy="true">
      <div className="flex items-start justify-between border-b border-line pb-4">
        <div>
          <div className="h-10 w-36 animate-pulse rounded bg-[rgba(248,246,241,0.1)]" />
          <div className="mt-2.5 h-4 w-40 animate-pulse rounded bg-[rgba(248,246,241,0.07)]" />
        </div>
        <div className="h-8 w-28 animate-pulse rounded-full bg-[rgba(248,246,241,0.07)]" />
      </div>
      <div className="mt-3 space-y-4">
        {Array.from({ length: rows }).map((_, index) => (
          <div key={index} className="space-y-2 pl-5">
            <div className="h-5 w-2/5 animate-pulse rounded bg-[rgba(248,246,241,0.12)]" />
            <div className="h-4 w-4/5 animate-pulse rounded bg-[rgba(248,246,241,0.07)]" />
            <div className="h-4 w-3/5 animate-pulse rounded bg-[rgba(248,246,241,0.06)]" />
          </div>
        ))}
      </div>
      <p className="mt-5 text-center font-mono text-[13.5px] tracking-[0.08em] text-subtle">查询中…</p>
    </div>
  );
}
