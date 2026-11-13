// 在 node 环境用轻量 hooks 运行时驱动真实的 TranslateOverlay / OverlayView，
// 验证 StrictMode 清理、代数/阶段单调与推拉竞态、失焦保护期、以及扩缩窗期间 bounds 同步与 resize 重排。
import type { ReactElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CaptureView, OverlayBlock } from "../../lib/ipc";

type Cell = { v?: unknown; current?: unknown; fn?: unknown; deps?: unknown[]; cleanup?: unknown };

const rt = vi.hoisted(() => ({
  hooks: [] as Cell[],
  idx: 0,
  effects: [] as Array<() => void | (() => void)>,
  queue: [] as Array<() => void>,
  renders: 0,
}));

vi.mock("react", async (importOriginal) => {
  const original = await importOriginal<typeof import("react")>();
  return {
    ...original,
    useState: (init: unknown) => {
      const i = rt.idx++;
      if (!(i in rt.hooks)) {
        rt.hooks[i] = { v: typeof init === "function" ? (init as () => unknown)() : init };
      }
      const cell = rt.hooks[i];
      return [
        cell.v,
        (next: unknown) => {
          cell.v = typeof next === "function" ? (next as (prev: unknown) => unknown)(cell.v) : next;
        },
      ];
    },
    useRef: (init: unknown) => {
      const i = rt.idx++;
      if (!(i in rt.hooks)) rt.hooks[i] = { current: init };
      return rt.hooks[i];
    },
    useMemo: (fn: () => unknown, deps: unknown[]) => {
      const i = rt.idx++;
      const prev = rt.hooks[i];
      if (
        prev?.deps &&
        prev.deps.length === deps.length &&
        prev.deps.every((d, k) => Object.is(d, deps[k]))
      ) {
        return prev.v;
      }
      const v = fn();
      rt.hooks[i] = { v, deps };
      return v;
    },
    useCallback: (fn: unknown, deps: unknown[]) => {
      const i = rt.idx++;
      const prev = rt.hooks[i];
      if (
        prev?.deps &&
        prev.deps.length === deps.length &&
        prev.deps.every((d, k) => Object.is(d, deps[k]))
      ) {
        return prev.fn;
      }
      rt.hooks[i] = { fn, deps };
      return fn;
    },
    // useState / useRef / useMemo / useCallback / useEffect
    useEffect: (fn: () => void | (() => void), deps?: unknown[]) => {
      const i = rt.idx++;
      const cell: Cell = rt.hooks[i] ?? {};
      rt.hooks[i] = cell;
      rt.effects.push(fn);
      const changed =
        !cell.deps ||
        !deps ||
        cell.deps.length !== deps.length ||
        deps.some((d, k) => !Object.is(d, cell.deps?.[k]));
      if (changed) {
        rt.queue.push(() => {
          (cell.cleanup as (() => void) | undefined)?.();
          cell.cleanup = fn();
          cell.deps = deps;
        });
      }
    },
  };
});

const winMock = vi.hoisted(() => ({
  focusHandlers: [] as Array<(event: { payload: boolean }) => void>,
  focusUnlistenCalls: 0,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onFocusChanged: (handler: (event: { payload: boolean }) => void) => {
      winMock.focusHandlers.push(handler);
      return Promise.resolve(() => {
        winMock.focusUnlistenCalls++;
        winMock.focusHandlers = winMock.focusHandlers.filter((item) => item !== handler);
      });
    },
  }),
}));

const ipcMock = vi.hoisted(() => ({
  captureBlocks: vi.fn<() => Promise<CaptureView>>(),
  closeOverlay: vi.fn<(generation?: number) => Promise<void>>(),
  updateHandlers: [] as Array<(view: CaptureView) => void>,
  updateUnlistenCalls: 0,
}));

vi.mock("../../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/ipc")>();
  return {
    ...actual,
    api: {
      ...actual.api,
      captureBlocks: ipcMock.captureBlocks,
      closeOverlay: ipcMock.closeOverlay,
    },
    onCaptureUpdate: (handler: (view: CaptureView) => void) => {
      ipcMock.updateHandlers.push(handler);
      return Promise.resolve(() => {
        ipcMock.updateUnlistenCalls++;
        ipcMock.updateHandlers = ipcMock.updateHandlers.filter((item) => item !== handler);
      });
    },
  };
});

import { OverlayView, TranslateOverlay, sanitizeBounds } from "./TranslateOverlay";

type El = ReactElement<{
  onClick?: () => void;
  children?: unknown;
  [k: string]: unknown;
}>;

const windowState = {
  innerWidth: 800,
  innerHeight: 600,
  listeners: new Map<string, Set<(ev: unknown) => void>>(),
  nextTimerId: 1,
  clearedTimers: [] as number[],
};

function setupMockGlobals() {
  windowState.innerWidth = 800;
  windowState.innerHeight = 600;
  windowState.listeners.clear();
  windowState.nextTimerId = 1;
  windowState.clearedTimers = [];

  (globalThis as Record<string, unknown>).window = {
    get innerWidth() {
      return windowState.innerWidth;
    },
    set innerWidth(v: number) {
      windowState.innerWidth = v;
    },
    get innerHeight() {
      return windowState.innerHeight;
    },
    set innerHeight(v: number) {
      windowState.innerHeight = v;
    },
    addEventListener: (type: string, fn: (ev: unknown) => void) => {
      const set = windowState.listeners.get(type) ?? new Set();
      set.add(fn);
      windowState.listeners.set(type, set);
    },
    removeEventListener: (type: string, fn: (ev: unknown) => void) => {
      windowState.listeners.get(type)?.delete(fn);
    },
    setTimeout: (_fn: () => void, _ms: number) => windowState.nextTimerId++,
    clearTimeout: (id: number) => {
      windowState.clearedTimers.push(id);
    },
  };
  (globalThis as Record<string, unknown>).document = {
    createElement: () => ({ getContext: () => null }),
  };
}

function resetHooks() {
  rt.hooks = [];
  rt.idx = 0;
  rt.effects = [];
  rt.queue = [];
  rt.renders = 0;
}

function renderOverlay(commitEffects = true): El {
  rt.idx = 0;
  rt.effects = [];
  rt.queue = [];
  rt.renders++;
  const el = TranslateOverlay() as El;
  if (commitEffects) {
    for (const run of rt.queue.splice(0)) run();
  }
  return el;
}

function findOverlayViewProps(root: El): {
  blocks: OverlayBlock[] | null;
  error: string | null;
  notice: string | null;
  stage: "ocr" | "translating" | "done";
  hint: boolean;
  bounds?: { w: number; h: number } | null;
} {
  const boundary = root.props.children as El;
  const view = boundary.props.children as El;
  return view.props as ReturnType<typeof findOverlayViewProps>;
}

const flush = async () => {
  await Promise.resolve();
  await Promise.resolve();
};

const sampleBlock = (overrides: Partial<OverlayBlock> = {}): OverlayBlock => ({
  text: "新建任务",
  x: 10,
  y: 20,
  w: 80,
  h: 16,
  line_h: 16,
  translated: true,
  cover: { x: 8, y: 18, w: 84, h: 20 },
  ink: { x: 10, y: 20, w: 78, h: 14 },
  ...overrides,
});

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-11-13T10:00:00Z"));
  resetHooks();
  setupMockGlobals();
  winMock.focusHandlers = [];
  winMock.focusUnlistenCalls = 0;
  ipcMock.captureBlocks.mockReset();
  ipcMock.closeOverlay.mockReset();
  ipcMock.closeOverlay.mockResolvedValue();
  ipcMock.updateHandlers = [];
  ipcMock.updateUnlistenCalls = 0;
});

describe("TranslateOverlay 运行时状态机与扩缩窗回归测试", () => {
  it("StrictMode 双挂载与卸载：清理全部监听与定时器，不留重复订阅", async () => {
    ipcMock.captureBlocks.mockResolvedValue({
      blocks: [],
      error: null,
      notice: null,
      stage: "ocr",
      generation: 1,
      bounds: { w: 164, h: 40 },
    });

    // 手动模拟 StrictMode：effect 跑一次 → 立即 cleanup → 再跑一次
    rt.idx = 0;
    TranslateOverlay();
    const firstEffect = rt.effects[0];
    const firstCleanup = firstEffect();
    expect(typeof firstCleanup).toBe("function");
    (firstCleanup as () => void)();
    await flush();
    expect(ipcMock.updateUnlistenCalls).toBe(1);
    expect(winMock.focusUnlistenCalls).toBe(1);
    expect(windowState.listeners.get("keydown")?.size ?? 0).toBe(0);
    expect(windowState.clearedTimers.length).toBe(1);

    const secondCleanup = firstEffect();
    await flush();
    expect(ipcMock.updateHandlers.length).toBe(1);
    expect(winMock.focusHandlers.length).toBe(1);
    expect(windowState.listeners.get("keydown")?.size ?? 0).toBe(1);

    (secondCleanup as () => void)();
    expect(ipcMock.updateUnlistenCalls).toBe(2);
    expect(winMock.focusUnlistenCalls).toBe(2);
    expect(windowState.listeners.get("keydown")?.size ?? 0).toBe(0);
  });

  it("代数与阶段单调 + 推拉竞态：先收到 push 进阶视图后，迟到的 captureBlocks 与旧代推送不得倒退", async () => {
    let resolvePull!: (view: CaptureView) => void;
    ipcMock.captureBlocks.mockImplementation(
      () =>
        new Promise<CaptureView>((resolve) => {
          resolvePull = resolve;
        }),
    );

    renderOverlay();
    await flush();

    // 推送先到达：gen=3, translating, 已译 1 块
    ipcMock.updateHandlers[0]({
      blocks: [sampleBlock({ text: "第一块", translated: true })],
      error: null,
      notice: null,
      stage: "translating",
      generation: 3,
      bounds: { w: 164, h: 40 },
    });
    let props = findOverlayViewProps(renderOverlay());
    expect(props.stage).toBe("translating");
    expect(props.blocks?.map((b) => b.text)).toEqual(["第一块"]);

    // 迟到的首帧拉取回来了较早的 gen=3, stage=ocr, blocks=[]：由于已有同代或更晚推送，必须被忽略
    resolvePull({
      blocks: [],
      error: null,
      notice: null,
      stage: "ocr",
      generation: 3,
      bounds: { w: 164, h: 40 },
    });
    await flush();
    props = findOverlayViewProps(renderOverlay());
    expect(props.stage).toBe("translating");
    expect(props.blocks?.map((b) => b.text)).toEqual(["第一块"]);

    // 上一代 gen=2 的 done 推送也必须被忽略
    ipcMock.updateHandlers[0]({
      blocks: [sampleBlock({ text: "旧代脏数据" })],
      error: null,
      notice: null,
      stage: "done",
      generation: 2,
      bounds: { w: 200, h: 100 },
    });
    props = findOverlayViewProps(renderOverlay());
    expect(props.blocks?.map((b) => b.text)).toEqual(["第一块"]);

    // 新一代 gen=4 的 ocr 正常接管
    ipcMock.updateHandlers[0]({
      blocks: [],
      error: null,
      notice: null,
      stage: "ocr",
      generation: 4,
      bounds: { w: 164, h: 40 },
    });
    props = findOverlayViewProps(renderOverlay());
    expect(props.stage).toBe("ocr");
    expect(props.blocks).toEqual([]);
  });

  it("失焦保护期、Ocr 阶段防误关与 Esc/点击携带当前 generation", async () => {
    ipcMock.captureBlocks.mockResolvedValue({
      blocks: [],
      error: null,
      notice: null,
      stage: "ocr",
      generation: 7,
      bounds: { w: 164, h: 40 },
    });

    const root = renderOverlay();
    await flush();

    // Ocr 阶段即使超过 400ms 宽限期也不得因失焦关闭
    vi.advanceTimersByTime(600);
    winMock.focusHandlers[0]({ payload: false });
    expect(ipcMock.closeOverlay).not.toHaveBeenCalled();

    // 进入 done 并扩窗：刷新 400ms 宽限期，200ms 内触发失焦不得关闭
    ipcMock.updateHandlers[0]({
      blocks: [sampleBlock()],
      error: null,
      notice: null,
      stage: "done",
      generation: 7,
      bounds: { w: 300, h: 84 },
    });
    vi.advanceTimersByTime(200);
    winMock.focusHandlers[0]({ payload: false });
    expect(ipcMock.closeOverlay).not.toHaveBeenCalled();

    // 过了 400ms 宽限期后失焦，携带 generation=7 关闭
    vi.advanceTimersByTime(250);
    winMock.focusHandlers[0]({ payload: false });
    expect(ipcMock.closeOverlay).toHaveBeenCalledWith(7);

    // Esc 与点击都携带 generation=7
    const keydown = [...(windowState.listeners.get("keydown") ?? [])][0];
    keydown({ key: "Escape" });
    expect(ipcMock.closeOverlay).toHaveBeenCalledTimes(2);
    expect(ipcMock.closeOverlay).toHaveBeenLastCalledWith(7);

    root.props.onClick?.();
    expect(ipcMock.closeOverlay).toHaveBeenCalledTimes(3);
    expect(ipcMock.closeOverlay).toHaveBeenLastCalledWith(7);
  });

  it("P1-2 回归：window.innerWidth 滞留在 80x22 时，view.bounds={300,84} 与 resize 兜底都能避免遮盖裁成 0x0", async () => {
    windowState.innerWidth = 80;
    windowState.innerHeight = 22;

    const shifted = sampleBlock({
      x: 222,
      y: 64,
      w: 60,
      h: 16,
      cover: { x: 220, y: 62, w: 64, h: 20 },
      ink: { x: 222, y: 64, w: 58, h: 14 },
    });

    // 1) 通过 bounds 原子同步窗口目标尺寸（此时 window.innerWidth 仍旧是 80x22）
    resetHooks();
    rt.idx = 0;
    const withBoundsEl = OverlayView({
      blocks: [shifted],
      error: "超时",
      notice: null,
      stage: "done",
      hint: false,
      bounds: { w: 300, h: 84 },
    }) as El;
    const withBoundsCover = (withBoundsEl.props.children as El[])[0].props as {
      plans: Array<{ cover: { w: number; h: number } }>;
    };
    expect(withBoundsCover.plans[0].cover.w).toBeGreaterThan(0);
    expect(withBoundsCover.plans[0].cover.h).toBeGreaterThan(0);

    // 2) 不传 bounds 时退回 window + resize 监听：80x22 先被裁到边缘，触发 resize 为 300x84 后立刻重排恢复
    resetHooks();
    windowState.listeners.clear();
    rt.idx = 0;
    const staleEl = OverlayView({
      blocks: [shifted],
      error: "超时",
      notice: null,
      stage: "done",
      hint: false,
      bounds: null,
    }) as El;
    for (const run of rt.queue.splice(0)) run();
    expect(windowState.listeners.get("resize")?.size ?? 0).toBe(1);
    const staleCover = (staleEl.props.children as El[])[0].props as {
      plans: Array<{ cover: { w: number; h: number } }>;
    };
    expect(staleCover.plans[0].cover.w).toBe(0);

    windowState.innerWidth = 300;
    windowState.innerHeight = 84;
    for (const listener of windowState.listeners.get("resize") ?? []) {
      listener({});
    }
    rt.idx = 0;
    const resizedEl = OverlayView({
      blocks: [shifted],
      error: "超时",
      notice: null,
      stage: "done",
      hint: false,
      bounds: null,
    }) as El;
    const resizedCover = (resizedEl.props.children as El[])[0].props as {
      plans: Array<{ cover: { w: number; h: number } }>;
    };
    expect(resizedCover.plans[0].cover.w).toBeGreaterThan(0);
    expect(resizedCover.plans[0].cover.h).toBeGreaterThan(0);

    // sanitizeBounds 过滤非法值
    expect(sanitizeBounds({ w: 300, h: 84 })).toEqual({ w: 300, h: 84 });
    expect(sanitizeBounds({ w: 0, h: 84 })).toBeNull();
    expect(sanitizeBounds({ w: Number.NaN, h: 84 })).toBeNull();
    expect(sanitizeBounds(null)).toBeNull();
  });
});
