import { useEffect, useState } from "react";
import { StatsPanel } from "./StatsPanel";
import { PricingPanel } from "./PricingPanel";
import { SettingsPanel } from "./SettingsPanel";
import { SyncPanel } from "./SyncPanel";
import { ComparePanel } from "./ComparePanel";
import { ReportPanel } from "./ReportPanel";
import { DataProvider } from "./DataCache";
import { fetchPricing, fetchCurrency, saveCurrency } from "./api";
import type { Currency, PricingConfig } from "./types";

type View = "stats" | "pricing" | "sync" | "compare" | "report" | "settings";

export default function App() {
  const [view, setView] = useState<View>("stats");
  // 先用 localStorage 做即时初值（避免后端未就绪时闪一下默认值），
  // 再用后端偏好覆盖 —— 菜单栏标题以后端为准。
  const [currency, setCurrency] = useState<Currency>(() => {
    return (localStorage.getItem("zbar-currency") as Currency) || "cny";
  });
  const [pricing, setPricing] = useState<PricingConfig>({
    usd: {},
  });

  // 初始化：以后端货币偏好为准，覆盖前端本地缓存
  useEffect(() => {
    fetchCurrency()
      .then((c) => {
        setCurrency(c);
        try {
          localStorage.setItem("zbar-currency", c);
        } catch {
          /* 忽略：QuotaExceededError、隐私模式等（对齐 cache.ts） */
        }
      })
      .catch(() => {});
  }, []);

  // 切换货币：同步写后端 + 本地缓存，确保菜单栏标题随之刷新
  const handleCurrencyChange = (c: Currency) => {
    setCurrency(c);
    try {
      localStorage.setItem("zbar-currency", c);
    } catch {
      /* 忽略：QuotaExceededError、隐私模式等（对齐 cache.ts） */
    }
    saveCurrency(c).catch(() => {});
  };

  useEffect(() => {
    fetchPricing()
      .then(setPricing)
      .catch(() => {});
  }, []);

  const backToStats = () => {
    fetchPricing()
      .then((p) => {
        // 内容没变则复用旧引用：fetchPricing 每次都返回新对象，若直接 setPricing，
        // pricing 引用变化会让 DataProvider 的刷新函数重建并立即对 4 个预设范围
        // 发起 12 个并发命令（含 7d/30d 全量扫描）+ 多次全量缓存写盘，造成返回卡顿
        setPricing((prev) =>
          JSON.stringify(prev) === JSON.stringify(p) ? prev : p
        );
      })
      .catch(() => {});
    setView("stats");
  };

  return (
    <DataProvider pricing={pricing}>
      <div className="panel-shell">
        {view === "stats" ? (
          <StatsPanel
            currency={currency}
            pricing={pricing}
            onGoPricing={() => setView("pricing")}
            onGoSync={() => setView("sync")}
            onGoCompare={() => setView("compare")}
            onGoReport={() => setView("report")}
            onGoSettings={() => setView("settings")}
          />
        ) : view === "pricing" ? (
          <PricingPanel
            currency={currency}
            onCurrencyChange={handleCurrencyChange}
            onBack={backToStats}
          />
        ) : view === "compare" ? (
          <ComparePanel onBack={() => setView("stats")} />
        ) : view === "report" ? (
          <ReportPanel
            onBack={() => setView("stats")}
            pricing={pricing}
          />
        ) : view === "settings" ? (
          <SettingsPanel onBack={backToStats} />
        ) : (
          <SyncPanel onBack={backToStats} />
        )}
      </div>
    </DataProvider>
  );
}
