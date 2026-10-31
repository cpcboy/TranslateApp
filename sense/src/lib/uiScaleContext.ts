import { createContext } from "react";
import { SCALE_DEFAULT } from "./uiScale";

/**
 * 当前界面缩放倍数，由 App 提供。
 *
 * 界面缩放靠给面板内容套 CSS `zoom`。`position: fixed` 的东西（划词浮层）的坐标来自视口，
 * 套在 zoom 里写进 left / top / width 的长度会被再乘一遍倍数，要靠它除回来（见 reading.ts 的 `glossPosition`）。
 */
export const UiScaleContext = createContext<number>(SCALE_DEFAULT);
