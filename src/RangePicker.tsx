import { useState } from "react";
import type { RangePreset } from "./types";
import { dateStr, rangeToMs } from "./format";
import { DatePicker } from "./DatePicker";
import { useI18n, type MessageKey } from "./i18n";

interface Props {
  preset: RangePreset;
  custom: { from: string; to: string };
  /** 可选：from 下限（如数据保留期），不传则不限 */
  min?: string;
  onChange: (preset: RangePreset, custom: { from: string; to: string }) => void;
}

// 模式 A：常量表存词典键，渲染时查（label 跟随 UI 语言）
const PRESETS: { value: RangePreset; labelKey: MessageKey }[] = [
  { value: "today", labelKey: "range.today" },
  { value: "1d", labelKey: "range.24h" },
  { value: "7d", labelKey: "range.7d" },
  { value: "30d", labelKey: "range.30d" },
  { value: "custom", labelKey: "range.custom" },
];

export function RangePicker({ preset, custom, min, onChange }: Props) {
  const [showCustom, setShowCustom] = useState(preset === "custom");
  const { t } = useI18n();

  return (
    <div className="space-y-2">
      <div className="flex gap-1 p-0.5 rounded-xl bg-slate-900/4">
        {PRESETS.map((p) => (
          <button
            key={p.value}
            onClick={() => {
              onChange(p.value, custom);
              setShowCustom(p.value === "custom");
            }}
            className={`flex-1 px-1.5 py-1 rounded-lg text-[10px] font-medium transition-all duration-150 ${
              preset === p.value
                ? "bg-sky-500 text-white shadow-sm"
                : "text-slate-600/70 hover:text-slate-800 hover:bg-slate-900/5"
            }`}
          >
            {t(p.labelKey)}
          </button>
        ))}
      </div>
      {showCustom && (
        <div className="flex items-center gap-1.5 text-[11px]">
          <DatePicker
            value={custom.from}
            min={min}
            max={custom.to}
            onChange={(v) => onChange("custom", { ...custom, from: v })}
          />
          <span className="text-slate-700/40">→</span>
          <DatePicker
            value={custom.to}
            min={custom.from}
            max={dateStr(Date.now())}
            onChange={(v) => onChange("custom", { ...custom, to: v })}
          />
        </div>
      )}
    </div>
  );
}

export function resolveRange(
  preset: RangePreset,
  custom: { from: string; to: string }
): [number, number] {
  return rangeToMs(preset, custom);
}
