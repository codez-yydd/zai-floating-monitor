import { useEffect, useState } from "react";
import { StatsPanel } from "./StatsPanel";
import { PricingPanel } from "./PricingPanel";
import { fetchPricing } from "./api";
import type { Currency, PricingConfig } from "./types";

type View = "stats" | "pricing";

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

  return (
    <div className="panel-shell">
      {view === "stats" ? (
        <StatsPanel
          currency={currency}
          pricing={pricing}
          onGoPricing={() => setView("pricing")}
        />
      ) : (
        <PricingPanel
          currency={currency}
          onCurrencyChange={setCurrency}
          onBack={() => {
            fetchPricing()
              .then(setPricing)
              .catch(() => {});
            setView("stats");
          }}
        />
      )}
    </div>
  );
}
