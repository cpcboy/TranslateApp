import { create } from "zustand";
import { api, onDelta, type LookupError } from "./ipc";
import { detectLang, parseWordEntry, type WordEntry } from "./schema";
import { parseFinal, tryParsePartial, type PartialEntry } from "./stream";

export type LookupStatus = "idle" | "loading" | "streaming" | "done" | "error";

export type LookupMeta = {
  source: "cache" | "network";
  model: string;
  firstDeltaMs: number;
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
  run: (word: string) => Promise<void>;
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
};

/**
 * 查询序号：每次 run 自增。
 * 所有 set 之前都要确认「我还是最新的那一次」，否则旧查询会覆盖新查询的结果。
 */
let epoch = 0;

export const useLookup = create<LookupState>((set) => ({
  ...empty,

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
    const unlisten = await onDelta((event) => {
      if (event.request_id !== requestId || my !== epoch) return;
      buffer += event.text;
      const partial = tryParsePartial(buffer);
      set({
        buffer,
        partial,
        status: partial ? "streaming" : "loading",
      });
    });

    try {
      const first = await api.lookup({ word, requestId, temperature: 0.3 });
      if (my !== epoch) return;

      let parsed = validate(first.raw, first);

      if (!parsed.ok) {
        // 校验失败：先把这条缓存标废，再用 0.2 重试一次并跳过缓存。
        await api.invalidateLookup(word, detectLang(word)).catch(() => {});
        if (my !== epoch) return;

        buffer = "";
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
        meta: parsed.meta,
      });
    } catch (err) {
      if (my !== epoch) return;
      set({ status: "error", error: toLookupError(err) });
    } finally {
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
