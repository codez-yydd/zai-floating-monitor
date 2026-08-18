import { useState } from "react";
import type { RangePreset } from "./types";
import { dateStr, rangeToMs } from "./format";
import { DatePicker } from "./DatePicker";

interface Props {
  preset: RangePreset;
  custom: { from: string; to: string };
  onChange: (preset: RangePreset, custom: { from: string; to: string }) => void;
}

const PRESETS: { value: RangePreset; label: string }[] = [
  { value: "today", label: "今日" },
  { value: "1d", label: "24h" },
  { value: "7d", label: "7天" },
  { value: "30d", label: "30天" },
  { value: "custom", label: "自定义" },
];

export function RangePicker({ preset, custom, onChange }: Props) {
  const [showCustom, setShowCustom] = useState(preset === "custom");

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
            {p.label}
          </button>
        ))}
      </div>
      {showCustom && (
        <div className="flex items-center gap-1.5 text-[11px]">
          <DatePicker
            value={custom.from}
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
