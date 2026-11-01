import { cn } from "../../lib/cn";
import { canCheck, describeStatus, formatTime, showLastUpdate, visibleErrors } from "./describe";
import type { UpdateController } from "./useUpdate";

/** 设置页里的「更新」卡片：当前版本、状态、手动检查、自动检查开关。 */
export function UpdateCard({ update }: { update: UpdateController }) {
  const { status, busy, actionError, loadError } = update;

  // 状态一直取不到（多半是前后端版本对不上）：不能让这一块悄悄消失，说一声
  if (!status) {
    if (!loadError) return null;
    return (
      <div className="rounded-lg border border-line px-4 py-3">
        <span className="text-[13.5px] tracking-[0.08em] text-muted">更新</span>
        <p className="mt-2 text-[14.5px] text-paper">没能读到更新状态</p>
        <p className="mt-1 text-[13px] leading-relaxed text-[#f6b09b]">{loadError}</p>
      </div>
    );
  }

  const view = describeStatus(status);
  const phase = status.phase.state;
  const ready = phase === "ready";
  const on = status.auto_update;

  return (
    <div className="rounded-lg border border-line px-4 py-3">
      <div className="flex items-center justify-between gap-3">
        <span className="text-[13.5px] tracking-[0.08em] text-muted">更新</span>
        <span className="font-mono text-[14px] text-muted">当前 v{status.current_version}</span>
      </div>

      <p className="mt-2 text-[14.5px] text-paper">{view.headline}</p>
      {view.detail && (
        <p className="mt-1 whitespace-pre-line text-[13px] leading-relaxed text-subtle">{view.detail}</p>
      )}
      {visibleErrors(view.error, actionError?.message ?? null, loadError).map((line) => (
        <p key={line} className="mt-1 text-[13px] leading-relaxed text-[#f6b09b]">
          {line}
        </p>
      ))}

      {!status.unavailable && (
        <div className="mt-2.5 flex flex-wrap items-center gap-2">
          <button
            type="button"
            disabled={busy || !canCheck(status)}
            onClick={update.check}
            className="rounded-md border border-line-strong px-4 py-2 text-[14px] text-paper disabled:opacity-60"
          >
            {phase === "checking" ? "检查中…" : "检查更新"}
          </button>
          {ready && (
            <button
              type="button"
              disabled={busy}
              onClick={update.restart}
              className="rounded-md bg-acc-strong px-4 py-2 text-[14px] font-semibold text-[#0a0a0b] disabled:opacity-60"
            >
              立即重启并更新
            </button>
          )}
          {/* 名字固定、状态交给 aria-checked：读屏念「自动检查并下载新版本，开关，开」；里面的「开 / 关」小药丸只是给眼睛看的 */}
          <button
            type="button"
            role="switch"
            aria-checked={on}
            aria-label="自动检查并下载新版本"
            disabled={busy}
            onClick={() => update.setAutoUpdate(!on)}
            className="flex items-center gap-2 rounded-md px-3 py-2 text-[13.5px] text-muted hover:text-paper disabled:opacity-60"
          >
            <span aria-hidden="true">自动检查并下载新版本</span>
            <span
              aria-hidden="true"
              className={cn(
                "rounded-full px-2 py-0.5 text-[12px] font-semibold leading-none",
                on ? "bg-acc-strong text-[#0a0a0b]" : "bg-[rgba(248,246,241,0.1)] text-subtle",
              )}
            >
              {on ? "开" : "关"}
            </span>
          </button>
        </div>
      )}

      {showLastUpdate(status) && status.last_update && (
        <p className="mt-2 font-mono text-[13px] text-subtle">
          最近一次更新：v{status.last_update.from} → v{status.last_update.to} · {formatTime(status.last_update.at)}
        </p>
      )}
      {!status.unavailable && (
        <p className="mt-2 text-[13px] leading-relaxed text-subtle">
          新版本从 GitHub 下载，装之前先校验签名，不通过就丢弃。下载好后不会自己重启，等你点「立即重启」。
        </p>
      )}
    </div>
  );
}
