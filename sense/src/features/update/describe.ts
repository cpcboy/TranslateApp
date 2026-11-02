import type { UpdateStatus } from "../../lib/updateStatus";

/** 界面上这一块的文字与语气。语气决定颜色：error 红、ok 绿、busy 与 muted 用普通灰。 */
export type UpdateView = {
  headline: string;
  /** 补充说明（进度、上次检查时间、更新内容……） */
  detail: string | null;
  /** 失败原因；有就用红字单独一行 */
  error: string | null;
  tone: "ok" | "busy" | "error" | "muted";
};

/** 字节数 → 人看的：`512 B` / `3 KB` / `3.2 MB`。先四舍五入再换档，所以 1048575 B 是 `1.0 MB` 而不是 `1024 KB`。 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  const kilobytes = Math.round(bytes / 1024);
  if (kilobytes < 1024) return `${kilobytes} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** 下载百分比，0–100 的整数；不知道总长度（或总长度为 0）返回 null。 */
export function progressPercent(downloaded: number, total: number | null): number | null {
  if (total === null || !(total > 0)) return null;
  return Math.max(0, Math.min(100, Math.floor((downloaded / total) * 100)));
}

const two = (value: number) => String(value).padStart(2, "0");

/** 毫秒时间戳 → 本地时间 `11-01 14:30`。只显示月日时分：更新检查不需要年份，也不需要秒。 */
export function formatTime(ms: number): string {
  const date = new Date(ms);
  if (Number.isNaN(date.getTime())) return "—";
  return `${two(date.getMonth() + 1)}-${two(date.getDate())} ${two(date.getHours())}:${two(date.getMinutes())}`;
}

/** 能不能点「检查更新」：这台能更新，并且此刻没有在检查 / 下载 / 安装。 */
export function canCheck(status: UpdateStatus): boolean {
  if (status.unavailable) return false;
  const state = status.phase.state;
  return state !== "checking" && state !== "downloading" && state !== "installing";
}

/** 「最近一次更新」只在它确实是现在这一版时才显示：手动降级装了旧版之后，写着「→ 0.1.0」而当前是 0.0.9 就是误导。 */
export function showLastUpdate(status: UpdateStatus): boolean {
  return status.last_update !== null && status.last_update.to === status.current_version;
}

/**
 * 窗口顶部横幅要画什么，完全由状态决定（不依赖命令的返回值：事件和命令返回谁先到没有保证）：
 * - `ready`：新版本已下载好，等用户点「立即重启」；上一次安装失败了（取消授权……）但包还在，`error` 里是原因
 * - `installing`：正在装，按钮置灰（弹管理员授权的那几秒窗口里不能什么提示都没有）
 * - `failed`：安装 / 重启没成、而且包已经不能再试（旧版本没放回原位、新版本装好了却没能拉起来……），需要人处理：
 *   这些话必须在主界面看见，只放在设置页等于没说
 *
 * `alert` 是「锁存的失败」：安装从「正在安装」直接落到「失败、而且没有包可重试」的那一刻记下来（见 `useUpdate`），
 * 用户点「知道了」才放掉。没有它的话，下一次后台检查（最迟 6 小时、最快 1 分钟之后）成功或失败，都会把状态里的
 * `last_error` 顺手清掉 / 覆盖，横幅跟着无声消失——而窗口在弹管理员授权时会失焦被藏起来，用户回来时消息可能已经没了。
 *
 * 状态里带着安装类错误、但此刻正在下载别的版本（`downloading`）时不画「失败」：那多半是可重试的失败（取消了授权，旧包还在内存里）
 * 又赶上后台复查发现了更新的版本，横幅说「需要你处理」是错的；真正需要处理的失败由 `alert` 兜住。
 */
export type BannerModel = {
  kind: "ready" | "installing" | "failed";
  version: string;
  error: string | null;
};

export function bannerModel(status: UpdateStatus | null, alert: string | null = null): BannerModel | null {
  if (status === null || status.unavailable) return null;
  const phase = status.phase;
  const installError = status.error_kind === "install" ? status.last_error : null;
  if (phase.state === "installing") return { kind: "installing", version: phase.version, error: null };
  if (phase.state === "ready") return { kind: "ready", version: phase.version, error: installError ?? alert };
  if (installError && phase.state !== "downloading") return { kind: "failed", version: "", error: installError };
  if (alert) return { kind: "failed", version: "", error: alert };
  return null;
}

/** 要不要在窗口顶部放横幅。 */
export function showBanner(status: UpdateStatus | null, alert: string | null = null): boolean {
  return bannerModel(status, alert) !== null;
}

/**
 * 这条新状态要不要把「失败」锁存起来：必须是从「正在安装」直接落下来（`wasInstalling`），带着安装类错误，
 * 而且不是回到「已就绪」（那是可重试的失败：包还在，横幅里照样有「立即重启」）。
 * 只看这一个转换，所以后来的状态事件（下载别的版本、复查）和重新取到的初始状态都不会误锁。
 */
export function alertToLatch(wasInstalling: boolean, next: UpdateStatus): string | null {
  if (!wasInstalling) return null;
  if (next.error_kind !== "install" || !next.last_error) return null;
  if (next.phase.state === "ready" || next.phase.state === "installing") return null;
  return next.last_error;
}

/** 「稍后」收起的是哪一条：版本 + 当时的错误。换了版本、或者出了新的错误，都是另一条，横幅会再出现。 */
export function bannerKey(model: BannerModel): string {
  return `${model.version}|${model.error ?? ""}`;
}

/** 点过「稍后」的横幅要不要先收起来。正在安装的不收（是用户刚点的，要看到进度）。 */
export function isBannerDismissed(dismissedKey: string | null, model: BannerModel): boolean {
  return model.kind !== "installing" && dismissedKey !== null && dismissedKey === bannerKey(model);
}

/** 这条新状态要不要采纳：序号比已采纳的最大序号还旧的，是晚到的旧消息（命令返回值和事件会乱序），丢掉。 */
export function shouldAccept(newestSeq: number, nextSeq: number): boolean {
  return nextSeq >= newestSeq;
}

/**
 * 命令失败的原因什么时候该撤掉：失败之后又来了更新的状态（下一次检查、开关……），那次失败已经是过去的事。
 * 失败那一刻 Rust 发出的状态变化序号不比记下的大，不会把刚出的错误立刻撤掉。
 */
export function actionErrorExpired(errorSeq: number, statusSeq: number): boolean {
  return statusSeq > errorSeq;
}

/** 红字错误要显示哪几行：同一句话不显示两遍（Rust 写进状态的 `last_error` 和命令返回的错误常常是同一句）。 */
export function visibleErrors(...errors: Array<string | null>): string[] {
  const lines: string[] = [];
  for (const error of errors) {
    if (error && !lines.includes(error)) lines.push(error);
  }
  return lines;
}

/** 已下载好的更新，用户该怎么做、不做会怎样：包只在内存里，自己退出再打开不会装上去，只会重新下载一遍。 */
const READY_HINT = "点「立即重启」完成更新；不点就继续用现在这版（自己退出再打开不会更新，会重新下载）";

export function describeStatus(status: UpdateStatus): UpdateView {
  const checked = status.checked_at !== null ? `上次检查 ${formatTime(status.checked_at)}` : null;
  const error = status.last_error;

  if (status.unavailable) {
    return { headline: "这里不能自动更新", detail: status.unavailable, error: null, tone: "muted" };
  }

  const phase = status.phase;
  switch (phase.state) {
    case "idle": {
      if (error) {
        // 「之后会自动重试」只对检查 / 下载类失败、并且自动检查开着时才是真的：
        // 安装类失败不会自动重试（错误本身写着该怎么办），自动检查关着时更不会
        const retry =
          status.error_kind === "install"
            ? null
            : status.auto_update
              ? "之后会自动重试"
              : "自动检查已关闭，需要时再点「检查更新」";
        return {
          headline: "更新没成功",
          detail: [checked, retry].filter(Boolean).join("，") || null,
          error,
          tone: "error",
        };
      }
      return {
        headline: "还没检查过",
        detail: status.auto_update ? "启动半分钟后会自动检查，之后每 6 小时一次" : "自动检查已关闭，需要时点「检查更新」",
        error: null,
        tone: "muted",
      };
    }
    // 忙的时候不显示上一次的失败：重试时红字还挂着上次的原因，像是「这次又失败了」
    case "checking":
      return { headline: "正在检查更新…", detail: null, error: null, tone: "busy" };
    case "up_to_date":
      return { headline: "已是最新版本", detail: checked, error, tone: "ok" };
    case "downloading": {
      const percent = progressPercent(phase.downloaded, phase.total);
      const size =
        percent !== null
          ? `${percent}% · ${formatBytes(phase.downloaded)} / ${formatBytes(phase.total ?? 0)}`
          : `已下载 ${formatBytes(phase.downloaded)}`;
      return { headline: `正在下载 v${phase.version}`, detail: size, error: null, tone: "busy" };
    }
    case "ready":
      return {
        headline: `v${phase.version} 已下载并通过签名校验`,
        detail: status.notes ? `${status.notes}\n点「立即重启」完成更新；自己退出再打开不会更新` : READY_HINT,
        // 已经有可装的版本时，复查 / 安装失败仍要让用户看见
        error,
        tone: "ok",
      };
    case "installing":
      return { headline: `正在安装 v${phase.version}…`, detail: "装好后会自动重新打开 SENSE", error: null, tone: "busy" };
  }
}
