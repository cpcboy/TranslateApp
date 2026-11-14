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
  focusOverlay: vi.fn<(generation?: number) => Promise<void>>(),
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
      focusOverlay: ipcMock.focusOverlay,
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
  ipcMock.focusOverlay.mockReset();
  ipcMock.focusOverlay.mockResolvedValue();
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

  it("[R4-P2-1] 未按回车输入文字标注后直接点「存为图片」或「撤销」，均保持一致性不丢字", async () => {
    ipcMock.captureBlocks.mockResolvedValue({
      blocks: [sampleBlock()],
      error: null,
      notice: null,
      stage: "done",
      generation: 18,
      revision: 1,
      has_image: true,
      scale: 2,
      bounds: { w: 500, h: 200 },
      image_rect: { x: 0, y: 0, w: 500, h: 140 },
    });

    let root = renderOverlay();
    await flush();
    root = renderOverlay();

    // 1) 点击文字工具输入文字但不按 Enter，也不触发 onBlur
    (findToolbarProps(root)!.onSelectTool as (tool: "text") => void)("text");
    root = renderOverlay();
    const svg = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
    (svg.props.onPointerDown as (e: unknown) => void)({
      button: 0,
      pointerId: 1,
      clientX: 40,
      clientY: 30,
      stopPropagation: () => {},
      currentTarget: {},
    });
    root = renderOverlay();

    const textarea = findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!;
    (textarea.props.onChange as (e: { target: { value: string } }) => void)({
      target: { value: "此处文案有误" },
    });
    root = renderOverlay();

    // 尚未回车时「撤销」已可用（annotationsCount 含非空输入草稿）
    expect(findToolbarProps(root)!.annotationsCount).toBe(1);
    const liveToolbar = CaptureToolbar(findToolbarProps(root) as Parameters<typeof CaptureToolbar>[0]) as El;
    expect(findElement(liveToolbar, (el) => el.props["aria-label"] === "撤销")!.props.disabled).toBe(false);

    // 2) 直接点「存为图片」展开菜单：草稿自动落盘，保存菜单中可见「保存带标注原图」
    (findToolbarProps(root)!.onTogglePopover as (next: "save" | "lang" | "none") => void)("save");
    root = renderOverlay();
    expect(findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")).toBeNull();
    expect(findToolbarProps(root)!.annotationsCount).toBe(1);
    const saveMenuToolbar = CaptureToolbar(findToolbarProps(root) as Parameters<typeof CaptureToolbar>[0]) as El;
    const saveAnnotatedBtn = findElement(saveMenuToolbar, (el) => el.props.children === "保存带标注原图");
    expect(saveAnnotatedBtn).not.toBeNull();
    expect(saveAnnotatedBtn!.props.disabled).toBe(false);

    await (findToolbarProps(root)!.onSave as (kind: SaveImageKind) => Promise<void>)("original_annotated");
    root = renderOverlay();
    expect(exportMock.renderCapturePngBase64).toHaveBeenCalledWith(
      expect.objectContaining({
        includeTranslation: false,
        annotations: [expect.objectContaining({ kind: "text", text: "此处文案有误" })],
      }),
    );
    expect(ipcMock.saveCaptureImage).toHaveBeenCalledWith("original_annotated", "RAW_PNG_BASE64");

    // 3) 已有 1 条已提交标注时，新建第 2 条未回车文字草稿后点「撤销」，只撤掉未提交草稿而不误删第 1 条
    const svg2 = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
    (svg2.props.onPointerDown as (e: unknown) => void)({
      button: 0,
      pointerId: 2,
      clientX: 80,
      clientY: 60,
      stopPropagation: () => {},
      currentTarget: {},
    });
    root = renderOverlay();
    const textarea2 = findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!;
    (textarea2.props.onChange as (e: { target: { value: string } }) => void)({
      target: { value: "临时草稿" },
    });
    root = renderOverlay();
    expect(findToolbarProps(root)!.annotationsCount).toBe(2);

    (findToolbarProps(root)!.onUndo as () => void)();
    root = renderOverlay();
    expect(findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")).toBeNull();
    expect(findToolbarProps(root)!.annotationsCount).toBe(1);

    // 4) [R5-P2-1] 处于空白文字草稿时点「撤销」，只关闭空白输入框而不连带删除上一条已提交标注
    const svg3 = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
    (svg3.props.onPointerDown as (e: unknown) => void)({
      button: 0,
      pointerId: 3,
      clientX: 120,
      clientY: 70,
      stopPropagation: () => {},
      currentTarget: {},
    });
    root = renderOverlay();
    expect(findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")).not.toBeNull();
    (findToolbarProps(root)!.onUndo as () => void)();
    root = renderOverlay();
    expect(findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")).toBeNull();
    expect(findToolbarProps(root)!.annotationsCount).toBe(1);

    // 再次点「撤销」，此时才真正撤销上一条已提交标注
    (findToolbarProps(root)!.onUndo as () => void)();
    root = renderOverlay();
    expect(findToolbarProps(root)!.annotationsCount).toBe(0);
  });

  it("[FIX-TEXT-INPUT] 点击画布 preventDefault + 聚焦叠加窗 + 切换点击防旧失焦竞态 + IME/窗口失焦保护 + 刁钻输入实测", async () => {
    let appFocused = true;
    (globalThis as Record<string, unknown>).document = {
      createElement: () => ({ getContext: () => null }),
      hasFocus: () => appFocused,
    };

    ipcMock.captureBlocks.mockResolvedValue({
      blocks: [sampleBlock()],
      error: null,
      notice: null,
      stage: "done",
      generation: 33,
      revision: 1,
      has_image: true,
      scale: 2,
      bounds: { w: 800, h: 600 },
      image_rect: { x: 0, y: 0, w: 800, h: 600 },
    });

    let root = renderOverlay();
    await flush();
    root = renderOverlay();

    // 1) 选中文字工具时触发 focusOverlay
    (findToolbarProps(root)!.onSelectTool as (tool: "text") => void)("text");
    root = renderOverlay();
    expect(ipcMock.focusOverlay).toHaveBeenCalledWith(33);

    // 2) 在 A 点 (120, 90) 点击画布：必须调用 preventDefault() 阻断原生 mousedown 回抢焦点，再次调用 focusOverlay，
    // 并验证 svg.onMouseDown 与 CaptureToolbar 根容器（保护分隔缝隙/边距）均 preventDefault
    const svg1 = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
    const preventA = vi.fn();
    (svg1.props.onPointerDown as (e: unknown) => void)({
      button: 0,
      pointerId: 1,
      clientX: 120,
      clientY: 90,
      preventDefault: preventA,
      stopPropagation: () => {},
      currentTarget: {},
    });
    root = renderOverlay();
    expect(preventA).toHaveBeenCalledTimes(1);
    expect(ipcMock.focusOverlay).toHaveBeenCalledTimes(2);
    expect(ipcMock.focusOverlay).toHaveBeenLastCalledWith(33);

    const preventSvgMouseDown = vi.fn();
    ((findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!.props.onMouseDown) as (
      e: unknown,
    ) => void)({ button: 0, preventDefault: preventSvgMouseDown });
    expect(preventSvgMouseDown).toHaveBeenCalledTimes(1);

    const toolbarRoot = CaptureToolbar(findToolbarProps(root) as Parameters<typeof CaptureToolbar>[0]) as El;
    const preventToolbarWrap = vi.fn();
    (toolbarRoot.props.onMouseDown as (e: { preventDefault: () => void }) => void)({
      preventDefault: preventToolbarWrap,
    });
    expect(preventToolbarWrap).toHaveBeenCalledTimes(1);

    const taA = findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!;
    expect(taA).not.toBeNull();
    expect(taA.props["data-hit-rect"]).toBe("true");
    expect(findElement(root, (el) => el.props["data-testid"] === "annotation-text-placeholder")).not.toBeNull();

    // 挂载模拟 DOM node 验证聚焦管理与 window focus 恢复聚焦
    const mockTaDom = {
      value: "",
      dataset: { draftId: String(taA.props["data-draft-id"]) },
      focus: vi.fn(),
      setSelectionRange: vi.fn(),
    };
    const taRef = (taA.props.ref ?? (taA as unknown as { ref?: unknown }).ref) as
      | { current: unknown }
      | undefined;
    expect(taRef && typeof taRef === "object" && "current" in taRef).toBe(true);
    taRef!.current = mockTaDom;
    for (const fn of windowState.listeners.get("focus") ?? []) {
      fn({});
    }
    expect(mockTaDom.focus).toHaveBeenCalledWith({ preventScroll: true });

    // 输入 IME 组合中（isComposing / keyCode=229）按 Enter 和 Escape 不提交、不取消
    (taA.props.onCompositionStart as () => void)();
    root = renderOverlay();
    expect(findElement(root, (el) => el.props["data-testid"] === "annotation-text-placeholder")).toBeNull();

    const taADuringIme = findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!;
    (taADuringIme.props.onKeyDown as (e: unknown) => void)({
      key: "Enter",
      shiftKey: false,
      nativeEvent: { isComposing: true },
      preventDefault: () => {},
      stopPropagation: () => {},
    });
    for (const fn of windowState.listeners.get("keydown") ?? []) {
      fn({ key: "Escape", isComposing: true, keyCode: 229 });
    }
    root = renderOverlay();
    expect(findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")).not.toBeNull();

    // 窗口失焦（appFocused=false）触发 textarea onBlur 不销毁正在输入的草稿
    appFocused = false;
    (taADuringIme.props.onBlur as () => void)();
    root = renderOverlay();
    expect(findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")).not.toBeNull();
    appFocused = true;

    // 提交纯中文 + 引号 + 多空格文本（同步真实 DOM value）
    mockTaDom.value = '中文标注："引号"   三空格';
    (taADuringIme.props.onCompositionEnd as (e: unknown) => void)({
      currentTarget: mockTaDom,
    });
    root = renderOverlay();

    // 3) 直接点击新位置 B (340, 220)：自动提交 A，且旧草稿 A 迟到的 onBlur 不得销毁新草稿 B
    const svg2 = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
    (svg2.props.onPointerDown as (e: unknown) => void)({
      button: 0,
      pointerId: 2,
      clientX: 340,
      clientY: 220,
      preventDefault: () => {},
      stopPropagation: () => {},
      currentTarget: {},
    });
    root = renderOverlay();
    expect(findToolbarProps(root)!.annotationsCount).toBe(1);
    const taB = findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!;
    expect((taB.props.style as { left: number }).left).toBe(340);

    // 模拟旧输入框 A 迟到 onBlur：不得摧毁 B
    (taADuringIme.props.onBlur as () => void)();
    root = renderOverlay();
    const taBAfterStaleBlur = findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!;
    expect(taBAfterStaleBlur).not.toBeNull();
    expect((taBAfterStaleBlur.props.style as { left: number }).left).toBe(340);

    // 验证 AnnotationShape 渲染出的 SVG <text> 使用 whiteSpace: "pre" 保留连续空格
    const svgWithAnn = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
    const shapeHost = findElement(svgWithAnn, (el) => typeof el.type === "function" && "item" in el.props)!;
    const shapeEl = (shapeHost.type as (props: unknown) => El)(shapeHost.props);
    const svgText = findElement(shapeEl, (el) => el.type === "text")!;
    expect((shapeHost.props.item as { text: string }).text).toBe('中文标注："引号"   三空格');
    expect((svgText.props.style as { whiteSpace: string }).whiteSpace).toBe("pre");

    // 重复提交完全相同的中文文字（重名），并验证导出 PNG 收到两条独立 id 且都完整保留三空格
    (taBAfterStaleBlur.props.onChange as (e: unknown) => void)({ target: { value: '中文标注："引号"   三空格' } });
    root = renderOverlay();
    (findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!.props.onKeyDown as (
      e: unknown,
    ) => void)({
      key: "Enter",
      shiftKey: false,
      preventDefault: () => {},
      stopPropagation: () => {},
    });
    root = renderOverlay();
    expect(findToolbarProps(root)!.annotationsCount).toBe(2);

    await (findToolbarProps(root)!.onSave as (kind: SaveImageKind) => Promise<void>)("original_annotated");
    root = renderOverlay();
    expect(exportMock.renderCapturePngBase64).toHaveBeenCalledTimes(1);
    const dupExportArgs = exportMock.renderCapturePngBase64.mock.calls[0]![0] as {
      annotations: Array<{ id: string; text: string }>;
    };
    expect(dupExportArgs.annotations).toHaveLength(2);
    expect(dupExportArgs.annotations[0]!.id).not.toBe(dupExportArgs.annotations[1]!.id);
    expect(dupExportArgs.annotations[0]!.text).toBe('中文标注："引号"   三空格');
    expect(dupExportArgs.annotations[1]!.text).toBe('中文标注："引号"   三空格');

    // 新点一处后空着直接回车提交：不新增标注，仍维持 2 条
    const svgEmpty = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
    (svgEmpty.props.onPointerDown as (e: unknown) => void)({
      button: 0,
      pointerId: 3,
      clientX: 380,
      clientY: 250,
      preventDefault: () => {},
      stopPropagation: () => {},
      currentTarget: {},
    });
    root = renderOverlay();
    const taEmpty = findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!;
    (taEmpty.props.onChange as (e: unknown) => void)({ target: { value: "   " } });
    root = renderOverlay();
    (findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!.props.onKeyDown as (
      e: unknown,
    ) => void)({
      key: "Enter",
      shiftKey: false,
      preventDefault: () => {},
      stopPropagation: () => {},
    });
    root = renderOverlay();
    expect(findToolbarProps(root)!.annotationsCount).toBe(2);

    // 连续快速点击 6 个位置：不产生空标注、最终框稳定留在最后一个点击点 (450, 310)
    for (let i = 0; i < 6; i++) {
      const s = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
      (s.props.onPointerDown as (e: unknown) => void)({
        button: 0,
        pointerId: 10 + i,
        clientX: 200 + i * 50,
        clientY: 160 + i * 30,
        preventDefault: () => {},
        stopPropagation: () => {},
        currentTarget: {},
      });
      root = renderOverlay();
    }
    expect(findToolbarProps(root)!.annotationsCount).toBe(2);
    const finalTa = findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!;
    expect((finalTa.props.style as { left: number; top: number })).toMatchObject({ left: 450, top: 310 });
  });

  it("[FIX-TEXT-INPUT-EDGE] 旧草稿迟到 composition 不串扰、未重渲染即时失焦不丢字、切代重置 IME、Shift+Enter 换行与 Esc 取消", async () => {
    ipcMock.captureBlocks.mockResolvedValue({
      blocks: [sampleBlock()],
      error: null,
      notice: null,
      stage: "done",
      generation: 41,
      revision: 1,
      has_image: true,
      scale: 2,
      bounds: { w: 800, h: 600 },
      image_rect: { x: 0, y: 0, w: 800, h: 600 },
    });

    let root = renderOverlay();
    await flush();
    root = renderOverlay();

    (findToolbarProps(root)!.onSelectTool as (tool: "text") => void)("text");
    root = renderOverlay();

    // 1) 在 A 点开框并处于 IME 组合态，随后切去 B 点并在 B 开启组合后投递旧 A 迟到的 onCompositionEnd
    const svg1 = findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!;
    (svg1.props.onPointerDown as (e: unknown) => void)({
      button: 0,
      pointerId: 1,
      clientX: 100,
      clientY: 80,
      preventDefault: () => {},
      stopPropagation: () => {},
      currentTarget: {},
    });
    root = renderOverlay();
    const taA = findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!;
    (taA.props.onCompositionStart as () => void)();
    root = renderOverlay();

    (findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!.props.onPointerDown as (
      e: unknown,
    ) => void)({
      button: 0,
      pointerId: 2,
      clientX: 260,
      clientY: 160,
      preventDefault: () => {},
      stopPropagation: () => {},
      currentTarget: {},
    });
    root = renderOverlay();
    const taB = findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!;
    (taB.props.onCompositionStart as () => void)();
    root = renderOverlay();
    expect(findElement(root, (el) => el.props["data-testid"] === "annotation-text-placeholder")).toBeNull();

    // 旧 A 迟到触发 onCompositionEnd：因 draftId 校验不得清空 B 的 isComposingText 或覆写内容
    (taA.props.onCompositionEnd as (e: unknown) => void)({ currentTarget: { value: "旧草稿泄露" } });
    root = renderOverlay();
    expect(findElement(root, (el) => el.props["data-testid"] === "annotation-text-placeholder")).toBeNull();
    expect(findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!.props.value).toBe("");

    // 2) 测试同批次不重渲染即触发 onBlur 也不丢字，以及 Shift+Enter 多行递增 rows
    const liveTaB = findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!;
    (liveTaB.props.onCompositionEnd as (e: unknown) => void)({ currentTarget: { value: "第一行\n第二行" } });
    root = renderOverlay();
    const multiTa = findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!;
    expect(multiTa.props.rows).toBe(2);

    // Shift+Enter 不提前提交草稿
    const shiftPrevent = vi.fn();
    (multiTa.props.onKeyDown as (e: unknown) => void)({
      key: "Enter",
      shiftKey: true,
      preventDefault: shiftPrevent,
      stopPropagation: () => {},
    });
    root = renderOverlay();
    expect(shiftPrevent).not.toHaveBeenCalled();
    expect(findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")).not.toBeNull();

    // 不经重渲染直接在 onChange 后紧接 onBlur（模拟批处理/末尾瞬态），必须完整提交
    const multiTa2 = findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!;
    (multiTa2.props.onChange as (e: unknown) => void)({ target: { value: "第一行\n第二行末字" } });
    (multiTa2.props.onBlur as (e: unknown) => void)({ currentTarget: { value: "第一行\n第二行末字" } });
    root = renderOverlay();
    await (findToolbarProps(root)!.onSave as (kind: SaveImageKind) => Promise<void>)("translated");
    root = renderOverlay();
    const savedMulti = (
      exportMock.renderCapturePngBase64.mock.calls[0]![0] as { annotations: Array<{ text: string }> }
    ).annotations[0]!;
    expect(savedMulti.text).toBe("第一行\n第二行末字");

    // 3) 非空草稿按 Escape 取消不新增标注
    (findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!.props.onPointerDown as (
      e: unknown,
    ) => void)({
      button: 0,
      pointerId: 3,
      clientX: 180,
      clientY: 140,
      preventDefault: () => {},
      stopPropagation: () => {},
      currentTarget: {},
    });
    root = renderOverlay();
    const taEsc = findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!;
    (taEsc.props.onChange as (e: unknown) => void)({ target: { value: "要放弃的文字" } });
    root = renderOverlay();
    (findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!.props.onKeyDown as (
      e: unknown,
    ) => void)({
      key: "Escape",
      preventDefault: () => {},
      stopPropagation: () => {},
    });
    root = renderOverlay();
    expect(findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")).toBeNull();
    expect(findToolbarProps(root)!.annotationsCount).toBe(1);

    // 4) isComposingText === true 时触发新世代截图（genChanged）必须清空组合状态，使下一代占位符正常显示
    (findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!.props.onPointerDown as (
      e: unknown,
    ) => void)({
      button: 0,
      pointerId: 4,
      clientX: 200,
      clientY: 160,
      preventDefault: () => {},
      stopPropagation: () => {},
      currentTarget: {},
    });
    root = renderOverlay();
    (findElement(root, (el) => el.props["data-testid"] === "annotation-text-input")!.props.onCompositionStart as () => void)();
    root = renderOverlay();

    for (const h of ipcMock.updateHandlers) {
      h({
        blocks: [sampleBlock()],
        error: null,
        notice: null,
        stage: "done",
        generation: 42,
        revision: 1,
        has_image: true,
        scale: 2,
        bounds: { w: 800, h: 600 },
        image_rect: { x: 0, y: 0, w: 800, h: 600 },
      });
    }
    root = renderOverlay();
    (findToolbarProps(root)!.onSelectTool as (tool: "text") => void)("text");
    root = renderOverlay();
    (findElement(root, (el) => el.props["data-testid"] === "capture-annotations-svg")!.props.onPointerDown as (
      e: unknown,
    ) => void)({
      button: 0,
      pointerId: 5,
      clientX: 210,
      clientY: 170,
      preventDefault: () => {},
      stopPropagation: () => {},
      currentTarget: {},
    });
    root = renderOverlay();
    expect(findElement(root, (el) => el.props["data-testid"] === "annotation-text-placeholder")).not.toBeNull();
  });
});
