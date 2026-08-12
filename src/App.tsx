import { useEffect, useState } from "react";
import { StatsPanel } from "./StatsPanel";
import { PricingPanel } from "./PricingPanel";
import { SyncPanel } from "./SyncPanel";
import { ComparePanel } from "./ComparePanel";
import { ReportPanel } from "./ReportPanel";
import { fetchPricing, fetchCurrency, saveCurrency, fetchStats, computeCost, fetchTrend, fetchQuota } from "./api";
import { saveCache } from "./cache";
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

  // 后台定时预取：应用运行期间每隔几分钟刷新"今日"统计 + 额度到 localStorage，
  // 确保任何时候打开面板都能秒显缓存的较新数据，避免接口慢（首次冷启动查库 +
  // 额度网络请求）导致的白屏。独立于面板显隐——只要应用在运行就持续刷新缓存。
  useEffect(() => {
    const prefetch = () => {
      const now = Date.now();
      const todayStart = new Date();
      todayStart.setHours(0, 0, 0, 0);
      const from = todayStart.getTime();
      fetchStats(from, now)
        .then((s) => saveCache("zbar-stats", s))
        .catch(() => {});
      computeCost(from, now)
        .then((c) => saveCache("zbar-cost", c))
        .catch(() => {});
      fetchTrend(from, now, "hour")
        .then((t) => saveCache("zbar-trend", t))
        .catch(() => {});
      fetchQuota()
        .then((q) => saveCache("zbar-quota", q))
        .catch(() => {});
    };
    prefetch(); // 启动即预取一次，尽快填充缓存
    const timer = setInterval(prefetch, 3 * 60 * 1000); // 每 3 分钟刷新
    return () => clearInterval(timer);
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
        <ReportPanel
          onBack={() => setView("stats")}
          pricing={pricing}
        />
      ) : (
        <SyncPanel onBack={backToStats} />
      )}
    </div>
  );
}
