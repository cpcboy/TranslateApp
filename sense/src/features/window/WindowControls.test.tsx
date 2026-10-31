import { isValidElement, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

// IPC 换成假的：只看三颗灯各自接到了哪个命令
const mocks = vi.hoisted(() => ({
  quitApp: vi.fn(async () => {}),
  hideWindow: vi.fn(async () => {}),
  toggleZoom: vi.fn(async () => true),
}));
vi.mock("../../lib/ipc", () => ({ api: mocks }));

import { ResizeHandles } from "./ResizeHandles";
import { WindowControls } from "./WindowControls";

beforeEach(() => {
  vi.clearAllMocks();
});

/** 把 WindowControls 的元素树里三颗灯（Light）的 props 找出来，按从左到右的顺序 */
function lights() {
  const root = WindowControls() as ReactElement<{ children: ReactElement[] }>;
  const children = (Array.isArray(root.props.children) ? root.props.children : [root.props.children]).filter(isValidElement);
  return children.map((element) => element.props as { label: string; color: string; onClick: () => void });
}

describe("左上角三颗灯", () => {
  it("从左到右是红、黄、绿，颜色与系统一致", () => {
    expect(lights().map((light) => light.color)).toEqual(["#ff5f57", "#febc2e", "#28c840"]);
  });

  it("红 = 退出应用，黄 = 隐藏窗口，绿 = 放大 / 还原：各接到各自的命令，不串线", async () => {
    const [red, yellow, green] = lights();
    red.onClick();
    expect(mocks.quitApp).toHaveBeenCalledTimes(1);
    expect(mocks.hideWindow).not.toHaveBeenCalled();
    expect(mocks.toggleZoom).not.toHaveBeenCalled();

    yellow.onClick();
    expect(mocks.hideWindow).toHaveBeenCalledTimes(1);
    expect(mocks.quitApp).toHaveBeenCalledTimes(1);

    green.onClick();
    expect(mocks.toggleZoom).toHaveBeenCalledTimes(1);
    expect(mocks.hideWindow).toHaveBeenCalledTimes(1);
  });

  it("命令失败（窗口刚被收起、已经没了）也不抛：失败被接住，不变成未处理的 rejection，不打扰用户", async () => {
    const failing = Promise.reject("window gone");
    const caught = vi.spyOn(failing, "catch");
    mocks.quitApp.mockReturnValueOnce(failing);
    const [red] = lights();
    expect(() => red.onClick()).not.toThrow();
    expect(caught).toHaveBeenCalled();
    await failing.catch(() => {});
  });

  it("悬停提示 / 读屏文字用中文，说清各自的作用（红灯写明等同 ⌘Q，黄灯写明同 Esc）", () => {
    const [red, yellow, green] = lights();
    expect(red.label).toContain("退出");
    expect(red.label).toContain("⌘Q");
    expect(yellow.label).toContain("隐藏");
    expect(yellow.label).toContain("Esc");
    expect(green.label).toContain("放大");
  });

  it("渲染出来是三个带 aria-label 的 button，在一个有名字的 group 里", () => {
    const html = renderToStaticMarkup(<WindowControls />);
    expect(html.match(/<button /g)).toHaveLength(3);
    expect(html).toContain('role="group"');
    expect(html).toContain('aria-label="窗口控制"');
    for (const word of ["退出", "隐藏", "放大"]) expect(html).toContain(word);
  });
});

describe("八个缩放把手", () => {
  const html = renderToStaticMarkup(<ResizeHandles />);

  it("上下左右 + 四角，一个不少", () => {
    expect(html.match(/role="presentation"/g)).toHaveLength(8);
  });

  it("鼠标指针的形状按方向配对：上下 ns、左右 ew、左上右下 nwse、右上左下 nesw，各两个", () => {
    for (const cursor of ["cursor-ns-resize", "cursor-ew-resize", "cursor-nwse-resize", "cursor-nesw-resize"]) {
      expect(html.match(new RegExp(cursor, "g")), cursor).toHaveLength(2);
    }
  });

  it("把手对屏幕阅读器隐藏（它们只是指针的热区）", () => {
    expect(html.match(/aria-hidden="true"/g)!.length).toBeGreaterThanOrEqual(8);
  });
});
