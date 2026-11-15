import { Component, useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import {
  api,
  onCaptureUpdate,
  type CaptureHitRect,
  type CaptureStage,
  type CaptureView,
  type ImageRect,
  type OverlayBlock,
  type OverlayBounds,
  type SaveImageKind,
} from "../../lib/ipc";
import {
  ANNOTATION_COLORS,
  ANNOTATION_SIZES,
  TEXT_DRAFT_PAD_X,
  TEXT_DRAFT_PAD_Y,
  applyTextAnnotationFontSize,
  clampPoint,
  clampTextFontSize,
  computeArrowHeadPoints,
  computeTextAnnotationBounds,
  computeTextBoxLayout,
  estimateTextLineWidth,
  getShapeResizeHandles,
  getTextResizeHandles,
  getTextSelectionFrame,
  isMeaningfulAnnotation,
  moveAnnotationInStage,
  normalizeRect,
  resizeShapeAnnotationInStage,
  resizeTextAnnotationFontSize,
  wrapAnnotationText,
  type Annotation,
  type AnnotationPoint,
  type AnnotationSize,
  type AnnotationTool,
  type EditableAnnotation,
  type ResizeHandle,
  type TextResizeHandle,
} from "./captureAnnotations";
import { renderCapturePngBase64 } from "./captureExport";
import { CaptureToolbar, type ToolbarPopover } from "./CaptureToolbar";
import { OVERLAY_FONT, createCanvasMeasure, plainPlan, planOverlay, sanitizeBlocks, type BlockPlan } from "./overlayLayout";

const STAGE_ORDER: Record<CaptureStage, number> = { ocr: 0, translating: 1, done: 2 };
const MAX_UNDO_HISTORY = 60;

type ActiveTransform =
  | {
      mode: "move";
      id: string;
      startRaw: AnnotationPoint;
      startAnnotation: EditableAnnotation;
      beforeList: Annotation[];
    }
  | {
      mode: "resize-shape";
      id: string;
      handle: ResizeHandle;
      startRaw: AnnotationPoint;
      startAnnotation: Extract<Annotation, { kind: "rect" | "ellipse" }>;
      beforeList: Annotation[];
    }
  | {
      mode: "resize-text";
      id: string;
      handle: TextResizeHandle;
      startRaw: AnnotationPoint;
      startAnnotation: Extract<Annotation, { kind: "text" }>;
      beforeList: Annotation[];
      beforeTextFontSize: number;
    };

function isEditableAnnotation(item: Annotation | undefined | null): item is EditableAnnotation {
  return Boolean(
    item && (item.kind === "rect" || item.kind === "ellipse" || item.kind === "text"),
  );
}

function areAnnotationsEqual(a: Annotation, b: Annotation): boolean {
  if (a === b) return true;
  if (a.id !== b.id || a.kind !== b.kind || a.color !== b.color) return false;
  if (a.kind === "text" && b.kind === "text") {
    return a.x === b.x && a.y === b.y && a.text === b.text && a.fontSize === b.fontSize;
  }
  if (
    (a.kind === "rect" || a.kind === "ellipse" || a.kind === "arrow") &&
    (b.kind === "rect" || b.kind === "ellipse" || b.kind === "arrow")
  ) {
    return (
      a.x1 === b.x1 &&
      a.y1 === b.y1 &&
      a.x2 === b.x2 &&
      a.y2 === b.y2 &&
      a.strokeWidth === b.strokeWidth
    );
  }
  return false;
}

let cachedMeasureDoc: unknown;
let cachedMeasureCtx: CanvasRenderingContext2D | null = null;

function measureOverlayAnnotationText(line: string, fontSize: number): number {
  if (!line) return 0;
  const doc = typeof document !== "undefined" ? document : undefined;
  if (doc && typeof doc.createElement === "function") {
    if (doc !== cachedMeasureDoc) {
      cachedMeasureDoc = doc;
      try {
        cachedMeasureCtx = doc.createElement("canvas")?.getContext?.("2d") ?? null;
      } catch {
        cachedMeasureCtx = null;
      }
    }
    if (cachedMeasureCtx && typeof cachedMeasureCtx.measureText === "function") {
      cachedMeasureCtx.font = `600 ${fontSize}px ${OVERLAY_FONT}`;
      const w = cachedMeasureCtx.measureText(line)?.width;
      if (typeof w === "number" && Number.isFinite(w) && w > 0) {
        return w;
      }
    }
  }
  return estimateTextLineWidth(line, fontSize);
}

function isAnnotationTextInputTarget(target: unknown): boolean {
  if (!target || typeof target !== "object") return false;
  const el = target as { tagName?: unknown; dataset?: { testid?: unknown } };
  return (
    (typeof el.tagName === "string" && el.tagName.toUpperCase() === "TEXTAREA") ||
    el.dataset?.testid === "annotation-text-input"
  );
}

export function sanitizeBounds(raw: unknown): OverlayBounds | null {
  if (!raw || typeof raw !== "object") return null;
  const candidate = raw as { w?: unknown; h?: unknown };
  if (
    typeof candidate.w === "number" &&
    Number.isFinite(candidate.w) &&
    candidate.w > 0 &&
    typeof candidate.h === "number" &&
    Number.isFinite(candidate.h) &&
    candidate.h > 0
  ) {
    return { w: candidate.w, h: candidate.h };
  }
  return null;
}

export function sanitizeImageRect(raw: unknown): ImageRect | null {
  if (!raw || typeof raw !== "object") return null;
  const c = raw as { x?: unknown; y?: unknown; w?: unknown; h?: unknown };
  if (
    typeof c.x === "number" &&
    Number.isFinite(c.x) &&
    c.x >= 0 &&
    typeof c.y === "number" &&
    Number.isFinite(c.y) &&
    c.y >= 0 &&
    typeof c.w === "number" &&
    Number.isFinite(c.w) &&
    c.w > 0 &&
    typeof c.h === "number" &&
    Number.isFinite(c.h) &&
    c.h > 0
  ) {
    return { x: c.x, y: c.y, w: c.w, h: c.h };
  }
  return null;
}

/**
 * 译文与标注覆盖层：在固定桌面选区上渲染冻结原图、原位译文块、钉钉风格标注层与悬浮工具条。
 * 与旧版不同，截屏完成后用户可以在工具条内切换目标语言、对比原图、加颜色标记/箭头/文字、
 * 分别保存未翻译原图或翻译后图片到本机，因此失焦或点击画布本身不会自动关窗，需点「关闭」或按 Esc 退出。
 */
export function TranslateOverlay() {
  const [blocks, setBlocks] = useState<OverlayBlock[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [stage, setStage] = useState<CaptureStage>("done");
  const [bounds, setBounds] = useState<OverlayBounds | null>(null);
  const [imageRect, setImageRect] = useState<ImageRect | null>(null);
  const [hasImage, setHasImage] = useState(false);
  const [originalImageUrl, setOriginalImageUrl] = useState<string | null>(null);
  const [targetLang, setTargetLang] = useState<string | null>(null);
  const [scale, setScale] = useState(1);
  const [hint, setHint] = useState(true);

  const [showOriginal, setShowOriginal] = useState(false);
  const [retranslating, setRetranslating] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveToast, setSaveToast] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [openPopover, setOpenPopover] = useState<ToolbarPopover>("none");

  const [activeTool, setActiveTool] = useState<AnnotationTool>("none");
  const [activeColor, setActiveColor] = useState<string>(ANNOTATION_COLORS[0].hex);
  const [activeSize, setActiveSize] = useState<AnnotationSize["id"]>("md");
  const [textFontSize, setTextFontSize] = useState<number>(ANNOTATION_SIZES[1].fontSize);
  const [selectedAnnotationId, setSelectedAnnotationId] = useState<string | null>(null);
  const [isTransforming, setIsTransforming] = useState(false);
  const [transformRevision, setTransformRevision] = useState(0);
  const [annotations, setAnnotations] = useState<Annotation[]>([]);
  const [draftAnnotation, setDraftAnnotation] = useState<Annotation | null>(null);
  const [textDraft, setTextDraft] = useState<{ id: number; x: number; y: number; value: string } | null>(null);
  const [isComposingText, setIsComposingText] = useState(false);

  const [viewport, setViewport] = useState(() => ({
    w: typeof window !== "undefined" ? window.innerWidth : 0,
    h: typeof window !== "undefined" ? window.innerHeight : 0,
  }));
  const [toolbarWidth, setToolbarWidth] = useState(560);
  const [toolbarMainHeight, setToolbarMainHeight] = useState(40);
  const [toolbarSubbarHeight, setToolbarSubbarHeight] = useState(31);

  const generationRef = useRef<number | undefined>(undefined);
  const revisionRef = useRef<number>(0);
  const stageRef = useRef<CaptureStage>("ocr");
  const loadedImageGenRef = useRef<number | undefined>(undefined);
  const rootElRef = useRef<HTMLDivElement | null>(null);
  const stageElRef = useRef<HTMLDivElement | null>(null);
  const toolbarAnchorRef = useRef<HTMLDivElement | null>(null);
  const openPopoverRef = useRef<ToolbarPopover>("none");
  const textDraftRef = useRef<{ id: number; x: number; y: number; value: string } | null>(null);
  const textDraftSeqRef = useRef<number>(0);
  const annotationSeqRef = useRef<number>(0);
  const textInputRef = useRef<HTMLTextAreaElement | null>(null);
  const annotationsRef = useRef<Annotation[]>([]);
  const undoStackRef = useRef<Annotation[][]>([]);
  const selectedAnnotationIdRef = useRef<string | null>(null);
  const activeTransformRef = useRef<ActiveTransform | null>(null);
  const draftAnnotationRef = useRef<Annotation | null>(null);
  const activePointerIdRef = useRef<number | null>(null);
  const performUndoRef = useRef<() => void>(() => {});
  const originalUrlRef = useRef<string | null>(null);
  const originalLoadPromiseRef = useRef<Promise<string | null> | null>(null);
  const toastTimerRef = useRef<number | undefined>(undefined);

  const focusTextDraftInput = useCallback((placeCaretEnd = false) => {
    const ta = textInputRef.current;
    if (!ta || typeof ta.focus !== "function") return;
    try {
      ta.focus({ preventScroll: true });
    } catch {
      ta.focus();
    }
    if (placeCaretEnd && typeof ta.setSelectionRange === "function") {
      const len = ta.value?.length ?? 0;
      ta.setSelectionRange(len, len);
    }
  }, []);

  const ensureOverlayWindowFocused = useCallback(() => {
    void api.focusOverlay?.(generationRef.current)?.catch?.(() => {});
  }, []);

  useEffect(() => {
    if (textDraft?.id == null) return () => {};
    focusTextDraftInput(true);
    return () => {};
  }, [textDraft?.id, focusTextDraftInput]);

  const sizePreset = useMemo(
    () => ANNOTATION_SIZES.find((item) => item.id === activeSize) ?? ANNOTATION_SIZES[1],
    [activeSize],
  );
  const activeToolRef = useRef<AnnotationTool>(activeTool);
  const activeColorRef = useRef<string>(activeColor);
  const sizePresetRef = useRef<AnnotationSize>(sizePreset);
  const textFontSizeRef = useRef<number>(textFontSize);

  openPopoverRef.current = openPopover;
  textDraftRef.current = textDraft;
  annotationsRef.current = annotations;
  selectedAnnotationIdRef.current = selectedAnnotationId;
  draftAnnotationRef.current = draftAnnotation;
  originalUrlRef.current = originalImageUrl;
  activeToolRef.current = activeTool;
  activeColorRef.current = activeColor;
  sizePresetRef.current = sizePreset;
  textFontSizeRef.current = textFontSize;

  const pushUndoSnapshot = useCallback((snapshot: Annotation[]) => {
    const stack = undoStackRef.current;
    stack.push(snapshot);
    if (stack.length > MAX_UNDO_HISTORY) {
      stack.shift();
    }
  }, []);

  const cancelActiveTransform = useCallback(() => {
    const tf = activeTransformRef.current;
    if (!tf) return false;
    activeTransformRef.current = null;
    activePointerIdRef.current = null;
    annotationsRef.current = tf.beforeList;
    setAnnotations(tf.beforeList);
    if (tf.mode === "resize-text") {
      textFontSizeRef.current = tf.beforeTextFontSize;
      setTextFontSize(tf.beforeTextFontSize);
    }
    setIsTransforming(false);
    return true;
  }, []);

  const fetchOriginalForGen = useCallback(
    async (gen?: number, isDisposed?: () => boolean): Promise<string | null> => {
      const targetGen = gen ?? generationRef.current;
      if (
        originalUrlRef.current &&
        (targetGen === undefined || generationRef.current === targetGen)
      ) {
        return originalUrlRef.current;
      }
      if (
        targetGen !== undefined &&
        loadedImageGenRef.current === targetGen &&
        originalLoadPromiseRef.current
      ) {
        return originalLoadPromiseRef.current;
      }
      loadedImageGenRef.current = targetGen;
      const req = api
        .captureOriginalImage()
        .then((url) => {
          if (originalLoadPromiseRef.current === req) {
            originalLoadPromiseRef.current = null;
          }
          if (isDisposed?.()) return null;
          if (targetGen !== undefined && generationRef.current !== targetGen) return null;
          if (url) {
            originalUrlRef.current = url;
            setOriginalImageUrl(url);
            return url;
          }
          if (loadedImageGenRef.current === targetGen) {
            loadedImageGenRef.current = undefined;
          }
          return null;
        })
        .catch(() => {
          if (originalLoadPromiseRef.current === req) {
            originalLoadPromiseRef.current = null;
          }
          if (loadedImageGenRef.current === targetGen) {
            loadedImageGenRef.current = undefined;
          }
          return null;
        });
      originalLoadPromiseRef.current = req;
      return req;
    },
    [],
  );

  useEffect(() => {
    if (typeof window === "undefined" || typeof window.addEventListener !== "function") {
      return () => {};
    }
    const onResize = () => setViewport({ w: window.innerWidth, h: window.innerHeight });
    const onFocus = () => {
      if (textDraftRef.current) {
        focusTextDraftInput();
      }
    };
    const onBlur = () => {
      if (openPopoverRef.current !== "none") {
        openPopoverRef.current = "none";
        setOpenPopover("none");
      }
    };
    const onContextMenu = (event: Event) => {
      if (isAnnotationTextInputTarget((event as { target?: unknown })?.target)) return;
      event.preventDefault?.();
    };
    window.addEventListener("resize", onResize);
    window.addEventListener("focus", onFocus);
    window.addEventListener("blur", onBlur);
    window.addEventListener("contextmenu", onContextMenu, true);
    return () => {
      window.removeEventListener("resize", onResize);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("contextmenu", onContextMenu, true);
    };
  }, [focusTextDraftInput]);

  useEffect(() => {
    let disposed = false;
    let sawPushEvent = false;

    const loadOriginalForGen = (gen: number) => {
      void fetchOriginalForGen(gen, () => disposed);
    };

    const applyView = (view: CaptureView, fromPush: boolean) => {
      if (disposed) return;
      if (fromPush) sawPushEvent = true;
      const incomingGen = typeof view?.generation === "number" ? view.generation : undefined;
      const incomingRev = typeof view?.revision === "number" ? view.revision : 1;
      const incomingStage: CaptureStage = view?.stage ?? "done";
      const cleanBounds = sanitizeBounds(view?.bounds);
      const cleanImageRect = sanitizeImageRect(view?.image_rect);

      if (initializedRef.current && incomingGen !== undefined && generationRef.current !== undefined) {
        if (incomingGen < generationRef.current) return;
        if (incomingGen === generationRef.current) {
          if (incomingRev < revisionRef.current) return;
          if (incomingRev === revisionRef.current) {
            if (STAGE_ORDER[incomingStage] < STAGE_ORDER[stageRef.current]) return;
            if (!fromPush && sawPushEvent && STAGE_ORDER[incomingStage] <= STAGE_ORDER[stageRef.current]) {
              return;
            }
          }
        }
      }

      const genChanged = incomingGen !== undefined && incomingGen !== generationRef.current;
      initializedRef.current = true;
      if (incomingGen !== undefined) generationRef.current = incomingGen;
      revisionRef.current = incomingRev;
      stageRef.current = incomingStage;

      if (genChanged) {
        loadedImageGenRef.current = undefined;
        originalLoadPromiseRef.current = null;
        originalUrlRef.current = null;
        setOriginalImageUrl(null);
        annotationsRef.current = [];
        undoStackRef.current = [];
        setAnnotations([]);
        selectedAnnotationIdRef.current = null;
        setSelectedAnnotationId(null);
        activeTransformRef.current = null;
        setIsTransforming(false);
        setTransformRevision(0);
        setToolbarSubbarHeight(31);
        draftAnnotationRef.current = null;
        activePointerIdRef.current = null;
        setDraftAnnotation(null);
        textDraftRef.current = null;
        setTextDraft(null);
        setIsComposingText(false);
        setActiveTool("none");
        setShowOriginal(false);
        openPopoverRef.current = "none";
        setOpenPopover("none");
        setSaveToast(null);
        setActionError(null);
      }

      const { blocks: clean, problem } = sanitizeBlocks(view?.blocks);
      const malformed = !Array.isArray(view?.blocks);
      setStage(incomingStage);
      setBounds(cleanBounds);
      setImageRect(cleanImageRect);
      setHasImage(view?.has_image === true);
      if (view?.target_lang !== undefined) {
        setTargetLang(view.target_lang ?? null);
      }
      if (typeof view?.scale === "number" && Number.isFinite(view.scale) && view.scale > 0) {
        setScale(view.scale);
      }
      setBlocks(clean);
      setError(view?.error ?? (malformed ? problem : null));
      setNotice([view?.notice, malformed ? null : problem].filter(Boolean).join("；") || null);
      if (incomingStage === "done") {
        setRetranslating(false);
      }
      if (view?.has_image === true && incomingGen !== undefined && !originalUrlRef.current) {
        loadOriginalForGen(incomingGen);
      }
    };

    const initializedRef = { current: false };

    let unlistenUpdate: (() => void) | undefined;
    void onCaptureUpdate((view) => applyView(view, true)).then((fn) => {
      if (disposed) {
        fn();
        return;
      }
      unlistenUpdate = fn;
    });

    void api
      .captureBlocks()
      .then((view) => applyView(view, false))
      .catch((err) => {
        if (disposed) return;
        initializedRef.current = true;
        stageRef.current = "done";
        setStage("done");
        setBounds(null);
        setImageRect(null);
        setBlocks([]);
        setError(String(err).replace(/^Error:\s*/, ""));
      });

    const onKey = (event: KeyboardEvent) => {
      if (event.isComposing || event.keyCode === 229) return;
      if (event.key === "Escape") {
        if (activeTransformRef.current) {
          cancelActiveTransform();
          return;
        }
        if (draftAnnotationRef.current) {
          draftAnnotationRef.current = null;
          activePointerIdRef.current = null;
          setDraftAnnotation(null);
          return;
        }
        if (textDraftRef.current) {
          textDraftRef.current = null;
          setTextDraft(null);
          setIsComposingText(false);
          return;
        }
        if (openPopoverRef.current !== "none") {
          openPopoverRef.current = "none";
          setOpenPopover("none");
          return;
        }
        if (selectedAnnotationIdRef.current !== null) {
          selectedAnnotationIdRef.current = null;
          setSelectedAnnotationId(null);
          return;
        }
        void api.closeOverlay(generationRef.current);
        return;
      }
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "z") {
        if (textDraftRef.current) return;
        event.preventDefault();
        performUndoRef.current();
      }
    };
    window.addEventListener("keydown", onKey);
    const timer = window.setTimeout(() => setHint(false), 2200);

    return () => {
      disposed = true;
      window.removeEventListener("keydown", onKey);
      window.clearTimeout(timer);
      if (toastTimerRef.current !== undefined) {
        window.clearTimeout(toastTimerRef.current);
      }
      unlistenUpdate?.();
    };
  }, [cancelActiveTransform, fetchOriginalForGen]);

  const viewW = bounds?.w ?? (typeof window !== "undefined" ? window.innerWidth : viewport.w);
  const viewH = bounds?.h ?? (typeof window !== "undefined" ? window.innerHeight : viewport.h);
  const stageRect: ImageRect = useMemo(
    () => imageRect ?? { x: 0, y: 0, w: viewW, h: viewH },
    [imageRect, viewW, viewH],
  );

  const stagePlans = useMemo(() => {
    if (!blocks) return [];
    const targetBounds = { w: stageRect.w, h: stageRect.h };
    try {
      return planOverlay(blocks, createCanvasMeasure(), targetBounds);
    } catch (err) {
      console.error("[overlay] 排版出错，整屏按简单样式显示：", err);
      return blocks.map((block) => plainPlan(block, targetBounds));
    }
  }, [blocks, stageRect.w, stageRect.h]);

  const handleClose = useCallback(() => {
    void api.closeOverlay(generationRef.current);
  }, []);

  const showToast = useCallback((msg: string) => {
    setSaveToast(msg);
    if (toastTimerRef.current !== undefined) {
      window.clearTimeout(toastTimerRef.current);
    }
    toastTimerRef.current = window.setTimeout(() => setSaveToast(null), 2600);
  }, []);

  const rawStagePoint = useCallback((clientX: number, clientY: number): AnnotationPoint => {
    const rect = stageElRef.current?.getBoundingClientRect();
    if (!rect) return { x: clientX, y: clientY };
    return { x: clientX - rect.left, y: clientY - rect.top };
  }, []);

  const stageLocalPoint = useCallback(
    (clientX: number, clientY: number) => {
      const raw = rawStagePoint(clientX, clientY);
      return clampPoint(raw, stageRect.w, stageRect.h);
    },
    [rawStagePoint, stageRect.h, stageRect.w],
  );

  const flushTextDraft = useCallback(
    (expectedId?: number): Annotation[] => {
      const current = textDraftRef.current;
      if (!current) return annotationsRef.current;
      const targetId = typeof expectedId === "number" ? expectedId : undefined;
      if (targetId !== undefined && current.id !== targetId) {
        return annotationsRef.current;
      }
      textDraftRef.current = null;
      setTextDraft(null);
      setIsComposingText(false);
      const domTa = textInputRef.current;
      const rawValue =
        domTa && domTa.dataset?.draftId === String(current.id) && typeof domTa.value === "string"
          ? domTa.value
          : current.value;
      const trimmed = rawValue.trim();
      if (!trimmed) return annotationsRef.current;
      const draftFontSize = clampTextFontSize(textFontSizeRef.current);
      const layout = computeTextBoxLayout({
        x: current.x,
        y: current.y,
        value: trimmed,
        fontSize: draftFontSize,
        stageWidth: stageRect.w,
        stageHeight: stageRect.h,
        measureFn: measureOverlayAnnotationText,
      });
      const committedLines = layout.wrappedLines;
      const totalTextH = committedLines.length * layout.lineHeight;
      const committedY =
        stageRect.h > 0 && committedLines.length > layout.rows
          ? Math.max(
              TEXT_DRAFT_PAD_Y,
              Math.min(
                layout.textY,
                Math.max(TEXT_DRAFT_PAD_Y, stageRect.h - totalTextH - TEXT_DRAFT_PAD_Y),
              ),
            )
          : layout.textY;
      const nextItem: Annotation = {
        id: `ann-${Date.now()}-${++annotationSeqRef.current}`,
        kind: "text",
        x: layout.textX,
        y: committedY,
        text: trimmed,
        color: activeColorRef.current,
        fontSize: draftFontSize,
      };
      pushUndoSnapshot(annotationsRef.current);
      const nextList = [...annotationsRef.current, nextItem];
      annotationsRef.current = nextList;
      setAnnotations(nextList);
      selectedAnnotationIdRef.current = nextItem.id;
      setSelectedAnnotationId(nextItem.id);
      setTransformRevision((r) => r + 1);
      return nextList;
    },
    [pushUndoSnapshot, stageRect.h, stageRect.w],
  );

  const handleSelectLang = useCallback(
    (code: string) => {
      if (textDraftRef.current) {
        flushTextDraft();
      }
      setActionError(null);
      setShowOriginal(false);
      setRetranslating(true);
      void api
        .retranslateCapture(code)
        .catch((err) => {
          setRetranslating(false);
          setActionError(describeFailure(err));
        });
    },
    [flushTextDraft],
  );

  const handleTogglePopover = useCallback(
    (next: "lang" | "save" | "none") => {
      if (next !== "none" && textDraftRef.current) {
        flushTextDraft();
      }
      openPopoverRef.current = next;
      setOpenPopover(next);
    },
    [flushTextDraft],
  );

  const handleSave = useCallback(
    async (kind: SaveImageKind) => {
      if (saving) return;
      setActionError(null);
      setSaving(true);
      try {
        const committedAnnotations = flushTextDraft();
        let payload: string | null = null;
        if (kind !== "original") {
          let imgUrl = originalUrlRef.current ?? originalImageUrl;
          if (!imgUrl) {
            imgUrl = await fetchOriginalForGen(generationRef.current);
          }
          if (!imgUrl) {
            throw new Error("当前截屏原图尚未就绪，无法导出图片");
          }
          payload = await renderCapturePngBase64({
            originalImageUrl: imgUrl,
            stageWidth: stageRect.w,
            stageHeight: stageRect.h,
            scale,
            includeTranslation: kind === "translated",
            blocks: blocks ?? [],
            plans: stagePlans,
            annotations: committedAnnotations,
          });
        }
        const res = await api.saveCaptureImage(kind, payload);
        if (res.saved) {
          showToast(kind === "translated" ? "已保存翻译后图片" : "已保存图片");
        }
      } catch (err) {
        setActionError(describeFailure(err));
      } finally {
        setSaving(false);
      }
    },
    [blocks, fetchOriginalForGen, flushTextDraft, originalImageUrl, saving, scale, showToast, stagePlans, stageRect.h, stageRect.w],
  );

  const handleSelectTool = useCallback(
    (tool: AnnotationTool) => {
      if (activeTransformRef.current) {
        cancelActiveTransform();
      }
      if (draftAnnotationRef.current) {
        draftAnnotationRef.current = null;
        activePointerIdRef.current = null;
        setDraftAnnotation(null);
      }
      if (tool !== "text" && textDraftRef.current) {
        flushTextDraft();
      }
      if ((tool !== "none" && tool !== activeTool) || tool === "arrow" || tool === "pen") {
        selectedAnnotationIdRef.current = null;
        setSelectedAnnotationId(null);
      }
      if (tool !== "none") {
        ensureOverlayWindowFocused();
      }
      setActiveTool(tool);
    },
    [activeTool, cancelActiveTransform, ensureOverlayWindowFocused, flushTextDraft],
  );

  const handleSelectColor = useCallback(
    (hex: string) => {
      activeColorRef.current = hex;
      setActiveColor(hex);
      const selId = selectedAnnotationIdRef.current;
      if (selId && !textDraftRef.current) {
        const prev = annotationsRef.current;
        const idx = prev.findIndex((item) => item.id === selId && isEditableAnnotation(item));
        if (idx >= 0 && prev[idx]!.color !== hex) {
          pushUndoSnapshot(prev);
          const next = prev.map((item, i) => (i === idx ? { ...item, color: hex } : item));
          annotationsRef.current = next;
          setAnnotations(next);
        }
      }
    },
    [pushUndoSnapshot],
  );

  const handleSelectSize = useCallback(
    (sizeId: AnnotationSize["id"]) => {
      const preset = ANNOTATION_SIZES.find((item) => item.id === sizeId) ?? ANNOTATION_SIZES[1];
      const selId = selectedAnnotationIdRef.current;
      const prev = annotationsRef.current;
      const idx =
        selId && !textDraftRef.current
          ? prev.findIndex((item) => item.id === selId && isEditableAnnotation(item))
          : -1;
      const target = idx >= 0 ? (prev[idx] as EditableAnnotation) : null;
      const inTextContext =
        target?.kind === "text" || (!target && activeToolRef.current === "text");

      if (inTextContext) {
        textFontSizeRef.current = preset.fontSize;
        setTextFontSize(preset.fontSize);
      } else {
        sizePresetRef.current = preset;
        setActiveSize(sizeId);
      }

      if (target) {
        const updated: EditableAnnotation =
          target.kind === "text"
            ? applyTextAnnotationFontSize(
                target,
                preset.fontSize,
                stageRect.w,
                stageRect.h,
                measureOverlayAnnotationText,
              )
            : { ...target, strokeWidth: preset.strokeWidth };
        if (!areAnnotationsEqual(target, updated)) {
          pushUndoSnapshot(prev);
          const next = prev.map((item, i) => (i === idx ? updated : item));
          annotationsRef.current = next;
          setAnnotations(next);
          setTransformRevision((r) => r + 1);
        }
      }
    },
    [pushUndoSnapshot, stageRect.h, stageRect.w],
  );

  const handleAdjustFontSize = useCallback(
    (delta: number) => {
      const selId = selectedAnnotationIdRef.current;
      const prev = annotationsRef.current;
      const selIdx =
        selId && !textDraftRef.current
          ? prev.findIndex((item) => item.id === selId && item.kind === "text")
          : -1;
      const selText = selIdx >= 0 ? (prev[selIdx] as Extract<Annotation, { kind: "text" }>) : null;
      const baseFs = selText ? selText.fontSize : textFontSizeRef.current;
      const nextFs = clampTextFontSize(baseFs + delta);

      textFontSizeRef.current = nextFs;
      setTextFontSize(nextFs);

      if (selText) {
        const updated = applyTextAnnotationFontSize(
          selText,
          nextFs,
          stageRect.w,
          stageRect.h,
          measureOverlayAnnotationText,
        );
        if (!areAnnotationsEqual(selText, updated)) {
          pushUndoSnapshot(prev);
          const next = prev.map((item, i) => (i === selIdx ? updated : item));
          annotationsRef.current = next;
          setAnnotations(next);
          setTransformRevision((r) => r + 1);
        }
      }
    },
    [pushUndoSnapshot, stageRect.h, stageRect.w],
  );

  const handleUndo = useCallback(() => {
    if (activeTransformRef.current) {
      cancelActiveTransform();
      return;
    }
    if (draftAnnotationRef.current) {
      draftAnnotationRef.current = null;
      activePointerIdRef.current = null;
      setDraftAnnotation(null);
      return;
    }
    if (textDraftRef.current) {
      textDraftRef.current = null;
      setTextDraft(null);
      setIsComposingText(false);
      return;
    }
    const popped = undoStackRef.current.pop();
    const nextList = popped ?? annotationsRef.current.slice(0, -1);
    annotationsRef.current = nextList;
    setAnnotations(nextList);
    const selId = selectedAnnotationIdRef.current;
    if (selId) {
      const stillSelected = nextList.find((item) => item.id === selId);
      if (!stillSelected || !isEditableAnnotation(stillSelected)) {
        selectedAnnotationIdRef.current = null;
        setSelectedAnnotationId(null);
      }
    }
    setTransformRevision((r) => r + 1);
  }, [cancelActiveTransform]);

  performUndoRef.current = handleUndo;

  const isForeignPointerEvent = useCallback((event: unknown): boolean => {
    const activeId = activePointerIdRef.current;
    if (activeId === null || !event || typeof event !== "object") return false;
    const pid = (event as { pointerId?: unknown }).pointerId;
    return typeof pid === "number" && pid !== activeId;
  }, []);

  const handleTransformMouseDown = useCallback(
    (event: { button: number; ctrlKey?: boolean; preventDefault: () => void; stopPropagation?: () => void }) => {
      if (event.button === 0 && !event.ctrlKey) {
        event.preventDefault();
        event.stopPropagation?.();
      }
    },
    [],
  );

  const beginAnnotationTransform = useCallback(
    (
      event: ReactPointerEvent<Element>,
      buildTransform: (baseList: Annotation[], startRaw: AnnotationPoint) => ActiveTransform | null,
    ) => {
      if (event.button !== 0 || event.ctrlKey || draftAnnotationRef.current) return;
      event.preventDefault?.();
      event.stopPropagation();
      if (openPopoverRef.current !== "none") {
        openPopoverRef.current = "none";
        setOpenPopover("none");
      }
      const baseList = textDraftRef.current ? flushTextDraft() : annotationsRef.current;
      const startRaw = rawStagePoint(event.clientX, event.clientY);
      const tf = buildTransform(baseList, startRaw);
      if (!tf) return;

      selectedAnnotationIdRef.current = tf.id;
      setSelectedAnnotationId(tf.id);
      ensureOverlayWindowFocused();
      activePointerIdRef.current = typeof event.pointerId === "number" ? event.pointerId : null;
      try {
        event.currentTarget?.setPointerCapture?.(event.pointerId);
      } catch {
        // WebKit 合成指针或无捕获支持时由下方 window 捕获监听兜底。
      }
      activeTransformRef.current = tf;
      setIsTransforming(true);
    },
    [ensureOverlayWindowFocused, flushTextDraft, rawStagePoint],
  );

  const startMoveAnnotation = useCallback(
    (id: string, event: ReactPointerEvent<Element>) => {
      beginAnnotationTransform(event, (baseList, startRaw) => {
        const target = baseList.find((item) => item.id === id && isEditableAnnotation(item));
        if (!target || !isEditableAnnotation(target)) return null;
        return {
          mode: "move",
          id: target.id,
          startRaw,
          startAnnotation: target,
          beforeList: baseList,
        };
      });
    },
    [beginAnnotationTransform],
  );

  const startResizeShapeAnnotation = useCallback(
    (id: string, handle: ResizeHandle, event: ReactPointerEvent<Element>) => {
      beginAnnotationTransform(event, (baseList, startRaw) => {
        const target = baseList.find(
          (item): item is Extract<Annotation, { kind: "rect" | "ellipse" }> =>
            item.id === id && (item.kind === "rect" || item.kind === "ellipse"),
        );
        if (!target) return null;
        return {
          mode: "resize-shape",
          id: target.id,
          handle,
          startRaw,
          startAnnotation: target,
          beforeList: baseList,
        };
      });
    },
    [beginAnnotationTransform],
  );

  const startResizeTextAnnotation = useCallback(
    (id: string, handle: TextResizeHandle, event: ReactPointerEvent<Element>) => {
      beginAnnotationTransform(event, (baseList, startRaw) => {
        const target = baseList.find(
          (item): item is Extract<Annotation, { kind: "text" }> =>
            item.id === id && item.kind === "text",
        );
        if (!target) return null;
        return {
          mode: "resize-text",
          id: target.id,
          handle,
          startRaw,
          startAnnotation: target,
          beforeList: baseList,
          beforeTextFontSize: textFontSizeRef.current,
        };
      });
    },
    [beginAnnotationTransform],
  );

  const updateTransformAtClientPoint = useCallback(
    (clientX: number, clientY: number) => {
      const tf = activeTransformRef.current;
      if (!tf) return;
      const curRaw = rawStagePoint(clientX, clientY);
      const dx = curRaw.x - tf.startRaw.x;
      const dy = curRaw.y - tf.startRaw.y;

      let updated: EditableAnnotation;
      if (tf.mode === "move") {
        updated = moveAnnotationInStage(
          tf.startAnnotation,
          dx,
          dy,
          stageRect.w,
          stageRect.h,
          measureOverlayAnnotationText,
        );
      } else if (tf.mode === "resize-shape") {
        updated = resizeShapeAnnotationInStage(
          tf.startAnnotation,
          tf.handle,
          dx,
          dy,
          stageRect.w,
          stageRect.h,
        );
      } else {
        updated = resizeTextAnnotationFontSize(
          tf.startAnnotation,
          tf.handle,
          dx,
          dy,
          stageRect.w,
          stageRect.h,
          measureOverlayAnnotationText,
        );
      }

      const currentList = annotationsRef.current;
      const idx = currentList.findIndex((item) => item.id === tf.id);
      if (idx < 0) return;
      if (!areAnnotationsEqual(currentList[idx]!, updated)) {
        const nextList = currentList.map((item, i) => (i === idx ? updated : item));
        annotationsRef.current = nextList;
        setAnnotations(nextList);
        if (tf.mode === "resize-text" && updated.kind === "text") {
          textFontSizeRef.current = updated.fontSize;
          setTextFontSize(updated.fontSize);
        }
      }
    },
    [rawStagePoint, stageRect.h, stageRect.w],
  );

  const finishTransform = useCallback(
    (event?: { clientX?: unknown; clientY?: unknown; pointerId?: unknown }) => {
      const tf = activeTransformRef.current;
      if (!tf || isForeignPointerEvent(event)) return;
      if (
        event &&
        typeof event.clientX === "number" &&
        Number.isFinite(event.clientX) &&
        typeof event.clientY === "number" &&
        Number.isFinite(event.clientY)
      ) {
        updateTransformAtClientPoint(event.clientX, event.clientY);
      }
      const finalItem = annotationsRef.current.find((item) => item.id === tf.id);
      activeTransformRef.current = null;
      activePointerIdRef.current = null;
      setIsTransforming(false);
      if (finalItem && !areAnnotationsEqual(tf.startAnnotation, finalItem)) {
        pushUndoSnapshot(tf.beforeList);
        setTransformRevision((r) => r + 1);
      }
    },
    [isForeignPointerEvent, pushUndoSnapshot, updateTransformAtClientPoint],
  );

  const isAcceptedToolMouseButton = useCallback(
    (button: number, ctrlKey = false) => {
      if (activeTool === "none") return false;
      if (activeTool === "text") return button === 0 && !ctrlKey;
      return button === 0 || button === 2;
    },
    [activeTool],
  );

  const handleStageMouseDown = useCallback(
    (event: { button: number; ctrlKey?: boolean; preventDefault: () => void }) => {
      if (isAcceptedToolMouseButton(event.button, Boolean(event.ctrlKey))) {
        event.preventDefault();
      }
    },
    [isAcceptedToolMouseButton],
  );

  const handleStagePointerDown = useCallback(
    (event: ReactPointerEvent<Element>) => {
      if (activeTool === "none") {
        if (event.button === 0 && !event.ctrlKey && selectedAnnotationIdRef.current !== null) {
          selectedAnnotationIdRef.current = null;
          setSelectedAnnotationId(null);
        }
        return;
      }
      if (!isAcceptedToolMouseButton(event.button, Boolean(event.ctrlKey))) {
        return;
      }
      event.preventDefault?.();
      event.stopPropagation();
      if (draftAnnotationRef.current || activeTransformRef.current) return;
      if (openPopoverRef.current !== "none") {
        const isBackdrop =
          (event.currentTarget as { dataset?: { testid?: string } } | null)?.dataset?.testid ===
          "capture-annotation-backdrop";
        openPopoverRef.current = "none";
        setOpenPopover("none");
        if (activeTool === "text" && isBackdrop) {
          return;
        }
      }

      const pt = stageLocalPoint(event.clientX, event.clientY);
      if (activeTool === "text") {
        flushTextDraft();
        selectedAnnotationIdRef.current = null;
        setSelectedAnnotationId(null);
        const nextId = ++textDraftSeqRef.current;
        const fresh = { id: nextId, x: pt.x, y: pt.y, value: "" };
        textDraftRef.current = fresh;
        setTextDraft(fresh);
        setIsComposingText(false);
        ensureOverlayWindowFocused();
        return;
      }
      flushTextDraft();
      selectedAnnotationIdRef.current = null;
      setSelectedAnnotationId(null);
      ensureOverlayWindowFocused();

      activePointerIdRef.current = typeof event.pointerId === "number" ? event.pointerId : null;
      try {
        event.currentTarget?.setPointerCapture?.(event.pointerId);
      } catch {
        // WebKit 在辅助键或合成指针下 setPointerCapture 可能抛错，由下方 window 级拖拽监听统一兜底。
      }
      const id = `ann-${Date.now()}-${++annotationSeqRef.current}`;
      const nextDraft: Annotation =
        activeTool === "pen"
          ? {
              id,
              kind: "pen",
              points: [pt],
              color: activeColor,
              strokeWidth: sizePreset.strokeWidth,
            }
          : {
              id,
              kind: activeTool,
              x1: pt.x,
              y1: pt.y,
              x2: pt.x,
              y2: pt.y,
              color: activeColor,
              strokeWidth: sizePreset.strokeWidth,
            };
      draftAnnotationRef.current = nextDraft;
      setDraftAnnotation(nextDraft);
    },
    [
      activeColor,
      activeTool,
      ensureOverlayWindowFocused,
      flushTextDraft,
      isAcceptedToolMouseButton,
      sizePreset.strokeWidth,
      stageLocalPoint,
    ],
  );

  const updateDraftAtClientPoint = useCallback(
    (clientX: number, clientY: number) => {
      const current = draftAnnotationRef.current;
      if (!current) return;
      const pt = stageLocalPoint(clientX, clientY);
      let next: Annotation = current;
      if (current.kind === "pen") {
        const last = current.points[current.points.length - 1];
        if (!last || last.x !== pt.x || last.y !== pt.y) {
          next = { ...current, points: [...current.points, pt] };
        }
      } else if (current.kind !== "text") {
        if (current.x2 !== pt.x || current.y2 !== pt.y) {
          next = { ...current, x2: pt.x, y2: pt.y };
        }
      }
      if (next !== current) {
        draftAnnotationRef.current = next;
        setDraftAnnotation(next);
      }
    },
    [stageLocalPoint],
  );

  const handleStagePointerMove = useCallback(
    (event: ReactPointerEvent<Element>) => {
      if (isForeignPointerEvent(event)) return;
      if (activeTransformRef.current) {
        updateTransformAtClientPoint(event.clientX, event.clientY);
        return;
      }
      if (!draftAnnotationRef.current) return;
      updateDraftAtClientPoint(event.clientX, event.clientY);
    },
    [isForeignPointerEvent, updateDraftAtClientPoint, updateTransformAtClientPoint],
  );

  const finishDraft = useCallback(
    (event?: { clientX?: unknown; clientY?: unknown; pointerId?: unknown }) => {
      if (!draftAnnotationRef.current || isForeignPointerEvent(event)) return;
      if (
        event &&
        typeof event.clientX === "number" &&
        Number.isFinite(event.clientX) &&
        typeof event.clientY === "number" &&
        Number.isFinite(event.clientY)
      ) {
        updateDraftAtClientPoint(event.clientX, event.clientY);
      }
      const current = draftAnnotationRef.current;
      if (!current) return;
      draftAnnotationRef.current = null;
      activePointerIdRef.current = null;
      if (isMeaningfulAnnotation(current)) {
        pushUndoSnapshot(annotationsRef.current);
        const nextList = [...annotationsRef.current, current];
        annotationsRef.current = nextList;
        setAnnotations(nextList);
        if (current.kind === "rect" || current.kind === "ellipse") {
          selectedAnnotationIdRef.current = current.id;
          setSelectedAnnotationId(current.id);
          setTransformRevision((r) => r + 1);
        }
      }
      setDraftAnnotation(null);
    },
    [isForeignPointerEvent, pushUndoSnapshot, updateDraftAtClientPoint],
  );

  const finishPointerInteraction = useCallback(
    (event?: { clientX?: unknown; clientY?: unknown; pointerId?: unknown }) => {
      if (activeTransformRef.current) {
        finishTransform(event);
      }
      if (draftAnnotationRef.current) {
        finishDraft(event);
      }
    },
    [finishDraft, finishTransform],
  );

  const selectedAnnotation = useMemo<EditableAnnotation | null>(() => {
    if (!selectedAnnotationId || textDraft) return null;
    const found = annotations.find((item) => item.id === selectedAnnotationId);
    return isEditableAnnotation(found) ? found : null;
  }, [annotations, selectedAnnotationId, textDraft]);

  const selectedAnnotationKind = selectedAnnotation?.kind ?? null;
  const displayedColor = selectedAnnotation ? selectedAnnotation.color : activeColor;
  const displayedSize: AnnotationSize["id"] = useMemo(() => {
    if (selectedAnnotation && selectedAnnotation.kind !== "text") {
      return (
        ANNOTATION_SIZES.find((item) => item.strokeWidth === selectedAnnotation.strokeWidth)?.id ??
        activeSize
      );
    }
    return activeSize;
  }, [activeSize, selectedAnnotation]);
  const displayedFontSize =
    selectedAnnotation?.kind === "text" ? selectedAnnotation.fontSize : textFontSize;
  const hasSubbarVisible = activeTool !== "none" || selectedAnnotationKind !== null;

  const effectiveViewW = viewW > 0 ? viewW : stageRect.x + stageRect.w;
  const toolbarMaxWidth = effectiveViewW > 0 ? Math.max(200, effectiveViewW - 8) : undefined;
  const isTextSubbarContext =
    selectedAnnotationKind === "text" || (!selectedAnnotationKind && activeTool === "text");
  const estimatedSubbarH =
    isTextSubbarContext && toolbarMaxWidth !== undefined && toolbarMaxWidth < 368 ? 64 : 31;
  const effectiveSubbarH = Math.max(estimatedSubbarH, toolbarSubbarHeight);

  const effectiveBarH = Math.max(40, toolbarMainHeight);
  const needToolbarSpace = effectiveBarH + 12;
  const spaceBelow = Math.max(0, viewH - (stageRect.y + stageRect.h));
  const spaceAbove = Math.max(0, stageRect.y);
  const toolbarDock =
    spaceBelow >= needToolbarSpace ? "bottom" : spaceAbove >= needToolbarSpace ? "top" : "inside-bottom";
  const toolbarTop =
    toolbarDock === "bottom"
      ? stageRect.y + stageRect.h + 8
      : toolbarDock === "top"
        ? Math.max(4, stageRect.y - (effectiveBarH + 6))
        : Math.max(8, viewH - (effectiveBarH + 12));
  const spaceBelowBar = Math.max(0, viewH - (toolbarTop + effectiveBarH));
  const spaceAboveBar = Math.max(0, toolbarTop);
  const popoverUp =
    toolbarDock === "inside-bottom" ||
    (spaceBelowBar < 190 && spaceAboveBar >= spaceBelowBar);
  const subbarNeedSpace = effectiveSubbarH + 6;
  const subbarAbove =
    toolbarDock === "inside-bottom"
      ? spaceAboveBar >= subbarNeedSpace || spaceAboveBar >= spaceBelowBar
      : toolbarDock === "top"
        ? spaceAboveBar >= subbarNeedSpace
        : spaceBelowBar < subbarNeedSpace && spaceAboveBar >= subbarNeedSpace;
  const maxEdgeHeight = Math.max(
    24,
    Math.floor((popoverUp ? spaceAboveBar : spaceBelowBar) - 10),
  );
  const popoverMaxHeight = viewH > 0 ? Math.min(220, maxEdgeHeight) : 180;

  const chromeMinW = Math.max(stageRect.w, 300);
  const chromeWidth = Math.min(viewW > 0 ? viewW : chromeMinW, chromeMinW);
  const chromeLeft = Math.min(
    stageRect.x,
    Math.max(0, (viewW > 0 ? viewW : chromeMinW) - chromeWidth),
  );
  const chromeMinHeight = Math.max(
    stageRect.h,
    Math.min(Math.max(0, (viewH > 0 ? viewH : 84) - stageRect.y), 84),
  );

  const showToolbar = stage === "done" || retranslating;
  const hasPendingText = Boolean(textDraft && textDraft.value.trim().length > 0);
  const effectiveAnnotationsCount = annotations.length + (hasPendingText ? 1 : 0);
  const measuredBarW = Math.min(effectiveViewW, Math.max(200, toolbarWidth));
  const maxToolbarLeft = Math.max(0, effectiveViewW - measuredBarW - 4);
  const toolbarLeft = Math.max(
    Math.min(4, maxToolbarLeft),
    Math.min(maxToolbarLeft, stageRect.x + stageRect.w / 2 - measuredBarW / 2),
  );

  useEffect(() => {
    const el = toolbarAnchorRef.current;
    if (!showToolbar || !el) return () => {};
    const mainEl =
      typeof el.querySelector === "function"
        ? (el.querySelector("[data-testid='capture-toolbar-main']") as HTMLElement | null)
        : null;
    const subbarEl =
      typeof el.querySelector === "function"
        ? (el.querySelector("[data-testid='capture-subbar']") as HTMLElement | null)
        : null;
    const measure = () => {
      const target = mainEl ?? el;
      const rect = target.getBoundingClientRect?.();
      const w = rect?.width || target.offsetWidth;
      const h = rect?.height || target.offsetHeight;
      if (w > 0 && Number.isFinite(w)) {
        setToolbarWidth((prev) => (Math.abs(prev - w) > 0.5 ? Math.round(w) : prev));
      }
      if (h > 0 && Number.isFinite(h)) {
        const normalizedH = Math.max(40, Math.round(h));
        setToolbarMainHeight((prev) =>
          Math.abs(prev - normalizedH) > 0.5 ? normalizedH : prev,
        );
      }
      const curSubbar =
        typeof el.querySelector === "function"
          ? (el.querySelector("[data-testid='capture-subbar']") as HTMLElement | null)
          : subbarEl;
      if (curSubbar) {
        const sRect = curSubbar.getBoundingClientRect?.();
        const sh = sRect?.height || curSubbar.offsetHeight;
        if (sh > 0 && Number.isFinite(sh)) {
          const normalizedSh = Math.max(31, Math.round(sh));
          setToolbarSubbarHeight((prev) =>
            Math.abs(prev - normalizedSh) > 0.5 ? normalizedSh : prev,
          );
        }
      } else {
        setToolbarSubbarHeight((prev) => (prev !== 31 ? 31 : prev));
      }
    };
    measure();
    if (typeof ResizeObserver === "undefined") return () => {};
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    if (mainEl && mainEl !== el) {
      observer.observe(mainEl);
    }
    if (subbarEl && subbarEl !== el) {
      observer.observe(subbarEl);
    }
    return () => observer.disconnect();
  }, [
    showToolbar,
    targetLang,
    activeTool,
    selectedAnnotationKind,
    effectiveAnnotationsCount,
    saving,
    retranslating,
    toolbarMaxWidth,
  ]);

  const isDraggingAnnotation = draftAnnotation !== null || isTransforming;
  const isEditingText = textDraft !== null;
  const canInteractAnnotations =
    draftAnnotation === null &&
    (activeTool === "none" ||
      activeTool === "rect" ||
      activeTool === "ellipse" ||
      activeTool === "text");

  useEffect(() => {
    if (typeof window === "undefined" || typeof window.addEventListener !== "function") {
      return () => {};
    }
    const onMove = (event: Event) => {
      if (!draftAnnotationRef.current && !activeTransformRef.current) return;
      if (isForeignPointerEvent(event)) return;
      const e = event as { clientX?: unknown; clientY?: unknown };
      if (
        typeof e.clientX === "number" &&
        Number.isFinite(e.clientX) &&
        typeof e.clientY === "number" &&
        Number.isFinite(e.clientY)
      ) {
        if (activeTransformRef.current) {
          updateTransformAtClientPoint(e.clientX, e.clientY);
        } else {
          updateDraftAtClientPoint(e.clientX, e.clientY);
        }
      }
    };
    const onUp = (event: Event) => {
      if (!draftAnnotationRef.current && !activeTransformRef.current) return;
      finishPointerInteraction(
        event as { clientX?: unknown; clientY?: unknown; pointerId?: unknown },
      );
    };
    const onPointerCancel = (event: Event) => {
      if (!draftAnnotationRef.current && !activeTransformRef.current) return;
      if (isForeignPointerEvent(event)) return;
      finishPointerInteraction();
    };
    const onBlur = () => {
      if (!draftAnnotationRef.current && !activeTransformRef.current) return;
      finishPointerInteraction();
    };
    window.addEventListener("pointermove", onMove, true);
    window.addEventListener("mousemove", onMove, true);
    window.addEventListener("pointerup", onUp, true);
    window.addEventListener("mouseup", onUp, true);
    window.addEventListener("pointercancel", onPointerCancel, true);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("pointermove", onMove, true);
      window.removeEventListener("mousemove", onMove, true);
      window.removeEventListener("pointerup", onUp, true);
      window.removeEventListener("mouseup", onUp, true);
      window.removeEventListener("pointercancel", onPointerCancel, true);
      window.removeEventListener("blur", onBlur);
    };
  }, [
    finishPointerInteraction,
    isForeignPointerEvent,
    updateDraftAtClientPoint,
    updateTransformAtClientPoint,
  ]);

  useEffect(() => {
    const rects: CaptureHitRect[] = [];
    const root = rootElRef.current;
    if (root && typeof root.querySelectorAll === "function") {
      const nodes = root.querySelectorAll("[data-hit-rect='true']");
      for (const node of nodes) {
        const rect = node.getBoundingClientRect?.();
        if (
          rect &&
          Number.isFinite(rect.width) &&
          Number.isFinite(rect.height) &&
          rect.width > 0 &&
          rect.height > 0
        ) {
          rects.push({ x: rect.left, y: rect.top, w: rect.width, h: rect.height });
        }
      }
    }
    if (rects.length === 0 && stageRect.w > 0 && stageRect.h > 0) {
      if (activeTool !== "none" && effectiveViewW > 0 && viewH > 0) {
        rects.push({ x: 0, y: 0, w: effectiveViewW, h: viewH });
      } else {
        rects.push({ x: stageRect.x, y: stageRect.y, w: stageRect.w, h: stageRect.h });
        if (showToolbar) {
          rects.push({
            x: toolbarLeft,
            y:
              subbarAbove && hasSubbarVisible
                ? Math.max(0, toolbarTop - subbarNeedSpace)
                : toolbarTop,
            w: measuredBarW,
            h: effectiveBarH + (hasSubbarVisible ? subbarNeedSpace + 3 : 4),
          });
        }
      }
    }
    if (rects.length > 0) {
      void api.setCaptureHitRects(rects, isDraggingAnnotation, generationRef.current);
    }
    return () => {};
  }, [
    activeTool,
    annotations.length,
    blocks,
    effectiveBarH,
    effectiveViewW,
    error,
    actionError,
    hasSubbarVisible,
    isDraggingAnnotation,
    isEditingText,
    measuredBarW,
    notice,
    saveToast,
    openPopover,
    retranslating,
    selectedAnnotationId,
    showToolbar,
    activeSize,
    stage,
    stageRect.h,
    stageRect.w,
    stageRect.x,
    stageRect.y,
    targetLang,
    textDraft?.id ?? null,
    textDraft?.value.length ?? 0,
    textDraft?.x ?? null,
    textDraft?.y ?? null,
    toolbarLeft,
    toolbarTop,
    transformRevision,
    subbarAbove,
    subbarNeedSpace,
    viewH,
  ]);

  return (
    <div
      ref={rootElRef}
      className="relative h-full w-full select-none overflow-hidden"
      style={{ fontFamily: OVERLAY_FONT }}
      onContextMenu={(e) => {
        if (!isAnnotationTextInputTarget(e.target)) {
          e.preventDefault();
        }
      }}
      onDragStart={(e) => {
        if (!isAnnotationTextInputTarget(e.target)) {
          e.preventDefault();
        }
      }}
      onPointerUpCapture={(e) => finishPointerInteraction(e)}
      onMouseUpCapture={(e) => finishPointerInteraction(e)}
      onClick={() => {
        if (openPopover !== "none") {
          openPopoverRef.current = "none";
          setOpenPopover("none");
        }
      }}
    >
      <OverlayBoundary blocks={blocks}>
        {activeTool !== "none" && (
          <div
            data-testid="capture-annotation-backdrop"
            data-hit-rect="true"
            onPointerDown={handleStagePointerDown}
            onMouseDown={handleStageMouseDown}
            onPointerMove={handleStagePointerMove}
            onPointerUp={finishPointerInteraction}
            onPointerCancel={(e) => {
              if (!isForeignPointerEvent(e)) finishPointerInteraction();
            }}
            onContextMenu={(e) => e.preventDefault()}
            className={`absolute inset-0 z-[15] ${
              activeTool === "text" ? "cursor-text" : "cursor-crosshair"
            }`}
          />
        )}

        {/* 选区画布：冻结原图 + 原位译文层 + 钉钉标注 SVG 层共用同一原点 */}
        <div
          ref={stageElRef}
          data-testid="capture-stage"
          data-hit-rect="true"
          onPointerDown={(e) => {
            if (
              activeTool === "none" &&
              e.button === 0 &&
              !e.ctrlKey &&
              selectedAnnotationIdRef.current !== null
            ) {
              selectedAnnotationIdRef.current = null;
              setSelectedAnnotationId(null);
            }
          }}
          className="absolute"
          style={{
            left: stageRect.x,
            top: stageRect.y,
            width: stageRect.w,
            height: stageRect.h,
          }}
        >
          <div className="pointer-events-none absolute inset-0 overflow-hidden">
            {originalImageUrl && (
              <img
                src={originalImageUrl}
                alt=""
                draggable={false}
                data-testid="capture-original-image"
                className="pointer-events-none absolute inset-0 h-full w-full select-none object-fill"
              />
            )}

            <OverlayView
              blocks={blocks}
              error={actionError ?? error}
              notice={saveToast ?? notice}
              stage={stage}
              hint={hint}
              bounds={{ w: stageRect.w, h: stageRect.h }}
              hideTranslation={showOriginal}
              hideFloatingEsc={showToolbar}
              hideChrome
            />
          </div>

          {(annotations.length > 0 || draftAnnotation !== null || activeTool !== "none") && (
            <svg
              data-testid="capture-annotations-svg"
              width={stageRect.w}
              height={stageRect.h}
              viewBox={`0 0 ${stageRect.w} ${stageRect.h}`}
              style={{ overflow: "visible" }}
              onPointerDown={handleStagePointerDown}
              onMouseDown={handleStageMouseDown}
              onPointerMove={handleStagePointerMove}
              onPointerUp={finishPointerInteraction}
              onPointerCancel={(e) => {
                if (!isForeignPointerEvent(e)) finishPointerInteraction();
              }}
              onContextMenu={(e) => e.preventDefault()}
              className={`absolute inset-0 z-20 h-full w-full ${
                activeTool === "none"
                  ? "pointer-events-none"
                  : activeTool === "text"
                    ? "cursor-text"
                    : "cursor-crosshair"
              }`}
            >
              {annotations.map((item) => (
                <AnnotationShape key={item.id} item={item} stageWidth={stageRect.w} />
              ))}
              {draftAnnotation && <AnnotationShape item={draftAnnotation} stageWidth={stageRect.w} />}

              {canInteractAnnotations &&
                annotations.map((ann) => {
                  if (ann.kind === "rect") {
                    const r = normalizeRect(ann.x1, ann.y1, ann.x2, ann.y2);
                    const hitStroke = Math.max(12, ann.strokeWidth + 8);
                    return (
                      <rect
                        key={`hit-${ann.id}`}
                        data-testid={`annotation-hit-${ann.id}`}
                        x={r.x}
                        y={r.y}
                        width={r.w}
                        height={r.h}
                        fill="none"
                        stroke="transparent"
                        strokeWidth={hitStroke}
                        pointerEvents="stroke"
                        style={{ pointerEvents: "stroke", cursor: "move" }}
                        onMouseDown={handleTransformMouseDown}
                        onPointerDown={(e) => startMoveAnnotation(ann.id, e)}
                      />
                    );
                  }
                  if (ann.kind === "ellipse") {
                    const r = normalizeRect(ann.x1, ann.y1, ann.x2, ann.y2);
                    const hitStroke = Math.max(12, ann.strokeWidth + 8);
                    return (
                      <ellipse
                        key={`hit-${ann.id}`}
                        data-testid={`annotation-hit-${ann.id}`}
                        cx={r.x + r.w / 2}
                        cy={r.y + r.h / 2}
                        rx={r.w / 2}
                        ry={r.h / 2}
                        fill="none"
                        stroke="transparent"
                        strokeWidth={hitStroke}
                        pointerEvents="stroke"
                        style={{ pointerEvents: "stroke", cursor: "move" }}
                        onMouseDown={handleTransformMouseDown}
                        onPointerDown={(e) => startMoveAnnotation(ann.id, e)}
                      />
                    );
                  }
                  if (ann.kind === "text") {
                    const bounds = computeTextAnnotationBounds(
                      ann,
                      stageRect.w,
                      measureOverlayAnnotationText,
                    );
                    const frame = getTextSelectionFrame(bounds);
                    return (
                      <rect
                        key={`hit-${ann.id}`}
                        data-testid={`annotation-hit-${ann.id}`}
                        x={frame.x}
                        y={frame.y}
                        width={frame.w}
                        height={frame.h}
                        rx={3}
                        fill="transparent"
                        pointerEvents="all"
                        style={{ pointerEvents: "all", cursor: "move" }}
                        onMouseDown={handleTransformMouseDown}
                        onPointerDown={(e) => startMoveAnnotation(ann.id, e)}
                      />
                    );
                  }
                  return null;
                })}

              {canInteractAnnotations &&
                selectedAnnotation &&
                (() => {
                  if (selectedAnnotation.kind !== "text") {
                    const r = normalizeRect(
                      selectedAnnotation.x1,
                      selectedAnnotation.y1,
                      selectedAnnotation.x2,
                      selectedAnnotation.y2,
                    );
                    const allHandles = getShapeResizeHandles(r);
                    const orderedHandles = [
                      ...allHandles.filter((h) => h.handle.length === 1),
                      ...allHandles.filter((h) => h.handle.length === 2),
                    ];
                    return (
                      <g data-testid="annotation-selection-gizmo">
                        <rect
                          data-testid="annotation-selection-frame"
                          x={r.x}
                          y={r.y}
                          width={r.w}
                          height={r.h}
                          fill="none"
                          stroke="#2f7cf6"
                          strokeWidth={1}
                          strokeDasharray="4 3"
                          pointerEvents="none"
                          style={{ pointerEvents: "none" }}
                        />
                        {orderedHandles.map((h) => (
                          <rect
                            key={h.handle}
                            data-testid={`annotation-handle-${h.handle}`}
                            data-hit-rect="true"
                            x={h.x - 4}
                            y={h.y - 4}
                            width={8}
                            height={8}
                            rx={1.5}
                            fill="#ffffff"
                            stroke="#2f7cf6"
                            strokeWidth={1.5}
                            pointerEvents="all"
                            style={{ pointerEvents: "all", cursor: h.cursor }}
                            onMouseDown={handleTransformMouseDown}
                            onPointerDown={(e) =>
                              startResizeShapeAnnotation(selectedAnnotation.id, h.handle, e)
                            }
                          />
                        ))}
                      </g>
                    );
                  }

                  const bounds = computeTextAnnotationBounds(
                    selectedAnnotation,
                    stageRect.w,
                    measureOverlayAnnotationText,
                  );
                  const frame = getTextSelectionFrame(bounds);
                  const handles = getTextResizeHandles(bounds);
                  return (
                    <g data-testid="annotation-selection-gizmo">
                      <rect
                        data-testid="annotation-selection-frame"
                        x={frame.x}
                        y={frame.y}
                        width={frame.w}
                        height={frame.h}
                        rx={3}
                        fill="none"
                        stroke="#2f7cf6"
                        strokeWidth={1.2}
                        strokeDasharray="4 3"
                        pointerEvents="none"
                        style={{ pointerEvents: "none" }}
                      />
                      {handles.map((h) => (
                        <rect
                          key={h.handle}
                          data-testid={`annotation-handle-${h.handle}`}
                          data-hit-rect="true"
                          x={h.x - 4}
                          y={h.y - 4}
                          width={8}
                          height={8}
                          rx={1.5}
                          fill="#ffffff"
                          stroke="#2f7cf6"
                          strokeWidth={1.5}
                          pointerEvents="all"
                          style={{ pointerEvents: "all", cursor: h.cursor }}
                          onMouseDown={handleTransformMouseDown}
                          onPointerDown={(e) =>
                            startResizeTextAnnotation(selectedAnnotation.id, h.handle, e)
                          }
                        />
                      ))}
                    </g>
                  );
                })()}
            </svg>
          )}

          {textDraft && (() => {
            const draftId = textDraft.id;
            const box = computeTextBoxLayout({
              x: textDraft.x,
              y: textDraft.y,
              value: textDraft.value,
              fontSize: textFontSize,
              stageWidth: stageRect.w,
              stageHeight: stageRect.h,
              measureFn: measureOverlayAnnotationText,
            });
            return (
              <>
                <textarea
                  key={draftId}
                  ref={textInputRef}
                  data-testid="annotation-text-input"
                  data-draft-id={String(draftId)}
                  data-hit-rect="true"
                  aria-label="输入标注文字"
                  autoFocus
                  rows={box.rows}
                  value={textDraft.value}
                  onPointerDown={(e) => e.stopPropagation()}
                  onMouseDown={(e) => e.stopPropagation()}
                  onCompositionStart={() => {
                    if (textDraftRef.current?.id !== draftId) return;
                    setIsComposingText(true);
                  }}
                  onCompositionEnd={(e) => {
                    if (textDraftRef.current?.id !== draftId) return;
                    setIsComposingText(false);
                    const nextVal = (e.currentTarget as HTMLTextAreaElement | undefined)?.value;
                    if (typeof nextVal === "string" && textDraftRef.current.value !== nextVal) {
                      textDraftRef.current = { ...textDraftRef.current, value: nextVal };
                      setTextDraft((prev) => {
                        if (!prev || prev.id !== draftId || prev.value === nextVal) return prev;
                        return { ...prev, value: nextVal };
                      });
                    }
                  }}
                  onChange={(e) => {
                    const val = e.target.value;
                    if (textDraftRef.current?.id === draftId) {
                      textDraftRef.current = { ...textDraftRef.current, value: val };
                    }
                    setTextDraft((prev) => {
                      if (!prev || prev.id !== draftId) return prev;
                      return { ...prev, value: val };
                    });
                  }}
                  onBlur={(e) => {
                    if (
                      typeof document !== "undefined" &&
                      typeof document.hasFocus === "function" &&
                      !document.hasFocus()
                    ) {
                      return;
                    }
                    const currentVal = (e?.currentTarget as HTMLTextAreaElement | undefined)?.value;
                    if (
                      typeof currentVal === "string" &&
                      textDraftRef.current?.id === draftId &&
                      textDraftRef.current.value !== currentVal
                    ) {
                      textDraftRef.current = { ...textDraftRef.current, value: currentVal };
                    }
                    flushTextDraft(draftId);
                  }}
                  onKeyDown={(e) => {
                    e.stopPropagation();
                    const native = (e as { nativeEvent?: { isComposing?: boolean; keyCode?: number } }).nativeEvent;
                    if (native?.isComposing || (e as { keyCode?: number }).keyCode === 229) {
                      return;
                    }
                    if (e.key === "Enter" && !e.shiftKey) {
                      e.preventDefault();
                      flushTextDraft(draftId);
                    } else if (e.key === "Escape") {
                      e.preventDefault();
                      textDraftRef.current = null;
                      setTextDraft(null);
                      setIsComposingText(false);
                    }
                  }}
                  className="absolute z-30 box-border select-text resize-none overflow-x-hidden overflow-y-auto rounded border border-white/75 bg-black/55 font-semibold outline-none"
                  style={{
                    left: box.left,
                    top: box.top,
                    width: box.width,
                    minWidth: box.minWidth,
                    maxWidth: box.maxWidth,
                    maxHeight: box.maxHeight,
                    padding: `${TEXT_DRAFT_PAD_Y - 1}px ${TEXT_DRAFT_PAD_X - 1}px`,
                    lineHeight: `${box.lineHeight}px`,
                    color: activeColor,
                    fontSize: textFontSize,
                    whiteSpace: "pre-wrap",
                    overflowWrap: "break-word",
                    wordBreak: "break-word",
                  }}
                />
                {!textDraft.value && !isComposingText && box.width > TEXT_DRAFT_PAD_X * 2 && (
                  <span
                    data-testid="annotation-text-placeholder"
                    aria-hidden="true"
                    className="pointer-events-none absolute z-30 select-none overflow-hidden whitespace-nowrap font-semibold text-white/55"
                    style={{
                      left: box.textX,
                      top: box.textY,
                      maxWidth: Math.max(0, box.width - TEXT_DRAFT_PAD_X * 2),
                      lineHeight: `${box.lineHeight}px`,
                      fontSize: textFontSize,
                    }}
                  >
                    输入文字
                  </span>
                )}
              </>
            );
          })()}
        </div>

        <div
          data-testid="capture-chrome"
          className="pointer-events-none absolute z-10"
          style={{
            left: chromeLeft,
            top: stageRect.y,
            width: chromeWidth,
            minHeight: chromeMinHeight,
          }}
        >
          <OverlayChrome
            blocks={blocks}
            error={actionError ?? error}
            notice={saveToast ?? notice}
            stage={stage}
            hint={hint}
            hideTranslation={showOriginal}
            hideFloatingEsc={showToolbar}
          />
        </div>
      </OverlayBoundary>

      {showToolbar && (
        <div
          ref={toolbarAnchorRef}
          data-testid="capture-toolbar-anchor"
          className="pointer-events-none absolute z-30"
          style={{ left: toolbarLeft, top: toolbarTop }}
        >
          <CaptureToolbar
            targetLang={targetLang}
            retranslating={retranslating}
            showOriginal={showOriginal}
            onToggleOriginal={setShowOriginal}
            onSelectLang={handleSelectLang}
            activeTool={activeTool}
            onSelectTool={handleSelectTool}
            activeColor={displayedColor}
            onSelectColor={handleSelectColor}
            activeSize={displayedSize}
            onSelectSize={handleSelectSize}
            selectedAnnotationKind={selectedAnnotationKind}
            currentFontSize={displayedFontSize}
            onAdjustFontSize={handleAdjustFontSize}
            annotationsCount={effectiveAnnotationsCount}
            onUndo={handleUndo}
            onSave={handleSave}
            saving={saving}
            hasImage={hasImage || !!originalImageUrl}
            openPopover={openPopover}
            onTogglePopover={handleTogglePopover}
            popoverUp={popoverUp}
            subbarAbove={subbarAbove}
            popoverMaxHeight={popoverMaxHeight}
            maxWidth={toolbarMaxWidth}
            onClose={handleClose}
          />
        </div>
      )}
    </div>
  );
}

function AnnotationShape({ item, stageWidth }: { item: Annotation; stageWidth?: number }) {
  if (item.kind === "rect") {
    const r = normalizeRect(item.x1, item.y1, item.x2, item.y2);
    return (
      <rect
        x={r.x}
        y={r.y}
        width={r.w}
        height={r.h}
        fill="none"
        stroke={item.color}
        strokeWidth={item.strokeWidth}
      />
    );
  }
  if (item.kind === "ellipse") {
    const r = normalizeRect(item.x1, item.y1, item.x2, item.y2);
    return (
      <ellipse
        cx={r.x + r.w / 2}
        cy={r.y + r.h / 2}
        rx={r.w / 2}
        ry={r.h / 2}
        fill="none"
        stroke={item.color}
        strokeWidth={item.strokeWidth}
      />
    );
  }
  if (item.kind === "arrow") {
    const [tip, left, right] = computeArrowHeadPoints(
      item.x1,
      item.y1,
      item.x2,
      item.y2,
      item.strokeWidth,
    );
    const baseMidX = (left.x + right.x) / 2;
    const baseMidY = (left.y + right.y) / 2;
    return (
      <g>
        <line
          x1={item.x1}
          y1={item.y1}
          x2={baseMidX}
          y2={baseMidY}
          stroke={item.color}
          strokeWidth={item.strokeWidth}
          strokeLinecap="round"
        />
        <polygon
          points={`${tip.x},${tip.y} ${left.x},${left.y} ${right.x},${right.y}`}
          fill={item.color}
        />
      </g>
    );
  }
  if (item.kind === "pen") {
    const pts = item.points.map((p) => `${p.x},${p.y}`).join(" ");
    return (
      <polyline
        points={pts}
        fill="none"
        stroke={item.color}
        strokeWidth={item.strokeWidth}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    );
  }
  if (item.kind !== "text") return null;
  const lineH = Math.round(item.fontSize * 1.25);
  const maxLineW =
    typeof stageWidth === "number" && stageWidth > 0
      ? Math.max(1, stageWidth - item.x - TEXT_DRAFT_PAD_X)
      : Infinity;
  const lines = Number.isFinite(maxLineW)
    ? wrapAnnotationText(item.text, item.fontSize, maxLineW, measureOverlayAnnotationText)
    : item.text.split("\n");
  return (
    <text
      x={item.x}
      y={item.y}
      fill={item.color}
      fontSize={item.fontSize}
      fontWeight={600}
      dominantBaseline="text-before-edge"
      style={{ textShadow: "0 1px 2px rgba(0,0,0,0.45)", whiteSpace: "pre" }}
    >
      {lines.map((line, idx) => (
        <tspan
          key={idx}
          x={item.x}
          y={item.y + idx * lineH}
          dominantBaseline="text-before-edge"
        >
          {line}
        </tspan>
      ))}
    </text>
  );
}

export function OverlayChrome({
  blocks,
  error,
  notice,
  stage,
  hint,
  hideTranslation = false,
  hideFloatingEsc = false,
}: {
  blocks: OverlayBlock[] | null;
  error: string | null;
  notice: string | null;
  stage: CaptureStage;
  hint: boolean;
  hideTranslation?: boolean;
  hideFloatingEsc?: boolean;
}) {
  const nothingToReplace = stage === "done" && isNothingToReplace(blocks);
  const statusText = stage === "ocr" ? "识别中… · Esc 取消" : stage === "translating" && !error ? "翻译中… · Esc 取消" : null;

  return (
    <>
      {statusText && (
        <div
          role="status"
          className="pointer-events-none absolute left-2 top-2 z-10 whitespace-nowrap rounded-full bg-[rgba(14,14,18,0.9)] px-3 py-1 text-[12px] text-[#f6f3ed]"
        >
          {statusText}
        </div>
      )}

      {nothingToReplace && !error && !hideTranslation && (
        <div
          role="status"
          data-hit-rect="true"
          className="pointer-events-auto absolute inset-x-1 top-1 z-10 rounded bg-[rgba(14,14,18,0.95)] px-2.5 py-1.5 text-[12.5px] leading-snug text-[#f6f3ed]"
        >
          译文和原文一样，没有要替换的内容。点击关闭按钮或 Esc 退出。
        </div>
      )}

      {stage === "done" && blocks !== null && blocks.length === 0 && !error && (
        <div
          role="status"
          data-hit-rect="true"
          className="pointer-events-auto absolute inset-x-1 top-1 z-10 rounded bg-[rgba(14,14,18,0.95)] px-2.5 py-1.5 text-[12.5px] leading-snug text-[#f6f3ed]"
        >
          没识别到文字，仍可在工具条里标注或保存原图 · Esc 关闭
        </div>
      )}

      {error && (
        <div
          role="alert"
          data-hit-rect="true"
          className="pointer-events-auto absolute inset-x-1 top-1 z-10 max-h-[calc(100%-8px)] overflow-y-auto rounded border border-[rgba(255,107,74,0.6)] bg-[rgba(48,14,10,0.97)] px-2.5 py-1.5 text-[12.5px] leading-snug text-[#ffd8cc]"
        >
          翻译失败：{error.replace(/[。.\s]+$/, "")}。
          {blocks && blocks.length > 0 ? "下面是识别出的原文，" : ""}可在工具条切换目标语言重试，或按 Esc 关闭。
        </div>
      )}

      {notice && !error && (
        <div
          role="status"
          data-hit-rect="true"
          className="pointer-events-auto absolute inset-x-1 bottom-1 z-10 max-h-[calc(100%-8px)] overflow-y-auto rounded border border-[rgba(240,180,94,0.6)] bg-[rgba(44,32,10,0.97)] px-2.5 py-1.5 text-[12.5px] leading-snug text-[#f6e3bd]"
        >
          {notice}
        </div>
      )}

      {hint && !hideFloatingEsc && stage === "done" && !error && !notice && !nothingToReplace && blocks !== null && blocks.length > 0 && (
        <div className="pointer-events-none absolute right-2 top-2 rounded-full bg-[rgba(14,14,18,0.9)] px-3 py-1 text-[12px] text-[#b0aaa1]">
          Esc 关闭
        </div>
      )}
    </>
  );
}

/** 排版、画层和各种提示条：全部数据相关的计算都在这里，出错由外面的错误边界接住。 */
export function OverlayView({
  blocks,
  error,
  notice,
  stage,
  hint,
  bounds,
  hideTranslation = false,
  hideFloatingEsc = false,
  hideChrome = false,
}: {
  blocks: OverlayBlock[] | null;
  error: string | null;
  notice: string | null;
  stage: CaptureStage;
  hint: boolean;
  bounds?: OverlayBounds | null;
  hideTranslation?: boolean;
  hideFloatingEsc?: boolean;
  hideChrome?: boolean;
}) {
  const [viewport, setViewport] = useState(() => ({
    w: typeof window !== "undefined" ? window.innerWidth : 0,
    h: typeof window !== "undefined" ? window.innerHeight : 0,
  }));

  useEffect(() => {
    if (typeof window === "undefined" || typeof window.addEventListener !== "function") return;
    const onResize = () => setViewport({ w: window.innerWidth, h: window.innerHeight });
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const viewW = bounds?.w ?? (typeof window !== "undefined" ? window.innerWidth : viewport.w);
  const viewH = bounds?.h ?? (typeof window !== "undefined" ? window.innerHeight : viewport.h);

  const plans = useMemo(() => {
    if (!blocks) return [];
    const targetBounds = { w: viewW, h: viewH };
    try {
      return planOverlay(blocks, createCanvasMeasure(), targetBounds);
    } catch (err) {
      console.error("[overlay] 排版出错，整屏按简单样式显示：", err);
      return blocks.map((block) => plainPlan(block, targetBounds));
    }
  }, [blocks, viewW, viewH, viewport.w, viewport.h]);

  return (
    <>
      {blocks && !hideTranslation && (
        <OverlayLayers blocks={blocks} plans={plans} pending={stage === "translating"} />
      )}

      {!hideChrome && (
        <OverlayChrome
          blocks={blocks}
          error={error}
          notice={notice}
          stage={stage}
          hint={hint}
          hideTranslation={hideTranslation}
          hideFloatingEsc={hideFloatingEsc}
        />
      )}
    </>
  );
}

/** 把抛出来的东西说成一句人话（可能是 Error、字符串、甚至 undefined）。 */
export function describeFailure(error: unknown): string {
  const text = error instanceof Error ? error.message : typeof error === "string" ? error : "未知错误";
  return text.trim().slice(0, 200) || "未知错误";
}

/**
 * 画的部分出错时的退路：一条错误提示 + 逐块列出译文原文本。
 * 越简单越好——它自己要是也抛，就没有下一层了。不做任何计算，只对数据做最保守的取用。
 */
export function OverlayFallback({ blocks, failure }: { blocks: OverlayBlock[] | null; failure: string }) {
  const texts = (Array.isArray(blocks) ? blocks : [])
    .map((block) => (block !== null && typeof block === "object" && typeof block.text === "string" ? block.text : ""))
    .filter((text) => text.trim() !== "");
  return (
    <div
      role="alert"
      className="absolute inset-x-1 top-1 z-10 max-h-[calc(100%-8px)] overflow-y-auto rounded border border-[rgba(255,107,74,0.6)] bg-[rgba(48,14,10,0.97)] px-2.5 py-1.5 text-[12.5px] leading-snug text-[#ffd8cc]"
    >
      没能原位显示译文：{failure}。{texts.length > 0 ? "下面是译文，" : ""}点击关闭按钮或 Esc 退出。
      {texts.map((text, index) => (
        <div key={index} className="mt-1 whitespace-pre-wrap text-[#f6f3ed]">
          {text}
        </div>
      ))}
    </div>
  );
}

/** 错误边界：里面渲染抛出来，就换成 `OverlayFallback`，外面的关闭处理器不受影响。 */
export class OverlayBoundary extends Component<
  { blocks: OverlayBlock[] | null; children: ReactNode },
  { failure: string | null }
> {
  state: { failure: string | null } = { failure: null };

  static getDerivedStateFromError(error: unknown) {
    return { failure: describeFailure(error) };
  }

  componentDidCatch(error: unknown) {
    console.error("[overlay] 渲染出错，退成错误提示：", error);
  }

  render() {
    return this.state.failure === null ? (
      this.props.children
    ) : (
      <OverlayFallback blocks={this.props.blocks} failure={this.state.failure} />
    );
  }
}

/**
 * 所有块的译文都和原文一样（命令、版本号、专有名词这类会原样保留）：覆盖层里一块都不画，
 * 屏幕上看不出任何变化，得说一声，别让人以为没反应。
 */
export function isNothingToReplace(blocks: OverlayBlock[] | null): boolean {
  return !!blocks && blocks.length > 0 && blocks.every((block) => block?.unchanged === true);
}

/**
 * 画层：先把所有原文盖住，再画译文——相邻两块的遮盖矩形不能压在对方的译文上。
 * 译文和原文一样的块不画，原来的像素一点不动（比用另一种字体照着再画一遍更像原文，粗体标题也不会被换成常规字重）。
 */
export function OverlayLayers({
  blocks,
  plans,
  pending = false,
}: {
  blocks: OverlayBlock[];
  plans: BlockPlan[];
  pending?: boolean;
}) {
  const someTranslated = blocks.some((block) => block?.translated === true);
  const shouldDraw = (block: OverlayBlock | undefined) => {
    if (!block || block.unchanged) return false;
    return !pending || block.translated === true;
  };
  return (
    <>
      {plans.map((plan, index) => (shouldDraw(blocks[index]) ? <CoverView key={`cover-${index}`} plan={plan} /> : null))}
      {plans.map((plan, index) =>
        shouldDraw(blocks[index]) ? (
          <TextView
            key={`text-${index}`}
            plan={plan}
            translated={blocks[index]?.translated ?? false}
            flagUntranslated={!pending && someTranslated}
          />
        ) : null,
      )}
    </>
  );
}

/** 盖住原文：用原文底下的底色纯色填满，没有圆角——圆角会在四个角上露出原文的毛边。 */
function CoverView({ plan }: { plan: BlockPlan }) {
  const { cover } = plan;
  return (
    <div
      aria-hidden
      data-role="cover"
      className="absolute"
      style={{ left: cover.x, top: cover.y, width: cover.w, height: cover.h, background: plan.bg }}
    />
  );
}

/**
 * 译文：一行一个元素，行已经折好了（见 `wrapText`），浏览器不再自己折，排出来才和预估的一致。
 */
function TextView({ plan, translated, flagUntranslated }: { plan: BlockPlan; translated: boolean; flagUntranslated: boolean }) {
  const flagged = !translated && flagUntranslated;
  return (
    <div
      data-role="text"
      data-size={plan.fontSize}
      data-truncated={plan.truncated ? "true" : undefined}
      data-anchor={plan.anchor}
      data-untranslated={flagged ? "true" : undefined}
      className="absolute"
      style={{
        left: plan.x,
        top: plan.y,
        transform: plan.anchor === "right" ? "translateX(-100%)" : undefined,
        textAlign: plan.anchor === "right" ? "right" : undefined,
        fontSize: plan.fontSize,
        lineHeight: `${plan.lineHeight}px`,
        color: plan.fg,
        whiteSpace: "pre",
        opacity: translated ? 1 : 0.82,
        ...(flagged ? { textDecoration: "underline dotted", textUnderlineOffset: 2 } : {}),
      }}
    >
      {plan.lines.map((line, index) => (
        <div key={index} dir={plan.dir} style={{ height: plan.lineHeight }}>
          {line}
        </div>
      ))}
    </div>
  );
}
