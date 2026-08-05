"""SQLite 数据库操作：自动建库建表 + 增删改查。

关键点（与 Rust 版一致）：
- 服务端数据库是自建自用的，首次启动 usage.db 不存在时，
  sqlite3.connect() 会自动创建文件 + 执行建表 SQL，对用户透明。
- 表结构、字段名与 Rust 版完全一致，保证客户端无需改动。
- model_usage 是 append-only，用 (device_id, local_rowid) 作主键去重。
"""

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

SCHEMA_USAGE_RECORDS = """
CREATE TABLE IF NOT EXISTS usage_records (
    device_id                   TEXT    NOT NULL,
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
    PRIMARY KEY (device_id, local_rowid)
)
"""

# 额度快照表：客户端每次查询额度后追加一条，周期解析靠 weekly_reset 跳变。
# 与客户端本地 quota_history.jsonl 字段对齐，多一个 device_id 用于按设备筛选。
SCHEMA_QUOTA_SNAPSHOTS = """
CREATE TABLE IF NOT EXISTS quota_snapshots (
    device_id    TEXT    NOT NULL,
    ts           INTEGER NOT NULL,
    level        TEXT,
    weekly_pct   INTEGER NOT NULL DEFAULT 0,
    weekly_reset INTEGER,
    hour5_pct    INTEGER NOT NULL DEFAULT 0,
    mcp_pct      INTEGER NOT NULL DEFAULT 0,
    mcp_used     INTEGER,
    mcp_total    INTEGER,
    PRIMARY KEY (device_id, ts)
)
"""

INDEX_STARTED = "CREATE INDEX IF NOT EXISTS idx_started ON usage_records(started_at)"
INDEX_DEVICE_STARTED = "CREATE INDEX IF NOT EXISTS idx_device_started ON usage_records(device_id, started_at)"
INDEX_SNAPSHOT_TS = "CREATE INDEX IF NOT EXISTS idx_snapshot_ts ON quota_snapshots(ts)"

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
    INDEX_STARTED,
    INDEX_DEVICE_STARTED,
    INDEX_SNAPSHOT_TS,
    SCHEMA_CONFIG,
]


def init_db():
    """初始化数据库：创建数据目录 + 自动建库建表（幂等，可重复调用）。"""
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    conn = get_conn()
    try:
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("PRAGMA busy_timeout=5000")
        for sql in ALL_SCHEMA:
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
    """批量插入明细记录（INSERT OR IGNORE 去重）。返回实际写入条数。"""
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
                    INSERT OR IGNORE INTO usage_records
                        (device_id, local_rowid, started_at, model_id, provider_id,
                         input_tokens, output_tokens, cache_read_input_tokens,
                         cache_creation_input_tokens, reasoning_tokens,
                         computed_total_tokens, uploaded_at)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        device_id,
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
                    ),
                )
                accepted += cur.rowcount
            conn.commit()
            return accepted
        finally:
            conn.close()


def max_rowid_of(device_id):
    """查询某设备已上传的最大 local_rowid。"""
    conn = get_conn()
    try:
        row = conn.execute(
            "SELECT COALESCE(MAX(local_rowid), 0) FROM usage_records WHERE device_id = ?",
            (device_id,),
        ).fetchone()
        return row[0] if row else 0
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


def _query_overall_and_models(conn, from_ms, to_ms, device_ids):
    """查询整体汇总 + 模型分组。"""
    dev_frag, dev_params = _build_device_filter(device_ids)

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
        WHERE started_at >= ? AND started_at < ? {dev_frag}
        """,
        [from_ms, to_ms] + dev_params,
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

    # 模型分组
    model_rows = conn.execute(
        f"""
        SELECT model_id, provider_id, COUNT(*),
               COALESCE(SUM(input_tokens),0),
               COALESCE(SUM(output_tokens),0),
               COALESCE(SUM(cache_read_input_tokens),0),
               COALESCE(SUM(cache_creation_input_tokens),0),
               COALESCE(SUM(reasoning_tokens),0),
               COALESCE(SUM(computed_total_tokens),0) AS total_tokens
        FROM usage_records
        WHERE started_at >= ? AND started_at < ? {dev_frag}
        GROUP BY provider_id, model_id
        ORDER BY total_tokens DESC
        """,
        [from_ms, to_ms] + dev_params,
    ).fetchall()

    by_model = [
        {
            "model_id": r[0] or "",
            "provider_id": r[1] or "",
            "requests": r[2],
            "input_tokens": r[3],
            "output_tokens": r[4],
            "cache_read_tokens": r[5],
            "cache_write_tokens": r[6],
            "reasoning_tokens": r[7],
            "total_tokens": r[8],
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


def _query_trend(conn, from_ms, to_ms, bucket, device_ids):
    """查询分桶趋势（逐桶循环，与 Rust 版 query_trend 同思路）。
    label 返回桶起始 ms 字符串，前端按本地时区格式化 + 按 ms 合并。
    """
    width = 3_600_000 if bucket == "hour" else 86_400_000
    start = _align_bucket_start_utc(from_ms, bucket)
    dev_frag, dev_params = _build_device_filter(device_ids)

    out = []
    while start < to_ms:
        end = start + width
        params = [start, end] + dev_params
        model_rows = conn.execute(
            f"""
            SELECT model_id, provider_id, COUNT(*),
                   COALESCE(SUM(input_tokens),0),
                   COALESCE(SUM(output_tokens),0),
                   COALESCE(SUM(cache_read_input_tokens),0),
                   COALESCE(SUM(computed_total_tokens),0)
            FROM usage_records
            WHERE started_at >= ? AND started_at < ? {dev_frag}
            GROUP BY provider_id, model_id
            """,
            params,
        ).fetchall()

        by_model = [
            {
                "model_id": r[0] or "",
                "provider_id": r[1] or "",
                "requests": r[2],
                "input_tokens": r[3],
                "output_tokens": r[4],
                "cache_read_tokens": r[5],
                "total_tokens": r[6],
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


def query_usage(from_ms, to_ms, bucket, device_ids):
    """/usage 完整查询：返回 overall + by_model + trend。
    device_ids 为空 = 查全部；非空 = 仅这些设备。
    """
    conn = get_conn()
    try:
        overall, by_model = _query_overall_and_models(conn, from_ms, to_ms, device_ids)
        trend = _query_trend(conn, from_ms, to_ms, bucket, device_ids)
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
    """批量插入额度快照（INSERT OR IGNORE 去重，按 device_id+ts 主键）。
    返回实际写入条数。单条字段缺失时用默认值兜底。
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
                        (device_id, ts, level, weekly_pct, weekly_reset,
                         hour5_pct, mcp_pct, mcp_used, mcp_total)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        device_id,
                        s.get("ts", 0),
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
    """
    dev_frag, dev_params = _build_device_filter(device_ids)
    conn = get_conn()
    try:
        rows = conn.execute(
            f"""
            SELECT device_id, ts, level, weekly_pct, weekly_reset,
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


def query_period_detail(periods, device_ids):
    """按一组周期 [start, end) 返回远端各周期内的逐条用量明细。
    供客户端用本地 peak 配置折算消耗（服务端无 peak 配置）。
    每条含 started_at/model_id/各 token 字段，与本地 db::query_period_consumed 口径一致。
    """
    dev_frag, dev_params = _build_device_filter(device_ids)
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
                WHERE started_at >= ? AND started_at < ? {dev_frag}
                """,
                [start, end] + dev_params,
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
    """按时间清理：删除 started_at < cutoff_ms 的明细 + ts < cutoff 的快照，返回删除条数。"""
    with _db_lock:
        conn = get_conn()
        try:
            n = conn.execute(
                "DELETE FROM usage_records WHERE started_at < ?", (cutoff_ms,)
            ).rowcount
            conn.execute(
                "DELETE FROM quota_snapshots WHERE ts < ?", (cutoff_ms,)
            )
            conn.commit()
            return n
        finally:
            conn.close()


def delete_device_records(device_id):
    """按设备清理：删除指定设备的全部明细 + 快照，返回删除条数。"""
    with _db_lock:
        conn = get_conn()
        try:
            cur = conn.execute(
                "DELETE FROM usage_records WHERE device_id = ?", (device_id,)
            )
            conn.execute(
                "DELETE FROM quota_snapshots WHERE device_id = ?", (device_id,)
            )
            conn.commit()
            return cur.rowcount
        finally:
            conn.close()


def delete_all_usage():
    """全部清空：清 usage_records + quota_snapshots，保留设备注册。返回删除条数。"""
    with _db_lock:
        conn = get_conn()
        try:
            cur = conn.execute("DELETE FROM usage_records")
            conn.execute("DELETE FROM quota_snapshots")
            conn.commit()
            return cur.rowcount
        finally:
            conn.close()


def reset_all():
    """reset：连设备一起清，回到初始状态。返回 (usage_deleted, devices_deleted)。"""
    with _db_lock:
        conn = get_conn()
        try:
            u = conn.execute("DELETE FROM usage_records").rowcount
            conn.execute("DELETE FROM quota_snapshots")
            d = conn.execute("DELETE FROM devices").rowcount
            conn.commit()
            return u, d
        finally:
            conn.close()


def revoke_device(device_id):
    """撤销设备：同时删 devices 表记录、明细和快照。返回 (devices_deleted, usage_deleted)。"""
    with _db_lock:
        conn = get_conn()
        try:
            u = conn.execute(
                "DELETE FROM usage_records WHERE device_id = ?", (device_id,)
            ).rowcount
            conn.execute(
                "DELETE FROM quota_snapshots WHERE device_id = ?", (device_id,)
            )
            d = conn.execute(
                "DELETE FROM devices WHERE device_id = ?", (device_id,)
            ).rowcount
            conn.commit()
            return d, u
        finally:
            conn.close()
