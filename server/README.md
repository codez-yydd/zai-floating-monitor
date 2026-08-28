# zbar-sync · ZBar 多设备额度同步服务

ZBar 客户端把本地 ZCode 的用量数据增量上传到此服务，实现多台电脑（公司/家里）汇总查看。

**自托管、单用户**：你自己部署，数据存在你自己的服务器上。支持 HTTP / HTTPS / 裸 IP。

技术栈：**Python 3 + Flask + 标准库 sqlite3**，零编译、启动即用。

---

## 快速开始

### 1. 上传文件到服务器

把 `server/` 文件夹里**这几个文件**传到服务器（不要传 `.venv/` 和 `zbar-data/`）：

```
app.py          # 主程序（所有接口）
db.py           # 数据库操作（自动建库建表）
auth.py         # 鉴权（master token / device token）
config.py       # 配置（端口、数据目录）
static/
  index.html    # 手机端查看页面（Flask 自动托管在 /static/）
start.sh        # 启动脚本
requirements.txt
```

### 2. 安装依赖

**用终端执行**（不要用宝塔面板的「Python 项目管理器」，它可能报 pip 错误）：

```bash
cd /你的上传路径/zbar-sync
pip3 install flask
```

> 只需要 Flask 一个依赖。Python 3.8+ 即可，sqlite3 是标准库自带的。
> 如果 `pip3` 报错，先修复：`python3 -m ensurepip --upgrade`，或用 `python3 -m pip install flask`。

### 3. 启动

```bash
./start.sh
```

或者直接：

```bash
python3 app.py
```

首次启动会自动：
- 创建 `zbar-data/` 目录
- 创建 SQLite 数据库（`zbar-data/usage.db`）
- 生成 Master Token 并打印到日志

输出类似：

```
[zbar-sync] 初始化完成
[zbar-sync] MASTER_TOKEN: 9f3a7c2e8b1d4a6f...
[zbar-sync]   ↑ 复制此 token 到客户端「同步设置」注册设备
[zbar-sync] 监听端口: 3838
```

**复制这个 Master Token**（之后也可随时 `cat zbar-data/master.token` 查看）。

### 4. 放行端口

宝塔面板 → **安全** → 放行 **3838** 端口（TCP）。
云服务器（阿里云/腾讯云等）还要去**控制台安全组**放行 3838。

### 5. 客户端连接

ZBar 面板 → 点 **⇅** → 填写：
- 服务器地址：`http://你的服务器IP:3838`
- 准入凭证：粘贴 Master Token
- 设备名称：如 `work` / `home`

点「连接并注册」完成。

---

## 在宝塔面板设置开机自启（进程守护）

启动后如果终端关了进程会退出。用宝塔的**进程守护**让它常驻：

1. 宝塔 → **软件商店** → 搜索安装 **「进程守护管理器」**
2. 打开进程守护 → **添加守护进程**
3. 填写：
   - **名称**：`zbar-sync`
   - **启动用户**：`root`
   - **运行目录**：`/你的上传路径/zbar-sync`
   - **启动命令**：`python3 /你的上传路径/zbar-sync/app.py`
4. 保存并启动

这样即使服务器重启，进程也会自动拉起。

---

## 自定义端口

启动时指定环境变量：

```bash
PORT=8080 python3 app.py
```

或者在 `start.sh` 里改 `export PORT=8080`。

---

## 环境变量

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `PORT` | `3838` | 监听端口 |
| `DATA_DIR` | `./zbar-data` | 数据目录（库 + master.token） |
| `HOST` | `0.0.0.0` | 监听地址（0.0.0.0 = 对外可访问） |

---

## API 概览

| 接口 | 鉴权 | 用途 |
|------|------|------|
| `POST /register` | Master Token | 注册设备，返回 Device Token |
| `POST /sync` | Device Token | 增量上传用量明细（每条可带 source，缺省 zcode；proto 5 起可带会话/项目字段） |
| `GET /usage` | Device / View Token | 聚合查询（overall + by_model + trend，可选 source 过滤） |
| `GET /models` | Device / View Token | 全部设备 × 全部来源的模型清单（distinct，价格配置用） |
| `GET /snapshots` | Device / View Token | Z.ai 额度快照查询 |
| `GET /agent-quota-snapshots` | Device / View Token | Codex / Claude / Cursor 额度快照查询 |
| `POST /period_detail` | Device / View Token | 按周期返回逐条用量明细 |
| `GET /projects` | Device / View Token | 项目维度聚合查询（proto 5） |
| `GET /overview` | Device / View Token | 手机首屏聚合（周期汇总 + 额度 + 项目 Top + 设备） |
| `GET /devices` | Device / View Token | 设备列表 |
| `POST /view/token/regenerate` | Master Token | 重新生成 view token（手机页只读凭证），明文仅返回一次 |
| `GET /view/check` | View Token | view token 校验（手机页首次输入时调用） |
| `POST /device/revoke` | Master Token | 撤销设备 |
| `POST /cleanup` | Master Token | 数据清理（按设备/按时间/全清/reset） |
| `GET /cleanup/status` | Device Token | 数据量 + 自动清理配置 |
| `POST /cleanup/config` | Master Token | 配置自动定时清理 |
| `GET /health` | 无 | 健康检查 |

> 鉴权级别说明：**View Token 是只读凭证**，仅能调用上表标注「Device / View Token」的查询接口；所有写操作（注册、上传、设备管理、清理）只认 Master / Device Token。

---

## Codex / Claude 数据（source 维度）

新版客户端除了 ZCode 用量，还会上传 Codex CLI 与 Claude Code 的用量明细。各来源通过 `source` 字段区分（`zcode` / `codex` / `claude`），同一台设备、同一 `local_rowid` 在不同 source 下互不冲突（各来源 rowid 序列相互独立），上传与查询游标也相互独立。服务端表结构对 source 取值无限制，后续新增来源无需再改服务端。

**升级方式**：拉取新代码后重启服务即可。首次启动自动迁移 `usage_records` 表结构（新增 `source` 列，主键改为 `(device_id, source, local_rowid)`），老数据全部自动标记为 `zcode`，无损保留，索引自动重建。建议服务端先于客户端升级。

**升级顺序保护**：新版客户端上传 Codex / Claude 数据前会先探测服务端协议版本（`/sync` 响应新增 `proto: 2` 字段）。旧服务端不返回该字段，客户端不会上传这些数据也不推进游标（同步日志提示"服务端版本过旧"），升级服务端后自动恢复——即使客户端先升级也不会丢数据；旧客户端不受任何影响。

**接口变化**（均向后兼容）：

- `POST /sync`：records 每条新增 `source` 字段，缺省 `zcode`（旧客户端不传即 zcode）。客户端保证每批 records 属同一来源，`last_rowid` / `max_rowid` 按该来源自己的 rowid 序列计数。同一主键重传且 `computed_total_tokens` 更大时覆盖旧值（Claude Code 会话流式落盘，客户端会把消息终值补传修正先前上传的中间值；zcode/codex 记录不可变，重传同值对覆盖守卫为无操作）。
- `GET /usage`：新增可选 query 参数 `source`（`zcode` / `codex` / `claude`），不传 = 全部来源合并；`by_model` 与 `trend.by_model` 每个分组新增 `source` 字段，便于前端区分展示。
- `POST /period_detail`：body 同样新增可选 `source` 字段。
- `GET /models`：返回全部设备 × 全部来源出现过的模型清单（`[{source, provider_id, model_id}]`，distinct）。客户端价格设置页据此展示"其他设备同步上来、本机没有"的模型并纳入价格更新检查。旧服务端无此接口时客户端静默降级（只用本地清单），升级服务端即可启用。

---

## 手机端查看页面

服务端自带一个单文件手机仪表盘（原生 JS，零外部资源，内网可直接用），在手机浏览器查看全部设备的用量汇总。

### 获取 view token

view token 是**只读**的「手机端查看页面访问令牌」，与 device token 相互独立：

- **首次启动自动生成**，明文只打印一次到服务端日志：

  ```
  [zbar-sync] VIEW_TOKEN: 3f8a…
  [zbar-sync]   ↑ 手机端查看页面访问令牌（浏览器打开 /static/index.html 时输入）
  ```

- **重新生成**（旧 token 立即失效，用于轮换或泄露后作废）：

  ```bash
  curl -X POST http://你的服务器:3838/view/token/regenerate \
       -H "Content-Type: application/json" \
       -d '{"master_token": "你的 MASTER_TOKEN"}'
  ```

  响应里的 `view_token` 明文仅返回这一次，请立即保存。

### 访问页面

浏览器打开 `http://你的服务器:3838/static/index.html`，首次输入 view token（通过 `GET /view/check` 校验），通过后保存在手机浏览器 `localStorage`，之后免输入。页面每 60 秒自动刷新，也支持下拉或点右上角按钮手动刷新。

页面内容：今日 / 近 7 天 / 近 30 天的 Token 与请求数、各来源分组占比、各订阅额度进度条、近 7 天项目 Top10、设备列表。不含花费展示（服务端无价格表）。

### 安全边界

- view token **只读**：只能调用 `/usage`、`/projects`、`/overview` 等查询接口，无法上传数据、管理设备或删除数据；写操作只认 Master / Device Token。
- 可随时通过 regenerate 接口轮换，泄露后旧 token 立即失效。
- 服务端只存 token 的 SHA-256 哈希，明文不落盘。
- 自托管 HTTP 下 view token 与 device token 一样是明文传输，建议仅在内网使用，或通过 Nginx 配置 HTTPS 反向代理。

---

## proto 5：会话 / 项目维度

`POST /sync` 上传的明细 records 每条新增三个可选字段：`session_id`（会话 ID）、`project_key`（归一化后的项目路径键，如 `/users/chacca/code/my-app`，无法归属时为 null）、`project_display`（原始形态路径）。服务端在 `usage_records` 表幂等补列存储（全部可空，不改主键），并新增只读接口 `GET /projects?from=<ms>&to=<ms>[&devices=id1,id2]` 按 `(project_key, source)` 聚合查询，`project_key` 为空的记录聚合为 `"__unknown__"`（手机页显示为「未知项目」）。**旧客户端（proto 2/3/4）完全不受影响**：不传新字段即照旧落库为 NULL，聚合归入 `"__unknown__"`；客户端探测 `/sync` 响应中的 `proto: 5` 后才会启用新字段上传。

---

## 数据清理

通过客户端「同步设置 → 数据管理」操作，或直接调 API。

| 操作 | 说明 | 不可逆 |
|------|------|--------|
| 按设备删除 | 删某台设备的全部明细 | ✅ |
| 按时间删除 | 删 N 天前的数据（缩短趋势图历史） | ✅ |
| 全部清空 | 清空用量数据，保留设备注册 | ✅ |
| Reset | 连设备一起清，回到初始状态 | ✅ |

> **清理即删除**：被清理的数据不会从客户端重新上传。

---

## 数据存储

```
zbar-data/
├── master.token     # Master Token（持久化，重启不重新生成）
├── usage.db         # SQLite 数据库（自动创建）
└── usage.db-wal     # WAL 日志（SQLite 自动管理）
```

备份：直接复制 `zbar-data/` 目录即可。

---

## 安全说明

- **HTTP 警告**：使用 HTTP 或裸 IP 时，Token 和用量数据在网络中明文传输。建议内网使用，或通过 Nginx 配置 HTTPS 反向代理。
- **鉴权**：Master Token 注册准入，Device Token 日常鉴权（服务端只存哈希）。
- **隐私**：同步的数据仅含模型名、Token 数量、时间戳，**不含代码和对话内容**。
