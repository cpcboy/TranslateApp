import { create } from "zustand";
import { api, type Language, type LanguagePair } from "./ipc";

/**
 * 翻译成哪种语言：偏好是 `auto` 或语言代码，由 Rust 存在 `settings.json` 里（长文翻译、截屏翻译共用），
 * 语言表和取舍规则都在 `lang.rs`。这里只管显示和把用户的选择交回去。
 */

/** 偏好里「不手选」的取值（与 Rust 的 `lang::AUTO` 一致） */
export const AUTO = "auto";

/** 「自动」的含义，写在选择器里，随时告诉用户默认怎么译 */
export const AUTO_HINT = "自动：中文 → 英文，其它语言 → 中文";

/** 没把握时标题上写的话：不报一个可能是错的语种 */
export const UNSURE_SOURCE = "自动识别";

function isLanguage(value: unknown): value is Language {
  if (!value || typeof value !== "object") return false;
  const item = value as Record<string, unknown>;
  return typeof item.code === "string" && typeof item.name === "string" && typeof item.native === "string";
}

/**
 * 把 Rust 发来的「两端语言」整形成 `LanguagePair`。字段缺了、类型不对就返回 null（界面退回只写「原文」「译文」）：
 * 标题上的语种是锦上添花，不该因为一条畸形的事件让阅读视图崩掉。
 */
export function parsePair(value: unknown): LanguagePair | null {
  if (!value || typeof value !== "object") return null;
  const item = value as Record<string, unknown>;
  if (!isLanguage(item.target)) return null;
  const source = isLanguage(item.source) ? item.source : null;
  return { source, source_confident: item.source_confident === true && source !== null, target: item.target };
}

/** 确认了的原文语言代码；没把握或认不出为 null。语法检查这类只对某种语言有意义的功能靠它判断 */
export function confirmedSourceCode(pair: LanguagePair | null): string | null {
  return pair && pair.source_confident && pair.source ? pair.source.code : null;
}

/** 两栏的标题：`原文 · 日语` / `译文 · 中文`；还没拿到语言（事件没到）就只写「原文」「译文」 */
export function paneTitles(pair: LanguagePair | null): { source: string; target: string } {
  if (!pair) return { source: "原文", target: "译文" };
  return {
    source: `原文 · ${confirmedSourceCode(pair) ? pair.source?.name : UNSURE_SOURCE}`,
    target: `译文 · ${pair.target.name}`,
  };
}

/** 输入框旁那颗按钮上写的字 */
export function pillLabel(pref: string, languages: Language[]): string {
  if (pref === AUTO) return "自动";
  return languages.find((language) => language.code === pref)?.name ?? "自动";
}

type TargetState = {
  pref: string;
  languages: Language[];
  /** 存偏好失败的原因（选择器里写出来）；没出过错为 null */
  error: string | null;
  /** 最近一次确认存下的值（load 取到的、或某次 choose 存成功的）：存失败时退回它，而不是"上一次点的"——那个可能也没存上 */
  saved: string;
  /** 正在存的那一次。同一个语言再点一次就等它的结果，不抢着说"选好了" */
  inflight: { code: string; promise: Promise<boolean> } | null;
  load: () => Promise<void>;
  /** 选一种语言；返回是否存成功了（界面据此决定要不要收起选择器——失败了要留着，把原因给人看） */
  choose: (code: string) => Promise<boolean>;
  /** 打开选择器时调：上一回存失败的提示说的是上一回的事，不该留到这一回 */
  clearError: () => void;
};

/** 选择的序号：连选几个时，只有最后一次的结果能动界面 */
let chooseSeq = 0;
/** 存成功的那次的序号：先发的后到，不能把后发的已经存下的值盖掉 */
let savedSeq = 0;

export const useTarget = create<TargetState>((set, get) => ({
  pref: AUTO,
  languages: [],
  error: null,
  saved: AUTO,
  inflight: null,

  load: async () => {
    try {
      const settings = await api.translateLanguages();
      const known = settings.target === AUTO || settings.languages.some((item) => item.code === settings.target);
      const persisted = known ? settings.target : AUTO;
      // 正在存的时候不动选择：这时取到的是存盘之前的值，会让界面闪回去；那一次存完自己会更新
      if (get().inflight) set({ languages: settings.languages });
      else set({ languages: settings.languages, pref: persisted, saved: persisted, error: null });
    } catch {
      // 取不到（比如不在 Tauri 里跑）：languages 留空，选择器就不出现，翻译照默认方向走
    }
  },

  choose: (code) => {
    const { inflight, pref } = get();
    if (inflight?.code === code) return inflight.promise; // 同一个正在存：等同一个结果
    if (inflight === null && code === pref) return Promise.resolve(true); // 已经是它了：不用存，也算选好了

    const my = ++chooseSeq;
    set({ pref: code, error: null }); // 先改界面，选了就有反应
    const entry = { code, promise: Promise.resolve(false) };
    const attempt = async (): Promise<boolean> => {
      try {
        const done = await api.setTranslateTarget(code);
        // 值确实存下了：记下来，哪怕这次已经被后来的选择取代（存盘是按顺序发生的，后来的失败了要退回到它）
        if (my > savedSeq) {
          savedSeq = my;
          set({ saved: done });
        }
        // 被后来的选择取代了：界面和收起选择器都归后来那次
        if (my !== chooseSeq) return false;
        set({ pref: done });
        return true;
      } catch (err) {
        if (my === chooseSeq) set({ pref: get().saved, error: String(err).replace(/^Error:\s*/, "") });
        return false;
      } finally {
        if (get().inflight === entry) set({ inflight: null });
      }
    };
    set({ inflight: entry });
    entry.promise = attempt();
    return entry.promise;
  },

  clearError: () => set({ error: null }),
}));
