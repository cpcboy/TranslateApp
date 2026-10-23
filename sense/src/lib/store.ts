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
  run: (word: string) => Promise<void>;
  runLong: (text: string) => Promise<void>;
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
};

/**
 * 查询序号：每次 run 自增。
 * 所有 set 之前都要确认「我还是最新的那一次」，否则旧查询会覆盖新查询的结果。
 */
let epoch = 0;

export const useLookup = create<LookupState>((set) => ({
  ...empty,

  runLong: async (raw: string) => {
    const text = raw.trim();
    if (!text) return;

    const my = ++epoch;
    const requestId = `long-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`;
    set({
      ...empty,
      status: "loading",
      query: text.slice(0, 24),
      longSource: text,
      longBusy: true,
    });

    const { listen } = await import("@tauri-apps/api/event");
    const unlisten = await listen<{ request_id: string; text: string }>(
      "sense://longtext-delta",
      (event) => {
        if (my !== epoch || event.payload.request_id !== requestId) return;
        set((state) => ({ longText: state.longText + event.payload.text, status: "streaming" }));
      },
    );

    try {
      // 整篇一次送：切块会丢上下文，译文会和原文意思有偏
      const result = await api.translateText(text, requestId);
      if (my !== epoch) return;
      set({ status: "done", longText: result, longBusy: false });
    } catch (err) {
      if (my !== epoch) return;
      set({
        status: "error",
        longBusy: false,
        error: { code: "longtext", message: String(err).replace(/^Error:\s*/, "") },
      });
    } finally {
      unlisten();
    }
  },

  reset: () => {
    epoch += 1;
    set({ ...empty });
  },

  run: async (rawWord: string) => {
    const word = rawWord.trim();
    if (!word) return;

    const my = ++epoch;
    const requestId = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
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
          set({
            status: "error",
            error: {
              code: "invalid_json",
              message: "这次返回格式不对，已自动重试一次，仍然不合格。",
            },
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
  | { ok: false };

function validate(
  raw: string,
  result: { source: string; model: string; first_delta_ms: number; total_ms: number },
): Validated {
  const final = parseFinal(raw);
  if (!final) return { ok: false };

  const parsed = parseWordEntry(final.value);
  if (!parsed.ok) return { ok: false };

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

function toLookupError(err: unknown): LookupError {
  if (err && typeof err === "object" && "code" in err && "message" in err) {
    const value = err as LookupError;
    return { code: String(value.code), message: String(value.message) };
  }
  if (typeof err === "string") return { code: "unknown", message: err };
  return { code: "unknown", message: "查询失败了，稍后再试。" };
}
