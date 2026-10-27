import { create } from "zustand";
import { api, onDelta, type LookupError } from "./ipc";
import { detectLang, parseWordEntry, type WordEntry } from "./schema";
import { parseFinal, tryParsePartial, type PartialEntry } from "./stream";

export type LookupStatus = "idle" | "loading" | "streaming" | "done" | "error";

export type LookupMeta = {
  source: "cache" | "network";
  model: string;
  /** 第一个 token 到达（首字节） */
  firstDeltaMs: number;
  /** 第一个完整义项可以渲染，这才是用户眼里的「出结果」；没测到就是 null */
  firstSenseMs: number | null;
  totalMs: number;
};

type LookupState = {
  status: LookupStatus;
  query: string;
  buffer: string;
  /** 流式过程中已完整的义项（每个都过了 zod，仅用于先画出来） */
  partial: PartialEntry | null;
  entry: WordEntry | null;
  warnings: string[];
  error: LookupError | null;
  meta: LookupMeta | null;
  /** 长文模式：原文、流式译文、是否还在生成 */
  longSource: string | null;
  longText: string;
  longBusy: boolean;
  longTruncated: boolean;
  /** 译文没有正常写完的原因（被停止、内容策略、没收到结束标记……）；完整时为 null */
  longIncomplete: string | null;
  run: (word: string) => Promise<void>;
  runLong: (text: string) => Promise<void>;
  cancelLong: () => Promise<void>;
  reset: () => void;
};

const empty = {
  status: "idle" as LookupStatus,
  query: "",
  buffer: "",
  partial: null,
  entry: null,
  warnings: [] as string[],
  error: null,
  meta: null,
  longSource: null,
  longText: "",
  longBusy: false,
  longTruncated: false,
  longIncomplete: null,
};

/**
 * 查询序号：每次 run 自增。
 * 所有 set 之前都要确认「我还是最新的那一次」，否则旧查询会覆盖新查询的结果。
 */
let epoch = 0;

function newRequestId(): string {
  return `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

/**
 * 离开阅读视图、开始新查询时，把还在生成的长文停掉。
 * 前端丢弃增量不等于后端停了：不停的话，那条流会一直生成、一直计费。
 */
function stopLongIfBusy(busy: boolean): Promise<void> {
  return busy ? api.cancelTranslate().catch(() => {}) : Promise.resolve();
}

export const useLookup = create<LookupState>((set, get) => ({
  ...empty,

  runLong: async (raw: string) => {
    const text = raw.trim();
    if (!text) return;

    // 先停掉上一条，再开新的：取消按请求编号算，等它生效后再领新编号
    await stopLongIfBusy(get().longBusy);
    const my = ++epoch;
    const requestId = `long-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`;
    set({
      ...empty,
      status: "loading",
      query: text.slice(0, 24),
      longSource: text,
      longBusy: true,
    });

    // 增量按 60ms 合流：8192 token 的输出如果每片都 setState，右栏会卡成幻灯片
    let buffer = "";
    let pending: ReturnType<typeof setTimeout> | null = null;
    const flush = () => {
      pending = null;
      // 旧请求的定时器可能在新请求开始后才触发，不能把旧片段写进新状态
      if (my !== epoch || !buffer) return;
      const chunk = buffer;
      buffer = "";
      set((state) => ({ longText: state.longText + chunk, status: "streaming" }));
    };

    const { listen } = await import("@tauri-apps/api/event");
    const unlisten = await listen<{ request_id: string; text: string }>(
      "sense://longtext-delta",
      (event) => {
        if (my !== epoch || event.payload.request_id !== requestId) return;
        buffer += event.payload.text;
        if (pending === null) pending = setTimeout(flush, 60);
      },
    );

    try {
      // 整篇一次送：切块会丢上下文，译文会和原文意思有偏
      const result = await api.translateText(text, requestId);
      if (my !== epoch) return;
      flush();
      set({
        status: "done",
        longText: result.text,
        longTruncated: result.truncated,
        longIncomplete: result.incomplete,
        longBusy: false,
      });
    } catch (err) {
      if (my !== epoch) return;
      // 断流前已收到、还在合流缓冲里的部分也要上屏；界面会把它标成「不完整」
      flush();
      set({
        status: "error",
        longBusy: false,
        error: { code: "longtext", message: String(err).replace(/^Error:\s*/, "") },
      });
    } finally {
      if (pending !== null) clearTimeout(pending);
      unlisten();
    }
  },

  cancelLong: async () => {
    // 已经结束了（「停止」按钮还没来得及消失就点中）：没什么可停的。
    // 照常往下走会把一篇完整的译文标成「不完整」，还跳过语法检查
    if (!get().longBusy) return;
    // 先把「不完整」记上再通知后端：translate_text 要等后端看到取消位才返回，
    // 这段空档里 longBusy 已经是 false。「不完整」没记上的话，界面会以为译文是完整的，
    // 语法检查就会对着被放弃的译文白白发一次请求。后端返回的结果会覆盖它（同样是「你点了停止」）
    set({ longBusy: false, longIncomplete: "你点了停止" });
    await api.cancelTranslate().catch(() => {});
  },

  reset: () => {
    void stopLongIfBusy(get().longBusy);
    epoch += 1;
    set({ ...empty });
  },

  run: async (rawWord: string) => {
    const word = rawWord.trim();
    if (!word) return;

    void stopLongIfBusy(get().longBusy);
    const my = ++epoch;
    // 重试会换新的 requestId：第一轮迟到的增量带的是旧 id，混不进第二轮的预览
    let requestId = newRequestId();
    set({ ...empty, status: "loading", query: word });

    let buffer = "";
    let attemptStart = performance.now();
    let firstSenseMs: number | null = null;
    let pending: ReturnType<typeof setTimeout> | null = null;
    let dirty = false;

    // 增量合流：每个 token 都解析一次整包 JSON 再重渲染，主线程会被拖住，
    // 看起来比实际更慢。按 66ms（≈4 帧）合并一次足够跟手。
    const flush = () => {
      pending = null;
      // 旧查询的定时器可能在 epoch++ 之后才触发，这里必须再挡一次
      if (my !== epoch || !dirty) return;
      dirty = false;
      const partial = tryParsePartial(buffer);
      if (!firstSenseMs && partial) {
        firstSenseMs = Math.round(performance.now() - attemptStart);
      }
      set({
        buffer,
        partial,
        status: partial ? "streaming" : "loading",
      });
    };

    const unlisten = await onDelta((event) => {
      if (event.request_id !== requestId || my !== epoch) return;
      buffer += event.text;
      dirty = true;
      if (pending === null) pending = setTimeout(flush, 66);
    });

    try {
      const first = await api.lookup({ word, requestId, temperature: 0.3 });
      if (my !== epoch) return;

      let parsed = validate(first.raw, first);

      if (!parsed.ok) {
        // 校验失败：先停掉上一轮的合流（否则它会把 attempt-1 的半截写回去），
        // 再把这条缓存标废，用 0.2 重试一次并跳过缓存。
        if (pending !== null) {
          clearTimeout(pending);
          pending = null;
        }
        dirty = false;

        await api.invalidateLookup(word, detectLang(word)).catch(() => {});
        if (my !== epoch) return;

        buffer = "";
        firstSenseMs = null;
        attemptStart = performance.now();
        requestId = newRequestId();
        set({ status: "loading", buffer, partial: null });

        const second = await api.lookup({
          word,
          requestId,
          temperature: 0.2,
          bypassCache: true,
        });
        if (my !== epoch) return;

        parsed = validate(second.raw, second);
        if (!parsed.ok) {
          // 重试写进库的也是坏数据（Rust 只做结构门）：不标废的话，
          // 以后每查一次这个词都是「命中坏缓存 + 一次必然失败的请求」
          await api.invalidateLookup(word, detectLang(word)).catch(() => {});
          if (my !== epoch) return;
          set({
            status: "error",
            error: { code: "invalid_json", message: describeFailure(second, parsed.reason) },
          });
          return;
        }
      }

      set({
        status: "done",
        entry: parsed.entry,
        warnings: parsed.warnings,
        error: null,
        buffer: "",
        partial: null,
        meta: { ...parsed.meta, firstSenseMs },
      });
    } catch (err) {
      if (my !== epoch) return;
      set({ status: "error", error: toLookupError(err) });
    } finally {
      if (pending !== null) clearTimeout(pending);
      pending = null;
      unlisten();
    }
  },
}));

type Validated =
  | { ok: true; entry: WordEntry; warnings: string[]; meta: LookupMeta }
  /** reason 是具体哪一处不合格，会原样进错误提示 */
  | { ok: false; reason: string };

function validate(
  raw: string,
  result: { source: string; model: string; first_delta_ms: number; total_ms: number },
): Validated {
  const final = parseFinal(raw);
  if (!final) return { ok: false, reason: "不是合法的 JSON" };

  const parsed = parseWordEntry(final.value);
  if (!parsed.ok) return { ok: false, reason: parsed.error };

  const warnings = [...parsed.warnings];
  if (final.repaired) {
    warnings.push("模型输出被截断，卡片按已收到的内容修复，义项可能不全");
  }
  if (parsed.degraded) warnings.push("部分义项格式不合法，已略过");

  return {
    ok: true,
    entry: parsed.entry,
    warnings,
    meta: {
      source: result.source === "cache" ? "cache" : "network",
      model: result.model,
      firstDeltaMs: result.first_delta_ms,
      firstSenseMs: null,
      totalMs: result.total_ms,
    },
  };
}

/**
 * 重试后仍然校验失败时给用户看的说明。
 *
 * finish_reason 是模型服务自己报的结束原因：`stop` 表示它正常写完了，
 * 问题出在内容不符合词典格式，绝不能说成「流被中断」——
 * interj 那次就是被这句话误导，把契约问题当成了网络问题去查。
 * 只有拿不到结束原因时，才说「可能在中途被截断」。
 */
export function describeFailure(
  result: { raw: string; finish_reason?: string | null },
  reason: string,
): string {
  if (result.finish_reason === "content_filter") {
    return "模型服务在输出途中以「内容策略」中止了这次请求，与应用无关——应用不做任何词语过滤。换一个 provider 或模型再试。";
  }
  if (result.finish_reason === "length") {
    return "模型输出撞到长度上限被截断（这次不是格式问题）。可以重试，或换个输出上限更大的模型。";
  }

  // 把模型的原文露出来：多数情况是它没按词典格式回答（比如自己拒答了），
  // 这时候藏着只会让人以为是应用屏蔽了词。
  const flat = result.raw.trim().replace(/\s+/g, " ");
  if (!flat) return "模型返回了空内容，已自动重试一次。";
  const snippet = flat.length > 180 ? `${flat.slice(0, 180)}…` : flat;

  if (result.finish_reason === "stop") {
    return `模型输出不符合词典格式（${reason}）。已收到的内容：「${snippet}」`;
  }
  if (!result.finish_reason) {
    return `没有收到结束标记，输出可能在中途被截断（${reason}）。已收到的内容：「${snippet}」`;
  }
  // 其它结束原因（如 insufficient_system_resource）：是服务端提前收工，不是格式问题
  return `模型服务以「${result.finish_reason}」结束了输出，内容不完整（${reason}）。已收到的内容：「${snippet}」`;
}

function toLookupError(err: unknown): LookupError {
  if (err && typeof err === "object" && "code" in err && "message" in err) {
    const value = err as LookupError;
    return { code: String(value.code), message: String(value.message) };
  }
  if (typeof err === "string") return { code: "unknown", message: err };
  return { code: "unknown", message: "查询失败了，稍后再试。" };
}
