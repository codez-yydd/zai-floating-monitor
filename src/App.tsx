import { useEffect, useState } from "react";
import { StatsPanel } from "./StatsPanel";
import { PricingPanel } from "./PricingPanel";
import { SyncPanel } from "./SyncPanel";
import { ComparePanel } from "./ComparePanel";
import { fetchPricing } from "./api";
import type { Currency, PricingConfig } from "./types";

type View = "stats" | "pricing" | "sync" | "compare";

export default function App() {
  const [view, setView] = useState<View>("stats");
  const [currency, setCurrency] = useState<Currency>(() => {
    return (localStorage.getItem("zbar-currency") as Currency) || "cny";
  });
  const [pricing, setPricing] = useState<PricingConfig>({
    cny: {},
    usd: {},
  });

  useEffect(() => {
    localStorage.setItem("zbar-currency", currency);
  }, [currency]);

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
        />
      ) : view === "pricing" ? (
        <PricingPanel
          currency={currency}
          onCurrencyChange={setCurrency}
          onBack={backToStats}
        />
      ) : view === "compare" ? (
        <ComparePanel onBack={() => setView("stats")} />
      ) : (
        <SyncPanel onBack={backToStats} />
      )}
    </div>
  );
}
