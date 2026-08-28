"""SQLite 数据库操作：自动建库建表 + 增删改查。

关键点（与 Rust 版一致）：
- 服务端数据库是自建自用的，首次启动 usage.db 不存在时，
  sqlite3.connect() 会自动创建文件 + 执行建表 SQL，对用户透明。
- 表结构、字段名与 Rust 版完全一致，保证客户端无需改动。
- model_usage 是 append-only，用 (device_id, local_rowid) 作主键去重。
- 新版支持多数据来源（source 维度）：'zcode'（ZCode 本地库）/ 'codex'
  （Codex CLI 导入库），主键改为 (device_id, source, local_rowid)。
  旧库首次启动自动迁移（见 _migrate_usage_records），老数据无损。
"""

import math
import sqlite3
import threading
import time

from config import DB_PATH, DATA_DIR

# 线程锁：Flask 多线程模式下保护 sqlite 写入
_db_lock = threading.Lock()

# ===== 建表 SQL（与 Rust 版 schema.rs 完全一致）=====

SCHEMA_DEVICES = """
CREATE TABLE IF NOT EXISTS devices (
    device_id   TEXT    PRIMARY KEY,
    device_name TEXT    NOT NULL,
    token_hash  TEXT    NOT NULL,
    created_at  INTEGER NOT NULL
)
"""


def _usage_records_schema(table):
    """usage_records 建表 SQL。

    source 标记数据来源（'zcode' / 'codex'）：同一台设备、同一 local_rowid
    在不同 source 下互不冲突（两套 rowid 序列各自从 1 递增）。
    迁移时用同一份定义建 usage_records_new，保证新旧表结构严格一致。
    session_id / project_key / project_display 为 proto 5 新增的会话与
    项目维度列（全部可空，缺省 NULL）；旧库由 _migrate_usage_records_columns
    用 ALTER TABLE 原地补列，补列后列序与本定义一致（都在末尾）。
    """
    return f"""
CREATE TABLE IF NOT EXISTS {table} (
    device_id                   TEXT    NOT NULL,
    source                      TEXT    NOT NULL DEFAULT 'zcode',
    local_rowid                 INTEGER NOT NULL,
    started_at                  INTEGER NOT NULL,
    model_id                    TEXT,
    provider_id                 TEXT,
    input_tokens                INTEGER NOT NULL DEFAULT 0,
    output_tokens               INTEGER NOT NULL DEFAULT 0,
    cache_read_input_tokens     INTEGER NOT NULL DEFAULT 0,
    cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens            INTEGER NOT NULL DEFAULT 0,
    computed_total_tokens       INTEGER NOT NULL DEFAULT 0,
    uploaded_at                 INTEGER NOT NULL,
    session_id                  TEXT,
    project_key                 TEXT,
    project_display             TEXT,
    PRIMARY KEY (device_id, source, local_rowid)
)
"""


SCHEMA_USAGE_RECORDS = _usage_records_schema("usage_records")

# 额度快照表：客户端每次查询额度后追加一条，周期解析靠 weekly_reset 跳变。
# 与客户端本地 quota_history.jsonl 字段对齐，多一个 device_id 用于按设备筛选。
# account 为快照归属账号指纹（客户端多账号采样隔离用）：旧客户端不传，
# 统一存空串；主键含 account，同设备多账号同毫秒落盘的多条采样互不撞键
# （旧主键 (device_id, ts) 会把其他账号的采样静默吞掉）。
SCHEMA_QUOTA_SNAPSHOTS = """
CREATE TABLE IF NOT EXISTS quota_snapshots (
    device_id    TEXT    NOT NULL,
    ts           INTEGER NOT NULL,
    account      TEXT    NOT NULL DEFAULT '',
    level        TEXT,
    weekly_pct   INTEGER NOT NULL DEFAULT 0,
    weekly_reset INTEGER,
    hour5_pct    INTEGER NOT NULL DEFAULT 0,
    mcp_pct      INTEGER NOT NULL DEFAULT 0,
    mcp_used     INTEGER,
    mcp_total    INTEGER,
    PRIMARY KEY (device_id, ts, account)
)
"""

# Agent 额度快照按窗口拆行保存，查询时再组装回客户端使用的 snapshot 形状。
# plan_type 为空时统一保存为空字符串；快照身份不含 plan_type，避免套餐标签
# 变化后同一来源/时间/窗口产生重复采样。
SCHEMA_AGENT_QUOTA_SNAPSHOTS = """
CREATE TABLE IF NOT EXISTS agent_quota_snapshots (
    device_id   TEXT    NOT NULL,
    source      TEXT    NOT NULL,
    ts          INTEGER NOT NULL,
    plan_type   TEXT    NOT NULL DEFAULT '',
    window_key  TEXT    NOT NULL,
    used_pct    REAL    NOT NULL DEFAULT 0,
    reset_at    INTEGER,
    PRIMARY KEY (device_id, source, ts, window_key)
)
"""

INDEX_STARTED = "CREATE INDEX IF NOT EXISTS idx_started ON usage_records(started_at)"
INDEX_DEVICE_STARTED = "CREATE INDEX IF NOT EXISTS idx_device_started ON usage_records(device_id, started_at)"
INDEX_SNAPSHOT_TS = "CREATE INDEX IF NOT EXISTS idx_snapshot_ts ON quota_snapshots(ts)"
INDEX_AGENT_QUOTA_SNAPSHOT_TS = (
    "CREATE INDEX IF NOT EXISTS idx_agent_quota_snapshot_ts "
    "ON agent_quota_snapshots(ts)"
)
INDEX_AGENT_QUOTA_SNAPSHOT_SOURCE_TS = (
    "CREATE INDEX IF NOT EXISTS idx_agent_quota_snapshot_source_ts "
    "ON agent_quota_snapshots(source, ts)"
)

AGENT_QUOTA_WINDOWS = {
    "codex": {"hour5", "weekly"},
    "claude": {"hour5", "weekly"},
    "cursor": {"cursor_auto", "cursor_api"},
}

SCHEMA_CONFIG = """
CREATE TABLE IF NOT EXISTS config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
)
"""

ALL_SCHEMA = [
    SCHEMA_DEVICES,
    SCHEMA_USAGE_RECORDS,
    SCHEMA_QUOTA_SNAPSHOTS,
    SCHEMA_AGENT_QUOTA_SNAPSHOTS,
    INDEX_STARTED,
    INDEX_DEVICE_STARTED,
    INDEX_SNAPSHOT_TS,
    INDEX_AGENT_QUOTA_SNAPSHOT_TS,
    INDEX_AGENT_QUOTA_SNAPSHOT_SOURCE_TS,
    SCHEMA_CONFIG,
]


def _migrate_usage_records(conn):
    """usage_records 增加 source 维度的幂等自动迁移。

    检测现有表无 source 列时：建新表 → 旧数据全部按 source='zcode' 搬入 →
    删旧表 → 改名。整个过程单事务完成，老数据无损；已迁移/全新库直接跳过
    （可重复调用，幂等）。
    """
    cols = [r["name"] for r in conn.execute("PRAGMA table_info(usage_records)").fetchall()]
    if not cols or "source" in cols:
        return

    conn.isolation_level = None  # 手动管理事务，保证搬迁原子性
    try:
        conn.execute("BEGIN IMMEDIATE")
        conn.execute(_usage_records_schema("usage_records_new"))
        conn.execute(
            """
            INSERT INTO usage_records_new
                (device_id, source, local_rowid, started_at, model_id, provider_id,
                 input_tokens, output_tokens, cache_read_input_tokens,
                 cache_creation_input_tokens, reasoning_tokens,
                 computed_total_tokens, uploaded_at)
            SELECT device_id, 'zcode', local_rowid, started_at, model_id, provider_id,
                   input_tokens, output_tokens, cache_read_input_tokens,
                   cache_creation_input_tokens, reasoning_tokens,
                   computed_total_tokens, uploaded_at
            FROM usage_records
            """
        )
        conn.execute("DROP TABLE usage_records")
        conn.execute("ALTER TABLE usage_records_new RENAME TO usage_records")
        conn.execute("COMMIT")
        print("[zbar-sync] usage_records 已自动迁移：新增 source 维度，老数据无损保留")
    except Exception:
        conn.execute("ROLLBACK")
        raise
    finally:
        conn.isolation_level = ""


def _migrate_quota_snapshots(conn):
    """quota_snapshots 增加 account 维度的幂等自动迁移。

    检测现有表无 account 列时：建新表（主键扩为 device_id+ts+account）→
    旧数据全部按 account='' 搬入 → 删旧表 → 改名。单事务完成，老数据无损；
    已迁移/全新库直接跳过（可重复调用，幂等）。主键扩展后同设备多账号
    同毫秒的采样互不撞键；旧客户端（不上传 account）统一存 ''，
    与旧主键 (device_id, ts) 的去重行为一致。
    """
    cols = [r["name"] for r in conn.execute("PRAGMA table_info(quota_snapshots)").fetchall()]
    if not cols or "account" in cols:
        return

    conn.isolation_level = None  # 手动管理事务，保证搬迁原子性
    try:
        conn.execute("BEGIN IMMEDIATE")
        conn.execute(SCHEMA_QUOTA_SNAPSHOTS.replace(
            "IF NOT EXISTS quota_snapshots", "IF NOT EXISTS quota_snapshots_new"
        ))
        conn.execute(
            """
            INSERT INTO quota_snapshots_new
                (device_id, ts, account, level, weekly_pct, weekly_reset,
                 hour5_pct, mcp_pct, mcp_used, mcp_total)
            SELECT device_id, ts, '', level, weekly_pct, weekly_reset,
                   hour5_pct, mcp_pct, mcp_used, mcp_total
            FROM quota_snapshots
            """
        )
        conn.execute("DROP TABLE quota_snapshots")
        conn.execute("ALTER TABLE quota_snapshots_new RENAME TO quota_snapshots")
        conn.execute("COMMIT")
        print("[zbar-sync] quota_snapshots 已自动迁移：新增 account 维度，老数据无损保留")
    except Exception:
        conn.execute("ROLLBACK")
        raise
    finally:
        conn.isolation_level = ""


def _migrate_usage_records_columns(conn):
    """usage_records 幂等补列：session_id / project_key / project_display（proto 5）。

    proto 5 客户端上传的明细会带会话与项目维度字段；老库直接用
    ALTER TABLE ADD COLUMN 原地补列（全部可空、不改主键、不搬数据），
    已有行三列为 NULL——与 proto 2/3/4 客户端上传时的落库行为完全一致。
    ALTER 不会删除表上的索引，无需像 source/account 迁移那样重建索引。
    可重复调用，幂等。
    """
    cols = [r["name"] for r in conn.execute("PRAGMA table_info(usage_records)").fetchall()]
    if not cols:
        return
    added = False
    for name in ("session_id", "project_key", "project_display"):
        if name not in cols:
            conn.execute(f"ALTER TABLE usage_records ADD COLUMN {name} TEXT")
            added = True
    if added:
        conn.commit()
        print(
            "[zbar-sync] usage_records 已补列：session_id / project_key / "
            "project_display（proto 5 会话与项目维度，老记录为空）"
        )


def init_db():
    """初始化数据库：创建数据目录 + 自动建库建表（幂等，可重复调用）。

    老库（无 source 列）在此处自动迁移；迁移中 DROP TABLE 会连带删掉
    usage_records 上的两个索引，故迁移后重放一遍索引语句挂回新表。
    quota_snapshots 的 account 迁移同理（DROP 会删掉 ts 索引）。
    proto 5 的补列迁移用 ALTER TABLE 原地完成，不影响索引。
    """
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    conn = get_conn()
    try:
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("PRAGMA busy_timeout=5000")
        for sql in ALL_SCHEMA:
            conn.execute(sql)
        conn.commit()
        _migrate_usage_records(conn)
        _migrate_quota_snapshots(conn)
        _migrate_usage_records_columns(conn)
        if not conn.in_transaction:
            for sql in (INDEX_STARTED, INDEX_DEVICE_STARTED, INDEX_SNAPSHOT_TS):
                conn.execute(sql)
            conn.commit()
    finally:
        conn.close()


def get_conn():
    """获取一个新连接。每次操作开新连接（sqlite 轻量，单用户场景足够）。"""
    conn = sqlite3.connect(str(DB_PATH), timeout=5)
    conn.row_factory = sqlite3.Row
    return conn


def now_ms():
    """当前 UTC 毫秒时间戳。"""
    return int(time.time() * 1000)


# ===== 设备表操作 =====

def insert_device(device_id, device_name, token_hash, created_at):
    """注册新设备，写入 devices 表。"""
    with _db_lock:
        conn = get_conn()
        try:
            conn.execute(
                "INSERT INTO devices (device_id, device_name, token_hash, created_at) VALUES (?, ?, ?, ?)",
                (device_id, device_name, token_hash, created_at),
            )
            conn.commit()
        finally:
            conn.close()


def find_device_by_token_hash(token_hash):
    """按 device_token 哈希查找设备 id（鉴权用）。返回 device_id 或 None。"""
    conn = get_conn()
    try:
        row = conn.execute(
            "SELECT device_id FROM devices WHERE token_hash = ?",
            (token_hash,),
        ).fetchone()
        return row["device_id"] if row else None
    finally:
        conn.close()


def list_devices():
    """列出所有设备（附各设备记录数）。"""
    conn = get_conn()
    try:
        rows = conn.execute(
            """
            SELECT d.device_id, d.device_name, d.created_at,
                   (SELECT COUNT(*) FROM usage_records u WHERE u.device_id = d.device_id) AS cnt
            FROM devices d
            ORDER BY d.created_at ASC
            """
        ).fetchall()
        return [
            {
                "device_id": r["device_id"],
                "device_name": r["device_name"],
                "created_at": r["created_at"],
                "record_count": r["cnt"],
            }
            for r in rows
        ]
    finally:
        conn.close()


# ===== 同步写入 =====

def insert_usage_records(device_id, records, uploaded_at):
    """批量插入明细记录（主键 (device_id, source, local_rowid) 去重 + 修订覆盖）。

    每条记录的 source 缺省为 'zcode'（旧客户端不传即 zcode，向后兼容）。
    proto 5 起每条记录还可带 session_id / project_key / project_display
    （均可缺省，旧客户端不传时落库为 NULL）；主键去重不受影响。
    同主键重传且 computed_total_tokens 更大时覆盖旧值：Claude Code 会话流式
    落盘，客户端可能先上传某条消息的中间值、稍后本地修正为终值并补传
    （见客户端 claude 模块的 updated_at 修订机制）。另外允许同 Token 数的
    空 model_id 被后续非空模型名修正；已识别出的模型不会被空值覆盖。
    返回实际写入（插入或覆盖）条数。
    """
    if not records:
        return 0
    with _db_lock:
        conn = get_conn()
        try:
            cur = conn.cursor()
            accepted = 0
            for r in records:
                cur.execute(
                    """
                    INSERT INTO usage_records
                        (device_id, source, local_rowid, started_at, model_id, provider_id,
                         input_tokens, output_tokens, cache_read_input_tokens,
                         cache_creation_input_tokens, reasoning_tokens,
                         computed_total_tokens, uploaded_at,
                         session_id, project_key, project_display)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    ON CONFLICT(device_id, source, local_rowid) DO UPDATE SET
                        started_at = CASE
                            WHEN excluded.computed_total_tokens > usage_records.computed_total_tokens
                                THEN excluded.started_at
                            ELSE usage_records.started_at
                        END,
                        -- 空模型不能覆盖已经识别出的模型；反过来允许同 Token
                        -- 数的空模型被后续回填的非空模型修正。
                        model_id = CASE
                            WHEN COALESCE(excluded.model_id, '') != ''
                                THEN excluded.model_id
                            ELSE usage_records.model_id
                        END,
                        provider_id = CASE
                            WHEN excluded.computed_total_tokens > usage_records.computed_total_tokens
                                THEN excluded.provider_id
                            ELSE usage_records.provider_id
                        END,
                        input_tokens = CASE
                            WHEN excluded.computed_total_tokens > usage_records.computed_total_tokens
                                THEN excluded.input_tokens
                            ELSE usage_records.input_tokens
                        END,
                        output_tokens = CASE
                            WHEN excluded.computed_total_tokens > usage_records.computed_total_tokens
                                THEN excluded.output_tokens
                            ELSE usage_records.output_tokens
                        END,
                        cache_read_input_tokens = CASE
                            WHEN excluded.computed_total_tokens > usage_records.computed_total_tokens
                                THEN excluded.cache_read_input_tokens
                            ELSE usage_records.cache_read_input_tokens
                        END,
                        cache_creation_input_tokens = CASE
                            WHEN excluded.computed_total_tokens > usage_records.computed_total_tokens
                                THEN excluded.cache_creation_input_tokens
                            ELSE usage_records.cache_creation_input_tokens
                        END,
                        reasoning_tokens = CASE
                            WHEN excluded.computed_total_tokens > usage_records.computed_total_tokens
                                THEN excluded.reasoning_tokens
                            ELSE usage_records.reasoning_tokens
                        END,
                        computed_total_tokens = CASE
                            WHEN excluded.computed_total_tokens > usage_records.computed_total_tokens
                                THEN excluded.computed_total_tokens
                            ELSE usage_records.computed_total_tokens
                        END,
                        -- proto 5 会话/项目维度：仅在携带非 NULL 值时回填，
                        -- 不用 NULL 抹掉此前已写入的归属信息。
                        session_id = COALESCE(excluded.session_id, usage_records.session_id),
                        project_key = COALESCE(excluded.project_key, usage_records.project_key),
                        project_display = COALESCE(excluded.project_display, usage_records.project_display)
                    WHERE excluded.computed_total_tokens > usage_records.computed_total_tokens
                       OR (
                           COALESCE(usage_records.model_id, '') = ''
                           AND COALESCE(excluded.model_id, '') != ''
                       )
                    """,
                    (
                        device_id,
                        r.get("source", "zcode") or "zcode",
                        r["local_rowid"],
                        r["started_at"],
                        r.get("model_id", ""),
                        r.get("provider_id", ""),
                        r.get("input_tokens", 0),
                        r.get("output_tokens", 0),
                        r.get("cache_read_input_tokens", 0),
                        r.get("cache_creation_input_tokens", 0),
                        r.get("reasoning_tokens", 0),
                        r.get("computed_total_tokens", 0),
                        uploaded_at,
                        r.get("session_id"),
                        r.get("project_key"),
                        r.get("project_display"),
                    ),
                )
                accepted += cur.rowcount
            conn.commit()
            return accepted
        finally:
            conn.close()


def max_rowid_of(device_id, source="zcode"):
    """查询某设备某来源（source）已上传的最大 local_rowid。无数据返回 0。"""
    conn = get_conn()
    try:
        row = conn.execute(
            "SELECT COALESCE(MAX(local_rowid), 0) FROM usage_records "
            "WHERE device_id = ? AND source = ?",
            (device_id, source),
        ).fetchone()
        return row[0] if row else 0
    finally:
        conn.close()


def list_all_models():
    """全部设备、全部来源出现过的模型清单（distinct，价格配置用）。

    客户端价格设置页据此展示"其他设备同步上来、本机没有"的模型，
    并让价格检查能覆盖这些模型（如另一台设备在用的新模型）。
    返回 [{source, provider_id, model_id}, ...]，按 model_id 排序。
    """
    conn = get_conn()
    try:
        rows = conn.execute(
            "SELECT DISTINCT source, provider_id, model_id FROM usage_records "
            "ORDER BY model_id, source"
        ).fetchall()
        return [
            {
                "source": r[0] or "zcode",
                "provider_id": r[1] or "",
                "model_id": r[2] or "",
            }
            for r in rows
            if r[2]
        ]
    finally:
        conn.close()


# ===== 聚合查询（/usage 用）=====

def _build_device_filter(device_ids):
    """构造 device_id 过滤子句和参数。
    返回 (sql_fragment, params)。device_ids 为空时不过滤（查全部）。
    """
    if not device_ids:
        return "", []
    placeholders = ",".join("?" * len(device_ids))
    return f"AND device_id IN ({placeholders})", list(device_ids)


def _build_source_filter(source):
    """构造 source 过滤子句和参数。
    source 为空（None/""）时不过滤（全部来源合并），非空按来源精确匹配。
    """
    if not source:
        return "", []
    return "AND source = ?", [source]


def _query_overall_and_models(conn, from_ms, to_ms, device_ids, source=None):
    """查询整体汇总 + 模型分组。source 非空时只统计该来源。
    by_model 每个分组带 source 字段，供前端区分 ZCode / Codex。
    """
    dev_frag, dev_params = _build_device_filter(device_ids)
    src_frag, src_params = _build_source_filter(source)

    # 整体汇总
    overall_row = conn.execute(
        f"""
        SELECT COUNT(*),
               COALESCE(SUM(input_tokens),0),
               COALESCE(SUM(output_tokens),0),
               COALESCE(SUM(cache_read_input_tokens),0),
               COALESCE(SUM(cache_creation_input_tokens),0),
               COALESCE(SUM(reasoning_tokens),0),
               COALESCE(SUM(computed_total_tokens),0)
        FROM usage_records
        WHERE started_at >= ? AND started_at < ? {dev_frag} {src_frag}
        """,
        [from_ms, to_ms] + dev_params + src_params,
    ).fetchone()

    overall = {
        "requests": overall_row[0],
        "input_tokens": overall_row[1],
        "output_tokens": overall_row[2],
        "cache_read_tokens": overall_row[3],
        "cache_write_tokens": overall_row[4],
        "reasoning_tokens": overall_row[5],
        "total_tokens": overall_row[6],
    }

    # 模型分组（按 source + provider + model 分组，不同来源同名模型分开返回）
    model_rows = conn.execute(
        f"""
        SELECT source, model_id, provider_id, COUNT(*),
               COALESCE(SUM(input_tokens),0),
               COALESCE(SUM(output_tokens),0),
               COALESCE(SUM(cache_read_input_tokens),0),
               COALESCE(SUM(cache_creation_input_tokens),0),
               COALESCE(SUM(reasoning_tokens),0),
               COALESCE(SUM(computed_total_tokens),0) AS total_tokens
        FROM usage_records
        WHERE started_at >= ? AND started_at < ? {dev_frag} {src_frag}
        GROUP BY source, provider_id, model_id
        ORDER BY total_tokens DESC
        """,
        [from_ms, to_ms] + dev_params + src_params,
    ).fetchall()

    by_model = [
        {
            "source": r[0] or "zcode",
            "model_id": r[1] or "",
            "provider_id": r[2] or "",
            "requests": r[3],
            "input_tokens": r[4],
            "output_tokens": r[5],
            "cache_read_tokens": r[6],
            "cache_write_tokens": r[7],
            "reasoning_tokens": r[8],
            "total_tokens": r[9],
        }
        for r in model_rows
    ]

    return overall, by_model


def _align_bucket_start_utc(ms, bucket):
    """把毫秒时间戳对齐到桶起点（UTC 整除，与 Rust 版一致）。
    label 返回桶起始 ms 字符串，前端按本地时区格式化。
    """
    width = 3_600_000 if bucket == "hour" else 86_400_000
    return (ms // width) * width


def _query_trend(conn, from_ms, to_ms, bucket, device_ids, source=None):
    """查询分桶趋势（逐桶循环，与 Rust 版 query_trend 同思路）。
    label 返回桶起始 ms 字符串，前端按本地时区格式化 + 按 ms 合并。
    source 非空时只统计该来源。
    """
    width = 3_600_000 if bucket == "hour" else 86_400_000
    start = _align_bucket_start_utc(from_ms, bucket)
    dev_frag, dev_params = _build_device_filter(device_ids)
    src_frag, src_params = _build_source_filter(source)

    out = []
    while start < to_ms:
        end = start + width
        params = [start, end] + dev_params + src_params
        model_rows = conn.execute(
            f"""
            SELECT source, model_id, provider_id, COUNT(*),
                   COALESCE(SUM(input_tokens),0),
                   COALESCE(SUM(output_tokens),0),
                   COALESCE(SUM(cache_read_input_tokens),0),
                   COALESCE(SUM(computed_total_tokens),0)
            FROM usage_records
            WHERE started_at >= ? AND started_at < ? {dev_frag} {src_frag}
            GROUP BY source, provider_id, model_id
            """,
            params,
        ).fetchall()

        by_model = [
            {
                "source": r[0] or "zcode",
                "model_id": r[1] or "",
                "provider_id": r[2] or "",
                "requests": r[3],
                "input_tokens": r[4],
                "output_tokens": r[5],
                "cache_read_tokens": r[6],
                "total_tokens": r[7],
            }
            for r in model_rows
        ]

        total_tokens = sum(m["total_tokens"] for m in by_model)
        requests = sum(m["requests"] for m in by_model)
        out.append(
            {
                "label": str(start),  # 桶起始 ms，前端格式化
                "by_model": by_model,
                "total_tokens": total_tokens,
                "requests": requests,
            }
        )
        start = end
    return out


def query_usage(from_ms, to_ms, bucket, device_ids, source=None):
    """/usage 完整查询：返回 overall + by_model + trend。
    device_ids 为空 = 查全部；非空 = 仅这些设备。
    source 为空 = 全部来源；非空 = 仅该来源。
    """
    conn = get_conn()
    try:
        overall, by_model = _query_overall_and_models(conn, from_ms, to_ms, device_ids, source)
        trend = _query_trend(conn, from_ms, to_ms, bucket, device_ids, source)
        return {
            "from_ms": from_ms,
            "to_ms": to_ms,
            "overall": overall,
            "by_model": by_model,
            "trend": trend,
        }
    finally:
        conn.close()


def empty_usage_result(from_ms, to_ms):
    """构造全空的 usage 结果（显式过滤后无设备的边界情况）。"""
    return {
        "from_ms": from_ms,
        "to_ms": to_ms,
        "overall": {
            "requests": 0,
            "input_tokens": 0,
            "output_tokens": 0,
            "cache_read_tokens": 0,
            "cache_write_tokens": 0,
            "reasoning_tokens": 0,
            "total_tokens": 0,
        },
        "by_model": [],
        "trend": [],
    }


# ===== 额度快照（quota_snapshots）=====

def insert_snapshots(device_id, snaps, uploaded_at):
    """批量插入额度快照（INSERT OR IGNORE 去重，按 device_id+ts+account 主键）。
    返回实际写入条数。单条字段缺失时用默认值兜底；account 缺失（旧客户端）
    统一存空串。
    """
    if not snaps:
        return 0
    with _db_lock:
        conn = get_conn()
        try:
            cur = conn.cursor()
            accepted = 0
            for s in snaps:
                cur.execute(
                    """
                    INSERT OR IGNORE INTO quota_snapshots
                        (device_id, ts, account, level, weekly_pct, weekly_reset,
                         hour5_pct, mcp_pct, mcp_used, mcp_total)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        device_id,
                        s.get("ts", 0),
                        s.get("account") or "",
                        s.get("level", ""),
                        s.get("weekly_pct", 0),
                        s.get("weekly_reset"),
                        s.get("hour5_pct", 0),
                        s.get("mcp_pct", 0),
                        s.get("mcp_used"),
                        s.get("mcp_total"),
                    ),
                )
                accepted += cur.rowcount
            conn.commit()
            return accepted
        finally:
            conn.close()


def max_snapshot_ts_of(device_id):
    """查询某设备已上传快照的最大 ts（游标用）。无数据返回 0。"""
    conn = get_conn()
    try:
        row = conn.execute(
            "SELECT COALESCE(MAX(ts), 0) FROM quota_snapshots WHERE device_id = ?",
            (device_id,),
        ).fetchone()
        return row[0] if row else 0
    finally:
        conn.close()


def query_snapshots(from_ms, to_ms, device_ids):
    """查询时间范围内的快照（带 device_id，按 ts 升序）。
    device_ids 为空 = 全部设备；非空 = 仅这些设备。
    account 为快照归属账号指纹；空串（旧客户端数据/旧客户端上传）返回 None，
    与客户端 RemoteSnapshot.account 的 Option 语义对齐。
    """
    dev_frag, dev_params = _build_device_filter(device_ids)
    conn = get_conn()
    try:
        rows = conn.execute(
            f"""
            SELECT device_id, ts, account, level, weekly_pct, weekly_reset,
                   hour5_pct, mcp_pct, mcp_used, mcp_total
            FROM quota_snapshots
            WHERE ts >= ? AND ts < ? {dev_frag}
            ORDER BY ts ASC
            """,
            [from_ms, to_ms] + dev_params,
        ).fetchall()
        return [
            {
                "device_id": r["device_id"],
                "ts": r["ts"],
                "account": r["account"] or None,
                "level": r["level"] or "",
                "weekly_pct": r["weekly_pct"],
                "weekly_reset": r["weekly_reset"],
                "hour5_pct": r["hour5_pct"],
                "mcp_pct": r["mcp_pct"],
                "mcp_used": r["mcp_used"],
                "mcp_total": r["mcp_total"],
            }
            for r in rows
        ]
    finally:
        conn.close()


def insert_agent_quota_snapshots(device_id, snapshots, uploaded_at):
    """批量插入 Agent 额度快照，并按窗口行幂等去重。

    客户端上传的是一个 snapshot 对象数组，每个对象包含多个 windows；
    服务端将其展平存储。uploaded_at 为兼容现有同步写入接口保留，当前表
    不需要记录上传时间。
    """
    if not snapshots:
        return 0

    rows = []
    for snapshot in snapshots:
        if not isinstance(snapshot, dict):
            continue
        source = str(snapshot.get("source") or "").strip()
        if source not in AGENT_QUOTA_WINDOWS:
            continue
        try:
            ts = int(snapshot.get("ts"))
        except (TypeError, ValueError):
            continue
        plan_type = str(snapshot.get("plan_type") or "").strip()[:64]
        windows = snapshot.get("windows") or []
        if not isinstance(windows, (list, tuple)):
            continue

        for window in windows[:16]:
            if not isinstance(window, dict):
                continue
            window_key = str(window.get("key") or "").strip()
            if window_key not in AGENT_QUOTA_WINDOWS[source]:
                continue
            try:
                used_pct = float(window.get("used_pct", 0))
            except (TypeError, ValueError):
                continue
            if not math.isfinite(used_pct) or not 0 <= used_pct <= 100:
                continue

            reset_at = window.get("reset_at")
            if reset_at is not None:
                try:
                    reset_at = int(reset_at)
                except (TypeError, ValueError):
                    reset_at = None
            rows.append(
                (
                    device_id,
                    source,
                    ts,
                    plan_type,
                    window_key,
                    used_pct,
                    reset_at,
                )
            )

    if not rows:
        return 0

    with _db_lock:
        conn = get_conn()
        try:
            cur = conn.cursor()
            accepted = 0
            for row in rows:
                # 使用 INSERT OR IGNORE + UPDATE，兼容旧版 SQLite（部分宝塔
                # Python 运行时自带的 SQLite 低于 3.24，不支持 UPSERT 的
                # ON CONFLICT ... DO UPDATE 语法）。
                cur.execute(
                    """
                    INSERT OR IGNORE INTO agent_quota_snapshots
                        (device_id, source, ts, plan_type, window_key,
                         used_pct, reset_at)
                    VALUES (?, ?, ?, ?, ?, ?, ?)
                    """,
                    row,
                )
                if cur.rowcount:
                    accepted += 1
                    continue

                updated = cur.execute(
                    """
                    UPDATE agent_quota_snapshots
                    SET plan_type = ?, used_pct = ?, reset_at = ?
                    WHERE device_id = ? AND source = ? AND ts = ?
                      AND window_key = ? AND used_pct < ?
                    """,
                    (
                        row[3],
                        row[5],
                        row[6],
                        row[0],
                        row[1],
                        row[2],
                        row[4],
                        row[5],
                    ),
                )
                accepted += updated.rowcount
            conn.commit()
            return accepted
        finally:
            conn.close()


def max_agent_quota_snapshot_ts_of(device_id):
    """查询某设备已上传 Agent 快照的最大时间戳；无数据返回 0。"""
    conn = get_conn()
    try:
        row = conn.execute(
            "SELECT COALESCE(MAX(ts), 0) FROM agent_quota_snapshots "
            "WHERE device_id = ?",
            (device_id,),
        ).fetchone()
        return row[0] if row else 0
    finally:
        conn.close()


def _group_agent_quota_rows(rows):
    """把展平的 agent_quota_snapshots 行组装回客户端 snapshot 形状。

    按 (device_id, source, ts, plan_type) 分组，windows 逐行追加；
    /agent-quota-snapshots 与 /overview 的 quota_latest 共用本组装逻辑，
    保证两处返回口径一致。
    """
    grouped = {}
    for row in rows:
        key = (row["device_id"], row["source"], row["ts"], row["plan_type"])
        snapshot = grouped.get(key)
        if snapshot is None:
            snapshot = {
                "device_id": row["device_id"],
                "source": row["source"],
                "ts": row["ts"],
                "plan_type": row["plan_type"] or None,
                "windows": [],
            }
            grouped[key] = snapshot
        snapshot["windows"].append(
            {
                "key": row["window_key"],
                "used_pct": row["used_pct"],
                "reset_at": row["reset_at"],
            }
        )
    return list(grouped.values())


def query_agent_quota_snapshots(from_ms, to_ms, device_ids, source=None):
    """查询 Agent 额度快照，并组装为客户端可直接解析的 snapshot 形状。

    返回的每个对象包含 device_id、source、ts、plan_type 和 windows；
    device_ids 为空表示查询全部设备，source 为空表示查询全部来源。
    """
    dev_frag, dev_params = _build_device_filter(device_ids)
    src_frag, src_params = _build_source_filter(source)
    conn = get_conn()
    try:
        rows = conn.execute(
            f"""
            SELECT device_id, source, ts, plan_type, window_key,
                   used_pct, reset_at
            FROM agent_quota_snapshots
            WHERE ts >= ? AND ts < ? {dev_frag} {src_frag}
            ORDER BY ts ASC, device_id ASC, source ASC,
                     plan_type ASC, window_key ASC
            """,
            [from_ms, to_ms] + dev_params + src_params,
        ).fetchall()
    finally:
        conn.close()

    return _group_agent_quota_rows(rows)


def query_period_detail(periods, device_ids, source=None):
    """按一组周期 [start, end) 返回远端各周期内的逐条用量明细。
    供客户端用本地 peak 配置折算消耗（服务端无 peak 配置）。
    每条含 started_at/model_id/各 token 字段，与本地 db::query_period_consumed 口径一致。
    source 为空 = 全部来源；非空 = 仅该来源。
    """
    dev_frag, dev_params = _build_device_filter(device_ids)
    src_frag, src_params = _build_source_filter(source)
    conn = get_conn()
    try:
        out = []
        for start, end in periods:
            rows = conn.execute(
                f"""
                SELECT started_at, model_id,
                       COALESCE(input_tokens,0), COALESCE(output_tokens,0),
                       COALESCE(cache_read_input_tokens,0), COALESCE(computed_total_tokens,0)
                FROM usage_records
                WHERE started_at >= ? AND started_at < ? {dev_frag} {src_frag}
                """,
                [start, end] + dev_params + src_params,
            ).fetchall()
            out.append(
                {
                    "reset_at": start,
                    "end_at": end,
                    "rows": [
                        {
                            "started_at": r[0],
                            "model_id": r[1] or "",
                            "input_tokens": r[2],
                            "output_tokens": r[3],
                            "cache_read_tokens": r[4],
                            "total_tokens": r[5],
                        }
                        for r in rows
                    ],
                }
            )
        return out
    finally:
        conn.close()


# ===== 项目维度聚合 + overview 查询（proto 5）=====

UNKNOWN_PROJECT_KEY = "__unknown__"


def query_projects(from_ms, to_ms, device_ids):
    """按 (project_key, source) 聚合用量，供 GET /projects 与 /overview 使用。

    project_key / session_id 是 proto 5 新增列：proto 2/3/4 客户端上传的
    记录三列均为 NULL，统一聚合为 UNKNOWN_PROJECT_KEY 一个分组、sessions
    计 0（COUNT(DISTINCT NULL) = 0），与 Rust 侧汇总口径一致。
    tokens 口径为 SUM(computed_total_tokens)：computed_total_tokens 是客户端
    上报的计算后总额，与 /usage（_query_overall_and_models）及客户端项目
    聚合三方一致；codex 等来源的上游自报总额不等于四类原始 token 之和，
    不能用 input + output + cache_read + cache_write 四类相加，否则手机页
    与桌面端数字对不上。
    返回按 total_tokens 降序的
    [{key, display, by_source: [{source, tokens, requests, sessions}], total_tokens, requests}]。
    """
    dev_frag, dev_params = _build_device_filter(device_ids)
    conn = get_conn()
    try:
        rows = conn.execute(
            f"""
            SELECT COALESCE(project_key, '{UNKNOWN_PROJECT_KEY}') AS key,
                   MAX(project_display) AS display,
                   source,
                   COUNT(*) AS requests,
                   -- tokens 口径与 /usage 的 SUM(computed_total_tokens) 对齐，
                   -- 不用四类原始 token 相加（codex 上游自报总额与之不等）
                   COALESCE(SUM(computed_total_tokens), 0) AS total_tokens,
                   COUNT(DISTINCT session_id) AS sessions
            FROM usage_records
            WHERE started_at >= ? AND started_at < ? {dev_frag}
            GROUP BY COALESCE(project_key, '{UNKNOWN_PROJECT_KEY}'), source
            ORDER BY key ASC
            """,
            [from_ms, to_ms] + dev_params,
        ).fetchall()
    finally:
        conn.close()

    projects = {}
    order = []
    for r in rows:
        key = r["key"]
        # 与 SQL 的 SUM(computed_total_tokens) 对应（见上方口径说明）
        tokens = r[4]
        p = projects.get(key)
        if p is None:
            p = {
                "key": key,
                "display": r["display"],
                "by_source": [],
                "total_tokens": 0,
                "requests": 0,
            }
            projects[key] = p
            order.append(p)
        p["by_source"].append(
            {
                "source": r["source"] or "zcode",
                "tokens": tokens,
                "requests": r["requests"],
                "sessions": r["sessions"],
            }
        )
        p["total_tokens"] += tokens
        p["requests"] += r["requests"]
    order.sort(key=lambda p: -p["total_tokens"])
    return order


def query_usage_summary(from_ms, to_ms, device_ids, source=None):
    """/overview 周期卡用的整体汇总 + 模型分组。

    内部直接复用 /usage 的 _query_overall_and_models，保证与 /usage 的
    overall / by_model 字段与口径完全一致。
    """
    conn = get_conn()
    try:
        overall, by_model = _query_overall_and_models(conn, from_ms, to_ms, device_ids, source)
        return overall, by_model
    finally:
        conn.close()


def max_uploaded_at():
    """全部明细的最新上传时间（/overview 的 last_synced_at）。无数据返回 None。"""
    conn = get_conn()
    try:
        row = conn.execute("SELECT MAX(uploaded_at) FROM usage_records").fetchone()
        return row[0] if row else None
    finally:
        conn.close()


def last_uploaded_at_by_device():
    """各设备最新一次上传时间（/overview 设备列表用）。返回 {device_id: ms}。"""
    conn = get_conn()
    try:
        rows = conn.execute(
            "SELECT device_id, MAX(uploaded_at) FROM usage_records GROUP BY device_id"
        ).fetchall()
        return {r[0]: r[1] for r in rows}
    finally:
        conn.close()


def query_latest_quota_snapshots():
    """各来源最新一条额度快照（/overview 的 quota_latest）。

    返回 {"zai": 最新一条 Z.ai 快照（/snapshots 单条形状）或 None,
          "agent": {source: [snapshot, ...]}}；agent 快照复用
    /agent-quota-snapshots 的最新一条逻辑与组装形状（取各 source 的
    最大 ts，再取该 ts 的全部行按设备/账号分组）。
    """
    conn = get_conn()
    try:
        zai_row = conn.execute(
            """
            SELECT device_id, ts, account, level, weekly_pct, weekly_reset,
                   hour5_pct, mcp_pct, mcp_used, mcp_total
            FROM quota_snapshots
            ORDER BY ts DESC
            LIMIT 1
            """
        ).fetchone()

        agent_out = {}
        for source in AGENT_QUOTA_WINDOWS:
            max_ts = conn.execute(
                "SELECT MAX(ts) FROM agent_quota_snapshots WHERE source = ?",
                (source,),
            ).fetchone()[0]
            if not max_ts:
                continue
            rows = conn.execute(
                """
                SELECT device_id, source, ts, plan_type, window_key,
                       used_pct, reset_at
                FROM agent_quota_snapshots
                WHERE source = ? AND ts = ?
                ORDER BY device_id ASC, plan_type ASC, window_key ASC
                """,
                (source, max_ts),
            ).fetchall()
            agent_out[source] = _group_agent_quota_rows(rows)
    finally:
        conn.close()

    zai = None
    if zai_row is not None:
        zai = {
            "device_id": zai_row["device_id"],
            "ts": zai_row["ts"],
            "account": zai_row["account"] or None,
            "level": zai_row["level"] or "",
            "weekly_pct": zai_row["weekly_pct"],
            "weekly_reset": zai_row["weekly_reset"],
            "hour5_pct": zai_row["hour5_pct"],
            "mcp_pct": zai_row["mcp_pct"],
            "mcp_used": zai_row["mcp_used"],
            "mcp_total": zai_row["mcp_total"],
        }
    return {"zai": zai, "agent": agent_out}


# ===== 配置表（自动清理用）=====

def get_config(key):
    conn = get_conn()
    try:
        row = conn.execute("SELECT value FROM config WHERE key = ?", (key,)).fetchone()
        return row["value"] if row else None
    finally:
        conn.close()


def set_config(key, value):
    with _db_lock:
        conn = get_conn()
        try:
            conn.execute(
                "INSERT INTO config (key, value) VALUES (?, ?) "
                "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, value),
            )
            conn.commit()
        finally:
            conn.close()


# ===== 统计 + 清理 =====

def total_records():
    """服务端总记录数。"""
    conn = get_conn()
    try:
        return conn.execute("SELECT COUNT(*) FROM usage_records").fetchone()[0]
    finally:
        conn.close()


def delete_before(cutoff_ms):
    """按时间清理明细、Z.ai 快照和 Agent 快照，返回删除的明细条数。"""
    with _db_lock:
        conn = get_conn()
        try:
            n = conn.execute(
                "DELETE FROM usage_records WHERE started_at < ?", (cutoff_ms,)
            ).rowcount
            conn.execute(
                "DELETE FROM quota_snapshots WHERE ts < ?", (cutoff_ms,)
            )
            conn.execute(
                "DELETE FROM agent_quota_snapshots WHERE ts < ?", (cutoff_ms,)
            )
            conn.commit()
            return n
        finally:
            conn.close()


def delete_device_records(device_id):
    """按设备清理：删除指定设备的明细、Z.ai 快照和 Agent 快照。"""
    with _db_lock:
        conn = get_conn()
        try:
            cur = conn.execute(
                "DELETE FROM usage_records WHERE device_id = ?", (device_id,)
            )
            conn.execute(
                "DELETE FROM quota_snapshots WHERE device_id = ?", (device_id,)
            )
            conn.execute(
                "DELETE FROM agent_quota_snapshots WHERE device_id = ?", (device_id,)
            )
            conn.commit()
            return cur.rowcount
        finally:
            conn.close()


def delete_all_usage():
    """全部清空三类用量数据，保留设备注册；返回删除的明细条数。"""
    with _db_lock:
        conn = get_conn()
        try:
            cur = conn.execute("DELETE FROM usage_records")
            conn.execute("DELETE FROM quota_snapshots")
            conn.execute("DELETE FROM agent_quota_snapshots")
            conn.commit()
            return cur.rowcount
        finally:
            conn.close()


def reset_all():
    """reset：清空三类用量数据并删除设备，返回 (usage_deleted, devices_deleted)。"""
    with _db_lock:
        conn = get_conn()
        try:
            u = conn.execute("DELETE FROM usage_records").rowcount
            conn.execute("DELETE FROM quota_snapshots")
            conn.execute("DELETE FROM agent_quota_snapshots")
            d = conn.execute("DELETE FROM devices").rowcount
            conn.commit()
            return u, d
        finally:
            conn.close()


def revoke_device(device_id):
    """撤销设备：同时删除设备、明细、Z.ai 快照和 Agent 快照。"""
    with _db_lock:
        conn = get_conn()
        try:
            u = conn.execute(
                "DELETE FROM usage_records WHERE device_id = ?", (device_id,)
            ).rowcount
            conn.execute(
                "DELETE FROM quota_snapshots WHERE device_id = ?", (device_id,)
            )
            conn.execute(
                "DELETE FROM agent_quota_snapshots WHERE device_id = ?", (device_id,)
            )
            d = conn.execute(
                "DELETE FROM devices WHERE device_id = ?", (device_id,)
            ).rowcount
            conn.commit()
            return d, u
        finally:
            conn.close()


def merge_devices(source_id, target_id):
    """把 source 设备的全部数据合并到 target，然后删除 source。

    用量明细的主键是 (device_id, source, local_rowid)，而 local_rowid 来自
    各机本地的库 rowid（zcode 与 codex 两个来源各自从 1 开始递增）。直接改
    device_id 会撞主键；但若简单地接在 target 现有最大值之后（base+1..base+N）
    也不行——target 客户端的增量上传游标仍是 base，它接下来会用 rowid =
    base+1, base+2, ... 上传自己的真实记录，而服务端用 INSERT OR IGNORE
    去重，这些真实记录会被合并进来的历史记录占用而**静默丢弃、且因游标推进
    而永久丢失**。

    因此这里把合并记录放到一个 target 客户端在可预见未来都不可能触达的远端
    区段（当前最大值 + MERGE_ROWID_OFFSET）。重编号遍历 (source, local_rowid)
    组合按序分配编号——所有来源混在同一个编号序列里即可保证唯一，无需按来源
    分段。sqlite rowid 上限是 2^63，偏移量取 20 亿（即便每秒一条用量也要
    ~63 年才会长到），个人监控工具绝无可能撞上。额度快照主键是
    (device_id, ts)，先丢弃来源中与 target 同 ts 的条目再迁移。Agent 额度
    快照按完整主键 INSERT OR IGNORE 合并，冲突时保留 target 已有值。整个
    操作在一个事务内完成。

    返回 (records_moved, snapshots_moved)。source/target 不存在时抛 ValueError。
    """
    # 远超任何真实客户端 rowid 上限，避免与 target 后续真实上传撞主键
    MERGE_ROWID_OFFSET = 2_000_000_000

    if source_id == target_id:
        raise ValueError("来源设备与目标设备不能相同")

    with _db_lock:
        conn = get_conn()
        try:
            exists = conn.execute(
                "SELECT COUNT(*) AS c FROM devices WHERE device_id IN (?, ?)",
                (source_id, target_id),
            ).fetchone()["c"]
            if exists < 2:
                raise ValueError("来源或目标设备不存在")

            # 1) 用量明细：重编号到 target 客户端不可达的远端区段，再迁移。
            #    遍历 (source, local_rowid) 组合，混合编号保证跨来源唯一
            max_row = conn.execute(
                "SELECT COALESCE(MAX(local_rowid), 0) AS m "
                "FROM usage_records WHERE device_id = ?",
                (target_id,),
            ).fetchone()["m"]
            start = max_row + MERGE_ROWID_OFFSET
            src_rows = conn.execute(
                "SELECT source, local_rowid FROM usage_records WHERE device_id = ? "
                "ORDER BY local_rowid, source",
                (source_id,),
            ).fetchall()
            for i, r in enumerate(src_rows, start=1):
                conn.execute(
                    "UPDATE usage_records SET device_id = ?, local_rowid = ? "
                    "WHERE device_id = ? AND source = ? AND local_rowid = ?",
                    (target_id, start + i, source_id, r["source"], r["local_rowid"]),
                )
            records_moved = len(src_rows)

            # 2) 额度快照：丢弃与 target 同 ts 的来源条目，再迁移
            conn.execute(
                "DELETE FROM quota_snapshots WHERE device_id = ? "
                "AND ts IN (SELECT ts FROM quota_snapshots WHERE device_id = ?)",
                (source_id, target_id),
            )
            snap = conn.execute(
                "UPDATE quota_snapshots SET device_id = ? WHERE device_id = ?",
                (target_id, source_id),
            )
            snapshots_moved = snap.rowcount

            # 3) Agent 额度快照：按完整窗口主键迁移，冲突时保留较高用量。
            # 不使用 INSERT ... SELECT ... ON CONFLICT，兼容旧版 SQLite。
            agent_rows = conn.execute(
                """
                SELECT source, ts, plan_type, window_key, used_pct, reset_at
                FROM agent_quota_snapshots
                WHERE device_id = ?
                """,
                (source_id,),
            ).fetchall()
            for row in agent_rows:
                values = (
                    target_id,
                    row["source"],
                    row["ts"],
                    row["plan_type"],
                    row["window_key"],
                    row["used_pct"],
                    row["reset_at"],
                )
                inserted = conn.execute(
                    """
                    INSERT OR IGNORE INTO agent_quota_snapshots
                        (device_id, source, ts, plan_type, window_key,
                         used_pct, reset_at)
                    VALUES (?, ?, ?, ?, ?, ?, ?)
                    """,
                    values,
                ).rowcount
                if not inserted:
                    conn.execute(
                        """
                        UPDATE agent_quota_snapshots
                        SET plan_type = ?, used_pct = ?, reset_at = ?
                        WHERE device_id = ? AND source = ? AND ts = ?
                          AND window_key = ? AND used_pct < ?
                        """,
                        (
                            values[3],
                            values[5],
                            values[6],
                            values[0],
                            values[1],
                            values[2],
                            values[4],
                            values[5],
                        ),
                    )
            conn.execute(
                "DELETE FROM agent_quota_snapshots WHERE device_id = ?",
                (source_id,),
            )

            # 4) 删除来源设备记录
            conn.execute(
                "DELETE FROM devices WHERE device_id = ?", (source_id,)
            )

            conn.commit()
            return records_moved, snapshots_moved
        finally:
            conn.close()


def rename_device(device_id, new_name):
    """修改设备显示名。返回更新行数（0 表示设备不存在）。"""
    with _db_lock:
        conn = get_conn()
        try:
            n = conn.execute(
                "UPDATE devices SET device_name = ? WHERE device_id = ?",
                (new_name, device_id),
            ).rowcount
            conn.commit()
            return n
        finally:
            conn.close()
