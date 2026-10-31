import { describe, expect, it } from "vitest";
import {
  createDragSession,
  createFrameSender,
  MAX_SIDE,
  MIN_HEIGHT,
  MIN_WIDTH,
  resizeFrame,
  type Edge,
  type Frame,
} from "./windowFrame";
import tauriConf from "../../src-tauri/tauri.conf.json";
import windowRs from "../../src-tauri/src/window.rs?raw";

const START: Frame = { x: 200, y: 100, width: 960, height: 900 };

describe("窗口最小尺寸三处对得上", () => {
  it("与 tauri.conf.json 一致", () => {
    const main = tauriConf.app.windows.find((window) => window.label === "main");
    expect(main?.minWidth).toBe(MIN_WIDTH);
    expect(main?.minHeight).toBe(MIN_HEIGHT);
  });

  it("与 Rust window.rs 里的常量一致", () => {
    expect(windowRs).toContain(`pub const MIN_WIDTH: f64 = ${MIN_WIDTH}.0;`);
    expect(windowRs).toContain(`pub const MIN_HEIGHT: f64 = ${MIN_HEIGHT}.0;`);
    expect(windowRs).toContain(`const MAX_SIDE: f64 = ${MAX_SIDE}.0;`);
  });
});

describe("resizeFrame：拖哪条边，哪条边动，对面那条不动", () => {
  it("右边：只加宽，位置不变", () => {
    expect(resizeFrame(START, "e", 40, 999)).toEqual({ x: 200, y: 100, width: 1000, height: 900 });
  });

  it("下边：只加高，位置不变", () => {
    expect(resizeFrame(START, "s", 999, 30)).toEqual({ x: 200, y: 100, width: 960, height: 930 });
  });

  it("左边往左拖：变宽，左边界跟着指针走，右边界不动", () => {
    const next = resizeFrame(START, "w", -50, 0);
    expect(next).toEqual({ x: 150, y: 100, width: 1010, height: 900 });
    expect(next.x + next.width).toBe(START.x + START.width);
  });

  it("上边往上拖：变高，上边界跟着指针走，下边界不动", () => {
    const next = resizeFrame(START, "n", 0, -60);
    expect(next).toEqual({ x: 200, y: 40, width: 960, height: 960 });
    expect(next.y + next.height).toBe(START.y + START.height);
  });

  it("左边往右拖：变窄，右边界仍不动", () => {
    const next = resizeFrame(START, "w", 100, 0);
    expect(next).toEqual({ x: 300, y: 100, width: 860, height: 900 });
    expect(next.x + next.width).toBe(START.x + START.width);
  });

  it("四个角：横竖两条边同时动，对角那个点不动", () => {
    const cases: [Edge, number, number, Frame][] = [
      ["se", 30, 20, { x: 200, y: 100, width: 990, height: 920 }],
      ["sw", -30, 20, { x: 170, y: 100, width: 990, height: 920 }],
      ["ne", 30, -20, { x: 200, y: 80, width: 990, height: 920 }],
      ["nw", -30, -20, { x: 170, y: 80, width: 990, height: 920 }],
    ];
    for (const [edge, dx, dy, expected] of cases) {
      expect(resizeFrame(START, edge, dx, dy), edge).toEqual(expected);
    }
    // 对角点：拖 nw 角时右下角不动；拖 se 角时左上角不动
    const nw = resizeFrame(START, "nw", -30, -20);
    expect([nw.x + nw.width, nw.y + nw.height]).toEqual([START.x + START.width, START.y + START.height]);
    const se = resizeFrame(START, "se", 30, 20);
    expect([se.x, se.y]).toEqual([START.x, START.y]);
  });

  it("缩到最小尺寸就停：再往里拖，窗口不能被推着跑", () => {
    // 右边往里拖 500：宽夹在最小，位置不变
    expect(resizeFrame(START, "e", -500, 0)).toEqual({ x: 200, y: 100, width: MIN_WIDTH, height: 900 });
    // 左边往右拖 500：宽夹在最小，右边界仍然是原来的右边界
    const left = resizeFrame(START, "w", 500, 0);
    expect(left.width).toBe(MIN_WIDTH);
    expect(left.x + left.width).toBe(START.x + START.width);
    // 上边往下拖 500：高夹在最小，下边界不动
    const top = resizeFrame(START, "n", 0, 500);
    expect(top.height).toBe(MIN_HEIGHT);
    expect(top.y + top.height).toBe(START.y + START.height);
  });

  it("上限也夹住", () => {
    expect(resizeFrame(START, "se", 1e9, 1e9)).toMatchObject({ width: MAX_SIDE, height: MAX_SIDE });
  });

  it("小数指针位置取整；多屏时坐标可以是负的", () => {
    const start: Frame = { x: -1500, y: -40, width: 900, height: 800 };
    expect(resizeFrame(start, "w", -12.6, 0)).toEqual({ x: -1513, y: -40, width: 913, height: 800 });
  });

  it("没拖动（dx=dy=0）：外框原样", () => {
    for (const edge of ["n", "s", "e", "w", "ne", "nw", "se", "sw"] as Edge[]) {
      expect(resizeFrame(START, edge, 0, 0), edge).toEqual(START);
    }
  });

  it("起点本来就小于最小尺寸（外部改过）：一动就拉回最小尺寸", () => {
    const tiny: Frame = { x: 0, y: 0, width: 300, height: 300 };
    expect(resizeFrame(tiny, "se", 0, 0)).toMatchObject({ width: MIN_WIDTH, height: MIN_HEIGHT });
  });
});

describe("createFrameSender：只留最新", () => {
  const frame = (width: number): Frame => ({ x: 0, y: 0, width, height: 800 });

  /** 手动控制每次发送何时完成 */
  function manualSend() {
    const sent: Frame[] = [];
    const releases: Array<() => void> = [];
    const send = (f: Frame) => {
      sent.push(f);
      return new Promise<void>((resolve) => releases.push(resolve));
    };
    return { sent, send, release: () => releases.shift()?.() };
  }

  const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

  it("上一次没发完时来的新外框只覆盖「待发」：发完之后只补发最新的那个", async () => {
    const { sent, send, release } = manualSend();
    const sender = createFrameSender(send);

    sender.push(frame(800)); // 立刻发出
    sender.push(frame(810)); // 排队
    sender.push(frame(820)); // 覆盖 810
    sender.push(frame(830)); // 覆盖 820
    expect(sent.map((f) => f.width)).toEqual([800]);

    release(); // 800 发完
    await tick();
    expect(sent.map((f) => f.width)).toEqual([800, 830]); // 810、820 被跳过
  });

  it("松手前最后一个外框一定会被发出去", async () => {
    const { sent, send, release } = manualSend();
    const sender = createFrameSender(send);
    sender.push(frame(800));
    sender.push(frame(900)); // 最后一个
    release();
    await tick();
    release();
    await tick();
    expect(sent[sent.length - 1]?.width).toBe(900);
  });

  it("发送失败不会卡死后面的：下一个外框照常发", async () => {
    const sent: number[] = [];
    let failFirst = true;
    const sender = createFrameSender(async (f) => {
      sent.push(f.width);
      if (failFirst) {
        failFirst = false;
        throw new Error("window gone");
      }
    });
    sender.push(frame(800));
    await tick();
    sender.push(frame(850));
    await tick();
    expect(sent).toEqual([800, 850]);
  });

  it("空闲时每次 push 都直接发", async () => {
    const sent: number[] = [];
    const sender = createFrameSender(async (f) => {
      sent.push(f.width);
    });
    sender.push(frame(800));
    await tick();
    sender.push(frame(810));
    await tick();
    expect(sent).toEqual([800, 810]);
  });
});

describe("createDragSession：一次拖拽的时序", () => {
  const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

  /** 手动控制「量起始外框」何时完成、成功还是失败 */
  function rig(edge: Edge = "e", buttonsAtStart = 1) {
    const sent: Frame[] = [];
    let measured: { resolve: (frame: Frame) => void; reject: (error: unknown) => void } = {
      resolve: () => {},
      reject: () => {},
    };
    let ends = 0;
    const session = createDragSession({
      edge,
      startX: 1000,
      startY: 500,
      buttonsAtStart,
      measure: () => new Promise<Frame>((resolve, reject) => (measured = { resolve, reject })),
      send: async (frame) => {
        sent.push(frame);
      },
      onEnd: () => {
        ends += 1;
      },
    });
    return { session, sent, measured: () => measured, ends: () => ends };
  }

  it("量完起始外框之前的移动不动窗口；量完之后按偏移发新外框", async () => {
    const { session, sent, measured } = rig("e");
    session.move(1020, 500, 1); // 还没量完
    await tick();
    expect(sent).toHaveLength(0);

    measured().resolve(START);
    await tick();
    session.move(1040, 500, 1);
    await tick();
    expect(sent).toEqual([resizeFrame(START, "e", 40, 0)]);
  });

  it("量的时候用户已经松手：量完也不开始，之后的移动都不发（旧实现栽过的竞态）", async () => {
    const { session, sent, measured, ends } = rig();
    session.end(); // 松手发生在量的期间
    measured().resolve(START);
    await tick();
    session.move(1100, 500, 1);
    await tick();
    expect(sent).toHaveLength(0);
    expect(ends()).toBe(1);
  });

  it("松手丢了（窗口被隐藏、右键菜单打断）：移动事件里主键已经松开，就收尾，不再改窗口", async () => {
    const { session, sent, measured, ends } = rig();
    measured().resolve(START);
    await tick();
    session.move(1010, 500, 1);
    await tick();
    expect(sent).toHaveLength(1);

    session.move(1050, 500, 0); // 没按着键的移动：窗口再出现后鼠标一动就缩放的那个 bug
    session.move(1080, 500, 1); // 收尾之后即使又带着键值也不再响应
    await tick();
    expect(sent).toHaveLength(1);
    expect(ends()).toBe(1);
    expect(session.ended).toBe(true);
  });

  it("合成事件（按下时 buttons 就是 0；WebKit 的 buttons 取自系统按键状态，辅助功能 / 自动化工具发的事件永远是 0）：不信 buttons，拖拽照常进行", async () => {
    // 真机实测踩过：信 buttons 的话，第一个移动事件就被当成「已松手」，拖拽永远起不来
    const { session, sent, measured, ends } = rig("e", 0);
    measured().resolve(START);
    await tick();
    session.move(1020, 500, 0);
    session.move(1040, 500, 0);
    await tick();
    expect(sent.length).toBeGreaterThan(0);
    expect(sent[sent.length - 1]).toEqual(resizeFrame(START, "e", 40, 0));
    expect(ends()).toBe(0);
    expect(session.ended).toBe(false);
    session.end(); // 照常靠松手收尾
    expect(ends()).toBe(1);
  });

  it("量起始外框失败：放弃这次拖动，收尾一次", async () => {
    const { session, measured, ends, sent } = rig();
    measured().reject(new Error("window gone"));
    await tick();
    expect(session.ended).toBe(true);
    expect(ends()).toBe(1);
    session.move(1100, 500, 1);
    await tick();
    expect(sent).toHaveLength(0);
  });

  it("同步抛错的 measure 也一样：放弃，收尾一次", () => {
    let ends = 0;
    const session = createDragSession({
      edge: "e",
      startX: 0,
      startY: 0,
      buttonsAtStart: 1,
      measure: () => {
        throw new Error("sync");
      },
      send: async () => {},
      onEnd: () => {
        ends += 1;
      },
    });
    expect(session.ended).toBe(true);
    expect(ends).toBe(1);
  });

  it("收尾多次只生效一次（松手、失焦、指针捕获丢失会接连触发）", async () => {
    const { session, measured, ends } = rig();
    measured().resolve(START);
    await tick();
    session.end();
    session.end();
    session.end();
    expect(ends()).toBe(1);
  });

  it("每个方向的偏移都走 resizeFrame：拖左边时位置跟着变", async () => {
    const { session, sent, measured } = rig("w");
    measured().resolve(START);
    await tick();
    session.move(950, 500, 1);
    await tick();
    expect(sent).toEqual([resizeFrame(START, "w", -50, 0)]);
    expect(sent[0].x).toBe(START.x - 50);
  });
});
