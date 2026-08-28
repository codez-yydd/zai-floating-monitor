# ZBar · ZCode Token Monitor

![Tauri](https://img.shields.io/badge/Tauri-2.0-orange) ![React](https://img.shields.io/badge/React-19-blue) ![TypeScript](https://img.shields.io/badge/TypeScript-5.8-3178c6) ![TailwindCSS](https://img.shields.io/badge/Tailwind-v4-38bdf8) ![Rust](https://img.shields.io/badge/Rust-edition%202021-dea584) ![License](https://img.shields.io/badge/License-MIT-yellow) ![Platform](https://img.shields.io/badge/Platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey) [![GitHub Repo](https://img.shields.io/badge/GitHub-codez--yydd%2Fzai--floating--monitor-181717?logo=github)](https://github.com/codez-yydd/zai-floating-monitor) [![Gitee Repo](https://img.shields.io/badge/Gitee-codezwx%2Fzai--floating--monitor-C71D23?logo=gitee)](https://gitee.com/codezwx/zai-floating-monitor)

A lightweight menu-bar floating panel that tracks the Token usage and cost of the [ZCode](https://z.ai) CLI in real time. Click the menu-bar icon to summon a vibrancy panel showing today's / 7-day / 30-day breakdowns, grouped by model.

> On macOS: the menu-bar title shows today's cost + total tokens in real time; clicking the icon pops a popover panel hugging the menu bar; the Dock icon is hidden, and the panel auto-dismisses on blur.

---

## 📍 Repositories

- **GitHub**: [codez-yydd/zai-floating-monitor](https://github.com/codez-yydd/zai-floating-monitor)
- **Gitee**: [codezwx/zai-floating-monitor](https://gitee.com/codezwx/zai-floating-monitor)

Both repositories have exactly the same content — the latest version can be obtained from either one; the GitHub / Gitee dual-repo auto-fallback update source is also based on these addresses.

---

## 🎬 Live Wallpaper Demo

![ZCode live wallpaper demo](doc/img/zcode-skin.gif)

> Inject a live video wallpaper into the ZCode desktop app as the conversation background in one click — the original app is backed up automatically before installation and can be restored anytime.

---

## 📑 Contents

- [📍 Repositories](#-repositories)
- [🎬 Live Wallpaper Demo](#-live-wallpaper-demo)
- [📸 Screenshots](#-screenshots)
- [✨ Features](#-features)
- [🧱 Tech Stack](#-tech-stack)
- [📁 Project Structure](#-project-structure)
- [📋 Prerequisites](#-prerequisites)
- [🚀 Quick Start](#-quick-start)
- [📜 Scripts](#-scripts)
- [📦 Build & Release](#-build--release)
  - [🔄 In-app auto update (dual repos)](#-in-app-auto-update-dual-repos)
- [⚙️ Configuration](#️-configuration)
  - [Database Path](#database-path)
  - [Pricing](#pricing)
  - [Coding Plan Quota Monitor](#coding-plan-quota-monitor)
  - [Global Hotkey](#global-hotkey)
  - [Dynamic wallpaper](#dynamic-wallpaper)
  - [Cursor Stats](#cursor-stats)
  - [Kimi Stats](#kimi-stats)
  - [Weekly Compare & Reports](#weekly-compare--reports)
  - [Multi-device Sync](#multi-device-sync)
- [🧮 Billing Rules](#-billing-rules)
- [🔒 Data Safety](#-data-safety)
- [📄 License](#-license)

---

## 📸 Screenshots

<table>
  <tr>
    <td width="50%" align="center">
      <img src="doc/img/summary.png" width="320" alt="Summary view"/><br/>
      <b>Summary view</b> — Total cost / tokens across services & subscription quotas
    </td>
    <td width="50%" align="center">
      <img src="doc/img/zai-quota.png" width="320" alt="Z.ai view"/><br/>
      <b>Z.ai view</b> — Coding Plan 5-hour / weekly / MCP quotas
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <img src="doc/img/summary-trend.png" width="320" alt="Trend & model ranking"/><br/>
      <b>Trend & model ranking</b> — Hourly usage trend and model ranking
    </td>
    <td width="50%" align="center">
      <img src="doc/img/cursor.png" width="320" alt="Cursor view"/><br/>
      <b>Cursor view</b> — Pro / Auto / API quotas & usage stats
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <img src="doc/img/settings.png" width="320" alt="Pricing settings"/><br/>
      <b>Pricing settings</b> — USD unit prices, FX rate & global hotkey
    </td>
    <td width="50%" align="center">
      <img src="doc/img/sync.png" width="320" alt="Device sync"/><br/>
      <b>Device sync</b> — Incremental multi-device sync & data management
    </td>
  </tr>
</table>

---

## ✨ Features

**🖥 Core experience**

- **Live menu-bar title** — lives in the macOS menu bar, refreshing every 30s, showing today's total cost (`¥xx.xx`, or `$xx.xx` in USD mode) and total tokens (e.g. `3.7M`).
- **Floating stats panel** — summoned by clicking the tray icon; supports **today / 24h / 7d / 30d / custom** time ranges.
- **Native feel** — macOS uses the `popover` vibrancy material + transparent window; Windows/Linux panels unfold near the taskbar.
- **Auto refresh** — panel data is re-fetched every 30 seconds.
- **⌨️ Global hotkey** — summon / hide the panel with `alt+shift+z` by default; customizable or disable-able in settings.
- **🎨 Dynamic wallpaper** — inject a live video wallpaper into the ZCode desktop app as the conversation background in one click; the original app is backed up automatically before installation and can be restored anytime. The wallpaper is invalidated after a ZCode upgrade and needs to be reinstalled; on macOS, ZCode's built-in updater becomes unavailable after injection (even restoring cannot bring it back) — re-download ZCode from the official site.

**🤖 Multi-service usage stats**

- **Coding Plan quota monitor** — subscribers can view the **5-hour window**, **weekly quota** and **MCP monthly quota** progress bars at the top of the panel; the color escalates with usage (green → amber → red) and shows a reset countdown. Credentials and the API endpoint are read **automatically** from the local ZCode client's signed-in state (`~/.zcode/v2/config.json`) — zero configuration.
- **🖥 Cursor usage stats** — reads the local Cursor app's login credentials automatically; tracks Pro / Auto / API plan quotas and per-model token costs, with USD costs converted at the FX rate and merged into the summary view.
- **🌙 Kimi usage stats** — parses local `~/.kimi-code/sessions` session records (`wire.jsonl`) to tally token usage, cost and **token output speed (TPS)**; on machines signed in to the Kimi Code CLI, local credentials are detected automatically (OAuth expiry renewed in the background — no configuration needed) to fetch subscription quotas live: **5-hour rolling window / cycle quota** progress bars with reset countdowns, booster wallet balance and official membership tier name; a Kimi API Key can also be configured manually in settings.
- **🧭 Multi-service summary view** — summary / Z.ai / Cursor / Kimi tabs: total cost & tokens across services, subscription quota cards, hourly trend chart and model ranking.
- **⚡ Speed & TTFT** — average output speed (tok/s) and first-token latency (TTFT) from per-call durations, with noise filtering (whole-block delivery detection, timing-outlier rejection). Available on ZCode / Claude / Kimi panels (Kimi's tok/s is a TPS estimate derived from request durations with no TTFT, so first-token latency is hidden there just like Claude; Codex / Cursor data sources carry no duration and auto-hide these columns).
- **🎯 Current model** — each agent panel shows the "current model" (latest model actually used + relative time); the summary page shows it per service group.

**💰 Pricing & billing**

- **Pricing config** — **CNY / USD** dual currency; set input / output / cache-read unit prices (per million tokens) for each model, persisted to `~/.zbar/pricing.json`; one-click "check price updates" pulls the latest community-maintained prices from [models.dev](https://models.dev) (CNY reference prices converted at the current FX rate).
- **Cache-aware billing** — `input_tokens` already includes cache-read tokens; cache-read is billed at the cache rate and non-cache input at the input rate, avoiding double counting.
- **💱 Auto FX rate** — the USD→CNY rate is refreshed online daily by default (manual input supported); used to convert USD costs across services and for CNY reference prices.

**📊 Data & reports**

- **Token breakdown** — input, output, cache-read, cache-write and reasoning tokens, visualized by proportion.
- **Per-model grouping** — requests, tokens and cost per model; models without pricing are flagged with ⚠.
- **📈 Weekly quota compare** — compare quota usage across reset cycles based on local quota snapshots (90-day rolling retention), with cross-device merge.
- **📝 Daily / weekly reports** — one-click Markdown report (daily = today, weekly = last 7 days) saved locally.

**🔄 Sync & update**

- **🔄 Multi-device sync** — self-hosted sync server (`server/`) to aggregate usage across machines (office / home). Incremental detail upload + `(device, rowid)` dedup, with **device filtering** (all / local / specific device) and **data cleanup** (by device / by time / all + configurable auto cleanup). See [server/README.md](./server/README.md).
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
│   ├── KimiPanel.tsx         # Kimi view (quotas + usage stats)
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
│   │   ├── kimi.rs           # Kimi Code usage stats (wire.jsonl parsing + OAuth in-memory renewal for live quotas)
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
3. Add repository Secrets under **Settings → Secrets and variables → Actions**:
   - `TAURI_SIGNING_PRIVATE_KEY`: the **full content** of the private key file `~/.tauri/zbar-updater.key`
   - `GITEE_TOKEN` (optional): a Gitee personal access token (with the projects scope); when missing, the Gitee endpoint is skipped and only the GitHub source is published

**Releasing a new version** (built in the cloud by GitHub Actions for both platforms — no local toolchain needed):

```bash
# After the version is synced in package.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml and committed:
git tag v0.2.0
git push origin v0.2.0
```

The pipeline (`.github/workflows/release.yml`) automatically: builds and signs three platforms in parallel (Windows x64 / macOS Apple Silicon / macOS Intel, artifacts carry `.sig`) → generates two `latest.json` files (installer download URLs pointing at each repo) → creates a GitHub Release (tag `v{version}`, marked latest) → recreates the Gitee Release under the fixed tag `latest` (update metadata source). If the Gitee upload fails or the token is missing, re-run the `release` job from the Actions page to publish it alone.

> The macOS packages are not signed with a developer certificate nor notarized; on first launch right-click the app → "Open", or allow it in System Settings.

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

### Dynamic wallpaper

Open "Dynamic wallpaper" via the 🎨 button in the toolbar and inject a live video wallpaper into the ZCode desktop app as the conversation background. The original resources are backed up automatically before installation and can be restored anytime with one click.

Notes:

- The wallpaper is invalidated after a ZCode upgrade and needs to be reinstalled.
- On macOS, ZCode's built-in updater becomes unavailable after injection (even restoring cannot bring it back) — to upgrade ZCode, re-download it from the official website.

### Cursor Stats

Reads the local Cursor app's login credentials automatically (Cursor must be installed and signed in) — no configuration needed.

**FX rate** (Settings → FX rate): USD→CNY refreshed online daily by default, or entered manually; USD costs across services are converted to CNY at this rate. The config is stored at `~/.zbar/cursor.json` (locally only).

### Kimi Stats

Parses local `~/.kimi-code/sessions` session records (`wire.jsonl`). Prerequisite: the Kimi Code CLI is installed, signed in, and has produced local sessions. Subscription quotas are fetched live via the locally detected credentials (renewed automatically in the background when OAuth expires), or you can configure a Kimi API Key manually in settings.

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
