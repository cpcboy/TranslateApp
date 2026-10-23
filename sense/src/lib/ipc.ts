import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ProviderView = {
  id: string;
  label: string;
  base_url: string;
  model: string;
  requires_key: boolean;
  key_masked: string | null;
  active: boolean;
};

export type TestResult = {
  ok: boolean;
  model: string;
  latency_ms: number;
  message: string;
};

export type LookupResult = {
  raw: string;
  source: "cache" | "network";
  model: string;
  first_delta_ms: number;
  total_ms: number;
};

export type LookupError = { code: string; message: string };

export type DeltaEvent = { request_id: string; text: string };

export type HotkeyView = { kind: "lookup" | "capture"; display: string };

export type GrammarIssue = {
  fragment: string;
  replacement: string;
  explanation: string;
};

export type WordGloss = { term: string; gloss: string; pos: string; note: string };

export type OverlayBlock = {
  text: string;
  x: number;
  y: number;
  w: number;
  h: number;
  /** 单行高度：多行合并后 h 会变大，字号要按这个算 */
  line_h: number;
  translated: boolean;
};

export type LookupArgs = {
  word: string;
  lang?: "en" | "zh";
  context?: string;
  requestId: string;
  temperature?: number;
  /** 重试时跳过缓存，避免再次读到上一次写进去的坏数据 */
  bypassCache?: boolean;
};

export const api = {
  listProviders: () => invoke<ProviderView[]>("list_providers"),
  saveProvider: (id: string, baseUrl: string, model: string) =>
    invoke<void>("save_provider", { id, baseUrl, model }),
  setActiveProvider: (id: string) => invoke<void>("set_active_provider", { id }),
  setApiKey: (id: string, key: string) => invoke<string>("set_api_key", { id, key }),
  clearApiKey: (id: string) => invoke<void>("clear_api_key", { id }),
  testConnection: (id: string) => invoke<TestResult>("test_connection", { id }),
  lookup: (args: LookupArgs) => invoke<LookupResult>("lookup", { req: { ...args } }),
  hotkeyHint: () => invoke<string>("hotkey_hint"),
  hideWindow: () => invoke<void>("hide_window"),
  devFlags: () => invoke<{ dev_lookup: string | null }>("dev_flags"),
  hotkeyList: () => invoke<HotkeyView[]>("hotkey_list"),
  setHotkey: (kind: string, spec: string) =>
    invoke<HotkeyView>("set_hotkey", { kind, spec }),
  startCapture: () => invoke<void>("start_capture"),
  cancelCapture: () => invoke<void>("cancel_capture"),
  finishCapture: (x: number, y: number, w: number, h: number) =>
    invoke<number>("finish_capture", { x, y, w, h }),
  captureBlocks: () => invoke<OverlayBlock[]>("capture_blocks"),
  closeOverlay: () => invoke<void>("close_overlay"),
  translateText: (text: string, requestId: string) =>
    invoke<{ text: string; truncated: boolean }>("translate_text", { text, requestId }),
  cancelTranslate: () => invoke<void>("cancel_translate"),
  checkGrammar: (text: string) => invoke<GrammarIssue[]>("check_grammar", { text }),
  explainWord: (term: string, context: string) =>
    invoke<WordGloss>("explain_word", { term, context }),
  invalidateLookup: (word: string, lang: string, context?: string) =>
    invoke<void>("invalidate_lookup", { word, lang, context }),
};

export function onDelta(handler: (event: DeltaEvent) => void): Promise<UnlistenFn> {
  return listen<DeltaEvent>("sense://delta", (event) => handler(event.payload));
}
