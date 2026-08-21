import { useEffect, useState } from "react";
import { StatsPanel } from "./StatsPanel";
import { PricingPanel } from "./PricingPanel";
import { SettingsPanel } from "./SettingsPanel";
import { SyncPanel } from "./SyncPanel";
import { ComparePanel } from "./ComparePanel";
import { ReportPanel } from "./ReportPanel";
import { DataProvider } from "./DataCache";
import { fetchPricing, saveCurrency } from "./api";
import { useI18n } from "./i18n";
import type { Currency, PricingConfig } from "./types";
import {
  loadAgentVisibility,
  saveAgentVisibility,
  type AgentId,
  type AgentVisibility,
} from "./agentVisibility";

type View = "stats" | "pricing" | "sync" | "compare" | "report" | "settings";

export default function App() {
  const { locale } = useI18n();
  const [view, setView] = useState<View>("stats");
  // 币种从属语言：中文=人民币、英文=美元。语言是显式偏好，切换语言时币种跟随
  // （含启动对齐，菜单栏标题同步刷新）；价格表页仍可临时切换查看另一币种口径，
  // 切语言或重启后回到语言对应值
  const langCurrency: Currency = locale === "en" ? "usd" : "cny";
  const [currency, setCurrency] = useState<Currency>(langCurrency);
  const [pricing, setPricing] = useState<PricingConfig>({
    usd: {},
  });
  const [agentVisibility, setAgentVisibility] = useState<AgentVisibility>(() =>
    loadAgentVisibility()
  );

  const handleAgentVisibilityChange = (id: AgentId, visible: boolean) => {
    setAgentVisibility((current) => {
      const next = { ...current, [id]: visible };
      saveAgentVisibility(next);
      return next;
    });
  };

  // 语言对应币种生效：启动时把后端偏好对齐到语言（覆盖旧的手动值，保证菜单栏
  // 标题与界面一致），语言切换时立即跟随并持久化
  useEffect(() => {
    setCurrency(langCurrency);
    try {
      localStorage.setItem("zbar-currency", langCurrency);
    } catch {
      /* 忽略：QuotaExceededError、隐私模式等（对齐 cache.ts） */
    }
    saveCurrency(langCurrency).catch(() => {});
  }, [langCurrency]);

  // 切换货币（价格表页临时查看口径）：同步写后端 + 本地缓存，菜单栏标题随之刷新
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
            agentVisibility={agentVisibility}
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
          <ComparePanel
            onBack={() => setView("stats")}
            agentVisibility={agentVisibility}
          />
        ) : view === "report" ? (
          <ReportPanel
            onBack={() => setView("stats")}
            pricing={pricing}
            currency={currency}
            agentVisibility={agentVisibility}
          />
        ) : view === "settings" ? (
          <SettingsPanel
            onBack={backToStats}
            agentVisibility={agentVisibility}
            onAgentVisibilityChange={handleAgentVisibilityChange}
          />
        ) : (
          <SyncPanel onBack={backToStats} />
        )}
      </div>
    </DataProvider>
  );
}
