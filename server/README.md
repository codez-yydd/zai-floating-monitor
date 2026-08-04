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
| `POST /sync` | Device Token | 增量上传用量明细 |
| `GET /usage` | Device Token | 聚合查询（overall + by_model + trend） |
| `GET /devices` | Device Token | 设备列表 |
| `POST /device/revoke` | Master Token | 撤销设备 |
| `POST /cleanup` | Master Token | 数据清理（按设备/按时间/全清/reset） |
| `GET /cleanup/status` | Device Token | 数据量 + 自动清理配置 |
| `POST /cleanup/config` | Master Token | 配置自动定时清理 |
| `GET /health` | 无 | 健康检查 |

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
