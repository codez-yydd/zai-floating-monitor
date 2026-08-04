# ZBar · ZCode Token Monitor

A lightweight menu-bar floating panel that tracks the Token usage and cost of the [ZCode](https://z.ai) CLI in real time. Click the menu-bar icon to summon a vibrancy panel showing today's / 7-day / 30-day breakdowns, grouped by model.

> On macOS: the menu-bar title shows today's cost + total tokens in real time; clicking the icon pops a popover panel hugging the menu bar; the Dock icon is hidden, and the panel auto-dismisses on blur.

![Tauri](https://img.shields.io/badge/Tauri-2.0-orange) ![React](https://img.shields.io/badge/React-19-blue) ![TypeScript](https://img.shields.io/badge/TypeScript-5.8-3178c6) ![TailwindCSS](https://img.shields.io/badge/Tailwind-v4-38bdf8) ![Rust](https://img.shields.io/badge/Rust-edition%202021-dea584)

---

## ✨ Features

- **Live menu-bar title** — refreshes every 30s, showing today's total cost (`¥xx.xx`) and total tokens (e.g. `3.7M`).
- **Floating stats panel** — summoned by clicking the tray icon; supports **today / 24h / 7d / 30d / custom** time ranges.
- **Token breakdown** — input, output, cache-read, cache-write and reasoning tokens, visualized by proportion.
- **Per-model grouping** — requests, tokens and cost per model; models without pricing are flagged with ⚠.
- **Pricing config** — **CNY / USD** dual currency; set input / output / cache-read unit prices (per million tokens) for each model, persisted to `~/.zbar/pricing.json`.
- **Cache-aware billing** — `input_tokens` already includes cache-read tokens; cache-read is billed at the cache rate and non-cache input at the input rate, avoiding double counting.
- **Native feel** — macOS uses the `popover` vibrancy material + transparent window; Windows/Linux panels unfold near the taskbar.
- **Auto refresh** — panel data is re-fetched every 30 seconds.

---

## 🧱 Tech Stack

| Layer | Tech |
| --- | --- |
| Desktop | [Tauri 2](https://tauri.app/) (Rust backend + WebView frontend) |
| Frontend | React 19 + TypeScript + Vite 7 |
| Styling | Tailwind CSS v4 (`@tailwindcss/vite`) |
| Database | SQLite (`rusqlite`, read-only access to the ZCode DB) |
| macOS native | `objc2` / `objc2-app-kit` (vibrancy tuning, Accessory mode to hide Dock) |

---

## 📁 Project Structure

```
zai-floating-monitor/
├── src/                      # Frontend (React)
│   ├── App.tsx               # View router: stats / pricing
│   ├── StatsPanel.tsx        # Stats panel
│   ├── PricingPanel.tsx      # Pricing config panel
│   ├── RangePicker.tsx       # Time-range picker
│   ├── api.ts                # invoke wrappers (Rust commands)
│   ├── types.ts              # TS types mirroring the Rust structs
│   ├── format.ts             # Token / currency / percent formatting
│   └── main.tsx              # Entry
├── src-tauri/                # Rust backend
│   ├── src/
│   │   ├── lib.rs            # App entry, tray, panel logic, Tauri commands
│   │   ├── db.rs             # Read-only SQLite queries (stats / model list)
│   │   ├── pricing.rs        # Pricing config read/write
│   │   └── main.rs
│   ├── capabilities/         # Tauri permissions
│   └── tauri.conf.json       # Window / bundling config
├── index.html
└── vite.config.ts
```

---

## 📋 Prerequisites

1. **Node.js** ≥ 18 (20+ recommended) and npm
2. **Rust** (stable toolchain) — install via [rustup](https://rustup.rs/)
3. Tauri 2 system dependencies:
   - **macOS**: Xcode Command Line Tools (`xcode-select --install`)
   - **Windows**: Microsoft Edge WebView2 + MSVC build tools
   - **Linux**: `webkit2gtk`, `libayatana-appindicator`, etc. (see [Tauri docs](https://v2.tauri.app/start/prerequisites/))

4. **ZCode CLI** installed and having produced usage records (DB at `~/.zcode/cli/db/db.sqlite`)

---

## 🚀 Quick Start

```bash
# 1. Install dependencies
npm install

# 2. Dev mode (starts both Vite and the Tauri window with hot reload)
npm run tauri dev
```

The dev server runs at `http://localhost:1420`; Tauri loads it and opens the native window.

---

## 📜 Scripts

| Command | Description |
| --- | --- |
| `npm run dev` | Start the Vite dev server only (browser debugging, no native capabilities) |
| `npm run tauri dev` | **Dev mode**: frontend + native Tauri window, with hot reload |
| `npm run build` | Type-check + build the frontend into `dist/` |
| `npm run tauri build` | **Bundle**: produce distributable installers (`.app` / `.dmg` / `.msi` / `.AppImage`) |
| `npm run preview` | Locally preview the built frontend |

---

## 📦 Build & Release

```bash
# Build installers for the current platform; output in src-tauri/target/release/bundle/
npm run tauri build
```

- **macOS**: `ZBar.app`, `.dmg`
- **Windows**: `.msi`, NSIS `.exe`
- **Linux**: `.deb`, `.AppImage`, `.rpm`

To target a specific architecture:

```bash
npm run tauri build -- --target aarch64-apple-darwin
```

---

## ⚙️ Configuration

### Database Path

ZBar opens ZCode's SQLite database in **read-only** mode and never interferes with ZCode's writes.

Lookup priority:

1. Environment variable **`ZBAR_DB`** (path to a custom `.sqlite` file)
2. Default path: **`~/.zcode/cli/db/db.sqlite`**

### Pricing

Edit visually via the "⚙ Pricing" page inside the panel, or edit the file directly:

**Path: `~/.zbar/pricing.json`**

```json
{
  "cny": {
    "glm-4.6": { "input": 10, "output": 30, "cache_read": 1 },
    "glm-4.5-air": { "input": 2, "output": 6, "cache_read": 0.2 }
  },
  "usd": {
    "glm-4.6": { "input": 1.4, "output": 4.2, "cache_read": 0.14 }
  }
}
```

- Unit: price **per million tokens**
- Three fields: `input` (non-cache input), `output`, `cache_read`
- Only fill in the models you want billed; unpriced models show `—` and are flagged ⚠

In the panel, "⚙ Pricing → open directory" opens `~/.zbar/` directly in Finder.

---

## 🧮 Billing Rules

To avoid double counting, input tokens are split:

```
non_cache_input = input_tokens - cache_read_tokens

cost = non_cache_input * input_rate
     + output_tokens   * output_rate
     + cache_read      * cache_read_rate
        (per-million-token rates; result in the chosen currency)
```

The panel and the menu-bar title share the same Rust calculation logic, so the numbers always match.

---

## 🔒 Data Safety

- The DB connection uses `SQLITE_OPEN_READ_ONLY` — **strictly read-only**, never touching ZCode data.
- Pricing config is stored only in the user-local `~/.zbar/` directory; nothing is uploaded.
- The panel hides on blur; the window stays alive (not destroyed) and can be re-summoned by clicking the tray.

---

## 📄 License

Proprietary — all rights reserved.
