import { useCallback, useEffect, useMemo, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { MessageKey } from "./i18n";
import { useI18n } from "./i18n";
import type {
  CredentialKind,
  KimiDeviceAuthInfo,
  KimiDevicePollResult,
  ProviderCredentialMeta,
} from "./types";
import {
  addProviderCredential,
  pollKimiDeviceAuth,
  removeProviderCredential,
  resetProviderCredentials,
  startKimiDeviceAuth,
  updateProviderCredential,
} from "./api";
import { useDataCache } from "./DataCache";
import {
  disableAgentByCredential,
  isCredentialAgent,
  isLocalAgent,
  isPurePreferenceAgent,
  notifyCredentialsChanged,
} from "./agentVisibility";
import { formatResetStamp } from "./format";
import { SectionCard } from "./layout";
import { BrandIcon, type BrandIconName } from "./BrandIcon";

/** region 下拉选项（仅区分国内/国际站的 provider 传入） */
export interface CredentialRegionOption {
  value: "cn" | "global";
  label: string;
}

/**
 * 可选的 OAuth 网页登录流程（目前仅 kimi 传入）：设备码登录的两个后端
 * 命令封装。凭证仍在后端落库（成功即已保存），前端只负责发起/轮询/展示，
 * onOAuthSuccess 通知父组件刷新凭证列表与广播联动。
 */
export interface CredentialOAuthFlow {
  /** 发起设备码登录（region 为表单当前所选区域；null/空 = 默认站） */
  onStart: (region: string | null) => Promise<KimiDeviceAuthInfo>;
  /** 单次轮询（弹层按 interval 定时调用，pending 外状态自动停止） */
  onPoll: (sessionId: string) => Promise<KimiDevicePollResult>;
}

interface CredentialsCardProps {
  /** provider 标识（Rust 侧校验小写字母数字，对应 ~/.zbar/credentials/<provider>.json） */
  provider: string;
  /** 该 provider 的凭证类型（决定 secret 输入引导文案） */
  kind: CredentialKind;
  /** 获取凭证的引导文案键（含去哪获取凭证的说明） */
  guideKey: MessageKey;
  /** 品牌图标（空态引导卡显示；不传则用通用钥匙图标） */
  brand?: BrandIconName;
  /** 区域选项；不传则该 provider 无区域概念，弹层不显示下拉 */
  regionOptions?: ReadonlyArray<CredentialRegionOption>;
  /** 可选模式（本地型 provider）：数据不依赖凭证，空态改为「无需凭证」
   *  引导而非强引导填写；本地型（gemini/opencodego）手动凭证不参与查询，
   *  不再提供「添加凭证」入口（claude/cursor 等 optional 型仍可添加）。 */
  optional?: boolean;
}

/** 凭证类型徽章配色（sky=API Key / amber=Cookie / violet=Token）。
 *  导出供添加服务浮层（AddServiceMenu）复用同一套类型徽章视觉。 */
export const KIND_BADGE: Record<CredentialKind, { cls: string; key: MessageKey }> = {
  apiKey: { cls: "bg-sky-500/12 text-sky-700", key: "credentials.kindApiKey" },
  cookie: { cls: "bg-amber-500/12 text-amber-700", key: "credentials.kindCookie" },
  token: { cls: "bg-violet-500/12 text-violet-700", key: "credentials.kindToken" },
};

/**
 * 通用凭证管理卡片：某 provider 的凭证列表 + 添加/编辑/删除。
 * 数据走 DataCache 的 credentials 缓存（掩码元数据，无明文）；操作成功后
 * 广播 credentials-changed（DataCache 失效刷新 + App 做「有凭证自动显示」）。
 * 视觉与交互对齐 AccountsCard（同一套弹层/确认样式）。
 */
export function CredentialsCard({
  provider,
  kind,
  guideKey,
  brand,
  regionOptions,
  optional = false,
}: CredentialsCardProps) {
  const { t } = useI18n();
  const { credentials, refreshCredentials } = useDataCache();
  const entries = credentials[provider];
  // 本地型（gemini/opencodego）：手动凭证不参与查询，不提供添加入口，
  // 空态只保留本地数据源说明
  const localOnly = optional && isLocalAgent(provider);

  // 首挂载加载；读取失败只提示一次（旧缓存/事件刷新静默保留旧值）
  const [loadError, setLoadError] = useState<string | null>(null);
  useEffect(() => {
    if (entries === undefined) {
      refreshCredentials(provider).catch((e) =>
        setLoadError(t("credentials.loadFail", { msg: String(e) }))
      );
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [provider, entries === undefined]);

  // 空态「查看接入步骤」展开态（provider 切换时复位的本地 UI 态）
  const [guideExpanded, setGuideExpanded] = useState(false);
  useEffect(() => {
    setGuideExpanded(false);
  }, [provider]);

  // 弹层：添加 / 编辑 / 删除确认
  const [editing, setEditing] = useState<ProviderCredentialMeta | null>(null);
  const [adding, setAdding] = useState(false);
  const [confirmRemove, setConfirmRemove] =
    useState<ProviderCredentialMeta | null>(null);
  const [busy, setBusy] = useState(false);
  // 操作反馈：成功 flash / 失败保留
  const [flash, setFlash] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const showFlash = useCallback((text: string) => {
    setFlash(text);
    setTimeout(() => setFlash(null), 2000);
  }, []);

  // OAuth 网页登录流程（目前仅 kimi 提供）：成功后凭证已在后端落库，
  // 这里只负责刷新列表 + 广播（DataCache 额度补刷 / App「有凭证自动显示」）。
  // useMemo 稳定引用：api 函数为模块级导入（引用恒定），仅 provider 变化
  // 时重建——父组件每秒重渲染（时钟等）不得让轮询定时器被反复清除重建，
  // 否则 5s 轮询间隔永远等不到，授权成功无法感知。
  const oauthFlow: CredentialOAuthFlow | undefined = useMemo(
    () =>
      provider === "kimi"
        ? {
            onStart: (region) => startKimiDeviceAuth(region),
            onPoll: (sessionId) => pollKimiDeviceAuth(sessionId),
          }
        : undefined,
    [provider]
  );
  const handleOAuthSuccess = useCallback(() => {
    showFlash(t("credentials.oauthDone"));
    refreshCredentials(provider).catch(() => {});
    notifyCredentialsChanged(provider);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [provider, refreshCredentials, showFlash, t]);

  // 添加 / 更新共用提交路径（编辑时 secret 留空 = 不变更）。
  // kind：kimi 表单内可选（OAuth 令牌 token / API Key apiKey），
  // 未提供时用卡片默认类型，其余 provider 不受影响
  const handleSubmit = async (form: {
    label: string;
    secret: string;
    region: string | null;
    kind?: CredentialKind;
  }) => {
    setBusy(true);
    setActionError(null);
    try {
      if (editing) {
        await updateProviderCredential(provider, editing.id, {
          label: form.label.trim() ? form.label : null,
          secret: form.secret.trim() ? form.secret : null,
          // 编辑弹层始终携带 region 全量值：空串 = 清除
          region: form.region ?? "",
        });
      } else {
        // 添加路径：无区域下拉的服务 region 初始为空串，原样提交会被
        // 后端判为非法区域；空串/纯空白统一提交 null（未选择）
        const trimmedRegion = form.region?.trim() ?? "";
        await addProviderCredential(
          provider,
          form.label,
          form.kind ?? kind,
          form.secret,
          trimmedRegion.length > 0 ? trimmedRegion : null
        );
      }
      showFlash(t("credentials.saved"));
      setAdding(false);
      setEditing(null);
      // 广播：DataCache 失效刷新 + App 联动「有凭证自动显示」
      notifyCredentialsChanged(provider);
    } catch (e) {
      setActionError(t("credentials.saveFail", { msg: String(e) }));
    } finally {
      setBusy(false);
    }
  };

  const handleRemove = async (entry: ProviderCredentialMeta) => {
    setBusy(true);
    setActionError(null);
    try {
      await removeProviderCredential(provider, entry.id);
      setConfirmRemove(null);
      // 删除最后一条凭证（凭证型 agent）：回退该 agent 的展示偏好，
      // tab 不再因残留的「有凭证自动开启」而常驻（设置页可随时手动重开）。
      // kimi 属纯偏好控制的首批 provider（tab 承载本地 CLI 主面板），
      // 删凭证不影响其 tab 显隐（见 PURE_PREFERENCE_AGENTS）
      if (
        entries.length === 1 &&
        isCredentialAgent(provider) &&
        !isPurePreferenceAgent(provider)
      ) {
        disableAgentByCredential(provider);
      }
      notifyCredentialsChanged(provider);
    } catch (e) {
      setActionError(t("credentials.saveFail", { msg: String(e) }));
    } finally {
      setBusy(false);
    }
  };

  // 凭证文件损坏自愈：删除该 provider 凭证文件并重建空骨架（二次确认后执行），
  // 成功后清掉错误态并重拉列表，恢复增删改链路
  const [confirmReset, setConfirmReset] = useState(false);
  const handleReset = async () => {
    setBusy(true);
    setActionError(null);
    try {
      await resetProviderCredentials(provider);
      setConfirmReset(false);
      setLoadError(null);
      await refreshCredentials(provider);
      showFlash(t("credentials.resetDone"));
      // 文件内容已整体变化：广播让 DataCache 额度缓存与 App presence 同步
      notifyCredentialsChanged(provider);
    } catch (e) {
      setActionError(t("credentials.resetFail", { msg: String(e) }));
    } finally {
      setBusy(false);
    }
  };

  return (
    <SectionCard
      title={t("credentials.cardTitle")}
      action={
        <span className="flex items-center gap-1">
          {entries && entries.length > 0 && (
            <span className="num text-[9px] text-slate-500">
              {t("credentials.countBadge", { n: entries.length })}
            </span>
          )}
          {/* 本地型 provider 不提供添加入口（手动凭证不参与查询，避免误导） */}
          {!localOnly && (
            <button
              onClick={() => {
                setAdding(true);
                setActionError(null);
              }}
              disabled={busy}
              className="text-[9px] px-1.5 py-0.5 rounded bg-sky-500/10 text-sky-700/80 hover:bg-sky-500/20 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {t("credentials.add")}
            </button>
          )}
        </span>
      }
      className="relative"
    >
      {/* 空态引导：图标 + 指引 + 添加按钮（长引导文案只显示首句，完整步骤可展开） */}
      {entries !== undefined && entries.length === 0 && (
        <div className="flex items-start gap-2 rounded-md px-1.5 py-1.5 bg-slate-900/4">
          <span className="shrink-0 mt-0.5">
            {brand ? (
              <BrandIcon brand={brand} className="h-4 w-4" />
            ) : (
              <KeyGlyph className="h-3.5 w-3.5 text-slate-600/60" />
            )}
          </span>
          <div className="min-w-0">
            <div className="text-[10px] text-slate-900/80 font-medium">
              {localOnly
                ? t("credentials.localEmptyTitle")
                : t("credentials.emptyTitle")}
            </div>
            {/* 本地型（gemini/opencodego）走「无需凭证」说明；其余 optional
                provider（cursor：本地登录态可选 + 手动 Cookie）与普通 provider
                一样展示各自的首句引导与完整步骤展开，避免误用本地 CLI 文案 */}
            {localOnly ? (
              <p className="text-[9px] text-slate-500 leading-relaxed mt-0.5">
                {t("credentials.localEmptyHint")}
              </p>
            ) : (
              <>
                {/* 首句（去哪登录/获取）；前提条件等长尾说明收进展开区 */}
                <p className="text-[9px] text-slate-500 leading-relaxed mt-0.5">
                  {t(
                    `credentials.guideBrief.${provider}` as MessageKey
                  )}
                </p>
                {guideExpanded && (
                  <p className="text-[9px] text-slate-500 leading-relaxed mt-1">
                    {t(guideKey)}
                  </p>
                )}
                <button
                  onClick={() => setGuideExpanded((v) => !v)}
                  className="mt-1 text-[9px] text-sky-700/80 hover:text-sky-700 transition-colors"
                >
                  {guideExpanded
                    ? t("credentials.guideLess")
                    : t("credentials.guideMore")}
                </button>
              </>
            )}
            {!localOnly && (
              <button
                onClick={() => {
                  setAdding(true);
                  setActionError(null);
                }}
                className="mt-1.5 text-[9px] px-2 py-0.5 rounded-md bg-sky-500 text-white hover:bg-sky-600 transition-colors"
              >
                {t("credentials.add")}
              </button>
            )}
          </div>
        </div>
      )}

      {/* 凭证列表：状态点 + 备注名 + 类型/区域徽章 + 掩码 + 更新时间 + 操作 */}
      {entries && entries.length > 0 && (
        <div className="space-y-0.5">
          {entries.map((entry) => (
            <CredentialRow
              key={entry.id}
              entry={entry}
              busy={busy}
              onEdit={() => {
                setEditing(entry);
                setActionError(null);
              }}
              onRemove={() => {
                setActionError(null);
                setConfirmRemove(entry);
              }}
            />
          ))}
        </div>
      )}

      {/* 读取失败（仅首次加载失败提示，不阻塞后续操作重试） */}
      {entries === undefined && !loadError && (
        <div className="text-[10px] text-slate-500 py-1">
          {t("common.loading")}
        </div>
      )}
      {loadError && (
        <div className="mt-1">
          <p className="text-[9px] text-rose-600 leading-relaxed break-all">
            {loadError}
          </p>
          {/* 自愈入口：文件损坏（JSON 解析失败等）时增删改全废，提供
              「重置凭证文件」出口（红色危险样式 + 二次确认弹层） */}
          {actionError && (
            <p className="text-[9px] text-rose-600 leading-relaxed break-all">
              {actionError}
            </p>
          )}
          <button
            onClick={() => {
              setActionError(null);
              setConfirmReset(true);
            }}
            disabled={busy}
            className="mt-1.5 text-[9px] px-2 py-0.5 rounded bg-red-500/10 text-red-600 hover:bg-red-500/20 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
          >
            {t("credentials.resetFile")}
          </button>
        </div>
      )}
      {flash && (
        <p className="text-[9px] text-emerald-600 mt-1.5 leading-relaxed break-all">
          {flash}
        </p>
      )}

      {/* 添加 / 编辑弹层：busy 期间保持挂载（表单内容不丢失），
          错误信息渲染在弹层内部提交按钮上方，不被操作遮罩盖住 */}
      {(adding || editing) && (
        <CredentialFormDialog
          kind={kind}
          editing={editing}
          regionOptions={regionOptions}
          // kimi 添加表单内可选凭证类型（OAuth 令牌 / API Key）
          kindSelectable={provider === "kimi"}
          oauth={oauthFlow}
          onOAuthSuccess={handleOAuthSuccess}
          busy={busy}
          error={actionError}
          onCancel={() => {
            setAdding(false);
            setEditing(null);
          }}
          onSubmit={handleSubmit}
        />
      )}

      {/* 删除确认弹层（样式对齐 AccountsCard ConfirmDialog；busy 保持挂载，
          失败错误在浮层内可见） */}
      {confirmRemove && (
        <ConfirmRemoveOverlay
          name={confirmRemove.label}
          busy={busy}
          error={actionError}
          onCancel={() => setConfirmRemove(null)}
          onConfirm={() => handleRemove(confirmRemove)}
        />
      )}

      {/* 重置凭证文件确认弹层（凭证文件损坏自愈入口，红色危险键） */}
      {confirmReset && (
        <ConfirmResetOverlay
          busy={busy}
          error={actionError}
          onCancel={() => setConfirmReset(false)}
          onConfirm={() => void handleReset()}
        />
      )}

      {/* 操作进行中遮罩提示（薄层，z-40 在弹层 z-50 之下，不遮挡弹层内容） */}
      {busy && (
        <div className="absolute inset-0 z-40 rounded-2xl bg-white/40 dark:bg-black/20 flex items-center justify-center">
          <span className="text-[10px] text-slate-600/60">
            {t("common.saving")}
          </span>
        </div>
      )}
    </SectionCard>
  );
}

/** 单条凭证行 */
function CredentialRow({
  entry,
  busy,
  onEdit,
  onRemove,
}: {
  entry: ProviderCredentialMeta;
  busy: boolean;
  onEdit: () => void;
  onRemove: () => void;
}) {
  const { t } = useI18n();
  const check = entry.lastCheck;
  // 状态点：ok 绿 / error 红 / 未校验灰
  const dotCls = !check
    ? "bg-slate-400/50"
    : check.status === "ok"
      ? "bg-emerald-500"
      : "bg-rose-500";
  const checkTitle = !check
    ? t("credentials.notChecked")
    : check.status === "ok"
      ? t("credentials.checkOk")
      : `${t("credentials.checkFail")}${check.message ? `：${check.message}` : ""}`;
  const kindBadge = KIND_BADGE[entry.kind] ?? KIND_BADGE.apiKey;

  return (
    <div className="group flex items-center justify-between gap-2 rounded-md px-1.5 py-1 hover:bg-slate-900/5 transition-colors">
      <span className="min-w-0">
        <span className="flex items-center gap-1.5">
          <span
            className={`w-1.5 h-1.5 rounded-full shrink-0 ${dotCls}`}
            title={checkTitle}
          />
          <span className="text-[10px] text-slate-900/80 truncate">
            {entry.label}
          </span>
          <span
            className={`shrink-0 px-1 py-px rounded text-[8px] font-medium ${kindBadge.cls}`}
          >
            {t(kindBadge.key)}
          </span>
          {entry.region && (
            <span className="shrink-0 px-1 py-px rounded text-[8px] font-medium bg-slate-900/8 text-slate-600">
              {entry.region === "cn"
                ? t("credentials.regionCn")
                : t("credentials.regionGlobal")}
            </span>
          )}
        </span>
        <span className="flex items-center gap-1.5 mt-0.5">
          <span
            className="num text-[9px] text-slate-700/45 truncate"
            title={checkTitle}
          >
            {entry.maskedSecret}
          </span>
          <span className="num text-[8px] text-slate-500/60 shrink-0">
            {t("credentials.updatedAt", {
              time: formatResetStamp(entry.updatedAt),
            })}
          </span>
        </span>
      </span>
      <span className="flex items-center gap-1 shrink-0">
        <button
          onClick={onEdit}
          disabled={busy}
          className="text-[9px] px-1.5 py-0.5 rounded bg-sky-500/10 text-sky-700/80 hover:bg-sky-500/20 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
        >
          {t("credentials.edit")}
        </button>
        <button
          onClick={onRemove}
          disabled={busy}
          className="text-[9px] px-1.5 py-0.5 rounded bg-rose-500/10 text-rose-600/90 hover:bg-rose-500/20 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
        >
          {t("common.delete")}
        </button>
      </span>
    </div>
  );
}

/** 添加 / 编辑弹层（覆盖整卡，样式对齐 AccountsCard 弹层）。
 *  busy 期间保持挂载：按钮禁用 + 进行中文案，label/secret 等表单内部
 *  state 不因卸载丢失；失败错误渲染在提交按钮上方（弹层 z-50 在卡片
 *  操作遮罩 z-40 之上，始终可见）。
 *  导出供 StatsPanel 的「＋添加服务」入口直接复用（根元素 absolute inset-0，
 *  挂在 relative 容器内即覆盖该容器；titleText 可覆盖默认标题以显示服务名）。 */
/** OAuth 设备码弹层内视图阶段：starting=发起中 / waiting=等待浏览器确认
 *  （按 interval 轮询）/ success=成功 / denied / expired / error 为终态 */
type OAuthPhase =
  | "starting"
  | "waiting"
  | "success"
  | "denied"
  | "expired"
  | "error";

export function CredentialFormDialog({
  kind,
  editing,
  regionOptions,
  kindSelectable = false,
  oauth,
  onOAuthSuccess,
  busy,
  error,
  titleText,
  onCancel,
  onSubmit,
}: {
  kind: CredentialKind;
  editing: ProviderCredentialMeta | null;
  regionOptions?: ReadonlyArray<CredentialRegionOption>;
  /** 添加态提供凭证类型切换（目前仅 kimi：OAuth 令牌 token / API Key），
   *  提交时经 form.kind 传出；false 时按 kind 常规表单，其余 provider 零影响 */
  kindSelectable?: boolean;
  /** OAuth 网页登录流程（不传则无入口；仅添加态提供，编辑态语义不适用） */
  oauth?: CredentialOAuthFlow;
  /** OAuth 登录成功（凭证已在后端落库）：父组件刷新列表 + 广播联动 */
  onOAuthSuccess?: () => void;
  busy: boolean;
  error: string | null;
  /** 覆盖默认标题（不传保持「添加/编辑凭证」原行为） */
  titleText?: string;
  onCancel: () => void;
  onSubmit: (form: {
    label: string;
    secret: string;
    region: string | null;
    kind?: CredentialKind;
  }) => void;
}) {
  const { t } = useI18n();
  const [label, setLabel] = useState(editing?.label ?? "");
  const [secret, setSecret] = useState("");
  // secret 明文显隐切换（默认掩码，小眼睛切换，防粘贴长内容时被旁观）
  const [showSecret, setShowSecret] = useState(false);
  // 编辑时初始 region：条目现值（null → ""）；添加时无默认
  const [region, setRegion] = useState<string>(editing?.region ?? "");
  const isEdit = editing != null;
  // 凭证类型（仅 kindSelectable 的添加态可切换；默认 OAuth 令牌形态）。
  // 弹层按 adding/editing 条件挂载，state 随挂载初始化，无需跟随 prop 同步
  const [credKind, setCredKind] = useState<CredentialKind>(kind);
  const showKindSwitch = !isEdit && kindSelectable;
  const trimmedSecret = secret.trim();
  // 添加必须填 secret；编辑留空 = 不变更，均合法
  const secretValid = isEdit || trimmedSecret.length > 0;
  const trimmedLabel = label.trim();
  // 备注 32 字上限；编辑态必须非空——编辑提交走 update 的 label 全量语义
  //（空串会被后端判「备注名称不能为空」而整体失败），前端在校验态直接拦截
  const labelValid = trimmedLabel.length <= 32 && (!isEdit || trimmedLabel.length > 0);
  const valid = secretValid && labelValid;
  const secretPlaceholder = showKindSwitch
    ? // kimi 类型可选表单：占位符跟随所选形态（token = OAuth refresh_token）
      credKind === "apiKey"
      ? t("credentials.secretPlaceholderApiKey")
      : t("credentials.secretPlaceholderOauthToken")
    : kind === "cookie"
      ? t("credentials.secretPlaceholderCookie")
      : kind === "token"
        ? t("credentials.secretPlaceholderToken")
        : t("credentials.secretPlaceholderApiKey");
  const submit = () => {
    if (valid && !busy) {
      onSubmit({
        label: trimmedLabel,
        secret: trimmedSecret,
        region,
        kind: showKindSwitch ? credKind : undefined,
      });
    }
  };

  // ===== OAuth 网页登录（设备码流程；仅 oauth 存在且添加态时可用）=====
  const [oauthView, setOauthView] = useState(false);
  const [oauthInfo, setOauthInfo] = useState<KimiDeviceAuthInfo | null>(null);
  const [oauthPhase, setOauthPhase] = useState<OAuthPhase>("starting");
  // denied/expired/error 的服务端中文原因
  const [oauthMessage, setOauthMessage] = useState<string | null>(null);
  // 发起登录时表单所选区域（设备码视图展示，提醒国际站账号先选对区域）
  const [oauthRegion, setOauthRegion] = useState<string | null>(null);
  // 确认码复制反馈：成功/失败短暂展示后回退
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");

  const startOauth = () => {
    if (!oauth) return;
    setOauthView(true);
    setOauthInfo(null);
    setOauthPhase("starting");
    setOauthMessage(null);
    setCopyState("idle");
    // 携带表单当前所选区域（无下拉 / 未选 → null = 默认站），并记录用于展示
    const trimmedRegion = region.trim();
    const effectiveRegion = trimmedRegion.length > 0 ? trimmedRegion : null;
    setOauthRegion(effectiveRegion);
    oauth
      .onStart(effectiveRegion)
      .then((info) => {
        setOauthInfo(info);
        setOauthPhase("waiting");
      })
      .catch((e) => {
        setOauthPhase("error");
        setOauthMessage(String(e));
      });
  };

  const backToForm = () => {
    // effect cleanup 会随 oauthInfo 清空停掉轮询定时器
    setOauthView(false);
    setOauthInfo(null);
    setOauthPhase("starting");
    setOauthMessage(null);
  };

  const openVerification = () => {
    if (!oauthInfo) return;
    // 优先带 user_code 的完整地址（免手输），打开失败退化为 window.open
    const url = oauthInfo.verificationUriComplete || oauthInfo.verificationUri;
    openUrl(url).catch(() => window.open(url, "_blank"));
  };

  const copyUserCode = () => {
    if (!oauthInfo) return;
    // 成功/失败均给可见反馈（失败静默会让用户误以为已复制）；定时回退
    const done = (state: "copied" | "failed") => {
      setCopyState(state);
      setTimeout(() => setCopyState("idle"), 1500);
    };
    const clipboard = navigator.clipboard;
    if (clipboard && clipboard.writeText) {
      clipboard.writeText(oauthInfo.userCode).then(
        () => done("copied"),
        () => done("failed")
      );
    } else {
      done("failed");
    }
  };

  // 设备码轮询：waiting 态按 interval（最小 2s 防误配）定时单次轮询，
  // pending 继续、其余状态终止（success 落凭证后由下方 effect 收尾）
  useEffect(() => {
    if (oauthPhase !== "waiting" || !oauthInfo || !oauth) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = () => {
      oauth
        .onPoll(oauthInfo.sessionId)
        .then((result) => {
          if (cancelled) return;
          if (result.status === "pending") {
            timer = setTimeout(tick, Math.max(oauthInfo.interval, 2) * 1000);
            return;
          }
          if (result.status === "success") {
            // 凭证已落库：立即通知父组件（刷新列表 + 广播联动），不随
            // 900ms 关闭延迟——延迟窗口内的操作不应造成「已保存但 UI 不感知」
            onOAuthSuccess?.();
            setOauthPhase("success");
            return;
          }
          // denied / expired 原样呈现；其余归为 error
          setOauthPhase(
            result.status === "denied" || result.status === "expired"
              ? result.status
              : "error"
          );
          setOauthMessage(result.message ?? null);
        })
        .catch((e) => {
          if (cancelled) return;
          setOauthPhase("error");
          setOauthMessage(String(e));
        });
    };
    timer = setTimeout(tick, Math.max(oauthInfo.interval, 2) * 1000);
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
    // onOAuthSuccess 有意不进依赖：StatsPanel 侧为普通函数（每次渲染变化），
    // 列入会让定时器随渲染反复重建（H1）；回调内只用 setState 与当次值，安全
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [oauthPhase, oauthInfo, oauth]);

  // 登录成功：父组件通知已在轮询回调里即时完成，这里仅短暂展示成功提示
  // 后自动关闭弹层（success 态下操作按钮已隐藏，无需担心窗口内误操作）
  useEffect(() => {
    if (oauthPhase !== "success") return;
    const timer = setTimeout(onCancel, 900);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [oauthPhase]);

  // 弹层标题：OAuth 视图独立标题（其余沿用添加/编辑）
  const dialogTitle =
    oauthView && !isEdit
      ? t("credentials.oauthTitle")
      : titleText ?? (isEdit ? t("credentials.edit") : t("credentials.add"));

  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/30 rounded-2xl">
      <div className="mx-3 w-full rounded-lg bg-elevated border border-slate-900/10 p-3 shadow-xl">
        <div className="text-[12px] font-semibold text-slate-900 mb-2">
          {dialogTitle}
        </div>

        {oauthView && !isEdit ? (
          <OAuthDeviceView
            info={oauthInfo}
            phase={oauthPhase}
            message={oauthMessage}
            region={oauthRegion}
            copyState={copyState}
            onOpen={openVerification}
            onCopy={copyUserCode}
            onRestart={startOauth}
            onBack={backToForm}
          />
        ) : (
        <>
        {/* OAuth 网页登录入口（仅添加态且 provider 提供流程时显示；
            携带下方 region 下拉的当前值发起，设备码视图可随时返回） */}
        {!isEdit && oauth && (
          <div className="mb-2 rounded-md bg-slate-900/4 px-2 py-1.5">
            <div className="flex items-center justify-between gap-2">
              <span className="text-[10px] font-medium text-slate-900/80">
                {t("credentials.oauthEntryTitle")}
              </span>
              {/* busy（手动提交进行中）期间同样禁用，避免与保存动作交叠 */}
              <button
                type="button"
                onClick={startOauth}
                disabled={busy}
                className="text-[9px] px-2 py-0.5 rounded-md bg-indigo-500 text-white hover:bg-indigo-600 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
              >
                {t("credentials.oauthEntryButton")}
              </button>
            </div>
            <p className="text-[9px] text-slate-500 leading-relaxed mt-0.5">
              {t("credentials.oauthEntryHint")}
            </p>
          </div>
        )}

        {/* 凭证类型切换（仅 kindSelectable 的添加态显示；默认 OAuth 令牌）。
            OAuth 网页登录固定落 refresh_token 形态，不受此选择影响 */}
        {showKindSwitch && (
          <div className="flex items-center gap-1.5 mb-2">
            <span className="text-[10px] text-slate-600 shrink-0">
              {t("credentials.kindLabel")}
            </span>
            <button
              type="button"
              onClick={() => setCredKind("token")}
              className={`text-[9px] px-2 py-0.5 rounded transition-colors ${
                credKind === "token"
                  ? "bg-violet-500/15 text-violet-700 font-medium"
                  : "bg-slate-900/5 text-slate-600 hover:bg-slate-900/10"
              }`}
            >
              {t("credentials.kindOAuthToken")}
            </button>
            <button
              type="button"
              onClick={() => setCredKind("apiKey")}
              className={`text-[9px] px-2 py-0.5 rounded transition-colors ${
                credKind === "apiKey"
                  ? "bg-sky-500/15 text-sky-700 font-medium"
                  : "bg-slate-900/5 text-slate-600 hover:bg-slate-900/10"
              }`}
            >
              {t("credentials.kindApiKey")}
            </button>
          </div>
        )}

        <label className="flex flex-col gap-1 text-[10px] mb-2">
          <span className="text-slate-600">
            {t("credentials.label")}
            <span className="text-slate-400 ml-1">
              {t("credentials.labelHint")}
            </span>
          </span>
          <input
            type="text"
            value={label}
            maxLength={32}
            autoFocus
            placeholder={t("credentials.labelPlaceholder")}
            onChange={(e) => setLabel(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") submit();
            }}
            className="w-full px-1.5 py-1 rounded-md bg-slate-900/5 border border-slate-900/10 text-[11px] focus:outline-none focus:border-sky-400/60"
          />
          {/* 校验态提示（非提交后才报）：编辑态清空备注时提交按钮同步禁用 */}
          {!labelValid && (
            <span className="text-rose-600 text-[9px]">
              {t("credentials.labelRequired")}
            </span>
          )}
        </label>

        <label className="flex flex-col gap-1 text-[10px] mb-2">
          <span className="text-slate-600">
            {t("credentials.secret")}
            <span className="text-slate-400 ml-1">
              {/* 编辑时提示留空不变更；cookie 型添加时给出「请求头 / cURL
                  均可粘贴」的引导（后端会自动归一） */}
              {isEdit
                ? t("credentials.secretKeepHint")
                : kind === "cookie"
                  ? t("credentials.secretHintCookie")
                  : ""}
            </span>
          </span>
          <span className="relative block">
            <input
              type={showSecret ? "text" : "password"}
              value={secret}
              autoComplete="off"
              placeholder={isEdit ? t("credentials.secretKeepHint") : secretPlaceholder}
              onChange={(e) => setSecret(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") submit();
              }}
              className="w-full px-1.5 py-1 pr-6 rounded-md bg-slate-900/5 border border-slate-900/10 text-[11px] focus:outline-none focus:border-sky-400/60"
            />
            <button
              type="button"
              onClick={() => setShowSecret((v) => !v)}
              title={showSecret ? t("credentials.secretHide") : t("credentials.secretShow")}
              className="absolute right-1 top-1/2 -translate-y-1/2 text-slate-500/60 hover:text-slate-700 transition-colors"
            >
              {showSecret ? <EyeOffGlyph /> : <EyeGlyph />}
            </button>
          </span>
          {!secretValid && (
            <span className="text-rose-600 text-[9px]">
              {t("credentials.secretRequired")}
            </span>
          )}
        </label>

        {/* region 下拉：仅提供选项的 provider 显示 */}
        {regionOptions && regionOptions.length > 0 && (
          <label className="flex flex-col gap-1 text-[10px] mb-2">
            <span className="text-slate-600">{t("credentials.region")}</span>
            <select
              value={region}
              onChange={(e) => setRegion(e.target.value)}
              className="w-full px-1.5 py-1 rounded-md bg-slate-900/5 border border-slate-900/10 text-[11px] focus:outline-none focus:border-sky-400/60"
            >
              <option value="">{t("credentials.regionNone")}</option>
              {regionOptions.map((opt) => (
                <option key={opt.value} value={opt.value}>
                  {opt.label}
                </option>
              ))}
            </select>
          </label>
        )}

        {/* 失败错误：渲染在弹层内部（提交按钮上方），不被卡片遮罩盖住 */}
        {error && (
          <p className="text-[9px] text-rose-600 leading-relaxed break-all mb-2">
            {error}
          </p>
        )}

        <div className="flex gap-1.5">
          <button
            onClick={onCancel}
            disabled={busy}
            className="flex-1 text-[11px] py-1 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
          >
            {t("common.cancel")}
          </button>
          <button
            disabled={!valid || busy}
            onClick={submit}
            className="flex-1 text-[11px] py-1 rounded-md bg-sky-500 text-white hover:bg-sky-600 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
          >
            {busy ? t("common.saving") : t("common.confirm")}
          </button>
        </div>
        </>
        )}
      </div>
    </div>
  );
}

/** OAuth 设备码视图：user_code 醒目展示 + 打开授权页 + 阶段状态提示。
 *  授权成功后凭证已在后端落库（父组件通知即时完成），弹层自动关闭；
 *  拒绝/过期/失败给出中文提示并可重新发起。 */
function OAuthDeviceView({
  info,
  phase,
  message,
  region,
  copyState,
  onOpen,
  onCopy,
  onRestart,
  onBack,
}: {
  info: KimiDeviceAuthInfo | null;
  phase: OAuthPhase;
  message: string | null;
  /** 发起登录时表单所选区域（null = 默认站；展示提醒区域归属） */
  region: string | null;
  /** 确认码复制反馈：copied / failed 短暂展示后回退 idle */
  copyState: "idle" | "copied" | "failed";
  onOpen: () => void;
  onCopy: () => void;
  onRestart: () => void;
  onBack: () => void;
}) {
  const { t } = useI18n();
  const regionLabel =
    region === "global"
      ? t("credentials.regionGlobal")
      : region === "cn"
        ? t("credentials.regionCn")
        : t("credentials.regionNone");
  return (
    <div>
      <p className="text-[10px] text-slate-600 leading-relaxed mb-1">
        {t("credentials.oauthStepHint")}
      </p>
      {/* 当前会话使用的区域（发起时表单所选）：国际站账号需与账号站点一致 */}
      <p className="text-[9px] text-slate-500 leading-relaxed mb-2">
        {t("credentials.oauthRegionCurrent", { region: regionLabel })}
      </p>

      {/* user_code 醒目展示 + 复制（验证页要求输入时用） */}
      <div className="rounded-md bg-slate-900/5 border border-slate-900/10 px-2 py-2 mb-2 flex items-center justify-between gap-2">
        <span className="num text-[16px] font-bold tracking-[0.2em] text-slate-900 truncate">
          {info?.userCode ?? "····"}
        </span>
        <button
          type="button"
          onClick={onCopy}
          disabled={!info}
          className="shrink-0 text-[9px] px-1.5 py-0.5 rounded bg-slate-900/8 text-slate-600 hover:bg-slate-900/12 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
        >
          {copyState === "copied"
            ? t("credentials.oauthCopied")
            : copyState === "failed"
              ? t("credentials.oauthCopyFail")
              : t("credentials.oauthCopy")}
        </button>
      </div>

      {/* 阶段状态：发起中 / 等待授权（含有效期）/ 成功 / 终态原因 */}
      {phase === "starting" && (
        <p className="text-[10px] text-slate-500 animate-pulse mb-2">
          {t("credentials.oauthStarting")}
        </p>
      )}
      {phase === "waiting" && (
        <p className="text-[10px] text-amber-700/90 animate-pulse mb-2">
          {t("credentials.oauthWaiting")}
          {info && info.expiresIn > 0
            ? ` ${t("credentials.oauthValidFor", {
                minutes: Math.max(1, Math.round(info.expiresIn / 60)),
              })}`
            : ""}
        </p>
      )}
      {phase === "success" && (
        <p className="text-[10px] text-emerald-600 mb-2">
          {t("credentials.oauthSuccess")}
        </p>
      )}
      {(phase === "denied" || phase === "expired" || phase === "error") && (
        <p
          className={`text-[9px] leading-relaxed break-all mb-2 ${
            phase === "expired" ? "text-amber-700/90" : "text-rose-600"
          }`}
        >
          {message ??
            t(
              phase === "denied"
                ? "credentials.oauthDenied"
                : "credentials.oauthExpired"
            )}
        </p>
      )}

      {/* 操作区：成功态已无需操作（弹层即将自动关闭），隐藏避免误点 */}
      {phase !== "success" && (
        <div className="flex gap-1.5">
          <button
            type="button"
            onClick={onBack}
            className="flex-1 text-[11px] py-1 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 transition-colors"
          >
            {t("credentials.oauthBack")}
          </button>
          {phase === "waiting" || phase === "starting" ? (
            <button
              type="button"
              onClick={onOpen}
              disabled={!info}
              className="flex-1 text-[11px] py-1 rounded-md bg-sky-500 text-white hover:bg-sky-600 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
            >
              {t("credentials.oauthOpen")}
            </button>
          ) : (
            <button
              type="button"
              onClick={onRestart}
              className="flex-1 text-[11px] py-1 rounded-md bg-sky-500 text-white hover:bg-sky-600 transition-colors"
            >
              {t("credentials.oauthRetry")}
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/** 删除确认弹层（红色确认键，样式对齐 AccountsCard；busy 保持挂载，
 *  失败错误在浮层内可见，删除键禁用 + 进行中文案） */
function ConfirmRemoveOverlay({
  name,
  busy,
  error,
  onCancel,
  onConfirm,
}: {
  name: string;
  busy: boolean;
  error: string | null;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const { t } = useI18n();
  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/30 rounded-2xl">
      <div className="mx-3 w-full rounded-lg bg-elevated border border-slate-900/10 p-3 shadow-xl">
        <div className="text-[12px] font-semibold text-slate-900 mb-1">
          {t("credentials.deleteTitle")}
        </div>
        <p className="text-[10px] text-slate-700/65 leading-relaxed mb-2">
          {t("credentials.deleteConfirm", { name })}
        </p>
        {error && (
          <p className="text-[9px] text-rose-600 leading-relaxed break-all mb-2">
            {error}
          </p>
        )}
        <div className="flex gap-1.5">
          <button
            onClick={onCancel}
            disabled={busy}
            className="flex-1 text-[11px] py-1 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
          >
            {t("common.cancel")}
          </button>
          <button
            onClick={onConfirm}
            disabled={busy}
            className="flex-1 text-[11px] py-1 rounded-md bg-red-500 text-white hover:bg-red-600 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
          >
            {busy ? t("common.deleting") : t("common.delete")}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 重置凭证文件确认弹层（样式对齐 ConfirmRemoveOverlay；busy 保持挂载，
 *  失败错误在浮层内可见） */
function ConfirmResetOverlay({
  busy,
  error,
  onCancel,
  onConfirm,
}: {
  busy: boolean;
  error: string | null;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const { t } = useI18n();
  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/30 rounded-2xl">
      <div className="mx-3 w-full rounded-lg bg-elevated border border-slate-900/10 p-3 shadow-xl">
        <div className="text-[12px] font-semibold text-slate-900 mb-1">
          {t("credentials.resetTitle")}
        </div>
        <p className="text-[10px] text-slate-700/65 leading-relaxed mb-2">
          {t("credentials.resetConfirm")}
        </p>
        {error && (
          <p className="text-[9px] text-rose-600 leading-relaxed break-all mb-2">
            {error}
          </p>
        )}
        <div className="flex gap-1.5">
          <button
            onClick={onCancel}
            disabled={busy}
            className="flex-1 text-[11px] py-1 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
          >
            {t("common.cancel")}
          </button>
          <button
            onClick={onConfirm}
            disabled={busy}
            className="flex-1 text-[11px] py-1 rounded-md bg-red-500 text-white hover:bg-red-600 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
          >
            {busy ? t("common.saving") : t("credentials.resetFile")}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 通用钥匙图标（无品牌图标的 provider 空态用） */
function KeyGlyph({ className = "" }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden
    >
      <circle cx="7.5" cy="15.5" r="4.5" />
      <path d="m10.8 12.2 8.2-8.2M16 3l3 3M13 6l3 3" />
    </svg>
  );
}

/** secret 显隐切换：睁眼（当前明文，点击隐藏） */
function EyeGlyph() {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className="h-3 w-3"
      aria-hidden
    >
      <path d="M2 12s3.5-6.5 10-6.5S22 12 22 12s-3.5 6.5-10 6.5S2 12 2 12Z" />
      <circle cx="12" cy="12" r="2.5" />
    </svg>
  );
}

/** secret 显隐切换：闭眼（当前掩码，点击显示） */
function EyeOffGlyph() {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className="h-3 w-3"
      aria-hidden
    >
      <path d="M10.6 5.9A10.9 10.9 0 0 1 12 5.5c6.5 0 10 6.5 10 6.5a17.6 17.6 0 0 1-2.4 3.2M6.1 6.9A16.9 16.9 0 0 0 2 12s3.5 6.5 10 6.5a10 10 0 0 0 4-.8" />
      <path d="m3 3 18 18" />
      <path d="M9.9 9.9a2.5 2.5 0 0 0 3.5 3.5" />
    </svg>
  );
}
