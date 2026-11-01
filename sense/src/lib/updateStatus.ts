import { z } from "zod";

/**
 * Rust `updater.rs` 里 `UpdateStatus` 的 zod 镜像：命令的返回值、`sense://update-status` 事件的载荷都按它解析。
 *
 * 契约样本是 `updateStatus.fixture.json`——Rust 侧的测试（`status_json_matches_the_frontend_fixture`）
 * 断言自己序列化出来的 JSON 与它一致，这里的测试断言它能被解析。改字段两边一起改，哪边漏了哪边红。
 */

export const phaseSchema = z.discriminatedUnion("state", [
  /** 还没检查过 */
  z.object({ state: z.literal("idle") }),
  z.object({ state: z.literal("checking") }),
  z.object({ state: z.literal("up_to_date") }),
  /** total 为 null = 服务器没给长度，只能显示已下载的字节数 */
  z.object({
    state: z.literal("downloading"),
    version: z.string(),
    downloaded: z.number(),
    total: z.number().nullable(),
  }),
  /** 已下载、验过签，等用户点「立即重启」 */
  z.object({ state: z.literal("ready"), version: z.string() }),
  z.object({ state: z.literal("installing"), version: z.string() }),
]);

export const updateStatusSchema = z.object({
  /** 每次变化 +1：先取状态、后收事件会乱序，靠它丢掉晚到的旧状态 */
  seq: z.number(),
  current_version: z.string(),
  /** 有值 = 这个进程不能自动更新（开发版、不在 .app 里、被系统临时挪了位置……），值就是原因 */
  unavailable: z.string().nullable(),
  auto_update: z.boolean(),
  phase: phaseSchema,
  /** 最近一次检查的时间，毫秒时间戳，不论成败 */
  checked_at: z.number().nullable(),
  /** 新版本的更新说明，只在下载中 / 已就绪时有 */
  notes: z.string().nullable(),
  /** 最近一次检查 / 下载 / 安装失败的原因；成功一次就清掉 */
  last_error: z.string().nullable(),
  /**
   * 上面那个失败属于哪一类：`check` = 检查 / 下载 / 验签没成（到点会自动重试）；
   * `install` = 安装 / 重启没成（不会自动重试，错误里写着该怎么办）。有 `last_error` 就有它。
   */
  error_kind: z.enum(["check", "install"]).nullable(),
  last_update: z.object({ from: z.string(), to: z.string(), at: z.number() }).nullable(),
});

export type UpdatePhase = z.infer<typeof phaseSchema>;
export type UpdateStatus = z.infer<typeof updateStatusSchema>;

/** 认不出来返回 null：事件载荷格式不对时丢掉，不让一条坏消息把界面带崩 */
export function parseUpdateStatus(raw: unknown): UpdateStatus | null {
  const parsed = updateStatusSchema.safeParse(raw);
  return parsed.success ? parsed.data : null;
}
