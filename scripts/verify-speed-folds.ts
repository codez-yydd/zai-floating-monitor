/**
 * 速度折叠契约常驻测试（npm run test:speed；无测试框架，断言失败抛错 →
 * node 非零退出 → npm script 失败）。被测模块：src/modelName.ts（fold）
 * 与 src/merge.ts（mergeStats）。经 scripts/tsconfig.speed.json 以
 * CommonJS 编译到 .tmp-speed-test/ 后由 run-speed-tests.mjs 加载执行。
 *
 * 覆盖的四个反例/回归：
 * 1. 本机速度权重快照：claude/kimi request_average 行折叠按本机输出
 *    Token 加权——mergeStats 并入远端 Token 后本机显示速度不得改变
 *    （A/B/远端反例：10/20 t/s 两变体 → 恒 15 t/s，修复前 ≈ 10.8）；
 *    无远端纯本机行为不变；旧契约行（无 speedQuality）速度降级 null。
 * 2. TTFT 按有效样本数加权：100ms×1000 + 200ms×1 → ≈100.1ms、1001
 *    样本（修复前取代表行值）；缺样本数旧行不参与加权；全缺回退代表行。
 * 3. 既有回归：generation 口径 181.8 t/s 折叠（分子/分母加权）、折叠
 *    幂等（含 TTFT 加权输出与权重快照回填字段的再折叠）、远端独有模型
 *    不带速度、overall 速度保留本机值、foldByModelStats null 透传。
 */

import {
  foldByModelStats,
  foldModelStatRows,
  type FoldedModelStat,
} from "../src/modelName";
import { mergeStats } from "../src/merge";
import type { ModelStat, RemoteUsage, Stats } from "../src/types";

/** 断言工具：失败即抛（node 非零退出） */
function check(cond: boolean, msg: string): void {
  if (!cond) throw new Error(`断言失败: ${msg}`);
}

function close(
  actual: number | null | undefined,
  expected: number,
  eps: number,
  msg: string
): void {
  check(
    actual != null && Math.abs(actual - expected) < eps,
    `${msg}（期望 ≈${expected}，实际 ${actual}）`
  );
}

function eqNull(actual: unknown, expected: unknown, msg: string): void {
  check(actual === expected, `${msg}（期望 ${String(expected)}，实际 ${String(actual)}）`);
}

/** ModelStat 行构造（Token 之外的聚合字段填 0，速度字段按需 spread） */
function row(
  modelId: string,
  providerId: string,
  requests: number,
  outputTokens: number,
  extra: Partial<ModelStat> = {}
): ModelStat {
  return {
    model_id: modelId,
    provider_id: providerId,
    requests,
    input_tokens: 0,
    output_tokens: outputTokens,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 0,
    total_tokens: outputTokens,
    ...extra,
  };
}

function emptyStats(byModel: ModelStat[]): Stats {
  return {
    from_ms: 0,
    to_ms: 0,
    overall: {
      requests: 0,
      input_tokens: 0,
      output_tokens: 0,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      reasoning_tokens: 0,
      total_tokens: 0,
    },
    by_model: byModel,
    earliest_ms: null,
    latest_ms: null,
  };
}

function remoteUsage(byModel: RemoteUsage["by_model"]): RemoteUsage {
  return {
    from_ms: 0,
    to_ms: 0,
    overall: {
      requests: 0,
      input_tokens: 0,
      output_tokens: 0,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      reasoning_tokens: 0,
      total_tokens: 0,
    },
    by_model: byModel,
    trend: [],
  };
}

// ===== 1. 修复 1：request_average 折叠权重只反映本机输出 =====

// 本机两个变体行（同名模型 claude 请求平均口径）：变体 A 均速 10 t/s、
// 本机输出 100；变体 B 均速 20 t/s、本机输出 100 → 加权 = 15 t/s
function abVariants(): ModelStat[] {
  return [
    row("Claude-Sonnet", "anthropic", 2, 100, {
      avg_tps: 10,
      max_tps: 12,
      speedQuality: "request_average",
    }),
    row("claude-sonnet", "anthropic", 3, 100, {
      avg_tps: 20,
      max_tps: 25,
      speedQuality: "request_average",
    }),
  ];
}

// 无远端纯本机（无权重快照字段，等价 fetchStats 直查形态）：回退
// output_tokens，行为与修复前一致
{
  const [folded] = foldModelStatRows(abVariants());
  close(folded.avg_tps, 15, 1e-9, "纯本机 A/B 折叠均速");
  eqNull(folded.speedQuality, "request_average", "纯本机折叠口径标记");
  // 折叠输出回填快照合计（100+100），再折叠走快照 → 幂等
  eqNull(folded.localSpeedWeightTokens, 200, "折叠输出快照合计");
  const [refolded] = foldModelStatRows([folded as ModelStat]);
  close(refolded.avg_tps, 15, 1e-9, "快照回填后再折叠均速（幂等）");
  eqNull(refolded.localSpeedWeightTokens, 200, "再折叠快照不变");
}

// 反例：远端同名模型 1000 输出并入变体 A 后，本机显示速度必须仍是
// 15 t/s（修复前权重被远端 Token 污染 → ≈ 10.8）
{
  const local = emptyStats(abVariants());
  const remote = remoteUsage([
    {
      model_id: "Claude-Sonnet",
      provider_id: "anthropic",
      source: "claude",
      requests: 50,
      input_tokens: 0,
      output_tokens: 1000,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      reasoning_tokens: 0,
      total_tokens: 1000,
    },
  ]);
  const merged = mergeStats(local, remote);
  const byId = new Map(merged.by_model.map((m) => [m.model_id, m]));
  const rowA = byId.get("Claude-Sonnet");
  check(rowA != null, "合并后变体 A 行存在");
  eqNull(rowA!.output_tokens, 1100, "远端 Token 正确叠加进 output_tokens");
  eqNull(rowA!.localSpeedWeightTokens ?? null, 100, "合并行冻结本机权重快照 100");
  const [folded] = foldModelStatRows(merged.by_model);
  close(folded.avg_tps, 15, 1e-9, "远端 Token 并入后本机速度不变（修复 1 反例）");
  eqNull(folded.total_tokens, 1200, "Token 展示列含远端（不丢量）");
}

// 旧契约行（有 avg_tps 无 speedQuality/快照）：不以全部 Token 伪造权重，
// 速度折叠为 null（界面显示 —）
{
  const [folded] = foldModelStatRows([
    row("legacy", "p", 1, 500, { avg_tps: 10, max_tps: 10 }),
  ]);
  eqNull(folded.avg_tps, null, "旧契约行速度降级 null");
  eqNull(folded.speedQuality, undefined, "旧契约行无口径标记");
}

// ===== 2. 修复 4：TTFT 按有效样本数加权 =====

{
  const a = row("GLM-5.3", "z", 1, 100, {
    avg_ttft_ms: 100,
    ttftSampleCount: 1000,
  });
  const b = row("glm-5.3", "z", 1, 100, {
    avg_ttft_ms: 200,
    ttftSampleCount: 1,
  });
  const [folded] = foldModelStatRows([a, b]);
  // Σ(avg×count)÷Σcount = (100×1000 + 200×1)÷1001 ≈ 100.0999（修复前取
  // 代表行值 200）
  close(folded.avg_ttft_ms, 100200 / 1001, 1e-6, "TTFT 样本数加权均值");
  eqNull(folded.ttftSampleCount, 1001, "TTFT 有效样本数求和");
  // 输出行携带合计样本数 → 再折叠走同一公式，幂等
  const [refolded] = foldModelStatRows([folded as ModelStat]);
  close(refolded.avg_ttft_ms ?? 0, folded.avg_ttft_ms ?? -1, 1e-6, "TTFT 再折叠幂等");
  eqNull(refolded.ttftSampleCount, 1001, "TTFT 样本数再折叠不变");
}

// 缺样本数的旧行不参与加权（无权重信息）：只加权带样本数的行
{
  const [folded] = foldModelStatRows([
    row("M", "p", 1, 100, { avg_ttft_ms: 300 }),
    row("m", "p", 1, 100, { avg_ttft_ms: 200, ttftSampleCount: 1 }),
  ]);
  close(folded.avg_ttft_ms ?? 0, 200, 1e-9, "缺样本数行不参与 TTFT 加权");
  eqNull(folded.ttftSampleCount, 1, "样本数只含参与加权的行");
}

// 组内全部缺样本数（旧数据）：保持旧行为取代表行值（total_tokens 最大行）
{
  const [folded] = foldModelStatRows([
    row("M", "p", 1, 500, { avg_ttft_ms: 150 }),
    row("m", "p", 1, 900, { avg_ttft_ms: 250 }),
  ]);
  close(folded.avg_ttft_ms ?? 0, 250, 1e-9, "全缺样本数回退代表行值");
  eqNull(folded.ttftSampleCount ?? null, null, "全缺样本数无样本数输出");
}

// ===== 3. 既有回归：generation 折叠 / 幂等 / merge 保留 =====

{
  // 经典反例：两个 100tok 变体（1s 与 0.1s 生成）→ 200/1.1 ≈ 181.8 t/s，
  // 不是逐行算术平均的 550
  const rows = [
    row("GLM-5.3", "z", 1, 100, {
      avg_tps: 100,
      speedOutputTokens: 100,
      speedGenerationMs: 1000,
      speedSampleCount: 1,
      speedQuality: "generation",
      avg_ttft_ms: 100,
      ttftSampleCount: 1000,
    }),
    row("glm-5.3", "z", 1, 100, {
      avg_tps: 1000,
      max_tps: 1000,
      speedOutputTokens: 100,
      speedGenerationMs: 100,
      speedSampleCount: 1,
      speedQuality: "generation",
      avg_ttft_ms: 200,
      ttftSampleCount: 1,
    }),
  ];
  const [folded] = foldModelStatRows(rows);
  close(folded.avg_tps, 200000 / 1100, 1e-6, "generation 分子分母加权 ≈ 181.8");
  close(folded.max_tps ?? 0, 1000, 1e-9, "generation 组内最快值保留");
  eqNull(folded.speedSampleCount, 2, "generation 样本数求和");
  eqNull(folded.speedQuality, "generation", "generation 口径标记");
  eqNull(folded.localSpeedWeightTokens, undefined, "generation 行不带权重快照");
  // 含 TTFT 加权输出的整表幂等：fold(fold(rows)) 关键字段与 fold(rows) 一致
  const once = foldModelStatRows(rows);
  const twice = foldModelStatRows(once as ModelStat[]);
  for (const key of [
    "model_id",
    "requests",
    "output_tokens",
    "total_tokens",
    "avg_tps",
    "max_tps",
    "avg_ttft_ms",
  ] as const) {
    eqNull(twice[0][key], once[0][key], `幂等字段 ${key}`);
  }
  eqNull(twice[0].speedOutputTokens, once[0].speedOutputTokens, "幂等 speedOutputTokens");
  eqNull(twice[0].ttftSampleCount, once[0].ttftSampleCount, "幂等 ttftSampleCount");
  eqNull(twice[0].localSpeedWeightTokens, once[0].localSpeedWeightTokens, "幂等权重快照");
}

// mergeStats 既有行为回归：本地速度字段保留、远端独有模型不带速度、
// overall 速度保留本机值
{
  const local = emptyStats([
    row("GLM-5.3", "z", 2, 200, {
      avg_tps: 181.8,
      max_tps: 300,
      speedOutputTokens: 200,
      speedGenerationMs: 1100,
      speedSampleCount: 2,
      speedQuality: "generation",
      avg_ttft_ms: 100,
      ttftSampleCount: 3,
    }),
  ]);
  local.overall = {
    ...local.overall,
    avg_tps: 181.8,
    speedQuality: "generation",
  };
  const merged = mergeStats(local, remoteUsage([
    {
      model_id: "GLM-5.3",
      provider_id: "z",
      source: "zcode",
      requests: 9,
      input_tokens: 0,
      output_tokens: 800,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      reasoning_tokens: 0,
      total_tokens: 800,
    },
    {
      model_id: "remote-only",
      provider_id: "z",
      source: "zcode",
      requests: 1,
      input_tokens: 0,
      output_tokens: 10,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      reasoning_tokens: 0,
      total_tokens: 10,
    },
  ]));
  const byId = new Map(merged.by_model.map((m) => [m.model_id, m]));
  const glm = byId.get("GLM-5.3");
  check(glm != null, "合并后 GLM 行存在");
  close(glm!.avg_tps ?? 0, 181.8, 1e-9, "generation 速度保留本机值");
  eqNull(glm!.speedOutputTokens, 200, "generation 分子保留");
  eqNull(glm!.speedGenerationMs, 1100, "generation 分母保留");
  eqNull(glm!.ttftSampleCount, 3, "TTFT 样本数保留");
  const remoteOnly = byId.get("remote-only");
  check(remoteOnly != null, "远端独有模型行存在");
  eqNull(remoteOnly!.avg_tps ?? null, null, "远端独有模型不凭远端输出构造速度");
  eqNull(remoteOnly!.speedQuality, undefined, "远端独有模型无口径标记");
  eqNull(merged.overall.avg_tps ?? null, 181.8, "overall 速度保留本机值");
}

// foldByModelStats：null 透传与 by_model 折叠
{
  eqNull(foldByModelStats<Stats>(null), null, "foldByModelStats null 透传");
  const folded = foldByModelStats(emptyStats(abVariants()));
  eqNull(folded.by_model.length, 1, "foldByModelStats 折叠为一行");
  close(folded.by_model[0].avg_tps ?? 0, 15, 1e-9, "foldByModelStats 行速度正确");
}

// 折叠输出排序与变体明细（既有行为抽查）
{
  const folded: FoldedModelStat[] = foldModelStatRows(abVariants());
  eqNull(folded.length, 1, "同名变体折叠为一行");
  eqNull(folded[0].model_id, "Claude-Sonnet", "显示名取代表行（total 最大者）");
  eqNull(folded[0].variants?.length, 2, "变体明细保留两条");
}

// 断言脚本无输出即成功（runner 在加载成功后统一打印结果；
// 本脚本编译目标 lib 无 DOM/node 类型，不引用 console）
