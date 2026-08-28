import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import {
  applyPanelAlpha,
  applyTheme,
  applyUiScale,
  loadPanelAlpha,
  loadTheme,
  loadUiScale,
} from "./appearance";
import { applyLocale, detectLocale, loadLocale } from "./i18n/locale";
import { I18nProvider } from "./i18n";
import { restoreWindowSize } from "./windowSize";
import "./index.css";

// 首帧前应用外观偏好（主题/透明度/整体缩放）与语言偏好，避免暗色/英文用户看到闪烁
if (loadTheme() === "dark") applyTheme("dark");
applyPanelAlpha(loadPanelAlpha());
applyUiScale(loadUiScale());
applyLocale(loadLocale() ?? detectLocale());

// 恢复持久化的窗口尺寸（百分比 → 当前显示器像素）：未存储时内部早退保持默认，
// 窗口此时仍隐藏无闪烁；fire-and-forget 不阻塞首帧
void restoreWindowSize();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <I18nProvider>
      <App />
    </I18nProvider>
  </React.StrictMode>,
);
