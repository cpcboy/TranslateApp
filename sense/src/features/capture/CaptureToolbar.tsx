import type { Language, SaveImageKind } from "../../lib/ipc";
import { CAPTURE_LANGUAGES } from "../../lib/languages";
import {
  ANNOTATION_COLORS,
  ANNOTATION_SIZES,
  type AnnotationSize,
  type AnnotationTool,
} from "./captureAnnotations";

export type ToolbarDock = "bottom" | "top" | "inside-bottom";
export type ToolbarPopover = "none" | "lang" | "save";

const TOOL_ITEMS: { id: Exclude<AnnotationTool, "none">; label: string }[] = [
  { id: "rect", label: "矩形" },
  { id: "ellipse", label: "椭圆" },
  { id: "arrow", label: "箭头" },
  { id: "pen", label: "画笔" },
  { id: "text", label: "文字" },
];

function ToolIcon({ tool }: { tool: Exclude<AnnotationTool, "none"> }) {
  if (tool === "rect") {
    return (
      <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.5">
        <rect x="2.5" y="3.5" width="11" height="9" rx="1" />
      </svg>
    );
  }
  if (tool === "ellipse") {
    return (
      <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.5">
        <ellipse cx="8" cy="8" rx="5.5" ry="4.2" />
      </svg>
    );
  }
  if (tool === "arrow") {
    return (
      <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
        <line x1="3" y1="13" x2="12.5" y2="3.5" />
        <polyline points="7 3.5 12.5 3.5 12.5 9" />
      </svg>
    );
  }
  if (tool === "pen") {
    return (
      <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
        <path d="M2.5 11.5c1.5-3.5 3.2-5 4.8-3.8 1.5 1.2 1.8 3.6 3.5 2.4 1-.7 2-2.8 2.7-5.1" />
      </svg>
    );
  }
  return <span className="text-[12px] font-semibold leading-none">T</span>;
}

export function CaptureToolbar({
  targetLang,
  retranslating,
  showOriginal,
  onToggleOriginal,
  onSelectLang,
  activeTool,
  onSelectTool,
  activeColor,
  onSelectColor,
  activeSize,
  onSelectSize,
  annotationsCount,
  onUndo,
  onSave,
  saving,
  hasImage,
  openPopover,
  onTogglePopover,
  popoverUp,
  subbarAbove,
  popoverMaxHeight,
  onClose,
}: {
  targetLang: string | null;
  retranslating: boolean;
  showOriginal: boolean;
  onToggleOriginal: (original: boolean) => void;
  onSelectLang: (code: string) => void;
  activeTool: AnnotationTool;
  onSelectTool: (tool: AnnotationTool) => void;
  activeColor: string;
  onSelectColor: (hex: string) => void;
  activeSize: AnnotationSize["id"];
  onSelectSize: (size: AnnotationSize["id"]) => void;
  annotationsCount: number;
  onUndo: () => void;
  onSave: (kind: SaveImageKind) => void;
  saving: boolean;
  hasImage: boolean;
  openPopover: ToolbarPopover;
  onTogglePopover: (popover: ToolbarPopover) => void;
  popoverUp: boolean;
  subbarAbove: boolean;
  popoverMaxHeight?: number;
  onClose: () => void;
}) {
  const languages: Language[] = CAPTURE_LANGUAGES;
  const activeLangName = targetLang
    ? (languages.find((item) => item.code === targetLang)?.name ?? targetLang)
    : "自动";

  const popoverPosClass = popoverUp ? "bottom-full mb-1.5" : "top-full mt-1.5";
  const subbarOrderClass = subbarAbove ? "flex-col-reverse" : "flex-col";

  return (
    <div
      data-testid="capture-toolbar"
      className={`pointer-events-auto flex ${subbarOrderClass} items-center gap-1.5 select-none`}
      onClick={(e) => e.stopPropagation()}
      onPointerDown={(e) => e.stopPropagation()}
    >
      {/* 主工具条 */}
      <div
        data-testid="capture-toolbar-main"
        data-hit-rect="true"
        className="relative flex flex-nowrap items-center gap-1 rounded-xl border border-white/15 bg-[rgba(22,23,28,0.94)] px-2 py-1 text-[12px] text-[#f4f2ec] shadow-[0_10px_28px_rgba(0,0,0,0.45)] backdrop-blur-md"
      >
        {/* 显示原文 / 显示结果 */}
        <div role="group" aria-label="视图切换" className="flex items-center rounded-lg bg-white/8 p-0.5">
          <button
            type="button"
            aria-pressed={showOriginal}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => onToggleOriginal(true)}
            className={`whitespace-nowrap rounded-md px-2 py-1 text-[11.5px] transition-colors ${
              showOriginal
                ? "bg-white text-[#141418] font-medium"
                : "text-[#c8c4bc] hover:text-white"
            }`}
          >
            显示原文
          </button>
          <button
            type="button"
            aria-pressed={!showOriginal}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => onToggleOriginal(false)}
            className={`whitespace-nowrap rounded-md px-2 py-1 text-[11.5px] transition-colors ${
              !showOriginal
                ? "bg-white text-[#141418] font-medium"
                : "text-[#c8c4bc] hover:text-white"
            }`}
          >
            显示结果
          </button>
        </div>

        <div className="mx-0.5 h-4 w-px bg-white/15" />

        {/* 翻译目标语言选择 */}
        <div className="relative">
          <button
            type="button"
            data-testid="capture-lang-btn"
            aria-expanded={openPopover === "lang"}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => onTogglePopover(openPopover === "lang" ? "none" : "lang")}
            className={`flex items-center gap-1 whitespace-nowrap rounded-lg px-2 py-1 text-[11.5px] transition-colors ${
              openPopover === "lang"
                ? "bg-white/20 text-white"
                : "bg-white/8 text-[#f4f2ec] hover:bg-white/15"
            }`}
          >
            <span>{retranslating ? "重译中…" : `译为 ${activeLangName}`}</span>
            <span className="text-[10px] text-[#a7a299]">▾</span>
          </button>

          {openPopover === "lang" && (
            <div
              role="menu"
              data-hit-rect="true"
              aria-label="选择翻译目标语言"
              style={popoverMaxHeight ? { maxHeight: popoverMaxHeight } : undefined}
              className={`absolute left-0 ${popoverPosClass} z-30 w-[240px] overflow-y-auto rounded-xl border border-white/15 bg-[rgba(24,25,31,0.98)] p-1.5 shadow-[0_12px_32px_rgba(0,0,0,0.55)]`}
            >
              <button
                type="button"
                role="menuitem"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => {
                  onTogglePopover("none");
                  onSelectLang("auto");
                }}
                className="mb-1 flex w-full items-center justify-between rounded-lg px-2.5 py-1.5 text-left text-[11.5px] text-[#e7e4dc] hover:bg-white/12"
              >
                <span>自动识别（混合语种默认译中文）</span>
              </button>
              <div className="grid grid-cols-2 gap-0.5 pr-0.5">
                {languages.map((lang) => {
                  const selected = lang.code === targetLang;
                  return (
                    <button
                      key={lang.code}
                      type="button"
                      role="menuitem"
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => {
                        onTogglePopover("none");
                        onSelectLang(lang.code);
                      }}
                      className={`flex items-center justify-between rounded-md px-2 py-1 text-left text-[11.5px] transition-colors ${
                        selected
                          ? "bg-[#2f7cf6] text-white font-medium"
                          : "text-[#dcd8cf] hover:bg-white/10"
                      }`}
                    >
                      <span className="truncate">{lang.name}</span>
                    </button>
                  );
                })}
              </div>
            </div>
          )}
        </div>

        <div className="mx-0.5 h-4 w-px bg-white/15" />

        {/* 钉钉风格标注工具 */}
        <div role="group" aria-label="图片编辑工具" className="flex items-center gap-0.5">
          {TOOL_ITEMS.map(({ id, label }) => {
            const active = activeTool === id;
            return (
              <button
                key={id}
                type="button"
                title={label}
                aria-label={label}
                aria-pressed={active}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => onSelectTool(active ? "none" : id)}
                className={`flex h-7 w-7 items-center justify-center rounded-lg transition-colors ${
                  active
                    ? "bg-[#2f7cf6] text-white"
                    : "text-[#d2cec5] hover:bg-white/12 hover:text-white"
                }`}
              >
                <ToolIcon tool={id} />
              </button>
            );
          })}
          <button
            type="button"
            title="撤销标注"
            aria-label="撤销"
            disabled={annotationsCount === 0}
            onMouseDown={(e) => e.preventDefault()}
            onClick={onUndo}
            className="flex h-7 items-center justify-center rounded-lg px-2 text-[11.5px] text-[#d2cec5] transition-colors hover:bg-white/12 hover:text-white disabled:cursor-not-allowed disabled:opacity-35"
          >
            撤销
          </button>
        </div>

        <div className="mx-0.5 h-4 w-px bg-white/15" />

        {/* 存为图片 */}
        <div className="relative">
          <button
            type="button"
            data-testid="capture-save-btn"
            disabled={saving}
            aria-expanded={openPopover === "save"}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => onTogglePopover(openPopover === "save" ? "none" : "save")}
            className="flex items-center gap-1 whitespace-nowrap rounded-lg bg-white/8 px-2 py-1 text-[11.5px] text-[#f4f2ec] transition-colors hover:bg-white/15 disabled:opacity-50"
          >
            <span>{saving ? "保存中…" : "存为图片"}</span>
            <span className="text-[10px] text-[#a7a299]">▾</span>
          </button>

          {openPopover === "save" && (
            <div
              role="menu"
              data-hit-rect="true"
              aria-label="保存图片选项"
              className={`absolute right-0 ${popoverPosClass} z-30 min-w-[156px] rounded-xl border border-white/15 bg-[rgba(24,25,31,0.98)] p-1 shadow-[0_12px_32px_rgba(0,0,0,0.55)]`}
            >
              <button
                type="button"
                role="menuitem"
                disabled={!hasImage}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => {
                  onTogglePopover("none");
                  onSave("original");
                }}
                className="flex w-full items-center rounded-lg px-2.5 py-1.5 text-left text-[11.5px] text-[#e7e4dc] hover:bg-white/12 disabled:cursor-not-allowed disabled:opacity-40"
              >
                保存未翻译原图
              </button>
              {annotationsCount > 0 && (
                <button
                  type="button"
                  role="menuitem"
                  disabled={!hasImage}
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => {
                    onTogglePopover("none");
                    onSave("original_annotated");
                  }}
                  className="flex w-full items-center rounded-lg px-2.5 py-1.5 text-left text-[11.5px] text-[#e7e4dc] hover:bg-white/12 disabled:cursor-not-allowed disabled:opacity-40"
                >
                  保存带标注原图
                </button>
              )}
              <button
                type="button"
                role="menuitem"
                disabled={!hasImage}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => {
                  onTogglePopover("none");
                  onSave("translated");
                }}
                className="flex w-full items-center rounded-lg px-2.5 py-1.5 text-left text-[11.5px] text-[#e7e4dc] hover:bg-white/12 disabled:cursor-not-allowed disabled:opacity-40"
              >
                保存翻译后图片
              </button>
            </div>
          )}
        </div>

        {/* 关闭 */}
        <button
          type="button"
          data-testid="capture-close-btn"
          aria-label="关闭"
          onMouseDown={(e) => e.preventDefault()}
          onClick={onClose}
          className="ml-0.5 whitespace-nowrap rounded-lg bg-[#e5484d]/90 px-2.5 py-1 text-[11.5px] font-medium text-white transition-colors hover:bg-[#e5484d]"
        >
          关闭
        </button>
      </div>

      {/* 钉钉风格二级属性条（颜色 + 粗细） */}
      {activeTool !== "none" && (
        <div
          data-testid="capture-subbar"
          data-hit-rect="true"
          className="flex items-center gap-2 rounded-xl border border-white/15 bg-[rgba(22,23,28,0.92)] px-2.5 py-1 text-[11px] text-[#dcd8cf] shadow-[0_6px_18px_rgba(0,0,0,0.4)] backdrop-blur-md"
        >
          <div role="group" aria-label="标注颜色" className="flex items-center gap-1.5">
            {ANNOTATION_COLORS.map((c) => {
              const selected = activeColor === c.hex;
              return (
                <button
                  key={c.id}
                  type="button"
                  title={c.label}
                  aria-label={`颜色${c.label}`}
                  aria-pressed={selected}
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => onSelectColor(c.hex)}
                  className={`h-4 w-4 rounded-full border transition-transform ${
                    selected ? "scale-110 border-white ring-2 ring-[#2f7cf6]" : "border-white/30 hover:scale-105"
                  }`}
                  style={{ backgroundColor: c.hex }}
                />
              );
            })}
          </div>
          <div className="h-3.5 w-px bg-white/15" />
          <div role="group" aria-label="线条粗细" className="flex items-center gap-1">
            {ANNOTATION_SIZES.map((s) => {
              const selected = activeSize === s.id;
              return (
                <button
                  key={s.id}
                  type="button"
                  aria-label={`粗细${s.label}`}
                  aria-pressed={selected}
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => onSelectSize(s.id)}
                  className={`rounded-md px-1.5 py-0.5 transition-colors ${
                    selected ? "bg-white/22 text-white font-medium" : "text-[#bdb8af] hover:bg-white/10"
                  }`}
                >
                  {s.label}
                </button>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
