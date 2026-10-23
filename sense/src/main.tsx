import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { CaptureOverlay } from "./features/capture/CaptureOverlay";
import { TranslateOverlay } from "./features/capture/TranslateOverlay";
import "./styles/index.css";

// 同一个前端包服务三个窗口：主窗口、选区遮罩、译文覆盖层。
const view = new URLSearchParams(location.search).get("view");

const root = ReactDOM.createRoot(document.getElementById("root") as HTMLElement);
root.render(
  <React.StrictMode>
    {view === "capture" ? <CaptureOverlay /> : view === "overlay" ? <TranslateOverlay /> : <App />}
  </React.StrictMode>,
);
