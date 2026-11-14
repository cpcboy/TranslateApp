// 在 node 环境用轻量 hooks 运行时驱动真实的 TranslateOverlay / OverlayView，
// 验证 StrictMode 清理、代数/修订/阶段单调与推拉竞态、工具条重译/切换原文/标注/保存/关闭、
// 以及扩缩窗期间 bounds 同步与 resize 重排。
import type { ReactElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CaptureView, OverlayBlock, SaveImageKind, SaveImageResult } from "../../lib/ipc";

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

const ipcMock = vi.hoisted(() => ({
  captureBlocks: vi.fn<() => Promise<CaptureView>>(),
  captureOriginalImage: vi.fn<() => Promise<string | null>>(),
  setCaptureHitRects: vi.fn<(rects: unknown, lock?: boolean, gen?: number) => Promise<void>>(),
  retranslateCapture: vi.fn<(target: string) => Promise<void>>(),
  saveCaptureImage: vi.fn<(kind: SaveImageKind, pngBase64: string | null) => Promise<SaveImageResult>>(),
  closeOverlay: vi.fn<(generation?: number) => Promise<void>>(),
  updateHandlers: [] as Array<(view: CaptureView) => void>,
  updateUnlistenCalls: 0,
}));

const exportMock = vi.hoisted(() => ({
  renderCapturePngBase64: vi.fn<(options: unknown) => Promise<string>>(),
}));

vi.mock("../../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/ipc")>();
  return {
    ...actual,
    api: {
      ...actual.api,
      captureBlocks: ipcMock.captureBlocks,
      captureOriginalImage: ipcMock.captureOriginalImage,
      setCaptureHitRects: ipcMock.setCaptureHitRects,
      retranslateCapture: ipcMock.retranslateCapture,
      saveCaptureImage: ipcMock.saveCaptureImage,
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

vi.mock("./captureExport", () => ({
  renderCapturePngBase64: exportMock.renderCapturePngBase64,
}));

import { CaptureToolbar } from "./CaptureToolbar";
import { OverlayView, TranslateOverlay, sanitizeBounds, sanitizeImageRect } from "./TranslateOverlay";

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

function findElement(node: unknown, predicate: (el: El) => boolean): El | null {
  if (!node || typeof node !== "object") return null;
  if (Array.isArray(node)) {
    for (const child of node) {
      const found = findElement(child, predicate);
      if (found) return found;
    }
    return null;
  }
  const el = node as El;
  if ("type" in el && predicate(el)) return el;
  if (el.props && "children" in el.props) {
    return findElement(el.props.children, predicate);
  }
  return null;
}

function findOverlayViewProps(root: El): {
  blocks: OverlayBlock[] | null;
  error: string | null;
  notice: string | null;
  stage: "ocr" | "translating" | "done";
  hint: boolean;
  bounds?: { w: number; h: number } | null;
  hideTranslation?: boolean;
  hideFloatingEsc?: boolean;
} {
  const el = findElement(root, (item) => item.type === OverlayView);
  if (!el) throw new Error("OverlayView not found");
  return el.props as ReturnType<typeof findOverlayViewProps>;
}

function findToolbarProps(root: El): Record<string, unknown> | null {
  const el = findElement(root, (item) => item.type === CaptureToolbar);
  return el ? (el.props as Record<string, unknown>) : null;
}

const flush = async () => {
  await Promise.resolve();
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
  ipcMock.captureBlocks.mockReset();
  ipcMock.captureOriginalImage.mockReset();
  ipcMock.captureOriginalImage.mockResolvedValue("data:image/png;base64,iVBORw0KGgo=");
  ipcMock.setCaptureHitRects.mockReset();
  ipcMock.setCaptureHitRects.mockResolvedValue();
  ipcMock.retranslateCapture.mockReset();
  ipcMock.retranslateCapture.mockResolvedValue();
  ipcMock.saveCaptureImage.mockReset();
  ipcMock.saveCaptureImage.mockResolvedValue({ saved: true, path: "/tmp/out.png" });
  ipcMock.closeOverlay.mockReset();
  ipcMock.closeOverlay.mockResolvedValue();
  ipcMock.updateHandlers = [];
  ipcMock.updateUnlistenCalls = 0;
  exportMock.renderCapturePngBase64.mockReset();
  exportMock.renderCapturePngBase64.mockResolvedValue("RAW_PNG_BASE64");
});

describe("TranslateOverlay 运行时状态机与工具条回归测试", () => {
  it("StrictMode 双挂载与卸载：清理全部监听与定时器，不留重复订阅", async () => {
    ipcMock.captureBlocks.mockResolvedValue({
      blocks: [],
      error: null,
      notice: null,
      stage: "ocr",
      generation: 1,
      revision: 1,
      bounds: { w: 164, h: 40 },
    });

    rt.idx = 0;
    TranslateOverlay();
    const firstCleanups = rt.effects.map((eff) => eff());
    for (const cleanup of firstCleanups) {
      expect(typeof cleanup).toBe("function");
      (cleanup as () => void)();
    }
    await flush();
    expect(ipcMock.updateUnlistenCalls).toBe(1);
    expect(windowState.listeners.get("resize")?.size ?? 0).toBe(0);
    expect(windowState.listeners.get("keydown")?.size ?? 0).toBe(0);
    expect(windowState.clearedTimers.length).toBe(1);

    const secondCleanups = rt.effects.map((eff) => eff());
    await flush();
    expect(ipcMock.updateHandlers.length).toBe(1);
    expect(windowState.listeners.get("resize")?.size ?? 0).toBe(1);
    expect(windowState.listeners.get("keydown")?.size ?? 0).toBe(1);

    for (const cleanup of secondCleanups) {
      (cleanup as () => void)();
    }
    expect(ipcMock.updateUnlistenCalls).toBe(2);
    expect(windowState.listeners.get("resize")?.size ?? 0).toBe(0);
    expect(windowState.listeners.get("keydown")?.size ?? 0).toBe(0);
  });

  it("代数、修订号与阶段单调 + 推拉竞态：旧 revision / 旧 generation 推送与迟到拉取均不倒退", async () => {
    let resolvePull!: (view: CaptureView) => void;
    ipcMock.captureBlocks.mockImplementation(
      () =>
        new Promise<CaptureView>((resolve) => {
          resolvePull = resolve;
        }),
    );

    renderOverlay();
    await flush();

    // gen=3, rev=1, translating 先到达
    ipcMock.updateHandlers[0]({
      blocks: [sampleBlock({ text: "第一块", translated: true })],
      error: null,
      notice: null,
      stage: "translating",
      generation: 3,
      revision: 1,
      bounds: { w: 480, h: 160 },
      image_rect: { x: 40, y: 0, w: 400, h: 100 },
      has_image: true,
    });
    let props = findOverlayViewProps(renderOverlay());
    expect(props.stage).toBe("translating");
    expect(props.blocks?.map((b) => b.text)).toEqual(["第一块"]);
    expect(ipcMock.captureOriginalImage).toHaveBeenCalledTimes(1);

    // 迟到拉取返回 gen=3, rev=1, stage=ocr：不得倒退
    resolvePull({
      blocks: [],
      error: null,
      notice: null,
      stage: "ocr",
      generation: 3,
      revision: 1,
      bounds: { w: 480, h: 160 },
    });
    await flush();
    props = findOverlayViewProps(renderOverlay());
    expect(props.stage).toBe("translating");
    expect(props.blocks?.map((b) => b.text)).toEqual(["第一块"]);

    // 切换目标语言后 rev=2 的 done 到达
    ipcMock.updateHandlers[0]({
      blocks: [sampleBlock({ text: "新しいタスク", translated: true })],
      error: null,
      notice: null,
      stage: "done",
      generation: 3,
      revision: 2,
      target_lang: "ja",
      bounds: { w: 480, h: 160 },
      image_rect: { x: 40, y: 0, w: 400, h: 100 },
      has_image: true,
    });
    props = findOverlayViewProps(renderOverlay());
    expect(props.stage).toBe("done");
    expect(props.blocks?.map((b) => b.text)).toEqual(["新しいタスク"]);

    // 旧 rev=1 的 done 迟到，即使 stage=done 也不得覆盖 rev=2
    ipcMock.updateHandlers[0]({
      blocks: [sampleBlock({ text: "旧修订脏数据" })],
      error: null,
      notice: null,
      stage: "done",
      generation: 3,
      revision: 1,
      bounds: { w: 480, h: 160 },
    });
    props = findOverlayViewProps(renderOverlay());
    expect(props.blocks?.map((b) => b.text)).toEqual(["新しいタスク"]);

    // 新一代 gen=4 的 ocr 正常接管并重新拉原图
    ipcMock.updateHandlers[0]({
      blocks: [],
      error: null,
      notice: null,
      stage: "ocr",
      generation: 4,
      revision: 1,
      has_image: true,
      bounds: { w: 480, h: 160 },
    });
    await flush();
    props = findOverlayViewProps(renderOverlay());
    expect(props.stage).toBe("ocr");
    expect(props.blocks).toEqual([]);
    expect(ipcMock.captureOriginalImage).toHaveBeenCalledTimes(2);
  });

  it("点击画布不关闭，按 Esc 依次收起文字草稿/弹层再关窗，工具条关闭按钮立即携带 generation 退出", async () => {
    ipcMock.captureBlocks.mockResolvedValue({
      blocks: [sampleBlock()],
      error: null,
      notice: null,
      stage: "done",
      generation: 7,
      revision: 1,
      has_image: true,
      target_lang: "zh",
      bounds: { w: 480, h: 180 },
      image_rect: { x: 40, y: 8, w: 400, h: 100 },
    });

    let root = renderOverlay();
    await flush();
    root = renderOverlay();

    // 单击背景不能关窗（用户需要在画布上标注和点工具条）
    root.props.onClick?.();
    expect(ipcMock.closeOverlay).not.toHaveBeenCalled();

    // 打开语言弹层后按 Esc：先关弹层不退出
    const toolbar = findToolbarProps(root)!;
    expect(toolbar).not.toBeNull();
    (toolbar.onTogglePopover as (p: "lang") => void)("lang");
    root = renderOverlay();
    const keydown = [...(windowState.listeners.get("keydown") ?? [])][0];
    keydown({ key: "Escape" });
    expect(ipcMock.closeOverlay).not.toHaveBeenCalled();

    // 弹层已收起，再按 Esc 才关闭 generation=7
    root = renderOverlay();
    keydown({ key: "Escape" });
    expect(ipcMock.closeOverlay).toHaveBeenCalledTimes(1);
    expect(ipcMock.closeOverlay).toHaveBeenLastCalledWith(7);

    // 点工具条「关闭」同样携带 generation=7
    const toolbarAgain = findToolbarProps(root)!;
    (toolbarAgain.onClose as () => void)();
    expect(ipcMock.closeOverlay).toHaveBeenCalledTimes(2);
    expect(ipcMock.closeOverlay).toHaveBeenLastCalledWith(7);
  });

  it("工具条完整闭环：切换原文/结果、切换目标语言重译、画标注与撤销、分别导出原图与译后图", async () => {
    ipcMock.captureBlocks.mockResolvedValue({
      blocks: [sampleBlock()],
      error: null,
      notice: null,
      stage: "done",
      generation: 9,
      revision: 1,
      has_image: true,
      target_lang: "zh",
      scale: 2,
      bounds: { w: 500, h: 200 },
      image_rect: { x: 0, y: 0, w: 500, h: 140 },
    });

    let root = renderOverlay();
    await flush();
    root = renderOverlay();

    // 1) 冻结原图已加载，切到显示原文 -> hideTranslation=true，切回显示结果 -> hideTranslation=false
    expect(findElement(root, (el) => el.props["data-testid"] === "capture-original-image")).not.toBeNull();
    (findToolbarProps(root)!.onToggleOriginal as (v: boolean) => void)(true);
    root = renderOverlay();
    expect(findOverlayViewProps(root).hideTranslation).toBe(true);
    (findToolbarProps(root)!.onToggleOriginal as (v: boolean) => void)(false);
    root = renderOverlay();
    expect(findOverlayViewProps(root).hideTranslation).toBe(false);

    // 2) 选择目标语言 -> 调 retranslateCapture("ja") 且工具条进入 retranslating，切入 translating 流式期冻结原图保持挂载不透底，收到 done 后清除
    (findToolbarProps(root)!.onSelectLang as (code: string) => void)("ja");
    root = renderOverlay();
    expect(ipcMock.retranslateCapture).toHaveBeenCalledWith("ja");
    expect(findToolbarProps(root)!.retranslating).toBe(true);
    ipcMock.updateHandlers[0]({
      blocks: [sampleBlock({ text: "新規タスク", translated: true })],
      error: null,
      notice: null,
      stage: "translating",
      generation: 9,
      revision: 2,
      target_lang: "ja",
      has_image: true,
      bounds: { w: 500, h: 200 },
      image_rect: { x: 0, y: 0, w: 500, h: 140 },
    });
    root = renderOverlay();
    expect(findElement(root, (el) => el.props["data-testid"] === "capture-original-image")).not.toBeNull();
    ipcMock.updateHandlers[0]({
      blocks: [sampleBlock({ text: "新規タスク" })],
      error: null,
      notice: null,
      stage: "done",
      generation: 9,
      revision: 2,
      target_lang: "ja",
      has_image: true,
      bounds: { w: 500, h: 200 },
      image_rect: { x: 0, y: 0, w: 500, h: 140 },
    });
    root = renderOverlay();
    expect(findToolbarProps(root)!.retranslating).toBe(false);
    expect(findToolbarProps(root)!.targetLang).toBe("ja");

    // 3) 选中「矩形」在标注 SVG 上拖拽一条矩形标注，再用「撤销」撤回
    (findToolbarProps(root)!.onSelectTool as (tool: "rect") => void)("rect");
    root = renderOverlay();
    const svg = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
    expect(svg).not.toBeNull();
    (svg.props.onPointerDown as (e: unknown) => void)({
      button: 0,
      pointerId: 1,
      clientX: 10,
      clientY: 12,
      stopPropagation: () => {},
      currentTarget: {},
    });
    root = renderOverlay();
    const svgMoving = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
    (svgMoving.props.onPointerMove as (e: unknown) => void)({ clientX: 80, clientY: 60 });
    root = renderOverlay();
    const svgUp = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
    (svgUp.props.onPointerUp as () => void)();
    root = renderOverlay();
    expect(findToolbarProps(root)!.annotationsCount).toBe(1);

    // 4) 分别保存未翻译原图（null 载荷走 Rust 直接复制原生 PNG）与翻译后图片（带 Canvas 合成 Base64）
    await (findToolbarProps(root)!.onSave as (kind: SaveImageKind) => Promise<void>)("original");
    expect(ipcMock.saveCaptureImage).toHaveBeenLastCalledWith("original", null);

    await (findToolbarProps(root)!.onSave as (kind: SaveImageKind) => Promise<void>)("translated");
    expect(exportMock.renderCapturePngBase64).toHaveBeenCalledTimes(1);
    expect(ipcMock.saveCaptureImage).toHaveBeenLastCalledWith("translated", "RAW_PNG_BASE64");

    // 撤销清空标注
    (findToolbarProps(root)!.onUndo as () => void)();
    root = renderOverlay();
    expect(findToolbarProps(root)!.annotationsCount).toBe(0);
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

    expect(sanitizeBounds({ w: 300, h: 84 })).toEqual({ w: 300, h: 84 });
    expect(sanitizeBounds({ w: 0, h: 84 })).toBeNull();
    expect(sanitizeBounds({ w: Number.NaN, h: 84 })).toBeNull();
    expect(sanitizeBounds(null)).toBeNull();
    expect(sanitizeImageRect({ x: 10, y: 0, w: 300, h: 120 })).toEqual({ x: 10, y: 0, w: 300, h: 120 });
    expect(sanitizeImageRect({ x: -1, y: 0, w: 300, h: 120 })).toBeNull();
  });

  it("极小选区 60x24 扩窗后：状态/报错浮层不被裁剪到 60x24，且语言下拉传出适配窗口余高的 popoverMaxHeight", async () => {
    ipcMock.captureBlocks.mockResolvedValue({
      blocks: [],
      error: "超时",
      notice: null,
      stage: "done",
      generation: 12,
      revision: 1,
      has_image: true,
      bounds: { w: 580, h: 292 },
      image_rect: { x: 40, y: 0, w: 60, h: 24 },
    });
    let root = renderOverlay();
    await flush();
    root = renderOverlay();

    const chrome = findElement(root, (el) => el.props["data-testid"] === "capture-chrome")!;
    expect(chrome).not.toBeNull();
    const chromeStyle = chrome.props.style as { width: number; minHeight: number };
    expect(chromeStyle.width).toBeGreaterThanOrEqual(300);
    expect(chromeStyle.minHeight).toBeGreaterThanOrEqual(84);

    const toolbar = findToolbarProps(root)!;
    expect(typeof toolbar.popoverMaxHeight).toBe("number");
    expect(toolbar.popoverMaxHeight as number).toBeGreaterThanOrEqual(108);
    expect(toolbar.popoverMaxHeight as number).toBeLessThanOrEqual(220);
  });

  it("[R3-P1-2] 窄选区工具条水平正对选区居中，拖拽期间锁定指针且窗口失焦自动收起弹层", async () => {
    ipcMock.captureBlocks.mockResolvedValue({
      blocks: [sampleBlock()],
      error: null,
      notice: null,
      stage: "done",
      generation: 15,
      revision: 1,
      has_image: true,
      bounds: { w: 580, h: 348 },
      image_rect: { x: 190, y: 0, w: 200, h: 80 },
    });

    let root = renderOverlay();
    await flush();
    root = renderOverlay();

    // 工具条水平正对 image_rect (center=290) 居中于 580px 窗口，而不是左偏移到 x=190
    const anchor = findElement(root, (el) => el.props["data-testid"] === "capture-toolbar-anchor")!;
    expect(anchor).not.toBeNull();
    expect((anchor.props.style as { left: number }).left).toBe(10);

    // 初始 hit_rects 已同步且未锁指针
    expect(ipcMock.setCaptureHitRects).toHaveBeenCalled();
    const initialHitCall = ipcMock.setCaptureHitRects.mock.calls[ipcMock.setCaptureHitRects.mock.calls.length - 1] as unknown[];
    expect(initialHitCall[1]).toBe(false);
    expect(initialHitCall[2]).toBe(15);

    // 展开语言菜单后触发 window blur -> 自动收起弹窗（配合透明区鼠标穿透切回桌面）
    (findToolbarProps(root)!.onTogglePopover as (next: "lang" | "save" | "none") => void)("lang");
    root = renderOverlay();
    expect(findToolbarProps(root)!.openPopover).toBe("lang");
    for (const listener of windowState.listeners.get("blur") ?? []) {
      listener({});
    }
    root = renderOverlay();
    expect(findToolbarProps(root)!.openPopover).toBe("none");

    // 拖拽矩形期间 pointer_lock=true，松手后恢复 false
    (findToolbarProps(root)!.onSelectTool as (tool: "rect") => void)("rect");
    root = renderOverlay();
    const svg = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
    (svg.props.onPointerDown as (e: unknown) => void)({
      button: 0,
      pointerId: 1,
      clientX: 210,
      clientY: 20,
      stopPropagation: () => {},
      currentTarget: {},
    });
    root = renderOverlay();
    const callsAtDragStart = ipcMock.setCaptureHitRects.mock.calls.length;
    const lockedCall = ipcMock.setCaptureHitRects.mock.calls[callsAtDragStart - 1] as unknown[];
    expect(lockedCall[1]).toBe(true);

    // [R4-P2-1] 连续 pointerMove 不改变 isDraggingAnnotation 布尔态，不得触发高频 setCaptureHitRects IPC
    const svgMove1 = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
    (svgMove1.props.onPointerMove as (e: unknown) => void)({ clientX: 230, clientY: 30 });
    root = renderOverlay();
    const svgMove2 = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
    (svgMove2.props.onPointerMove as (e: unknown) => void)({ clientX: 250, clientY: 45 });
    root = renderOverlay();
    expect(ipcMock.setCaptureHitRects.mock.calls.length).toBe(callsAtDragStart);

    const svgUp = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
    (svgUp.props.onPointerUp as () => void)();
    root = renderOverlay();
    const unlockedCall = ipcMock.setCaptureHitRects.mock.calls[ipcMock.setCaptureHitRects.mock.calls.length - 1] as unknown[];
    expect(unlockedCall[1]).toBe(false);
  });

  it("[R3-P1-3] 文字标注边缘夹逼不跳变、切色保活焦点不提前落盘、点击保存先自动落盘再合成", async () => {
    ipcMock.captureBlocks.mockResolvedValue({
      blocks: [sampleBlock()],
      error: null,
      notice: null,
      stage: "done",
      generation: 16,
      revision: 1,
      has_image: true,
      scale: 2,
      bounds: { w: 500, h: 200 },
      image_rect: { x: 0, y: 0, w: 500, h: 140 },
    });

    let root = renderOverlay();
    await flush();
    root = renderOverlay();

    // 1) 在靠近右下角 (492, 134) 启动文字标注，输入时切换二级属性条颜色与字号应保持编辑态不失焦
    (findToolbarProps(root)!.onSelectTool as (tool: "text") => void)("text");
    root = renderOverlay();
    const svg = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
    (svg.props.onPointerDown as (e: unknown) => void)({
      button: 0,
      pointerId: 1,
      clientX: 492,
      clientY: 134,
      stopPropagation: () => {},
      currentTarget: {},
    });
    root = renderOverlay();

    const textarea = findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!;
    expect(textarea).not.toBeNull();
    const areaStyle = textarea.props.style as { left: number; top: number };
    expect(areaStyle.left).toBe(390);
    // 默认中号 18px 时 lineHeight=23, boxHeight=29 -> top=140-29=111
    expect(areaStyle.top).toBe(111);

    // 验证二级属性条的颜色/字号按钮均在 onMouseDown 调用 preventDefault，防止浏览器先触发 textarea.onBlur
    const toolbarEl = CaptureToolbar(findToolbarProps(root) as Parameters<typeof CaptureToolbar>[0]) as El;
    const orangeBtn = findElement(toolbarEl, (el) => el.props["aria-label"] === "颜色橙")!;
    const largeBtn = findElement(toolbarEl, (el) => el.props["aria-label"] === "粗细粗")!;
    const prevent1 = vi.fn();
    const prevent2 = vi.fn();
    (orangeBtn.props.onMouseDown as (e: { preventDefault: () => void }) => void)({ preventDefault: prevent1 });
    (largeBtn.props.onMouseDown as (e: { preventDefault: () => void }) => void)({ preventDefault: prevent2 });
    expect(prevent1).toHaveBeenCalledTimes(1);
    expect(prevent2).toHaveBeenCalledTimes(1);

    (findToolbarProps(root)!.onSelectColor as (hex: string) => void)("#ff9500");
    (findToolbarProps(root)!.onSelectSize as (id: "sm" | "md" | "lg") => void)("lg");
    root = renderOverlay();

    // 仍在编辑态中，输入文字且尚未失焦，直接点击「保存翻译后图片」
    const editingArea = findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!;
    expect(editingArea).not.toBeNull();
    (editingArea.props.onChange as (e: { target: { value: string } }) => void)({ target: { value: "重要提示" } });
    root = renderOverlay();

    await (findToolbarProps(root)!.onSave as (kind: SaveImageKind) => Promise<void>)("translated");
    root = renderOverlay();

    // 保存时自动把草稿用新颜色/粗号落盘，且落盘坐标精确对齐 textarea 内部文本偏移 (left+6, top+3)
    expect(exportMock.renderCapturePngBase64).toHaveBeenCalledTimes(1);
    const exportArgs = exportMock.renderCapturePngBase64.mock.calls[0]![0] as {
      annotations: Array<{ kind: string; x: number; y: number; text: string; color: string; fontSize: number }>;
    };
    expect(exportArgs.annotations).toHaveLength(1);
    expect(exportArgs.annotations[0]).toMatchObject({
      kind: "text",
      text: "重要提示",
      color: "#ff9500",
      fontSize: 24,
      x: 396,
      // 粗号 24px 时 lineHeight=30, boxHeight=36 -> top=104, textY=107
      y: 107,
    });

    // 保存后再迟到的 onBlur 不得重复提交一条相同的文字标注
    (editingArea.props.onBlur as () => void)();
    root = renderOverlay();
    expect(findToolbarProps(root)!.annotationsCount).toBe(1);
  });

  it("[R3-P2-1] 原图初次返回 null 时允许后续按需补拉，仍取不到时报错并拦截黑底保存，菜单项同步禁用", async () => {
    // 1) 工具条在 hasImage=false 时三个保存菜单项均禁用
    ipcMock.captureBlocks.mockResolvedValue({
      blocks: [sampleBlock()],
      error: null,
      notice: null,
      stage: "done",
      generation: 17,
      revision: 1,
      has_image: false,
      bounds: { w: 500, h: 200 },
      image_rect: { x: 0, y: 0, w: 500, h: 140 },
    });
    ipcMock.captureOriginalImage.mockResolvedValue(null);

    let root = renderOverlay();
    await flush();
    root = renderOverlay();

    (findToolbarProps(root)!.onTogglePopover as (next: "save") => void)("save");
    root = renderOverlay();
    const disabledToolbar = CaptureToolbar(findToolbarProps(root) as Parameters<typeof CaptureToolbar>[0]) as El;
    expect(findElement(disabledToolbar, (el) => el.props.children === "保存未翻译原图")!.props.disabled).toBe(true);
    expect(findElement(disabledToolbar, (el) => el.props.children === "保存翻译后图片")!.props.disabled).toBe(true);

    // 点击「保存翻译后图片」按需拉取原图仍返回 null -> 展示错误且不导出黑底、不弹窗
    await (findToolbarProps(root)!.onSave as (kind: SaveImageKind) => Promise<void>)("translated");
    root = renderOverlay();
    expect(findOverlayViewProps(root).error).toContain("原图尚未就绪");
    expect(exportMock.renderCapturePngBase64).not.toHaveBeenCalled();
    expect(ipcMock.saveCaptureImage).not.toHaveBeenCalled();

    // 2) 后续原图就绪后，再次点击「保存翻译后图片」不被 loadedImageGenRef 锁死，可按需拉取并成功保存
    ipcMock.captureOriginalImage.mockResolvedValueOnce("data:image/png;base64,RECOVERED");
    await (findToolbarProps(root)!.onSave as (kind: SaveImageKind) => Promise<void>)("translated");
    root = renderOverlay();
    expect(ipcMock.captureOriginalImage).toHaveBeenCalledTimes(2);
    expect(exportMock.renderCapturePngBase64).toHaveBeenCalledWith(
      expect.objectContaining({ originalImageUrl: "data:image/png;base64,RECOVERED" }),
    );
    expect(ipcMock.saveCaptureImage).toHaveBeenCalledWith("translated", "RAW_PNG_BASE64");
  });
});
