import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { applyPanelAlpha, applyTheme, loadPanelAlpha, loadTheme } from "./appearance";
import { applyLocale, detectLocale, loadLocale } from "./i18n/locale";
import { I18nProvider } from "./i18n";
import "./index.css";

// 首帧前应用外观偏好（主题/透明度）与语言偏好，避免暗色/英文用户看到闪烁
if (loadTheme() === "dark") applyTheme("dark");
applyPanelAlpha(loadPanelAlpha());
applyLocale(loadLocale() ?? detectLocale());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <I18nProvider>
      <App />
    </I18nProvider>
  </React.StrictMode>,
);
