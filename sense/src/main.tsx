import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import { TranslateOverlay } from "./features/capture/TranslateOverlay";
import "./styles/index.css";

// 同一个前端包服务两个窗口：主窗口、译文覆盖层（选区由系统的 screencapture -i 完成，没有自绘窗口）。
// 用窗口 label 判断，而不是 URL query——生产环境里 `?view=` 可能拿不到，
// 页面会白屏（用户看到的是 WebView 的 Reload 提示）。
const label = "__TAURI_INTERNALS__" in window ? getCurrentWindow().label : "";
const query = new URLSearchParams(location.search).get("view");
const view = query ?? (label === "overlay" ? "overlay" : null);

const root = ReactDOM.createRoot(document.getElementById("root") as HTMLElement);
root.render(
  <React.StrictMode>
    {view === "overlay" ? <TranslateOverlay /> : <App />}
  </React.StrictMode>,
);
