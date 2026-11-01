import type { Route } from "../../lib/route";

/**
 * 输入框、目标语言选择器的交互流程里和 React 无关的部分。
 * 单独放出来是为了能写单测：组件本身在 node 下跑不起来（没有 DOM），接线写在组件里就没人盯着。
 */

export type InputActions = {
  /** 查词卡还是普通翻译；被更新的一次提交取代了返回 null */
  routeInput: (value: string) => Promise<Route | null>;
  run: (value: string) => void;
  runLong: (value: string) => void;
  clearInput: () => void;
  closePicker: () => void;
};

/**
 * 提交一次输入：先收起选择器，再问路由，再开跑。
 * 走翻译时原文已经进了阅读视图的左栏，输入框腾出来给下一篇；走查词卡时输入框里的词留着（改一个字母再查很常见）。
 */
export async function submitInput(raw: string, actions: InputActions): Promise<void> {
  const value = raw.trim();
  if (!value) return;
  actions.closePicker();
  const route = await actions.routeInput(value);
  if (route === null) return; // 被更新的一次提交取代了：什么也别做
  if (route === "long") {
    actions.runLong(value);
    actions.clearInput();
    return;
  }
  actions.run(value);
}

/**
 * 选了一种语言之后：存成功才收起选择器、把焦点还给输入框（接着就是打字）；
 * 存失败了面板留着，原因写在里面——选择已经退回去了，不能悄悄退。
 */
export async function pickTarget(
  code: string,
  choose: (code: string) => Promise<boolean>,
  after: { closePicker: () => void; focusInput: () => void },
): Promise<void> {
  if (!(await choose(code))) return;
  after.closePicker();
  after.focusInput();
}

/** `KeyboardEvent` 里这里用到的那几项（测试里不必造一个完整的事件） */
export type EscEvent = {
  key: string;
  isComposing?: boolean;
  keyCode?: number;
  preventDefault: () => void;
  stopImmediatePropagation: () => void;
};

/**
 * 选择器展开时 Esc 先把它收起来。挂在 window 的捕获阶段：拦下之后，`App.tsx` 里冒泡阶段的监听就不会再把整个窗口一起收了。
 * 输入法组字时的 Esc 是取消候选词，不拦；别的键一概不管。
 */
export function pickerEscHandler(after: { closePicker: () => void; focusInput: () => void }): (event: EscEvent) => void {
  return (event) => {
    if (event.key !== "Escape" || event.isComposing || event.keyCode === 229) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    after.closePicker();
    after.focusInput();
  };
}
