import { useEffect, useRef, useState } from "react";
import { StatsPanel } from "./StatsPanel";
import { PricingPanel } from "./PricingPanel";
import { SettingsPanel } from "./SettingsPanel";
import { SyncPanel } from "./SyncPanel";
import { DataProvider } from "./DataCache";
import { ResizeHandles } from "./ResizeHandles";
import { ReportsPanel } from "./ReportsPanel";
import { ThemePanel } from "./ThemePanel";
import { ADD_SERVICE_EVENT } from "./AddServiceMenu";
import { fetchPricing, hasProviderCredentials, saveCurrency } from "./api";
import { startUpdateScheduler } from "./updater";
import { useI18n } from "./i18n";
import type { Currency, PricingConfig } from "./types";
import {
  AGENT_VISIBILITY_EVENT,
  CREDENTIALS_CHANGED_EVENT,
  CREDENTIAL_AGENTS,
  PRESENCE_PROVIDERS,
  clearAgentManuallyDisabled,
  enableAgentByCredential,
  loadAgentVisibility,
  markAgentManuallyDisabled,
  saveAgentVisibility,
  type AgentId,
  type AgentVisibility,
} from "./agentVisibility";

type View = "stats" | "pricing" | "sync" | "reports" | "theme" | "settings";

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
  // 凭证驱动 provider 的「已有凭证」状态（provider → bool）：
  // 启动批量探测 + 凭证增删改事件后单点重查（本地文件级检查，开销可忽略）
  const [credentialPresence, setCredentialPresence] = useState<
    Record<string, boolean>
  >({});
  // 设置页「添加凭证」快捷入口的跳转目标：暂存 provider，切回统计页时
  // 交给 StatsPanel 打开该服务的添加表单（挂载即消费，消费后清空）
  const [pendingAddProvider, setPendingAddProvider] = useState<string | null>(
    null
  );

  // 快捷入口事件（SettingsPanel 派发）：切回统计页并携带目标 provider
  useEffect(() => {
    const open = (e: Event) => {
      const provider = (e as CustomEvent<unknown>).detail;
      if (typeof provider === "string" && provider) {
        setPendingAddProvider(provider);
        setView("stats");
      }
    };
    window.addEventListener(ADD_SERVICE_EVENT, open);
    return () => window.removeEventListener(ADD_SERVICE_EVENT, open);
  }, []);

  const handleAgentVisibilityChange = (id: AgentId, visible: boolean) => {
    // 显式关闭标记：设置页手动关闭的凭证型 agent，此后 presence 无→有
    // 不再被「有凭证自动显示」打开；重新手动开启即清除标记恢复联动
    if (visible) {
      clearAgentManuallyDisabled(id);
    } else {
      markAgentManuallyDisabled(id);
    }
    setAgentVisibility((current) => {
      const next = { ...current, [id]: visible };
      saveAgentVisibility(next);
      return next;
    });
  };

  // ===== 凭证驱动的 provider 联动 =====
  // 1) presence 探测：启动时批量查全部探测名单（凭证驱动的新 provider +
  //    claude/cursor「其他账号」区）；凭证增删改事件后重查该 provider
  //    （事件源：CredentialsCard 的添加/编辑/删除操作）。
  useEffect(() => {
    const probe = (providers: readonly string[]) => {
      for (const p of providers) {
        hasProviderCredentials(p)
          .then((has) =>
            setCredentialPresence((prev) =>
              prev[p] === has ? prev : { ...prev, [p]: has }
            )
          )
          .catch(() => {
            // 探测失败视为无凭证（tab 不显示），不阻断启动
          });
      }
    };
    probe(PRESENCE_PROVIDERS);
    const onCredentialsChanged = (e: Event) => {
      const provider = (e as CustomEvent<{ provider: string }>).detail
        ?.provider;
      if (
        typeof provider === "string" &&
        provider &&
        PRESENCE_PROVIDERS.includes(provider)
      ) {
        probe([provider]);
      }
    };
    window.addEventListener(CREDENTIALS_CHANGED_EVENT, onCredentialsChanged);
    return () =>
      window.removeEventListener(
        CREDENTIALS_CHANGED_EVENT,
        onCredentialsChanged
      );
  }, []);

  // 2) 「有凭证自动显示」：presence 由无到有的瞬间自动开启对应 agent
  //    （写 localStorage + 广播）。只在状态迁移时触发；用户在设置页显式
  //    关闭过的 agent 不再自动开启（见 enableAgentByCredential 的标记检查），
  //    删除全部凭证时由 disableAgentByCredential 回退偏好。
  const prevPresenceRef = useRef<Record<string, boolean>>({});
  useEffect(() => {
    const prev = prevPresenceRef.current;
    const next: Record<string, boolean> = { ...prev };
    for (const id of CREDENTIAL_AGENTS) {
      const has = !!credentialPresence[id];
      if (has && !prev[id]) {
        enableAgentByCredential(id);
      }
      next[id] = has;
    }
    prevPresenceRef.current = next;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [credentialPresence]);

  // 3) visibility 层广播：设置页之外的写入（enableAgentByCredential）
  //    经事件通知本组件重读 localStorage，保持单一真相源。
  useEffect(() => {
    const sync = () => setAgentVisibility(loadAgentVisibility());
    window.addEventListener(AGENT_VISIBILITY_EVENT, sync);
    return () => window.removeEventListener(AGENT_VISIBILITY_EVENT, sync);
  }, []);

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

  // 启动更新调度：延迟 10s 首次检查，之后每 1h 后台检查并下载；
  // 下载完成后写 localStorage 供设置入口红点展示，失败静默不打扰
  useEffect(() => startUpdateScheduler(), []);

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
    <DataProvider pricing={pricing} credentialPresence={credentialPresence}>
      <div className="panel-shell">
        {view === "stats" ? (
          <StatsPanel
            currency={currency}
            pricing={pricing}
            agentVisibility={agentVisibility}
            credentialPresence={credentialPresence}
            onGoPricing={() => setView("pricing")}
            onGoSync={() => setView("sync")}
            onGoReports={() => setView("reports")}
            onGoTheme={() => setView("theme")}
            onGoSettings={() => setView("settings")}
            initialAdd={pendingAddProvider}
            onInitialAddConsumed={() => setPendingAddProvider(null)}
          />
        ) : view === "pricing" ? (
          <PricingPanel
            currency={currency}
            onCurrencyChange={handleCurrencyChange}
            onBack={backToStats}
          />
        ) : view === "reports" ? (
          <ReportsPanel
            onBack={() => setView("stats")}
            pricing={pricing}
            currency={currency}
            agentVisibility={agentVisibility}
          />
        ) : view === "theme" ? (
          <ThemePanel onBack={backToStats} />
        ) : view === "settings" ? (
          <SettingsPanel
            onBack={backToStats}
            agentVisibility={agentVisibility}
            onAgentVisibilityChange={handleAgentVisibilityChange}
          />
        ) : (
          <SyncPanel onBack={backToStats} />
        )}
        {/* 边缘拖拽热区：所有 view 共用，放在 panel-shell 最后一个子节点 */}
        <ResizeHandles />
      </div>
    </DataProvider>
  );
}
