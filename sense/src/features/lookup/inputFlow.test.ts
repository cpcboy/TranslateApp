import { describe, expect, it, vi } from "vitest";
import type { Route } from "../../lib/route";
import { pickerEscHandler, pickTarget, submitInput, type EscEvent, type InputActions } from "./inputFlow";

/** 一组假的动作，按调用顺序记在 calls 里 */
function actions(route: Route | null | Promise<Route | null>) {
  const calls: string[] = [];
  const fake: InputActions = {
    routeInput: vi.fn(async (value: string) => {
      calls.push(`route:${value}`);
      return route;
    }),
    run: (value) => calls.push(`run:${value}`),
    runLong: (value) => calls.push(`runLong:${value}`),
    clearInput: () => calls.push("clearInput"),
    closePicker: () => calls.push("closePicker"),
  };
  return { calls, fake };
}

describe("submitInput：输入框的提交流程", () => {
  it("空输入、只有空白：什么也不做（不问路由，也不收选择器）", async () => {
    for (const raw of ["", "   ", "\n\t "]) {
      const { calls, fake } = actions("word");
      await submitInput(raw, fake);
      expect(calls, JSON.stringify(raw)).toEqual([]);
    }
  });

  it("先收起选择器，再问路由；去掉首尾空白后的内容才送出去", async () => {
    const { calls, fake } = actions("word");
    await submitInput("  charge \n", fake);
    expect(calls).toEqual(["closePicker", "route:charge", "run:charge"]);
  });

  it("路由说查词卡：run，输入框里的词留着（改一个字母再查很常见）", async () => {
    const { calls, fake } = actions("word");
    await submitInput("charge", fake);
    expect(calls).not.toContain("clearInput");
    expect(calls).not.toContain("runLong:charge");
  });

  it("路由说普通翻译：runLong，并清空输入框（原文已经进了阅读视图的左栏）", async () => {
    const { calls, fake } = actions("long");
    await submitInput("bonjour tout le monde", fake);
    expect(calls).toEqual(["closePicker", "route:bonjour tout le monde", "runLong:bonjour tout le monde", "clearInput"]);
    expect(calls.some((call) => call.startsWith("run:"))).toBe(false);
  });

  it("被更新的一次提交取代了（路由返回 null）：什么也别开跑，也别清输入框", async () => {
    const { calls, fake } = actions(null);
    await submitInput("charge", fake);
    expect(calls).toEqual(["closePicker", "route:charge"]);
  });

  it("路由是异步的：问完才开跑，不会在 await 之前就先 run", async () => {
    let answer: (route: Route) => void = () => {};
    const { calls, fake } = actions(new Promise<Route | null>((resolve) => (answer = resolve)));
    const submitting = submitInput("charge", fake);
    await Promise.resolve();
    expect(calls).toEqual(["closePicker", "route:charge"]); // 还没有答案：没开跑
    answer("long");
    await submitting;
    expect(calls.slice(2)).toEqual(["runLong:charge", "clearInput"]);
  });
});

describe("pickTarget：选了一种语言之后", () => {
  const after = () => {
    const calls: string[] = [];
    return { calls, after: { closePicker: () => calls.push("closePicker"), focusInput: () => calls.push("focusInput") } };
  };

  it("存成功：收起选择器，焦点还给输入框（接着就是打字）", async () => {
    const { calls, after: hooks } = after();
    const choose = vi.fn(async () => true);
    await pickTarget("ja", choose, hooks);
    expect(choose).toHaveBeenCalledWith("ja");
    expect(calls).toEqual(["closePicker", "focusInput"]);
  });

  it("存失败：选择器留着（原因写在里面），焦点也不抢——不能把失败收起来看不见", async () => {
    const { calls, after: hooks } = after();
    await pickTarget("ja", async () => false, hooks);
    expect(calls).toEqual([]);
  });

  it("要等存盘的结果：存盘没完成之前不收起", async () => {
    const { calls, after: hooks } = after();
    let settle: (ok: boolean) => void = () => {};
    const picking = pickTarget("ja", () => new Promise<boolean>((resolve) => (settle = resolve)), hooks);
    await Promise.resolve();
    expect(calls).toEqual([]);
    settle(true);
    await picking;
    expect(calls).toEqual(["closePicker", "focusInput"]);
  });
});

describe("pickerEscHandler：选择器开着时的 Esc", () => {
  const fakeEvent = (init: Partial<EscEvent>) => {
    const event = {
      key: "Escape",
      isComposing: false,
      keyCode: 27,
      preventDefault: vi.fn(),
      stopImmediatePropagation: vi.fn(),
      ...init,
    };
    return event;
  };
  const handler = () => {
    const calls: string[] = [];
    return { calls, onKey: pickerEscHandler({ closePicker: () => calls.push("closePicker"), focusInput: () => calls.push("focusInput") }) };
  };

  it("Esc：收起选择器、焦点回输入框，并且拦下事件——不让 App 把整个窗口一起收了", () => {
    const { calls, onKey } = handler();
    const event = fakeEvent({});
    onKey(event);
    expect(calls).toEqual(["closePicker", "focusInput"]);
    expect(event.preventDefault).toHaveBeenCalledTimes(1);
    expect(event.stopImmediatePropagation).toHaveBeenCalledTimes(1);
  });

  it("输入法组字时的 Esc 是取消候选词：不拦，也不收选择器（isComposing 与 keyCode 229 两种写法都认）", () => {
    for (const init of [{ isComposing: true }, { keyCode: 229 }]) {
      const { calls, onKey } = handler();
      const event = fakeEvent(init);
      onKey(event);
      expect(calls, JSON.stringify(init)).toEqual([]);
      expect(event.stopImmediatePropagation).not.toHaveBeenCalled();
      expect(event.preventDefault).not.toHaveBeenCalled();
    }
  });

  it("别的键一概不管：照常往下传，别的快捷键不受影响", () => {
    for (const key of ["Enter", "a", "Tab", "ArrowDown", " "]) {
      const { calls, onKey } = handler();
      const event = fakeEvent({ key });
      onKey(event);
      expect(calls, key).toEqual([]);
      expect(event.stopImmediatePropagation).not.toHaveBeenCalled();
    }
  });
});
