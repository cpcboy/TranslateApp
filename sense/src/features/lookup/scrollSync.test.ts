import { describe, expect, it } from "vitest";
import { createScrollSync, type ScrollBox } from "./scrollSync";

/**
 * 假滚动容器，照 WebKit 的行为：写 scrollTop 会取整并夹到 [0, 最大值]；
 * 位置真的变了才排一个 scroll 事件，事件下一帧才派发（读的是派发时的位置）。
 */
class FakePane implements ScrollBox {
  private top = 0;
  fired = false;
  constructor(
    readonly scrollHeight: number,
    readonly clientHeight: number,
  ) {}
  get scrollTop() {
    return this.top;
  }
  set scrollTop(value: number) {
    const max = Math.max(0, this.scrollHeight - this.clientHeight);
    const next = Math.min(max, Math.max(0, Math.floor(value)));
    if (next !== this.top) this.fired = true;
    this.top = next;
  }
  /** macOS 橡皮筋：用户拖过了边界，位置临时超出 [0, 最大值]，写入才会被夹回来 */
  overscroll(top: number) {
    this.top = top;
    this.fired = true;
  }
}

/** 把两栏接上同步，并模拟逐帧派发 scroll 事件；frameMs 是帧间隔（卡顿时可以很大） */
function mount(left: FakePane, right: FakePane, frameMs = 16) {
  let clock = 0;
  const sync = createScrollSync(() => clock);
  let handled = 0;
  return {
    /** 派发这一帧该到的 scroll 事件；返回是否还有事件在路上 */
    frame() {
      clock += frameMs;
      const events = [left, right].filter((pane) => pane.fired);
      events.forEach((pane) => (pane.fired = false));
      for (const pane of events) {
        handled += 1;
        if (pane === left) sync(left, right);
        else sync(right, left);
      }
      return left.fired || right.fired;
    },
    get handled() {
      return handled;
    },
  };
}

/** 用户拖动 pane 滚到 target：每帧走一小段（带小数，贴近触控板），然后停手 */
function userScroll(pane: FakePane, rig: ReturnType<typeof mount>, to: number) {
  // 有上限：同步逻辑退化成互相推的时候，用户永远滚不到目标，要让测试红灯而不是卡死
  for (let step = 0; step < 400 && Math.abs(pane.scrollTop - to) > 1; step += 1) {
    pane.scrollTop = pane.scrollTop + Math.sign(to - pane.scrollTop) * Math.min(37.3, Math.abs(to - pane.scrollTop));
    rig.frame();
  }
}

/** 停手之后再放 N 帧，数还有多少个事件被处理 */
function settle(rig: ReturnType<typeof mount>, frames = 300) {
  const before = rig.handled;
  for (let i = 0; i < frames; i += 1) rig.frame();
  return rig.handled - before;
}

describe("两栏同步滚动", () => {
  it("用户停手后，两栏不再互相推：停手后只处理一个回响事件，位置纹丝不动", () => {
    // 原文比译文长得多（比例 ≠ 1），取整才会让每个来回丢 1px
    const left = new FakePane(8370, 639);
    const right = new FakePane(5770, 639);
    const rig = mount(left, right);

    userScroll(left, rig, 602);
    const stopped = { left: left.scrollTop, right: right.scrollTop };

    const handledAfterStop = settle(rig);
    expect(handledAfterStop).toBeLessThanOrEqual(1);
    expect({ left: left.scrollTop, right: right.scrollTop }).toEqual(stopped);
  });

  it("译文比原文长：同样停得住", () => {
    const left = new FakePane(5770, 639);
    const right = new FakePane(8370, 639);
    const rig = mount(left, right);

    userScroll(left, rig, 1203);
    const stopped = { left: left.scrollTop, right: right.scrollTop };

    expect(settle(rig)).toBeLessThanOrEqual(1);
    expect({ left: left.scrollTop, right: right.scrollTop }).toEqual(stopped);
  });

  it("按比例跟随：滚到一半、到底、回到顶，另一栏同步", () => {
    const left = new FakePane(8000, 600); // 最大 7400
    const right = new FakePane(4000, 600); // 最大 3400
    const rig = mount(left, right);

    userScroll(left, rig, 3700);
    settle(rig, 5);
    expect(right.scrollTop).toBe(1700);

    userScroll(left, rig, 7400);
    settle(rig, 5);
    expect(right.scrollTop).toBe(3400);

    userScroll(left, rig, 0);
    settle(rig, 5);
    expect(right.scrollTop).toBe(0);
  });

  it("反方向也一样：用户滚右栏，左栏跟着，也停得住", () => {
    const left = new FakePane(8370, 639);
    const right = new FakePane(5770, 639);
    const rig = mount(left, right);

    userScroll(right, rig, 2500);
    settle(rig, 5);
    expect(left.scrollTop).toBeGreaterThan(3500); // 2500/5131 × 7731 ≈ 3767

    const stopped = { left: left.scrollTop, right: right.scrollTop };
    expect(settle(rig)).toBeLessThanOrEqual(1);
    expect({ left: left.scrollTop, right: right.scrollTop }).toEqual(stopped);
  });

  it("回响还没到、用户已经又滚了这一栏：不能被当成回响吞掉", () => {
    const left = new FakePane(8000, 600);
    const right = new FakePane(4000, 600);
    const rig = mount(left, right);

    left.scrollTop = 1000;
    rig.frame(); // 左栏事件：右栏被写成 459，回响在路上
    expect(right.scrollTop).toBe(459);

    right.scrollTop = 520; // 用户在回响到达前亲手往下滚了 61px（小幅滚动也得算数）
    rig.frame(); // 右栏的事件：位置对不上写入值，是真滚动

    expect(left.scrollTop).toBe(Math.floor((520 / 3400) * 7400));
  });

  it("回响到达时位置与写入值差 1px（设备像素取整）：仍认得出是回响", () => {
    const left = new FakePane(8000, 600);
    const right = new FakePane(4000, 600);
    const rig = mount(left, right);

    left.scrollTop = 1000;
    rig.frame(); // 写了右栏（459）
    right.scrollTop = right.scrollTop + 1; // 回响到达前，右栏被排版挪了 1px
    right.fired = true; // 它自己的 scroll 事件

    rig.frame(); // 右栏的事件：差 1px 以内，是回响，不能往回写左栏
    expect(left.scrollTop).toBe(1000);
  });

  it("主线程卡顿（一帧 250ms，远慢于 16ms）：回响迟到也停得住", () => {
    const left = new FakePane(8370, 639);
    const right = new FakePane(5770, 639);
    const rig = mount(left, right, 250);

    userScroll(left, rig, 602);
    const stopped = { left: left.scrollTop, right: right.scrollTop };

    expect(settle(rig)).toBeLessThanOrEqual(1);
    expect({ left: left.scrollTop, right: right.scrollTop }).toEqual(stopped);
  });

  it("对方被夹在边界上（橡皮筋拖过头）：回响认的是夹紧后的读数，不是算出来的目标", () => {
    const left = new FakePane(8000, 600); // 最大 7400
    const right = new FakePane(4000, 600); // 最大 3400
    const rig = mount(left, right);

    left.overscroll(8200); // 拖过了底：比例 > 1，算出的目标超过右栏的最大值
    rig.frame(); // 右栏被夹到 3400
    expect(right.scrollTop).toBe(3400);

    rig.frame(); // 右栏的回响事件：读数 3400 就是写入后的位置，不能当真滚动写回
    expect(left.scrollTop).toBe(8200);
  });

  it("没写动对方就不登记回响：之后用户 1px 的真滚动照常同步", () => {
    const left = new FakePane(8000, 600);
    const right = new FakePane(4000, 600);
    let now = 0;
    const sync = createScrollSync(() => now);

    right.scrollTop = 459; // 用户自己滚到的，没有回响条目
    left.scrollTop = 1000;
    sync(left, right); // 目标还是 459：没动，不会有事件，也不该记回响

    right.scrollTop = 460; // 用户再往下滚 1px
    sync(right, left);
    expect(left.scrollTop).toBe(Math.floor((460 / 3400) * 7400));
  });

  it("回响过期后，同一位置的事件算用户滚的", () => {
    let now = 0;
    const left = new FakePane(8000, 600);
    const right = new FakePane(4000, 600);
    const sync = createScrollSync(() => now);

    left.scrollTop = 1000;
    sync(left, right); // 写了右栏（459），记下回响
    expect(right.scrollTop).toBe(459);

    sync(right, left); // 回响还新鲜：被忽略，左栏纹丝不动
    expect(left.scrollTop).toBe(1000);

    now = 1500; // 超过保质期：同一位置的事件不再是回响
    sync(right, left);
    expect(left.scrollTop).toBe(Math.floor((459 / 3400) * 7400));
  });

  it("对方没有滚动范围（内容比窗口短）：不写、不报错", () => {
    const left = new FakePane(8000, 600);
    const right = new FakePane(500, 600);
    const rig = mount(left, right);

    userScroll(left, rig, 3000);
    expect(right.scrollTop).toBe(0);
    expect(settle(rig)).toBe(0);
  });
});
