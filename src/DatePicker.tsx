import { useEffect, useRef, useState } from "react";

interface DatePickerProps {
  /** 受控值，"YYYY-MM-DD" */
  value: string;
  onChange: (v: string) => void;
  /** 可选下限 "YYYY-MM-DD" */
  min?: string;
  /** 可选上限 "YYYY-MM-DD" */
  max?: string;
}

const WEEK_HEADERS = ["日", "一", "二", "三", "四", "五", "六"];

// 网格尺寸钉死为常量，避免 Tailwind 任意值宽度 / absolute shrink-to-fit
// 在不同环境下推断不一致，导致日历被父容器掐窄、日期挤成一团。
const GRID_COLS = "repeat(7, 1fr)";
const POPUP_WIDTH = 248;

function pad(n: number): string {
  return n < 10 ? "0" + n : String(n);
}

function toYMD(y: number, m: number, d: number): string {
  return `${y}-${pad(m + 1)}-${pad(d)}`;
}

function parseYMD(s: string): Date | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s);
  if (!m) return null;
  return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
}

/**
 * 轻量中文日期选择器：替代原生 <input type="date">。
 * 原生控件的弹出日历跟随系统 locale（此 WebView 下为英文）且样式不可控，
 * 这里自建以保证全中文 + 与毛玻璃面板一致的视觉。
 */
export function DatePicker({
  value,
  onChange,
  min,
  max,
}: DatePickerProps) {
  const [open, setOpen] = useState(false);
  const [view, setView] = useState(() => {
    const d = parseYMD(value) ?? new Date();
    return { year: d.getFullYear(), month: d.getMonth() };
  });
  const [popupTop, setPopupTop] = useState<number>(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  // 展开时：点击外部 / Esc 关闭
  useEffect(() => {
    if (!open) return;
    function onDown(e: MouseEvent) {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setOpen(false);
    }
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  function toggle() {
    setOpen((prev) => {
      const next = !prev;
      // 每次展开都把视图定位回当前选中值所在月份
      if (next) {
        const d = parseYMD(value);
        if (d) setView({ year: d.getFullYear(), month: d.getMonth() });
        // 弹出层 fixed 定位，top 取触发器底部
        const r = triggerRef.current?.getBoundingClientRect();
        if (r) setPopupTop(r.bottom + 4);
      }
      return next;
    });
  }

  function shiftMonth(delta: number) {
    setView(({ year, month }) => {
      const m = month + delta;
      if (m < 0) return { year: year - 1, month: 11 };
      if (m > 11) return { year: year + 1, month: 0 };
      return { year, month: m };
    });
  }

  function pick(ymd: string) {
    onChange(ymd);
    setOpen(false);
  }

  // 今天（本地时区），仅用于高亮显示
  const now = new Date();
  const todayStr = toYMD(now.getFullYear(), now.getMonth(), now.getDate());

  // 6 行 × 7 列网格，从当月首日所在周的周日开始
  const firstDay = new Date(view.year, view.month, 1);
  const startOffset = firstDay.getDay(); // 0 = 周日
  const gridStart = new Date(view.year, view.month, 1 - startOffset);
  const cells: { ymd: string; day: number; inMonth: boolean }[] = [];
  for (let i = 0; i < 42; i++) {
    const cur = new Date(gridStart);
    cur.setDate(gridStart.getDate() + i);
    cells.push({
      ymd: toYMD(cur.getFullYear(), cur.getMonth(), cur.getDate()),
      day: cur.getDate(),
      inMonth: cur.getMonth() === view.month,
    });
  }

  return (
    <div ref={rootRef} className="relative">
      <button
        ref={triggerRef}
        type="button"
        onClick={toggle}
        className={`num inline-flex items-center gap-1 px-2 py-1 rounded-md border text-[11px] transition-colors ${
          open
            ? "bg-sky-500/10 border-sky-400/60 text-sky-700"
            : "bg-slate-900/5 border-slate-900/10 text-slate-900/80 hover:bg-slate-900/10"
        }`}
      >
        <svg
          width="12"
          height="12"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.6"
          strokeLinecap="round"
          strokeLinejoin="round"
          className={open ? "text-sky-600" : "text-slate-400"}
        >
          <rect x="3" y="4" width="18" height="17" rx="2" />
          <path d="M3 9h18M8 2v4M16 2v4" />
        </svg>
        {value}
      </button>
      {open && (
        <div
          style={{
            position: "fixed",
            left: "50%",
            top: popupTop,
            transform: "translateX(-50%)",
            width: POPUP_WIDTH,
          }}
          className="z-50 p-2.5 rounded-lg bg-elevated backdrop-blur-md border border-slate-900/10 shadow-[0_6px_20px_-4px_rgba(15,23,42,0.14)]"
        >
          {/* 月份切换 */}
          <div className="flex items-center justify-between mb-2">
            <button
              type="button"
              onClick={() => shiftMonth(-1)}
              className="w-6 h-6 grid place-items-center rounded-md text-slate-500 hover:bg-slate-900/10 hover:text-slate-800 transition-colors"
            >
              <svg
                width="13"
                height="13"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2.2"
                strokeLinecap="round"
                strokeLinejoin="round"
              >
                <polyline points="15 18 9 12 15 6" />
              </svg>
            </button>
            <span className="num text-xs font-semibold text-slate-800 select-none">
              {view.year}年{view.month + 1}月
            </span>
            <button
              type="button"
              onClick={() => shiftMonth(1)}
              className="w-6 h-6 grid place-items-center rounded-md text-slate-500 hover:bg-slate-900/10 hover:text-slate-800 transition-colors"
            >
              <svg
                width="13"
                height="13"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2.2"
                strokeLinecap="round"
                strokeLinejoin="round"
              >
                <polyline points="9 18 15 12 9 6" />
              </svg>
            </button>
          </div>
          {/* 星期表头 */}
          <div
            className="grid mb-1"
            style={{ gridTemplateColumns: GRID_COLS }}
          >
            {WEEK_HEADERS.map((w) => (
              <div
                key={w}
                className="h-6 grid place-items-center text-[10px] font-medium text-slate-400 select-none"
              >
                {w}
              </div>
            ))}
          </div>
          {/* 日期网格 */}
          <div className="grid" style={{ gridTemplateColumns: GRID_COLS }}>
            {cells.map((c) => {
              const disabled =
                (min !== undefined && c.ymd < min) ||
                (max !== undefined && c.ymd > max);
              const selected = c.ymd === value;
              const isToday = c.ymd === todayStr;
              return (
                <div key={c.ymd} className="h-8 grid place-items-center">
                  <button
                    type="button"
                    disabled={disabled}
                    onClick={() => pick(c.ymd)}
                    className={`num w-7 h-7 grid place-items-center rounded-full text-[13px] font-medium tabular-nums transition-colors ${
                      selected
                        ? "bg-sky-500 text-white font-semibold"
                        : disabled
                        ? "text-slate-300 cursor-not-allowed"
                        : isToday
                        ? "text-sky-700 ring-1 ring-inset ring-sky-500/45 hover:bg-sky-500/10"
                        : c.inMonth
                        ? "text-slate-700 hover:bg-sky-500/10"
                        : "text-slate-400/60 hover:bg-sky-500/10"
                    }`}
                  >
                    {c.day}
                  </button>
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
