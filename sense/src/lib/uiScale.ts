/**
 * 界面缩放（⌘+ / ⌘- / ⌘0）的纯逻辑：档位、快捷键判断、读写本地偏好。
 *
 * 缩放的是整张面板里的内容（文字连同它的间距，和浏览器的 ⌘+ / ⌘- 一样），
 * 做法是给面板内容套 CSS `zoom`——App 里的字号全是 px 写死的，改根字号不起作用。
 * 窗口外圈的透明边距和缩放把手不跟着缩放。
 */

export const SCALE_DEFAULT = 1;
export const SCALE_MIN = 0.7;
export const SCALE_MAX = 1.6;
/** 每按一次 ⌘+ / ⌘- 变 10 个百分点 */
export const SCALE_STEP = 0.1;
export const STORAGE_KEY = "sense.ui-scale";

export type ScaleAction = "in" | "out" | "reset";

/** 取整到 1%，并夹进 [SCALE_MIN, SCALE_MAX]；不是有限数就回默认值 */
export function clampScale(value: number): number {
  if (!Number.isFinite(value)) return SCALE_DEFAULT;
  return Math.round(Math.min(SCALE_MAX, Math.max(SCALE_MIN, value)) * 100) / 100;
}

export function nextScale(current: number, action: ScaleAction): number {
  switch (action) {
    case "in":
      return clampScale(current + SCALE_STEP);
    case "out":
      return clampScale(current - SCALE_STEP);
    case "reset":
      return SCALE_DEFAULT;
  }
}

type KeyLike = Pick<KeyboardEvent, "key" | "code" | "metaKey" | "ctrlKey" | "altKey" | "isComposing" | "keyCode">;

/**
 * 这次按键是不是缩放快捷键。
 *
 * 要按住 ⌘（不带 ⌃ / ⌥）；Shift 随意——美式键盘上 + 就是 ⇧=，⌘= 和 ⌘⇧= 都算「放大」。
 * 按键认的是 `event.key`（打出来的字符），不是物理位置：别的键盘布局上 = 的位置可能是别的字符。
 * 小键盘的 + - 0 由 `code` 兜底。输入法组字中不拦（组字时 keyCode 是 229）。
 */
export function scaleActionFor(event: KeyLike): ScaleAction | null {
  if (!event.metaKey || event.ctrlKey || event.altKey) return null;
  if (event.isComposing || event.keyCode === 229) return null;
  const { key, code } = event;
  if (key === "=" || key === "+" || code === "NumpadAdd") return "in";
  if (key === "-" || key === "_" || code === "NumpadSubtract") return "out";
  if (key === "0" || code === "Numpad0") return "reset";
  return null;
}

/** 屏幕角落短暂显示的提示：当前字号，到头了要说一声（不然按了没反应像坏了） */
export function describeScale(action: ScaleAction, before: number, after: number): string {
  const percent = `${Math.round(after * 100)}%`;
  if (action === "reset") return `字号 ${percent}（已还原）`;
  if (after === before) return `字号 ${percent}（已是${action === "in" ? "最大" : "最小"}）`;
  return `字号 ${percent}`;
}

/** 读上次保存的缩放；没有、坏了、越界都回默认或夹进范围，不抛错 */
export function loadScale(storage: Pick<Storage, "getItem"> | undefined = browserStorage()): number {
  try {
    const raw = storage?.getItem(STORAGE_KEY);
    if (raw == null) return SCALE_DEFAULT;
    return clampScale(Number.parseFloat(raw));
  } catch {
    return SCALE_DEFAULT;
  }
}

/** 存当前缩放；存不了（隐私模式、存储被禁）就算了——这一轮照样生效，只是下次启动不记得 */
export function saveScale(value: number, storage: Pick<Storage, "setItem"> | undefined = browserStorage()): void {
  try {
    storage?.setItem(STORAGE_KEY, String(clampScale(value)));
  } catch {
    /* 存不了不影响使用 */
  }
}

function browserStorage(): Storage | undefined {
  try {
    return typeof window === "undefined" ? undefined : window.localStorage;
  } catch {
    return undefined; // 访问 localStorage 本身就可能抛
  }
}
