# ZBar · ZCode Token Monitor

A lightweight menu-bar floating panel that tracks the Token usage and cost of the [ZCode](https://z.ai) CLI in real time. Click the menu-bar icon to summon a vibrancy panel showing today's / 7-day / 30-day breakdowns, grouped by model.

> On macOS: the menu-bar title shows today's cost + total tokens in real time; clicking the icon pops a popover panel hugging the menu bar; the Dock icon is hidden, and the panel auto-dismisses on blur.

![Tauri](https://img.shields.io/badge/Tauri-2.0-orange) ![React](https://img.shields.io/badge/React-19-blue) ![TypeScript](https://img.shields.io/badge/TypeScript-5.8-3178c6) ![TailwindCSS](https://img.shields.io/badge/Tailwind-v4-38bdf8) ![Rust](https://img.shields.io/badge/Rust-edition%202021-dea584)

---

## 📸 Screenshots

| Summary view | Z.ai view |
|:---:|:---:|
| ![Summary view](doc/img/summary.png) | ![Z.ai view](doc/img/zai-quota.png) |
| Total cost / tokens across services & subscription quotas | Coding Plan 5-hour / weekly / MCP quotas |

| Trend & model ranking | Cursor view |
|:---:|:---:|
| ![Trend & model ranking](doc/img/summary-trend.png) | ![Cursor view](doc/img/cursor.png) |
| Hourly usage trend and model ranking | Pro / Auto / API quotas & usage stats |

| Pricing settings | Device sync |
|:---:|:---:|
| ![Pricing settings](doc/img/settings.png) | ![Device sync](doc/img/sync.png) |
| USD unit prices, FX rate & global hotkey | Incremental multi-device sync & data management |

---

## ✨ Features

- **Live menu-bar title** — lives in the macOS menu bar, refreshing every 30s, showing today's total cost (`¥xx.xx`, or `$xx.xx` in USD mode) and total tokens (e.g. `3.7M`).
- **Floating stats panel** — summoned by clicking the tray icon; supports **today / 24h / 7d / 30d / custom** time ranges.
- **Token breakdown** — input, output, cache-read, cache-write and reasoning tokens, visualized by proportion.
- **Per-model grouping** — requests, tokens and cost per model; models without pricing are flagged with ⚠.
- **Pricing config** — **CNY / USD** dual currency; set input / output / cache-read unit prices (per million tokens) for each model, persisted to `~/.zbar/pricing.json`; one-click "check price updates" pulls the latest community-maintained prices from [models.dev](https://models.dev) (CNY reference prices converted at the current FX rate).
- **Cache-aware billing** — `input_tokens` already includes cache-read tokens; cache-read is billed at the cache rate and non-cache input at the input rate, avoiding double counting.
- **Native feel** — macOS uses the `popover` vibrancy material + transparent window; Windows/Linux panels unfold near the taskbar.
- **Auto refresh** — panel data is re-fetched every 30 seconds.
- **Coding Plan quota monitor** — subscribers can view the **5-hour window**, **weekly quota** and **MCP monthly quota** progress bars at the top of the panel; the color escalates with usage (green → amber → red) and shows a reset countdown. Credentials and the API endpoint are read **automatically** from the local ZCode client's signed-in state (`~/.zcode/v2/config.json`) — zero configuration.
- **🖥 Cursor usage stats** — reads the local Cursor app's login credentials automatically; tracks Pro / Auto / API plan quotas and per-model token costs, with USD costs converted at the FX rate and merged into the summary view.
- **💱 Auto FX rate** — the USD→CNY rate is refreshed online daily by default (manual input supported); used to convert USD costs across services and for CNY reference prices.
- **🧭 Multi-service summary view** — summary / Z.ai / Cursor tabs: total cost & tokens across services, subscription quota cards, hourly trend chart and model ranking.
- **⌨️ Global hotkey** — summon / hide the panel with `alt+shift+z` by default; customizable or disable-able in settings.
- **📈 Weekly quota compare** — compare quota usage across reset cycles based on local quota snapshots (90-day rolling retention), with cross-device merge.
- **📝 Daily / weekly reports** — one-click Markdown report (daily = today, weekly = last 7 days) saved locally.
- **🔄 Multi-device sync** — self-hosted sync server (`server/`) to aggregate usage across machines (office / home). Incremental detail upload + `(device, rowid)` dedup, with **device filtering** (all / local / specific device) and **data cleanup** (by device / by time / all + configurable auto cleanup). See [server/README.md](./server/README.md).
- **⚡ Speed & TTFT** — average output speed (tok/s) and first-token latency (TTFT) from per-call durations, with noise filtering (whole-block delivery detection, timing-outlier rejection). Available on ZCode / Claude panels (Codex / Cursor data sources carry no duration; columns auto-hide).
- **🎯 Current model** — each agent panel shows the "current model" (latest model actually used + relative time); the summary page shows it per service group.
- **⬆️ In-app auto update** — Settings → "About & update" to check / download / silently install new versions; endpoints fall back between **GitHub and Gitee** automatically, packages are signature-verified, and a silent startup check lights a red dot on the settings entry when a new version is available.

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
│   ├── App.tsx               # View router: stats / pricing / sync / weekly compare / reports
│   ├── StatsPanel.tsx        # Stats panel (device filter + local/remote merge)
│   ├── SummaryTab.tsx        # Summary view (multi-service totals / trend / model ranking)
│   ├── CursorPanel.tsx       # Cursor view (quotas + usage stats)
│   ├── PricingPanel.tsx      # Pricing config panel
│   ├── SettingsPanel.tsx     # Settings page (opacity / language / autostart / sources / FX rate / hotkey)
│   ├── QuotaPanel.tsx        # Coding Plan quota monitor
│   ├── ComparePanel.tsx      # Weekly quota compare
│   ├── ReportPanel.tsx       # Daily / weekly reports (Markdown export)
│   ├── SyncPanel.tsx         # Device sync settings (register / data management)
│   ├── RangePicker.tsx       # Time-range picker
│   ├── api.ts                # invoke wrappers (Rust commands)
│   ├── types.ts              # TS types mirroring the Rust structs
│   ├── format.ts             # Token / currency / percent formatting
│   └── main.tsx              # Entry
├── src-tauri/                # Rust backend
│   ├── src/
│   │   ├── lib.rs            # App entry, tray, panel logic, Tauri commands
│   │   ├── db.rs             # Read-only SQLite queries (stats / model list)
│   │   ├── pricing.rs        # Pricing config read/write + models.dev updates
│   │   ├── quota.rs          # Coding Plan quota queries (auto ZCode-client credentials; 5h / weekly / MCP)
│   │   ├── quota_history.rs  # Quota snapshot history (JSONL, 90-day retention)
│   │   ├── cursor.rs         # Cursor usage stats (auto credentials / cookie / API)
│   │   ├── shortcut.rs       # Global hotkey config
│   │   ├── sync.rs           # Multi-device sync (config / incremental upload / cleanup)
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

### 🔄 In-app auto update (dual repos)

The app has a built-in updater: Settings → "About & update" checks, downloads and silently installs new versions; a silent check also runs at startup and lights a red dot on the settings entry. Update endpoints fall back between **GitHub / Gitee** automatically — either repo being reachable is enough.

**One-time setup for the first release**:

1. Generate the update signing keypair (if you lose the private key you can **never** push updates again — back it up somewhere safe):

   ```bash
   npx tauri signer generate -w ~/.tauri/zbar-updater.key
   ```

2. Put the public key (`~/.tauri/zbar-updater.key.pub`) into `plugins.updater.pubkey` of `src-tauri/tauri.conf.json` (already configured for this project).
3. Export `GITEE_TOKEN` (Gitee → Settings → Personal access tokens, with the projects scope) and install & login the `gh` CLI for GitHub.

**Releasing a new version**:

```bash
scripts/release.sh 0.2.0 --notes "Release notes"
# Build and verify artifacts only, without uploading:
scripts/release.sh 0.2.0 --no-upload
```

The script syncs the version in three places → builds with signing (artifacts carry `.sig`) → generates two `latest.json` files (installer download URLs pointing at each repo) → uploads a GitHub Release (tag `v{version}`, marked latest) → recreates the Gitee Release under the fixed tag `latest` (update metadata source). macOS artifacts need to be built on a Mac and uploaded afterwards.

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

The panel also supports one-click "check price updates": pull the latest community-maintained prices for all vendors from [models.dev](https://models.dev) (USD per million tokens), convert them to CNY reference prices at the current FX rate, and merge them into the local price table.

In the panel, "⚙ Pricing → open directory" opens `~/.zbar/` directly in Finder.

### Coding Plan Quota Monitor

GLM Coding Plan subscribers can view real-time usage of the 5-hour window, weekly quota and MCP monthly quota at the top of the stats panel.

**Zero configuration**: the credential is read automatically (read-only, never written back) from the local ZCode client's signed-in state — the apiKey of the built-in Coding Plan provider in `~/.zcode/v2/config.json` — and the API endpoint is inferred from that provider's baseURL (`open.bigmodel.cn` / `api.z.ai`). The only prerequisite is that the ZCode client on this machine is signed in to a Coding Plan subscription; otherwise the panel shows a sign-in prompt without affecting other features.

Quota data is fetched live via `GET /api/monitor/usage/quota/limit` and auto-refreshes every 30 seconds.

### Global Hotkey

Summon / hide the panel with `alt+shift+z` by default; change or disable it under "⚙ Pricing → Global hotkey". Persisted to `~/.zbar/shortcut.json`:

```json
{
  "enabled": true,
  "accelerator": "alt+shift+z"
}
```

### Cursor Stats

Reads the local Cursor app's login credentials automatically (Cursor must be installed and signed in) — no configuration needed.

**FX rate** (Settings → FX rate): USD→CNY refreshed online daily by default, or entered manually; USD costs across services are converted to CNY at this rate. The config is stored at `~/.zbar/cursor.json` (locally only).

### Weekly Compare & Reports

- **Weekly quota compare** — based on local quota snapshots (`~/.zbar/quota_history.jsonl`, append-only, 90-day rolling retention); compare quota usage across reset cycles, with cross-device merge.
- **Daily / weekly reports** — generate a Markdown report in one click (daily = today, weekly = last 7 days) and save it as a `.md` file.

### Multi-device Sync

Aggregate usage across machines (office / home) via a self-hosted sync server — the data stays on your own server.

**Deploy the server** (Python + Flask):

```bash
cd server
pip3 install -r requirements.txt   # Flask only
python3 app.py                      # the Master Token is printed on startup
```

See [server/README.md](./server/README.md).

**Client setup**: click the **⇅** icon at the top of the panel to open "Device sync", fill in the server URL + Master Token + device name (e.g. `work` / `home`), then click "Connect & register".

After registering:

- **Device filter** — a device dropdown appears at the top of the stats panel: "All (summary)" / "This device" / a specific device.
- **Sync mode** — manual ("Sync now") or automatic (upload on an interval).
- **Data management** — cleanup by device / by time / all, plus configurable server-side auto cleanup.

The config is stored at `~/.zbar/sync.json` (the Master Token is not persisted — discarded right after registration):

```json
{
  "enabled": true,
  "mode": "auto",
  "interval_seconds": 60,
  "server_url": "http://192.168.1.100:3838",
  "device_id": "uuid-xxx",
  "device_name": "work",
  "device_token": "...",
  "last_uploaded_rowid": 12345
}
```

**How it works**: ZCode usage records are append-only. Clients upload incrementally by `rowid`; the server dedups by `(device_id, local_rowid)`. Local data is queried locally, other devices' data remotely, then merged for display — no double counting.

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
- **Account switching** saves ZCode login snapshots under `~/.zbar/accounts/` (directory mode 700, file mode 600) for one-click switching. Snapshots **stay on this machine only** and are never synced. This app is an unofficial community tool, not affiliated with Zhipu AI.
- **Multi-device sync** is optional and off by default. When enabled, only model names, token counts and timestamps are synced — **never code or conversation content**. The server is self-hosted; the data stays on your own server.
- The panel hides on blur; the window stays alive (not destroyed) and can be re-summoned by clicking the tray.

---

## 📄 License

[MIT License](./LICENSE) · Copyright © 2026 小轩
