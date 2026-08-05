# ZBar · iOS 版（App + 小组件）

桌面版 ZBar（Tauri）的 iOS 配套应用。复用同一个自托管同步服务（`server/`），把 iPhone 当作一台新设备注册，在手机上查看今日花费 / Token / 额度，并通过锁屏 & 主屏小组件快速一览。

> **零后端改动**：iOS 端不新增任何服务端接口，直接调用 `server/app.py` 现有的 `/register`、`/usage`、`/snapshots`、`/period_detail`、`/devices`。额度数据则直连智谱开放平台。

---

## 目录结构

```
ios/
├── ZBar/
│   ├── ZBar.xcodeproj/         # Xcode 工程文件
│   └── ZBar/                   # 主 App 源码
│       ├── ZBarApp.swift       # @main 入口
│       ├── Models/             # Types.swift + PricingDefaults.json（内置价格表）
│       ├── Core/               # API 客户端 / 计费 / 折算 / 格式化 / 配置 / 数据加载
│       └── Views/              # 统计 / 额度 / 趋势 / 对比 / 设置 五个 Tab
└── ZBarWidget/                 # Widget Extension 源码
    ├── ZBarWidget.swift        # WidgetBundle（主屏 + 锁屏）
    ├── WidgetViews.swift       # 小/中尺寸 + 锁屏三个 family 的视图
    └── WidgetShared.swift      # ⚠️ 只编译进 Widget target 的类型定义
```

---

## 开发环境

- **macOS 14+**（Xcode 15+）
- **iOS 17.0+** 真机（用到了 iOS 17 的 SwiftUI 新 API：`onChange` 双参数签名、Widget `containerBackground`）
- **不需要 Apple 开发者账号**，用 Xcode 自带的免费 Personal Team 即可（自用、7 天签名）

---

## 方式一：直接打开工程（首选）

```bash
open ios/ZBar/ZBar.xcodeproj
```

打开后：

1. 顶部选择目标设备（你的 iPhone 真机）。
2. 左侧选 `ZBar` 项目 → **Signing & Capabilities**：
   - `Team`：选你的 **Personal Team**（标 "Personal Team" 的那项）。
   - `Bundle Identifier`：保持 `com.chacca.zbar`，**不要改**（改了 App Group 要跟着改）。
3. 对 `ZBarWidget` target 做同样操作。
4. 数据线连 iPhone → 点 **Run**（▶）。
5. 首次运行后 iPhone 会提示「不受信任的开发者」→ 去 **设置 → 通用 → VPN与设备管理** → 信任你的开发者证书。

> 如果工程打不开（报 "Project format is too old / 无法读取"），直接用下面的**方式二**，最稳。

---

## 方式二：手动新建工程（100% 可靠，兜底方案）

工程文件（`.xcodeproj`）格式在不同 Xcode 版本之间有差异。如果方式一失败，按下面重建：

### 1. 新建主 App

1. Xcode → **File → New → Project** → **iOS / App** → Next。
2. 填写：
   - Product Name：`ZBar`
   - Organization Identifier：`com.chacca`
   - Interface：**SwiftUI**
   - Language：**Swift**
   - 取消勾选 "Include Tests"（不需要）
   - 保存到 `ios/` 目录下（会提示覆盖，选覆盖或新建 `ZBar2` 后把源码挪过去）
3. 删除 Xcode 自动生成的 `ContentView.swift`、`ZBarApp.swift`、`Assets.xcassets/AccentColor` 等占位文件。

### 2. 拖入主 App 源码

把 `ios/ZBar/ZBar/` 下的所有文件拖进 Xcode 的 `ZBar` 组里：

- 勾选 **Copy items if needed** ✅
- 勾选 **Add to targets: ZBar** ✅（不要勾 ZBarWidget）
- 包括 `Models/`、`Core/`、`Views/` 三个文件夹，以及 `ZBarApp.swift`、`Info.plist`、`ZBar.entitlements`、`PricingDefaults.json`

### 3. 新建 Widget Extension

1. 选中 `ZBar` 项目 → **File → New → Target** → **iOS / Widget Extension** → Next。
2. 填写：
   - Product Name：`ZBarWidget`
   - 取消勾选 "Include Configuration App Intent"（不需要）
3. 删除 Xcode 自动生成的 `ZBarWidget.swift` 等占位文件。
4. 把 `ios/ZBarWidget/` 下的 `ZBarWidget.swift`、`WidgetViews.swift`、`WidgetShared.swift`、`ZBarWidget.entitlements` 拖进 `ZBarWidget` 组：
   - 勾选 **Add to targets: ZBarWidget** ✅（不要勾 ZBar）

### 4. 配置 App Group（关键！）

两个 target 都要配同一个 App Group，否则 Widget 读不到 App 写的数据：

**对 `ZBar` target**：
1. **Signing & Capabilities → + Capability → App Groups**。
2. 点 **+**，添加：`group.com.chacca.zbar`

**对 `ZBarWidget` target**：重复上面两步，加同一个 group。

### 5. 配置 ATS（允许 HTTP）

如果工程新建后没自动继承，手动在主 App 的 `Info.plist` 加：

```xml
<key>NSAppTransportSecurity</key>
<dict>
    <key>NSAllowsArbitraryLoads</key>
    <true/>
</dict>
```

（自托管服务可能是裸 HTTP，不加这个会连不上）

### 6. 签名 & 运行

两个 target 的 **Signing** 都选 **Personal Team** → Run。

---

## 首次使用：连接同步服务

App 装上后：

1. 打开 App → 底部 **设置** Tab。
2. **同步服务 → 连接同步服务…**：
   - **服务器**：你部署 `server/` 的地址，如 `http://192.168.1.100:3838`
   - **Master Token**：`server/` 启动时打印的 Master Token（或 `cat zbar-data/master.token`）
   - **设备名**：填 `iPhone` 或任意名字
   - 点 **连接并注册**
3. **Coding Plan 额度** 区块：填智谱开放平台的 Coding Plan Token，选端点（国内 / 国际），点 **保存并查询**。
4. 回到 **统计** / **额度** / **趋势** Tab 即可看到数据。

> 注册成功后，iPhone 会作为一台新设备出现在桌面版 ZBar 的「设备筛选器」里，多设备汇总自动生效。

---

## 添加桌面小组件

1. 在 iPhone 主屏长按空白处 → 进入编辑模式 → 点左上角 **+**。
2. 搜索 **ZBar**。
3. 选择尺寸：
   - **小**（2×2）：今日花费 + Token + 额度进度条
   - **中**（4×2）：左侧大数字 + 右侧三档额度详情 + 重置倒计时
4. 点 **添加小组件**。

锁屏小组件（iOS 17+）：
1. 锁屏长按 → 自定义 → 添加小组件 → 找到 ZBar。
2. 三种形态：
   - **长条**（Rectangular）：花费 + 周/5h 百分比
   - **圆形**（Circular）：weekly 百分比环形
   - **顶部行**（Inline）：一行文字 "ZBar ¥12.34 · 3.7M"

---

## 小组件刷新说明（重要限制）

iOS Widget **不能像桌面端那样 30 秒刷新一次**。系统限制：

- 由系统调度，通常 **15~40 分钟** 刷新一次，且系统会根据电量、使用频率降频。
- App 打开时会主动拉数据并写入共享容器，Widget 下次刷新时拿到最新数据。

**想更快看到最新数据**：打开 App 让它刷新一次，然后回桌面，Widget 会在下一个系统刷新窗口更新。

---

## 免费账号（Personal Team）的限制

| 限制 | 说明 |
|---|---|
| **7 天签名** | 过期后 App 打不开，需重新连 Mac 跑一次 Xcode（数据不丢，存在 UserDefaults / App Group） |
| **3 个 App ID/周** | 反复删除重建 Bundle ID 会占用配额 |
| **无推送** | 不能用远程推送触发 Widget 刷新 |
| **Widget 随 App 签名** | Widget 不能单独安装 |

自己用完全没问题，只是每周插一次数据线。

---

## 数据流（一图看懂）

```
┌─────────────┐   /usage /snapshots    ┌──────────────┐
│ 你的 server  │◄──────────────────────│  ZBar iOS App │
│  (Flask)     │   /period_detail       │  (主进程)      │
└─────────────┘   /register /devices    └──────┬───────┘
                                                │ 写 App Group
                                                ▼
┌─────────────┐   GET /quota/limit    ┌──────────────┐
│ 智谱开放平台  │◄─────────────────────│ Widget 读取    │
│ (直连)       │                       │ (独立进程)     │
└─────────────┘                       └──────────────┘
```

- **用量统计**：iOS App → 你的 server（Device Token 鉴权）
- **额度监控**：iOS App → 智谱开放平台（用户 Token，不经过 server）
- **Widget**：不联网，只读 App 写入的 App Group 缓存

---

## 计费逻辑（与桌面端 1:1）

iOS 端复刻了桌面版的核心计算（`src/merge.ts`、`src/peak.ts`、`src/format.ts`）：

| 文件 | 对应桌面端 | 说明 |
|---|---|---|
| `Core/Billing.swift` | `merge.ts` | `modelCost`：非缓存输入 = input − cache_read，避免重复计费 |
| `Core/Peak.swift` | `peak.ts` | V2/V3 高峰期倍率折算 + ZCode 0.67 优惠 |
| `Core/Format.swift` | `format.ts` | Token/金额/百分比/倒计时格式化 |
| `Core/PeriodParser.swift` | `quota_history.rs` | 按 `weekly_reset` 跳变切分重置周期 |
| `Models/PricingDefaults.json` | `public/pricing-defaults.json` | 内置参考价格表 |

价格表默认从 bundle 内置的 `PricingDefaults.json` 加载，可在 App 内编辑（设置 → 价格配置），编辑后存 UserDefaults。

---

## 已知限制 / TODO

- [ ] 高峰期时段（Peak Segment）目前 iOS 端只支持开关和订阅类型切换，不支持可视化编辑时段（沿用默认或桌面端配置）。如需自定义时段，建议在桌面端配好后保持默认。
- [ ] 小组件无主动刷新，依赖系统调度 + App 前台预拉。
- [ ] 未实现 Background Tasks（`BGAppRefreshTask`）后台预拉——可作为后续优化，提升 Widget 数据新鲜度。
- [ ] 暂无独立的"用量报告"页面（桌面版的 ReportPanel），统计 Tab 已覆盖主要信息。

---

## 故障排查

**小组件显示 "—" 或空数据**
- 确认 App 已打开并成功连上 server / 查询过额度（App 会把数据写进 App Group）。
- 确认两个 target 都配了同一个 App Group `group.com.chacca.zbar`。
- 删掉小组件重新添加一次。

**连不上 server（连接超时）**
- 确认 iPhone 和 server 在同一网络，或 server 有公网 IP。
- 确认 server 防火墙/安全组放行了端口（默认 3838）。
- 确认 ATS 已允许 HTTP（见上文）。
- 在 iPhone Safari 访问 `http://你的IP:3838/health`，应返回 `ok`。

**额度查询失败**
- 确认 Coding Plan Token 正确（不是 ZCode CLI 的 key，是智谱开放平台的 Token）。
- 端点选对：国内用户选 🇨🇳，海外选 🌐。

**签名 7 天后失效**
- 重新用数据线连 Mac，在 Xcode 点 Run 即可续签（数据保留）。
