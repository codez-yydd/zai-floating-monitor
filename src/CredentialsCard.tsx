import { useCallback, useEffect, useState } from "react";
import type { MessageKey } from "./i18n";
import { useI18n } from "./i18n";
import type {
  CredentialKind,
  ProviderCredentialMeta,
} from "./types";
import {
  addProviderCredential,
  removeProviderCredential,
  resetProviderCredentials,
  updateProviderCredential,
} from "./api";
import { useDataCache } from "./DataCache";
import { disableAgentByCredential, isCredentialAgent, isLocalAgent, notifyCredentialsChanged } from "./agentVisibility";
import { formatResetStamp } from "./format";
import { SectionCard } from "./layout";
import { BrandIcon, type BrandIconName } from "./BrandIcon";

/** region 下拉选项（仅区分国内/国际站的 provider 传入） */
export interface CredentialRegionOption {
  value: "cn" | "global";
  label: string;
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

  // 添加 / 更新共用提交路径（编辑时 secret 留空 = 不变更）
  const handleSubmit = async (form: {
    label: string;
    secret: string;
    region: string | null;
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
          kind,
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
      // tab 不再因残留的「有凭证自动开启」而常驻（设置页可随时手动重开）
      if (entries.length === 1 && isCredentialAgent(provider)) {
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
export function CredentialFormDialog({
  kind,
  editing,
  regionOptions,
  busy,
  error,
  titleText,
  onCancel,
  onSubmit,
}: {
  kind: CredentialKind;
  editing: ProviderCredentialMeta | null;
  regionOptions?: ReadonlyArray<CredentialRegionOption>;
  busy: boolean;
  error: string | null;
  /** 覆盖默认标题（不传保持「添加/编辑凭证」原行为） */
  titleText?: string;
  onCancel: () => void;
  onSubmit: (form: { label: string; secret: string; region: string | null }) => void;
}) {
  const { t } = useI18n();
  const [label, setLabel] = useState(editing?.label ?? "");
  const [secret, setSecret] = useState("");
  // secret 明文显隐切换（默认掩码，小眼睛切换，防粘贴长内容时被旁观）
  const [showSecret, setShowSecret] = useState(false);
  // 编辑时初始 region：条目现值（null → ""）；添加时无默认
  const [region, setRegion] = useState<string>(editing?.region ?? "");
  const isEdit = editing != null;
  const trimmedSecret = secret.trim();
  // 添加必须填 secret；编辑留空 = 不变更，均合法
  const secretValid = isEdit || trimmedSecret.length > 0;
  const trimmedLabel = label.trim();
  // 备注 32 字上限；编辑态必须非空——编辑提交走 update 的 label 全量语义
  //（空串会被后端判「备注名称不能为空」而整体失败），前端在校验态直接拦截
  const labelValid = trimmedLabel.length <= 32 && (!isEdit || trimmedLabel.length > 0);
  const valid = secretValid && labelValid;
  const secretPlaceholder =
    kind === "cookie"
      ? t("credentials.secretPlaceholderCookie")
      : kind === "token"
        ? t("credentials.secretPlaceholderToken")
        : t("credentials.secretPlaceholderApiKey");
  const submit = () => {
    if (valid && !busy) {
      onSubmit({ label: trimmedLabel, secret: trimmedSecret, region });
    }
  };

  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/30 rounded-2xl">
      <div className="mx-3 w-full rounded-lg bg-elevated border border-slate-900/10 p-3 shadow-xl">
        <div className="text-[12px] font-semibold text-slate-900 mb-2">
          {titleText ?? (isEdit ? t("credentials.edit") : t("credentials.add"))}
        </div>

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
      </div>
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
