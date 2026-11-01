import { cn } from "../../lib/cn";
import type { Language } from "../../lib/ipc";
import { AUTO, AUTO_HINT, pillLabel } from "../../lib/languages";

/**
 * 滚动条一直显示（WebKit 的自定义滚动条会取代 macOS 默认的叠加式滚动条）：默认的滚动条平时隐藏，
 * 用户看不出面板里下面还有语言。只在内容真的放不下、出现滚动条时才占地方。
 * 写成 Tailwind 的任意变体放在组件里，渲染测试盯得住（vitest 不处理 .css）。
 */
const VISIBLE_SCROLLBAR =
  "[&::-webkit-scrollbar]:w-2.5 [&::-webkit-scrollbar-track]:bg-transparent [&::-webkit-scrollbar-thumb]:rounded-[5px] " +
  "[&::-webkit-scrollbar-thumb]:border-2 [&::-webkit-scrollbar-thumb]:border-transparent " +
  "[&::-webkit-scrollbar-thumb]:bg-[rgba(248,246,241,0.4)] [&::-webkit-scrollbar-thumb]:bg-clip-content";

/**
 * 翻译成哪种语言的选择器：输入框旁一颗按钮 + 点开后输入框下面展开的一块（不是浮层）。
 * 不用 `<select>`：系统的下拉菜单会让无边框窗口失焦，而窗口失焦就自动收起；
 * 也不用悬浮的自绘菜单：展开在文档流里，不用管定位、界面缩放（CSS zoom）和点哪儿会关。
 */

/** 按钮上写着现在翻译成哪种语言。选了非默认的语言时高亮：它会改变查词的行为，不能让人忘了它开着 */
export function TargetButton({
  pref,
  languages,
  open,
  onClick,
}: {
  pref: string;
  languages: Language[];
  open: boolean;
  onClick: () => void;
}) {
  const label = pillLabel(pref, languages);
  return (
    <button
      type="button"
      onClick={onClick}
      aria-expanded={open}
      aria-label={`翻译成：${label}。点击修改`}
      title={pref === AUTO ? AUTO_HINT : `翻译成${label}`}
      className={cn(
        "shrink-0 whitespace-nowrap rounded-md border px-2.5 py-1 font-mono text-[13.5px]",
        pref === AUTO
          ? "border-line text-muted hover:border-line-strong hover:text-paper"
          : "border-acc/60 bg-[rgba(255,82,48,0.12)] text-paper",
      )}
    >
      → {label} {open ? "▴" : "▾"}
    </button>
  );
}

export function TargetPanel({
  pref,
  languages,
  error,
  onChoose,
}: {
  pref: string;
  languages: Language[];
  /** 存偏好失败的原因（已经退回了旧的选择）；没出错为 null */
  error: string | null;
  onChoose: (code: string) => void;
}) {
  return (
    // 最多占结果区上方 60% 的高度：22 种语言在小窗口、放大字号时放不下，语言那一块自己滚动。
    // 标题和错误提示留在滚动区外面——输入框不会被挤出去，存失败的原因也不会被滚到看不见的地方
    <section
      aria-label="翻译成哪种语言"
      className="flex max-h-[60%] min-h-0 shrink-0 flex-col rounded-xl border border-line bg-black/20 px-5 py-4"
    >
      <div className="flex shrink-0 flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
        <h2 className="text-[13.5px] font-normal tracking-[0.08em] text-muted">翻译成</h2>
        <p className="text-[13px] text-subtle">{AUTO_HINT}</p>
      </div>

      {error && (
        <p role="alert" className="mt-3 shrink-0 text-[13px] leading-relaxed text-[#f6b09b]">
          没能保存这个选择：{error}
        </p>
      )}

      <div className={cn(VISIBLE_SCROLLBAR, "mt-3 min-h-0 flex-1 overflow-y-auto pr-1")}>
        <div className="flex flex-wrap gap-2">
          <Chip active={pref === AUTO} onClick={() => onChoose(AUTO)}>
            自动
          </Chip>
          {languages.map((language) => (
            <Chip key={language.code} active={pref === language.code} onClick={() => onChoose(language.code)}>
              {language.name}
              {language.native !== language.name && <span className="ml-1.5 text-subtle">{language.native}</span>}
            </Chip>
          ))}
        </div>
        <p className="mt-3 text-[13px] leading-relaxed text-subtle">
          自动识别原文语言。识别有把握、而选的又恰好是原文语言时，仍按上面的默认方向译。查词卡只做中 ↔ 英，选了别的语言，单个词也按普通翻译处理。截屏翻译用同一个设置。
        </p>
      </div>
    </section>
  );
}

function Chip({ active, onClick, children }: { active: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      className={cn(
        "whitespace-nowrap rounded-full border px-3.5 py-2 text-[14.5px]",
        active
          ? "border-acc/60 bg-[rgba(255,82,48,0.12)] text-paper"
          : "border-line text-muted hover:border-line-strong hover:text-paper",
      )}
    >
      {children}
    </button>
  );
}
