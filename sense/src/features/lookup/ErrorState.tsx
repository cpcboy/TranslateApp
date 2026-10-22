import type { LookupError } from "../../lib/ipc";

const TITLES: Record<string, string> = {
  no_key: "还没接入模型",
  auth: "Key 无效或已过期",
  rate_limit: "请求太频繁",
  timeout: "模型没回",
  network: "连不上",
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
    <div className="rounded-2xl border border-[rgba(255,82,48,0.28)] bg-[rgba(255,82,48,0.06)] px-4 py-5">
      <div className="flex items-baseline gap-2">
        <span className="font-mono text-[10px] uppercase tracking-[0.14em] text-acc">
          {error.code}
        </span>
        <h2 className="text-[14px] font-semibold text-paper">
          {TITLES[error.code] ?? "查询失败"}
        </h2>
      </div>
      <p className="mt-2 text-[12px] leading-relaxed text-[#beb9b1]">{error.message}</p>

      <div className="mt-3 flex items-center gap-2">
        {needsSettings ? (
          <button
            type="button"
            onClick={onOpenSettings}
            className="rounded-md bg-acc px-3 py-1.5 text-[11.5px] font-medium text-[#0a0a0b] hover:opacity-90"
          >
            去设置
          </button>
        ) : (
          <button
            type="button"
            onClick={onRetry}
            className="rounded-md border border-line-strong px-3 py-1.5 text-[11.5px] text-paper hover:bg-[rgba(242,239,233,0.05)]"
          >
            重试
          </button>
        )}
      </div>
    </div>
  );
}
