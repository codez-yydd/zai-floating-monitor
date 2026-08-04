# ZBar · ZCode Token 监控

一个常驻菜单栏的轻量浮动面板，实时统计 [ZCode](https://z.ai) CLI 的 Token 用量与花费。点击菜单栏图标即可唤出毛玻璃面板查看今日 / 7 天 / 30 天的明细，并按模型分组展示。

> macOS 上的形态：菜单栏标题实时显示「今日花费 + 总 Token」，点击图标弹出贴近菜单栏的 popover 面板；Dock 不显示图标，失焦自动收起。

![Tauri](https://img.shields.io/badge/Tauri-2.0-orange) ![React](https://img.shields.io/badge/React-19-blue) ![TypeScript](https://img.shields.io/badge/TypeScript-5.8-3178c6) ![TailwindCSS](https://img.shields.io/badge/Tailwind-v4-38bdf8) ![Rust](https://img.shields.io/badge/Rust-edition%202021-dea584)

---

## ✨ 功能特性

- **实时菜单栏标题** — 每 30 秒刷新，显示今日自然日的总花费（`¥xx.xx`）与总 Token（如 `3.7M`）。
- **浮动统计面板** — 点击托盘图标唤出，支持 **今日 / 24h / 7天 / 30天 / 自定义** 时间范围切换。
- **Token 明细** — 输入、输出、缓存读、缓存写、推理 Token 分类统计，并按占比可视化。
- **按模型分组** — 列出每个模型的请求数、Token、花费；未配置价格的模型会标记 ⚠ 提示。
- **价格配置** — 支持 **人民币 / 美元** 双货币，为每个模型设置「输入 / 输出 / 缓存读」三项单价（每百万 Token），配置持久化到 `~/.zbar/pricing.json`。
- **缓存感知计费** — `input_tokens` 已包含缓存读部分，计费时缓存读按缓存价单独计算，非缓存输入按输入价计算，避免重复计费。
- **原生体验** — macOS 使用 `popover` 毛玻璃材质 + 透明窗口；Windows/Linux 面板贴近任务栏展开。
- **自动刷新** — 面板数据每 30 秒自动拉取一次。

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
│   ├── App.tsx               # 视图路由：统计 / 价格设置
│   ├── StatsPanel.tsx        # 统计面板
│   ├── PricingPanel.tsx      # 价格配置面板
│   ├── RangePicker.tsx       # 时间范围选择器
│   ├── api.ts                # invoke 封装（调用 Rust 命令）
│   ├── types.ts              # 与 Rust 结构一一对应的 TS 类型
│   ├── format.ts             # Token / 金额 / 百分比格式化
│   └── main.tsx              # 入口
├── src-tauri/                # Rust 后端
│   ├── src/
│   │   ├── lib.rs            # 应用入口、托盘、面板逻辑、Tauri 命令
│   │   ├── db.rs             # SQLite 只读查询（统计 / 模型列表）
│   │   ├── pricing.rs        # 价格配置读写
│   │   └── main.rs
│   ├── capabilities/         # Tauri 权限配置
│   └── tauri.conf.json       # 窗口 / 打包配置
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

在面板内点击「⚙ 价格设置 → 打开目录」可一键用 Finder 打开 `~/.zbar/`。

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
- 面板失焦自动隐藏，窗口常驻不销毁，点击托盘即可重新唤出。

---

## 📄 License

私有项目，保留所有权利。
