# ZBar · ZCode Token 监控

一个常驻菜单栏的轻量浮动面板，实时统计 [ZCode](https://z.ai) CLI 的 Token 用量与花费。点击菜单栏图标即可唤出毛玻璃面板查看今日 / 7 天 / 30 天的明细，并按模型分组展示。

> macOS 上的形态：菜单栏标题实时显示「今日花费 + 总 Token」，点击图标弹出贴近菜单栏的 popover 面板；Dock 不显示图标，失焦自动收起。

![Tauri](https://img.shields.io/badge/Tauri-2.0-orange) ![React](https://img.shields.io/badge/React-19-blue) ![TypeScript](https://img.shields.io/badge/TypeScript-5.8-3178c6) ![TailwindCSS](https://img.shields.io/badge/Tailwind-v4-38bdf8) ![Rust](https://img.shields.io/badge/Rust-edition%202021-dea584)

---

## 📸 界面截图

| 汇总视图 | Z.ai 视图 |
|:---:|:---:|
| ![汇总视图](doc/img/summary.png) | ![Z.ai 视图](doc/img/zai-quota.png) |
| 多服务合计花费 / Token 与订阅额度 | Coding Plan 5 小时 / 每周 / MCP 额度 |

| 趋势与模型排行 | Cursor 视图 |
|:---:|:---:|
| ![趋势与模型排行](doc/img/summary-trend.png) | ![Cursor 视图](doc/img/cursor.png) |
| 按时段用量趋势与模型排行 | Pro / Auto / API 额度与用量统计 |

| 价格设置 | 设备同步 |
|:---:|:---:|
| ![价格设置](doc/img/settings.png) | ![设备同步](doc/img/sync.png) |
| 双币种单价、额度监控、汇率与全局快捷键 | 多设备增量同步与数据管理 |

---

## ✨ 功能特性

- **实时菜单栏标题** — 常驻 macOS 顶部菜单栏，每 30 秒刷新，显示今日自然日的总花费（`¥xx.xx`，美元模式下为 `$xx.xx`）与总 Token（如 `3.7M`）。
- **浮动统计面板** — 点击托盘图标唤出，支持 **今日 / 24h / 7天 / 30天 / 自定义** 时间范围切换。
- **Token 明细** — 输入、输出、缓存读、缓存写、推理 Token 分类统计，并按占比可视化。
- **按模型分组** — 列出每个模型的请求数、Token、花费；未配置价格的模型会标记 ⚠ 提示。
- **价格配置** — 支持 **人民币 / 美元** 双货币，为每个模型设置「输入 / 输出 / 缓存读」三项单价（每百万 Token），配置持久化到 `~/.zbar/pricing.json`；支持一键「检查价格更新」，从社区维护的 [models.dev](https://models.dev) 拉取全厂商最新价格（按汇率换算人民币参考价）。
- **缓存感知计费** — `input_tokens` 已包含缓存读部分，计费时缓存读按缓存价单独计算，非缓存输入按输入价计算，避免重复计费。
- **原生体验** — macOS 使用 `popover` 毛玻璃材质 + 透明窗口；Windows/Linux 面板贴近任务栏展开。
- **自动刷新** — 面板数据每 30 秒自动拉取一次。
- **Coding Plan 额度监控** — 订阅用户可在面板顶部查看 **5 小时窗口**、**每周额度**与 **MCP 月度额度**的用量进度条，颜色随用量警示（绿→琥珀→红），并显示下次重置倒计时；支持**国内 / 国际**双端点切换。
- **🖥 Cursor 用量统计** — 自动读取本机 Cursor 应用的登录凭据（也支持手动 Cookie），统计 Pro / Auto / API 套餐额度与 Token 花费明细，美元花费按汇率折算后并入汇总视图。
- **💱 汇率自动更新** — USD→CNY 汇率默认每日自动联网更新（也可改为手动填写），用于 Cursor 花费折算与人民币参考价换算。
- **🧭 多服务汇总视图** — 「汇总 / Z.ai / Cursor」标签切换：多服务合计花费与 Token、订阅额度卡片、分时趋势图与模型排行。
- **⌨️ 全局快捷键** — 默认 `alt+shift+z` 唤起 / 隐藏面板，可在设置中自定义或停用。
- **📈 周额度对比** — 基于本地额度快照（90 天滚动保留）对比每个重置周期的额度用量，支持跨设备合并。
- **📝 日报 / 周报** — 一键生成 Markdown 日报（今日）/ 周报（近 7 天）并保存到本地。
- **🔄 多设备同步** — 自托管同步服务（`server/`），让公司 / 家里等多台电脑汇总查看全量用量。明细增量上传 + `(device, rowid)` 去重，支持**设备筛选**（全部 / 本机 / 指定设备）和**数据清理**（按设备 / 按时间 / 全清 + 可配置自动定时清理）。详见 [server/README.md](./server/README.md)。

---

## 🧱 技术栈

| 层 | 技术 |
| --- | --- |
| 桌面框架 | [Tauri 2](https://tauri.app/)（Rust 后端 + WebView 前端） |
| 前端 | React 19 + TypeScript + Vite 7 |
| 样式 | Tailwind CSS v4（`@tailwindcss/vite`） |
| 数据库 | SQLite（`rusqlite`，只读访问 ZCode 数据库） |
| macOS 原生 | `objc2` / `objc2-app-kit`（毛玻璃透明度调优、Accessory 模式隐藏 Dock） |

---

## 📁 项目结构

```
zai-floating-monitor/
├── src/                      # 前端（React）
│   ├── App.tsx               # 视图路由：统计 / 价格设置 / 设备同步 / 周额度对比 / 报表
│   ├── StatsPanel.tsx        # 统计面板（含设备筛选器 + 本地/远端数据合并）
│   ├── SummaryTab.tsx        # 汇总视图（多服务合计 / 趋势 / 模型排行）
│   ├── CursorPanel.tsx       # Cursor 视图（额度 + 用量统计）
│   ├── PricingPanel.tsx      # 价格配置面板（含 Coding Plan / Cursor 统计 / 快捷键）
│   ├── QuotaPanel.tsx        # Coding Plan 额度监控
│   ├── ComparePanel.tsx      # 周额度对比
│   ├── ReportPanel.tsx       # 日报 / 周报（Markdown 导出）
│   ├── SyncPanel.tsx         # 设备同步设置面板（注册 / 数据管理）
│   ├── RangePicker.tsx       # 时间范围选择器
│   ├── api.ts                # invoke 封装（调用 Rust 命令）
│   ├── types.ts              # 与 Rust 结构一一对应的 TS 类型
│   ├── format.ts             # Token / 金额 / 百分比格式化
│   └── main.tsx              # 入口
├── src-tauri/                # Rust 后端（客户端）
│   ├── src/
│   │   ├── lib.rs            # 应用入口、托盘、面板逻辑、Tauri 命令
│   │   ├── db.rs             # SQLite 只读查询（统计 / 模型列表 / 增量查询）
│   │   ├── pricing.rs        # 价格配置读写 + models.dev 价格更新
│   │   ├── quota.rs          # Coding Plan 额度查询（5 小时 / 每周 / MCP）
│   │   ├── quota_history.rs  # 额度快照历史（JSONL，90 天滚动保留）
│   │   ├── cursor.rs         # Cursor 用量统计（自动凭据 / Cookie / API）
│   │   ├── shortcut.rs       # 全局快捷键配置
│   │   ├── sync.rs           # 多设备同步（配置 / 增量上传 / 远端查询 / 清理）
│   │   └── main.rs
│   ├── capabilities/         # Tauri 权限配置
│   └── tauri.conf.json       # 窗口 / 打包配置
├── server/                   # 自托管同步服务（Python + Flask）
│   ├── app.py                # Flask 应用 + 所有接口
│   ├── db.py                 # SQLite 操作（自动建库建表）
│   ├── auth.py               # 鉴权（master token / device token）
│   ├── config.py             # 配置（端口 / 数据目录）
│   └── README.md             # 部署文档
├── index.html
└── vite.config.ts
```

---

## 📋 环境要求

1. **Node.js** ≥ 18（推荐 20+）及 npm
2. **Rust**（stable 工具链）—— 通过 [rustup](https://rustup.rs/) 安装
3. Tauri 2 的系统依赖：
   - **macOS**：Xcode Command Line Tools（`xcode-select --install`）
   - **Windows**：Microsoft Edge WebView2 + MSVC 构建工具
   - **Linux**：`webkit2gtk`、`libayatana-appindicator` 等（详见 [Tauri 文档](https://v2.tauri.app/start/prerequisites/)）

4. **ZCode CLI** 已安装并已产生使用记录（数据库位于 `~/.zcode/cli/db/db.sqlite`）

---

## 🚀 快速开始

```bash
# 1. 安装依赖
npm install

# 2. 开发模式（同时启动 Vite 与 Tauri 窗口，热重载）
npm run tauri dev
```

开发服务器运行在 `http://localhost:1420`，Tauri 会加载它并打开原生窗口。

---

## 📜 常用脚本

| 命令 | 说明 |
| --- | --- |
| `npm run dev` | 仅启动 Vite 前端开发服务器（浏览器调试，无 Tauri 原生能力） |
| `npm run tauri dev` | **开发模式**：启动前端 + Tauri 原生窗口，支持热重载 |
| `npm run build` | 类型检查 + 构建前端产物到 `dist/` |
| `npm run tauri build` | **打包**：生成可分发的安装包（`.app` / `.dmg` / `.msi` / `.AppImage`） |
| `npm run preview` | 本地预览已构建的前端产物 |

---

## 📦 打包发布

```bash
# 生成当前平台的安装包，输出在 src-tauri/target/release/bundle/
npm run tauri build
```

- **macOS**：`ZBar.app`、`.dmg`
- **Windows**：`.msi`、`NSIS .exe`
- **Linux**：`.deb`、`.AppImage`、`.rpm`

如需指定目标：

```bash
npm run tauri build -- --target aarch64-apple-darwin
```

---

## ⚙️ 配置说明

### 数据库路径

ZBar 以 **只读** 方式访问 ZCode 的 SQLite 数据库，不会干扰 ZCode 的写入。

定位优先级：

1. 环境变量 **`ZBAR_DB`**（指向自定义的 `.sqlite` 文件）
2. 默认路径：**`~/.zcode/cli/db/db.sqlite`**

### 价格配置

通过面板内的「⚙ 价格设置」可视化编辑，或直接编辑文件：

**路径：`~/.zbar/pricing.json`**

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

- 单位：**每百万 Token** 的价格
- 三个字段：`input`（非缓存输入）、`output`（输出）、`cache_read`（缓存读）
- 只需填写需要计费的模型；未填的模型在面板中显示 `—` 并标记 ⚠

面板内支持一键「检查价格更新」：从社区维护的 [models.dev](https://models.dev) 拉取全厂商最新模型价格（USD / 百万 Token），按当前汇率换算人民币参考价，确认后合并进本地价格表。

在面板内点击「⚙ 价格设置 → 打开目录」可一键用 Finder 打开 `~/.zbar/`。

### Coding Plan 额度监控

订阅 GLM Coding Plan 的用户可在统计面板顶部查看 5 小时窗口、每周额度与 MCP 月度额度的实时用量。

**配置方式**：打开「⚙ 价格设置」，在「Coding Plan 额度监控」区块填写：

- **API Token**：从智谱开放平台获取的 Coding Plan Token
- **端点**：国内用户选「🇨🇳 国内」（`open.bigmodel.cn`），海外用户选「🌐 国际」（`api.z.ai`）

配置保存在 **`~/.zbar/quota.json`**：

```json
{
  "token": "your-coding-plan-api-token",
  "endpoint": "cn"
}
```

额度数据通过 `GET /api/monitor/usage/quota/limit` 接口实时获取，每 30 秒自动刷新。未配置 Token 时面板显示「去配置」引导，不影响其他功能。

### 全局快捷键

默认 `alt+shift+z` 唤起 / 隐藏面板，可在「⚙ 价格设置 → 全局快捷键」中修改或停用，配置持久化到 `~/.zbar/shortcut.json`：

```json
{
  "enabled": true,
  "accelerator": "alt+shift+z"
}
```

### Cursor 统计

打开「⚙ 价格设置 → Cursor 统计」进行配置：

- **认证方式** — **自动**（默认，读取本机 Cursor 应用的本地登录凭据，需已安装并登录 Cursor）或 **手动 Cookie**。
- **汇率** — USD→CNY，默认每日自动联网更新，也可取消勾选后手动填写；Cursor 的美元花费按此汇率折算成人民币。

配置保存在 `~/.zbar/cursor.json`（手动 Cookie 模式下 Cookie 也保存在此文件，仅存于本机）。

### 周额度对比与报表

- **周额度对比** — 以额度快照历史为数据源（`~/.zbar/quota_history.jsonl`，append-only，90 天滚动保留），按重置周期对比额度用量，支持跨设备合并。
- **日报 / 周报** — 面板内一键生成 Markdown 报表（日报 = 今日，周报 = 近 7 天）并保存为 `.md` 文件。

### 多设备同步

多台电脑（公司 / 家里）汇总查看全量用量。通过自托管同步服务实现，数据存在你自己的服务器上。

**部署服务端**（Python + Flask，装个依赖直接跑）：

```bash
cd server
pip3 install -r requirements.txt   # 只需 Flask
python3 app.py                      # 启动后日志会打印 Master Token
```

详见 [server/README.md](./server/README.md)。

**客户端配置**：点击面板顶部 **⇅** 图标进入「设备同步」设置，填写服务器地址 + Master Token + 设备名称（如 `work` / `home`），点「连接并注册」。

注册成功后：

- **设备筛选器** — 统计面板顶部出现设备下拉，可选「全部（汇总）」「本机」或指定设备。
- **同步模式** — 手动（点「立即同步」）或自动（按间隔自动上传）。
- **数据管理** — 按设备 / 按时间 / 全部清空，可配置服务端自动定时清理。

配置保存在 **`~/.zbar/sync.json`**（Master Token 不持久化，注册后即丢弃）：

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

**同步原理**：ZCode 的用量记录是只增不删的，客户端按 `rowid` 增量上传，服务端用 `(device_id, local_rowid)` 去重。查询时本机数据查本地、其他设备数据查远端，合并展示，避免重复计算。

---

## 🧮 计费规则

为避免重复计费，输入类 Token 拆分计算：

```
非缓存输入 = input_tokens - cache_read_tokens

花费 = 非缓存输入 × input价
     + output_tokens   × output价
     + cache_read      × cache_read价
        （单位：每百万 Token，结果以货币计）
```

面板与菜单栏标题共用同一套 Rust 计算逻辑，保证数字一致。

---

## 🔒 数据安全

- 数据库连接使用 `SQLITE_OPEN_READ_ONLY`，**只读不写**，绝不影响 ZCode 数据。
- 价格配置仅保存在用户本地 `~/.zbar/` 目录，不上传任何数据。
- **多设备同步**为可选功能，默认关闭。启用后仅同步模型名、Token 数量、时间戳，**不含代码和对话内容**。服务端自托管，数据存在你自己的服务器上。
- 面板失焦自动隐藏，窗口常驻不销毁，点击托盘即可重新唤出。

---

## 📄 License

[MIT License](./LICENSE) · Copyright © 2026 小轩
