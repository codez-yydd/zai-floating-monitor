import { useEffect, useState } from "react";
import { StatsPanel } from "./StatsPanel";
import { PricingPanel } from "./PricingPanel";
import { SyncPanel } from "./SyncPanel";
import { ComparePanel } from "./ComparePanel";
import { ReportPanel } from "./ReportPanel";
import { fetchPricing, fetchCurrency, saveCurrency } from "./api";
import type { Currency, PricingConfig } from "./types";

type View = "stats" | "pricing" | "sync" | "compare" | "report";

export default function App() {
  const [view, setView] = useState<View>("stats");
  // 先用 localStorage 做即时初值（避免后端未就绪时闪一下默认值），
  // 再用后端偏好覆盖 —— 菜单栏标题以后端为准。
  const [currency, setCurrency] = useState<Currency>(() => {
    return (localStorage.getItem("zbar-currency") as Currency) || "cny";
  });
  const [pricing, setPricing] = useState<PricingConfig>({
    cny: {},
    usd: {},
  });

  // 初始化：以后端货币偏好为准，覆盖前端本地缓存
  useEffect(() => {
    fetchCurrency()
      .then((c) => {
        setCurrency(c);
        localStorage.setItem("zbar-currency", c);
      })
      .catch(() => {});
  }, []);

  // 切换货币：同步写后端 + 本地缓存，确保菜单栏标题随之刷新
  const handleCurrencyChange = (c: Currency) => {
    setCurrency(c);
    localStorage.setItem("zbar-currency", c);
    saveCurrency(c).catch(() => {});
  };

  useEffect(() => {
    fetchPricing()
      .then(setPricing)
      .catch(() => {});
  }, []);

  const backToStats = () => {
    fetchPricing()
      .then(setPricing)
      .catch(() => {});
    setView("stats");
  };

  return (
    <div className="panel-shell">
      {view === "stats" ? (
        <StatsPanel
          currency={currency}
          pricing={pricing}
          onGoPricing={() => setView("pricing")}
          onGoSync={() => setView("sync")}
          onGoCompare={() => setView("compare")}
          onGoReport={() => setView("report")}
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
        <ReportPanel onBack={() => setView("stats")} />
      ) : (
        <SyncPanel onBack={backToStats} />
      )}
    </div>
  );
}
