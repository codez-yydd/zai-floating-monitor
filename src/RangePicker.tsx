import { useState } from "react";
import type { RangePreset } from "./types";
import { dateStr, rangeToMs } from "./format";

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
      <div className="flex gap-1">
        {PRESETS.map((p) => (
          <button
            key={p.value}
            onClick={() => {
              onChange(p.value, custom);
              setShowCustom(p.value === "custom");
            }}
            className={`px-2 py-0.5 rounded-md text-[11px] transition-colors ${
              preset === p.value
                ? "bg-sky-500 text-white"
                : "bg-slate-900/5 text-slate-700/65 hover:bg-slate-900/10 hover:text-slate-900/80"
            }`}
          >
            {p.label}
          </button>
        ))}
      </div>
      {showCustom && (
        <div className="flex items-center gap-1.5 text-[11px]">
          <input
            type="date"
            value={custom.from}
            max={custom.to}
            onChange={(e) =>
              onChange("custom", { ...custom, from: e.target.value })
            }
            className="px-1.5 py-0.5 rounded-md bg-slate-900/5 border border-slate-900/10 text-slate-900/80 [color-scheme:light]"
          />
          <span className="text-slate-700/40">→</span>
          <input
            type="date"
            value={custom.to}
            min={custom.from}
            max={dateStr(Date.now())}
            onChange={(e) =>
              onChange("custom", { ...custom, to: e.target.value })
            }
            className="px-1.5 py-0.5 rounded-md bg-slate-900/5 border border-slate-900/10 text-slate-900/80 [color-scheme:light]"
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
