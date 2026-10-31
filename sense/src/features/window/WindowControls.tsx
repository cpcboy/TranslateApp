import type { ReactNode } from "react";
import { api } from "../../lib/ipc";

/**
 * 标题栏左上角的三颗灯，位置和样子照 macOS 的红黄绿，各自的作用写在悬停提示里：
 * 红 = 退出应用；黄 = 隐藏窗口（同 Esc，快捷键还能再呼出）；绿 = 放大到屏幕可用区域 / 还原。
 *
 * 窗口是无边框的，系统不给这三个按钮，只能自己画。
 */
export function WindowControls() {
  // 按钮点下去窗口可能已经没了（比如刚被收起），失败不用打扰用户
  const run = (action: () => Promise<unknown>) => () => {
    void action().catch(() => {});
  };

  return (
    <div role="group" aria-label="窗口控制" className="group flex shrink-0 items-center gap-2">
      <Light label="退出 SENSE（⌘Q）" color="#ff5f57" onClick={run(api.quitApp)}>
        <path d="M1.6 1.6 L6.4 6.4 M6.4 1.6 L1.6 6.4" />
      </Light>
      <Light label="隐藏窗口（Esc），快捷键可再呼出" color="#febc2e" onClick={run(api.hideWindow)}>
        <path d="M1.4 4 H6.6" />
      </Light>
      <Light label="放大 / 还原窗口" color="#28c840" onClick={run(api.toggleZoom)}>
        <path d="M1.6 6.4 L6.4 1.6 M3.8 1.6 H6.4 V4.2 M1.6 3.8 V6.4 H4.2" />
      </Light>
    </div>
  );
}

function Light({
  label,
  color,
  onClick,
  children,
}: {
  label: string;
  color: string;
  onClick: () => void;
  /** 悬停时露出来的符号：8×8 视口里的路径 */
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      style={{ backgroundColor: color }}
      className="flex h-[13px] w-[13px] items-center justify-center rounded-full border border-black/25 outline-offset-2 focus-visible:outline-2 focus-visible:outline-acc"
    >
      {/* 三颗灯一起亮符号（悬停在任何一颗上），和系统的一样 */}
      <svg
        width="8"
        height="8"
        viewBox="0 0 8 8"
        fill="none"
        stroke="rgba(0,0,0,0.62)"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
        className="opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100"
      >
        {children}
      </svg>
    </button>
  );
}
