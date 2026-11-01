import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";
import { bannerKey, bannerModel, isBannerDismissed, visibleErrors, type BannerModel } from "./describe";
import { isTauri, type UpdateController } from "./useUpdate";

/**
 * 窗口顶部的更新横幅，画什么完全由更新状态决定（见 `bannerModel`）：
 * 新版本已下载好等你重启 / 正在安装 / 安装或重启没成功需要你处理。
 *
 * 为什么要点一下才重启：窗口失焦就会藏起来，而长文翻译在窗口藏着的时候还在跑；
 * 后台自己重启会丢掉它，也会在用户打字的时候把新窗口弹到最前面抢走焦点。
 * 「稍后」只管这一次、这一条——下次窗口被呼出来（拿到焦点）横幅会再出现，出了更新的版本、出了新的错误也会再出现。
 * 「拿到焦点」用 Tauri 的原生窗口事件（`onFocusChanged`），和 LookupView 呼出后重新聚焦输入框是同一个、已经验证过的机制；
 * 不用 DOM 的 `focus` 事件——窗口被 Rust `hide()` / `show()` 时 WKWebView 里它触不触发没验证过。
 *
 * 「更新需要你处理」那一档（安装 / 重启没成而且包不能再试）的「知道了」是确认：锁存的失败在这里放掉，
 * 之后的后台检查把它从状态里清掉，它就不会再出现。（「更新没有完成」说不准：有时新版本其实已经装好了，只是没能自己重新打开。）
 *
 * 外面这层 `role="status"` 一直在：读屏软件只会播报「已经存在的 live region 里」后来出现的内容，
 * 横幅整个条件渲染的话，出现的那一下什么都不会播报。
 */
export function UpdateBanner({ update }: { update: UpdateController }) {
  const { status, busy, actionError, alert, clearActionError, dismissAlert } = update;
  // 点过「稍后」的那一条（版本 + 当时的错误）；null = 没点过
  const [dismissedKey, setDismissedKey] = useState<string | null>(null);

  useEffect(() => {
    if (!isTauri) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void getCurrentWindow()
      .onFocusChanged(({ payload }) => {
        if (payload) setDismissedKey(null);
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* 拿不到窗口事件，横幅照常显示；只是「稍后」之后，这一条在本次运行里不再提示（出了更新的版本、新的错误会再提示） */
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const model = bannerModel(status, alert);
  const visible = model !== null && !isBannerDismissed(dismissedKey, model);
  // 点「立即重启」这个动作本身失败了（状态里没有对应的错误，比如窗口不对、正在下载更新的版本）：补在横幅里
  const restartError = actionError?.source === "restart" ? actionError.message : null;

  return (
    <div role="status" aria-live="polite" className="shrink-0">
      {visible && (
        <BannerView
          model={model}
          busy={busy}
          extraError={restartError}
          onRestart={update.restart}
          onLater={() => {
            setDismissedKey(bannerKey(model));
            if (restartError !== null) clearActionError();
            // 「知道了」是确认：锁存的失败放掉（状态里的同一条错误还在的话，横幅仍按「稍后」的规矩，到下次拿到焦点再出现）
            if (model.kind === "failed") dismissAlert();
          }}
        />
      )}
    </div>
  );
}

/** 已下载好的更新，用户该怎么做、不做会怎样（包只在内存里：自己退出再打开不会装上去，只会重新下载） */
export const READY_TEXT = "已下载好。点「立即重启」完成更新（约 2 秒，会中断正在进行的翻译）；自己退出再打开不会更新";

/** 横幅本身：不带状态，方便单测里直接看它画了什么、按钮接到了哪 */
export function BannerView({
  model,
  busy,
  extraError,
  onRestart,
  onLater,
}: {
  model: BannerModel;
  /** 有命令在途（重启、复查、开关……） */
  busy: boolean;
  /** 命令返回的失败原因；和状态里的错误是同一句时只显示一遍 */
  extraError: string | null;
  onRestart: () => void;
  onLater: () => void;
}) {
  const { kind, version } = model;
  const errors = visibleErrors(model.error, extraError);
  const installing = kind === "installing";

  return (
    <div className="mx-4 mt-3 rounded-lg border border-line-strong bg-[rgba(248,246,241,0.06)] px-4 py-3 text-[14px] leading-relaxed text-paper">
      <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
        <span>
          {kind === "ready" && (
            <>
              新版本 <span className="font-mono">v{version}</span> {READY_TEXT}
            </>
          )}
          {installing && (
            <>
              正在安装 <span className="font-mono">v{version}</span>，装好后会自动重新打开 SENSE…
            </>
          )}
          {kind === "failed" && "更新需要你处理"}
        </span>
        <span className="flex shrink-0 items-center gap-2">
          {kind !== "failed" && (
            <button
              type="button"
              disabled={busy || installing}
              onClick={onRestart}
              className="rounded-md bg-acc-strong px-3.5 py-1.5 text-[13.5px] font-semibold text-[#0a0a0b] disabled:opacity-60"
            >
              {installing ? "正在安装…" : busy ? "请稍候…" : "立即重启"}
            </button>
          )}
          {!installing && (
            <button
              type="button"
              disabled={busy}
              onClick={onLater}
              className="rounded-md px-2.5 py-1.5 text-[13.5px] text-muted hover:text-paper disabled:opacity-60"
            >
              {kind === "failed" ? "知道了" : "稍后"}
            </button>
          )}
        </span>
      </div>
      {errors.map((line) => (
        <p key={line} className="mt-2 text-[13px] leading-relaxed text-[#f6b09b]">
          {line}
        </p>
      ))}
    </div>
  );
}
