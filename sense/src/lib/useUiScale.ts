import { useCallback, useEffect, useRef, useState } from "react";
import {
  describeScale,
  loadScale,
  nextScale,
  saveScale,
  scaleActionFor,
  type ScaleAction,
} from "./uiScale";

/** 提示在屏幕上停留多久 */
const BADGE_MS = 1400;

/**
 * ⌘+ / ⌘- / ⌘0 缩放界面。返回当前缩放倍数和一条短暂的提示文字。
 * 快捷键挂在 window 上，输入框聚焦时照样生效。
 */
export function useUiScale() {
  const [scale, setScale] = useState(loadScale);
  const [badge, setBadge] = useState<string | null>(null);
  // 按键处理里要读「最新」的缩放，又不想每次变化都重挂监听
  const current = useRef(scale);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const apply = useCallback((action: ScaleAction) => {
    const before = current.current;
    const after = nextScale(before, action);
    current.current = after;
    setScale(after);
    saveScale(after);
    setBadge(describeScale(action, before, after));
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = setTimeout(() => setBadge(null), BADGE_MS);
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const action = scaleActionFor(event);
      if (!action) return;
      event.preventDefault();
      apply(action);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      if (timer.current !== null) clearTimeout(timer.current);
    };
  }, [apply]);

  return { scale, badge };
}
