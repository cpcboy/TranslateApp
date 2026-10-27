import type { LookupError } from "../../lib/ipc";

const TITLES: Record<string, string> = {
  no_key: "还没接入模型",
  auth: "Key 无效或已过期",
  rate_limit: "请求太频繁",
  timeout: "模型没回",
  network: "连不上",
  interrupted: "传输中断",
  server: "模型服务出错",
  empty: "模型返回空内容",
  invalid_json: "返回格式不对",
  keychain: "钥匙串读写失败",
  too_long: "输入太长",
  no_provider: "没有可用的模型配置",
};

export function ErrorState({
  error,
  onRetry,
  onOpenSettings,
}: {
  error: LookupError;
  onRetry: () => void;
  onOpenSettings: () => void;
}) {
  const needsSettings = error.code === "no_key" || error.code === "auth" || error.code === "no_provider";

  return (
    <div className="rounded-2xl border border-[rgba(255,82,48,0.32)] bg-[rgba(255,82,48,0.07)] px-6 py-7">
      <div className="flex items-baseline gap-2.5">
        <span className="font-mono text-[14px] uppercase tracking-[0.12em] text-acc">
          {error.code}
        </span>
        <h2 className="text-[19px] font-semibold text-paper">
          {TITLES[error.code] ?? "查询失败"}
        </h2>
      </div>
      <p className="mt-3 text-[16px] leading-relaxed text-[#d5d0c7]">{error.message}</p>

      <div className="mt-4 flex items-center gap-2">
        {needsSettings ? (
          <button
            type="button"
            onClick={onOpenSettings}
            className="rounded-md bg-acc-strong px-4 py-2.5 text-[14.5px] font-semibold text-[#0a0a0b] hover:opacity-90"
          >
            去设置
          </button>
        ) : (
          <button
            type="button"
            onClick={onRetry}
            className="rounded-md border border-line-strong px-4 py-2.5 text-[14.5px] text-paper hover:bg-[rgba(248,246,241,0.06)]"
          >
            重试
          </button>
        )}
      </div>
    </div>
  );
}
