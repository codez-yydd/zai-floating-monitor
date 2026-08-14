import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { applyPanelAlpha, applyTheme, loadPanelAlpha, loadTheme } from "./appearance";
import "./index.css";

// 首帧前应用外观偏好（主题/透明度），避免暗色用户看到亮色闪烁
if (loadTheme() === "dark") applyTheme("dark");
applyPanelAlpha(loadPanelAlpha());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
