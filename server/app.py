"""zbar-sync：ZBar 多设备额度同步服务（自托管）。

Python + Flask 实现。启动自动建库、生成 master token 并打印到日志。
监听 HTTP 端口，提供同步上传/聚合查询/清理接口。

所有 API 路径、字段名、鉴权逻辑与 Rust 版完全一致，客户端无需改动。
"""

import json
import threading
import time

from flask import Flask, g, jsonify, request

import db
from auth import (
    hash_token,
    load_or_create_master_token,
    random_device_id,
    random_hex,
    safe_eq,
)
from config import HOST, PORT

app = Flask(__name__)

# 启动时读取/生成的 master token（全局）
MASTER_TOKEN = None

# 自动清理配置 key
CLEANUP_CONFIG_KEY = "cleanup_config"


def _agent_quota_storage_available():
    """判断当前加载的 db.py 是否完整支持 Agent 额度快照。

    宝塔更新文件时可能短暂只替换其中一个模块。兼容模式下继续提供
    原有同步服务，避免新版 app.py 调用旧版 db.py 直接抛 AttributeError。
    """
    return all(
        callable(getattr(db, name, None))
        for name in (
            "insert_agent_quota_snapshots",
            "max_agent_quota_snapshot_ts_of",
            "query_agent_quota_snapshots",
        )
    ) and isinstance(getattr(db, "AGENT_QUOTA_WINDOWS", None), dict)


def _quota_account_available():
    """判断当前加载的 db.py 是否支持额度快照的 account 维度（proto 4）。

    旧版 db.py 的 quota_snapshots 无 account 列（存不进也查不出），快照
    回传时 account 恒缺失，客户端多账号今日增量的多端合并会静默退化为
    仅本机。标记为 proto 4 供客户端探测；兼容模式下继续原有同步服务。
    """
    return callable(getattr(db, "_migrate_quota_snapshots", None))


def _session_project_available():
    """判断当前加载的 db.py 是否支持会话/项目维度（proto 5）。

    宝塔热替换场景下可能出现新 app.py + 旧 db.py：旧版 insert_usage_records
    会忽略 records 里的新字段（不报错、只是不落库），但 /projects 等新查询
    函数不存在。此处与 proto 2/3/4 的探测方式一致，按新能力标记 proto 5。
    """
    return all(
        callable(getattr(db, name, None))
        for name in ("_migrate_usage_records_columns", "query_projects")
    )


def _overview_storage_available():
    """判断当前加载的 db.py 是否具备 /overview 所需的全部查询函数。

    宝塔热替换兼容：缺任一函数时 /overview 返回全空结构而不是 500。
    """
    return all(
        callable(getattr(db, name, None))
        for name in (
            "query_usage_summary",
            "max_uploaded_at",
            "last_uploaded_at_by_device",
        )
    )


# ===== 鉴权辅助 =====

# view token（手机端查看页面只读凭证）在 config 表中的键：存 sha256，不存明文
VIEW_TOKEN_HASH_KEY = "view_token_hash"

def get_master_token():
    """全局 master token（启动时初始化）。"""
    return MASTER_TOKEN


def require_master_token(req_data):
    """校验 body 里的 master_token。失败返回 (error_response, None)。"""
    token = req_data.get("master_token", "")
    if not safe_eq(token, get_master_token()):
        return (jsonify({"error": "master_token 无效"}), 401)
    return None


def require_device_token():
    """校验 Header 里的 device_token，返回 device_id。
    失败返回 (error_response, None)。
    """
    auth_header = request.headers.get("Authorization", "")
    token = None
    if auth_header.startswith("Bearer "):
        token = auth_header[7:].strip()
    if not token:
        return (jsonify({"error": "缺少 Authorization 头"}), 401), None
    token_hash = hash_token(token)
    device_id = db.find_device_by_token_hash(token_hash)
    if not device_id:
        return (jsonify({"error": "device_token 无效"}), 401), None
    return None, device_id


def require_read_token():
    """校验 Header 里的 device_token 或 view_token（只读查询接口用）。

    view token 是手机端查看页面的只读凭证：只放行查询类接口；写操作
    （/register、/sync、/device/*、/cleanup*）仍只认 master / device token，
    不接受 view token。失败返回 (error_response, None)，与
    require_device_token 的返回形状一致。
    """
    auth_header = request.headers.get("Authorization", "")
    token = None
    if auth_header.startswith("Bearer "):
        token = auth_header[7:].strip()
    if not token:
        return (jsonify({"error": "缺少 Authorization 头"}), 401), None
    token_hash = hash_token(token)
    device_id = db.find_device_by_token_hash(token_hash)
    if device_id:
        return None, device_id
    view_hash = db.get_config(VIEW_TOKEN_HASH_KEY) or ""
    if view_hash and safe_eq(token_hash, view_hash):
        return None, None
    return (jsonify({"error": "token 无效"}), 401), None


# ===== 同步接口 =====

# /sync 上传明细（records）的来源白名单：source 由持有 device token 的客户端
# 上传，属于任意字符串，不校验会原样落库（usage_records.source）并回流到
# /usage、/projects 等展示接口，构成 XSS 注入面，这里统一收紧。
SYNC_SOURCE_WHITELIST = {"zcode", "codex", "claude", "kimi", "cursor"}


def _first_invalid_record_source(records):
    """返回 records 中第一个白名单外的 source 值（用于 400 错误信息），无则 None。

    与 db.insert_usage_records 的缺省语义保持一致：source 缺失或为空按 zcode。
    """
    for item in records:
        if not isinstance(item, dict):
            continue
        source = str(item.get("source") or "zcode")
        if source not in SYNC_SOURCE_WHITELIST:
            return source
    return None


@app.post("/register")
def register():
    """注册新设备。验证 master_token 后生成 device_id + device_token。"""
    data = request.get_json(force=True)
    err = require_master_token(data)
    if err:
        return err

    name = (data.get("device_name") or "").strip()
    if not name:
        return jsonify({"error": "设备名称不能为空"}), 400
    if len(name) > 32:
        return jsonify({"error": "设备名称过长（最多 32 字符）"}), 400

    device_id = data.get("device_id") or random_device_id()
    device_token = random_hex()
    token_hash = hash_token(device_token)
    db.insert_device(device_id, name, token_hash, db.now_ms())
    return jsonify(
        {
            "device_id": device_id,
            "device_token": device_token,
            "device_name": name,
        }
    )


@app.post("/sync")
def sync():
    """增量上传明细 + 额度快照。

    body 字段（两类 snapshots 均可选，向后兼容旧客户端）：
    - records: 用量明细数组（每条可带 source='zcode'|'codex'，缺省 zcode；
      客户端保证每批 records 属同一来源，zcode 与 codex 各自独立分批上传）。
      proto 5 起每条还可带 session_id / project_key / project_display
      （均可选）；旧客户端不传即落库 NULL，行为不变
    - last_rowid: 本批记录游标（按本批来源的 rowid 序列计数）
    - snapshots: Z.ai 额度快照数组（可选）
    - last_snapshot_ts: Z.ai 快照游标（可选）
    - agent_quota_snapshots: Codex / Claude / Cursor 额度快照数组（可选）
    - last_agent_quota_snapshot_ts: Agent 快照游标（可选）
    """
    err, device_id = require_device_token()
    if err:
        return err

    data = request.get_json(force=True)
    records = data.get("records", [])
    snapshots = data.get("snapshots", [])
    agent_quota_snapshots = data.get("agent_quota_snapshots", [])

    # source 白名单校验：批次中含白名单外来源时返回 400 拒绝整批，任何数据
    # 都不落库；游标（last_rowid 等）由客户端持有，校验失败时响应不携带游标，
    # 客户端游标不推进，修正来源后重新上传即可，不会丢数据或死循环
    # （客户端对 4xx 的处理是降级重试一次后报错）。
    invalid_source = _first_invalid_record_source(records)
    if invalid_source is not None:
        return (
            jsonify({"error": f"records 包含白名单外的 source 值: {invalid_source}"}),
            400,
        )
    # agent_quota_snapshots 的 source 不在此重复校验：db.insert_agent_quota_snapshots
    # 已按 AGENT_QUOTA_WINDOWS 白名单逐条过滤（非法来源静默跳过、不落库），
    # 注入入口已被覆盖，且该"单条忽略"语义由测试
    # test_invalid_agent_sources_and_percentages_are_ignored 锁定。

    now = db.now_ms()

    # 明细（source 逐条读取，缺省 zcode；主键 (device_id, source, local_rowid) 去重）
    accepted = 0
    if records:
        accepted = db.insert_usage_records(device_id, records, now)
    last_rowid = data.get("last_rowid")
    if last_rowid is None:
        # 兼容不传 last_rowid 的旧客户端。当前客户端恒传 last_rowid（sync.rs），
        # 此分支实际不会命中。注意：不能用 max_rowid_of(device_id, source) 作为
        # 回退——设备合并后该值会因合并记录的大偏移 rowid 而膨胀到 20 亿+，旧
        # 客户端会把它当游标写回，导致后续上传被永久跳过（静默丢数据）。返回 0
        # 让旧客户端全量重传（INSERT OR IGNORE 幂等去重，不丢数据），最坏只是
        # 多一次冗余上传。
        max_rowid = 0
    else:
        max_rowid = last_rowid

    # 快照（可选；旧客户端不传 snapshots，跳过）
    accepted_snaps = 0
    max_snapshot_ts = None
    if snapshots:
        accepted_snaps = db.insert_snapshots(device_id, snapshots, now)
        last_ts = data.get("last_snapshot_ts")
        max_snapshot_ts = last_ts if last_ts is not None else db.max_snapshot_ts_of(device_id)

    # Agent 额度快照（可选；新客户端使用，旧客户端不会传此字段）
    accepted_agent_quota_snapshots = 0
    max_agent_quota_snapshot_ts = None
    if agent_quota_snapshots and _agent_quota_storage_available():
        accepted_agent_quota_snapshots = db.insert_agent_quota_snapshots(
            device_id, agent_quota_snapshots, now
        )
        last_agent_ts = data.get("last_agent_quota_snapshot_ts")
        max_agent_quota_snapshot_ts = (
            last_agent_ts
            if last_agent_ts is not None
            else db.max_agent_quota_snapshot_ts_of(device_id)
        )

    return jsonify({
        "accepted": accepted,
        "max_rowid": max_rowid,
        "accepted_snapshots": accepted_snaps,
        "max_snapshot_ts": max_snapshot_ts,
        "accepted_agent_quota_snapshots": accepted_agent_quota_snapshots,
        "max_agent_quota_snapshot_ts": max_agent_quota_snapshot_ts,
        # 服务端协议版本：5 = 明细支持会话/项目维度（session_id / project_key /
        # project_display）；4 = 额度快照带 account 维度（多账号采样隔离与
        # 多端今日增量合并）；3 = 支持 Agent 额度快照同步；若当前加载的是
        # 旧版 db.py，则降级为 4/3/2，客户端按版本探测能力。
        # proto 2 的多来源 usage_records.source 行为。
        # 客户端据此探测——旧版服务端（无 source 列）会把 codex 记录按
        # (device_id, local_rowid) 撞键静默丢弃，客户端发现 proto < 2 时
        # 不会推进 codex 游标，升级服务端后自动恢复，数据不丢。
        "proto": (
            5
            if _session_project_available()
            else (
                4
                if _quota_account_available()
                else (3 if _agent_quota_storage_available() else 2)
            )
        ),
    })


def _resolve_device_filter(q_args, all_ids):
    """解析 devices / exclude_device 参数，得到最终要查询的设备集合。
    语义：devices 非空 → 仅查这些；否则 exclude_device 非空 → 排除这些；都空 → 全部。
    """
    devices_str = q_args.get("devices", "")
    exclude_str = q_args.get("exclude_device", "")

    if devices_str:
        want = [s.strip() for s in devices_str.split(",") if s.strip()]
        return [d for d in all_ids if d in want], True
    if exclude_str:
        exclude = [s.strip() for s in exclude_str.split(",") if s.strip()]
        return [d for d in all_ids if d not in exclude], True
    # 全部
    return list(all_ids), False


@app.get("/usage")
def usage():
    """聚合查询：返回指定设备集合在时间范围内的 overall + by_model + trend。

    可选 query 参数 source（'zcode' / 'codex'）：不传 = 全部来源合并；
    by_model 每个分组带 source 字段，供前端区分展示。
    """
    err, _ = require_read_token()
    if err:
        return err

    from_ms = int(request.args.get("from_ms", "0"))
    to_ms = int(request.args.get("to_ms", str(db.now_ms())))
    bucket = request.args.get("bucket", "day")
    if bucket not in ("hour", "day"):
        return jsonify({"error": "bucket 必须是 hour 或 day"}), 400
    source = (request.args.get("source") or "").strip()

    # 取所有 device_id 作为筛选池
    all_devices = db.list_devices()
    all_ids = [d["device_id"] for d in all_devices]
    filter_ids, has_filter_param = _resolve_device_filter(request.args, all_ids)

    # 边界：显式指定了 devices/exclude_device 但过滤后为空，返回空结果而非全部
    if has_filter_param and not filter_ids:
        return jsonify(db.empty_usage_result(from_ms, to_ms))

    result = db.query_usage(from_ms, to_ms, bucket, filter_ids, source=source or None)
    return jsonify(result)


@app.get("/models")
def models():
    """全部设备、全部来源出现过的模型清单（轻量，价格配置用）。

    客户端价格设置页把清单并入本地模型列表，让"其他设备同步上来、
    本机没有"的模型也能直接配价并参与价格更新检查。
    旧客户端不调用本接口，无兼容性问题。
    """
    err, _ = require_read_token()
    if err:
        return err
    return jsonify({"models": db.list_all_models()})


@app.get("/snapshots")
def snapshots():
    """额度快照查询：返回指定设备集合在时间范围内的快照（带 device_id）。

    复用 /usage 的 devices/exclude_device 过滤语义。
    用于对比页/报告页的跨设备周额度周期解析。
    """
    err, _ = require_read_token()
    if err:
        return err

    from_ms = int(request.args.get("from_ms", "0"))
    to_ms = int(request.args.get("to_ms", str(db.now_ms())))

    all_devices = db.list_devices()
    all_ids = [d["device_id"] for d in all_devices]
    filter_ids, has_filter_param = _resolve_device_filter(request.args, all_ids)

    if has_filter_param and not filter_ids:
        return jsonify({"snapshots": []})

    snaps = db.query_snapshots(from_ms, to_ms, filter_ids)
    return jsonify({"snapshots": snaps})


@app.get("/agent-quota-snapshots")
def agent_quota_snapshots():
    """查询 Codex / Claude / Cursor 额度快照（带 device_id）。

    返回的 snapshots 保持客户端 AgentQuotaSnapshot 形状，并在顶层增加
    device_id；支持来源、设备集合和时间范围筛选。
    """
    err, _ = require_read_token()
    if err:
        return err

    # 与旧版 db.py 配合时返回空结果，不能因为新增查询接口让原有服务报 500。
    if not _agent_quota_storage_available():
        return jsonify({"snapshots": []})

    from_ms = int(request.args.get("from_ms", "0"))
    to_ms = int(request.args.get("to_ms", str(db.now_ms())))
    source = (request.args.get("source") or "").strip()

    if source and source not in db.AGENT_QUOTA_WINDOWS:
        return jsonify({"snapshots": []})

    all_devices = db.list_devices()
    all_ids = [d["device_id"] for d in all_devices]
    filter_ids, has_filter_param = _resolve_device_filter(request.args, all_ids)

    if has_filter_param and not filter_ids:
        return jsonify({"snapshots": []})

    snapshots = db.query_agent_quota_snapshots(
        from_ms, to_ms, filter_ids, source=source or None
    )
    return jsonify({"snapshots": snapshots})


@app.post("/period_detail")
def period_detail():
    """按一组周期 [start, end) 返回远端逐条用量明细。

    供客户端用本地 peak 配置折算消耗（服务端无 peak 配置）。
    body: {periods: [[start,end],...], devices?, exclude_device?, source?}
    source 可选（'zcode' / 'codex'），不传 = 全部来源。
    """
    err, _ = require_read_token()
    if err:
        return err

    data = request.get_json(force=True)
    periods = data.get("periods", [])
    if not periods:
        return jsonify({"buckets": []})

    all_devices = db.list_devices()
    all_ids = [d["device_id"] for d in all_devices]
    filter_ids, has_filter_param = _resolve_device_filter(data, all_ids)
    if has_filter_param and not filter_ids:
        empty = [{"reset_at": s, "end_at": e, "rows": []} for s, e in periods]
        return jsonify({"buckets": empty})

    source = (data.get("source") or "").strip()
    buckets = db.query_period_detail(periods, filter_ids, source=source or None)
    return jsonify({"buckets": buckets})


@app.get("/devices")
def devices():
    """列出所有设备（附各设备记录数）。"""
    err, _ = require_read_token()
    if err:
        return err
    return jsonify(db.list_devices())


# ===== view token（手机端查看页面只读凭证）=====

def _load_or_create_view_token():
    """首次启动生成 view token：hash 存 config 表，明文仅打印一次。

    复刻 master / device token 的存储模式——库里只存 sha256，明文不落盘。
    已存在（含手动 regenerate 过）时返回 None，启动日志不再重复打印。
    """
    if db.get_config(VIEW_TOKEN_HASH_KEY):
        return None
    tok = random_hex()
    db.set_config(VIEW_TOKEN_HASH_KEY, hash_token(tok))
    return tok


@app.post("/view/token/regenerate")
def view_token_regenerate():
    """重新生成 view token（master token 鉴权）。

    旧 token 立即失效；新明文仅在本次响应返回一次，服务端只存哈希。
    """
    data = request.get_json(force=True)
    err = require_master_token(data)
    if err:
        return err
    tok = random_hex()
    db.set_config(VIEW_TOKEN_HASH_KEY, hash_token(tok))
    return jsonify({"view_token": tok})


@app.get("/view/check")
def view_check():
    """view token 校验（手机页首次输入 token 时调用）。"""
    err, _ = require_read_token()
    if err:
        return err
    return jsonify({"ok": True})


# ===== 项目维度 + 手机首屏聚合（proto 5）=====

@app.get("/projects")
def projects():
    """按项目维度聚合查询（proto 5）。

    query 参数：
    - from / to：毫秒时间戳（必填，[from, to) 半开区间，与 /usage 一致）
    - devices：可选，逗号分隔 device_id（复用 /usage 的过滤语义）
    project_key 为 NULL（proto 5 之前的记录或无法归属项目）聚合为
    "__unknown__"，客户端显示为「未知项目」。
    """
    err, _ = require_read_token()
    if err:
        return err

    # 宝塔热替换兼容：旧版 db.py 无项目维度查询函数时返回空列表
    if not _session_project_available():
        return jsonify([])

    try:
        from_ms = int(request.args.get("from", ""))
        to_ms = int(request.args.get("to", ""))
    except ValueError:
        return jsonify({"error": "from / to 必须是毫秒时间戳"}), 400

    all_ids = [d["device_id"] for d in db.list_devices()]
    filter_ids, has_filter_param = _resolve_device_filter(request.args, all_ids)
    if has_filter_param and not filter_ids:
        return jsonify([])

    return jsonify(db.query_projects(from_ms, to_ms, filter_ids))


@app.get("/overview")
def overview():
    """手机首屏聚合（view token / device token 均可查）。

    一次返回三个周期的 overall + by_model（与 /usage 口径一致）、各源最新
    额度快照、近 7 天项目 Top10、设备列表与最后同步时间。可选 query 参数
    today_start（本地今日零点 ms）：不传按 UTC 日界（与 /usage trend 的
    分桶对齐一致）；手机页传入本地零点让「今日」按访客时区统计。
    """
    err, _ = require_read_token()
    if err:
        return err

    now = db.now_ms()
    day = 86_400_000
    today_start = (now // day) * day
    raw_today = request.args.get("today_start", "")
    if raw_today:
        try:
            val = int(raw_today)
        except ValueError:
            val = 0
        if 0 < val <= now:
            today_start = val

    # 宝塔热替换兼容：旧版 db.py 缺查询函数时返回全空结构而不是 500
    if not _overview_storage_available():
        empty_overall = db.empty_usage_result(now, now)["overall"]
        return jsonify({
            "now": now,
            "today": {"overall": empty_overall, "by_model": []},
            "last_7d": {"overall": empty_overall, "by_model": []},
            "last_30d": {"overall": empty_overall, "by_model": []},
            "quota_latest": {"zai": None, "agent": {}},
            "projects_top": [],
            "devices": [],
            "last_synced_at": None,
        })

    all_devices = db.list_devices()
    all_ids = [d["device_id"] for d in all_devices]

    periods = {
        "today": (today_start, now),
        "last_7d": (now - 7 * day, now),
        "last_30d": (now - 30 * day, now),
    }
    usage = {}
    for name, (start, end) in periods.items():
        overall, by_model = db.query_usage_summary(start, end, all_ids)
        usage[name] = {"overall": overall, "by_model": by_model}

    quota_latest = {"zai": None, "agent": {}}
    if callable(getattr(db, "query_latest_quota_snapshots", None)):
        quota_latest = db.query_latest_quota_snapshots()

    projects_top = []
    if _session_project_available():
        projects_top = db.query_projects(now - 7 * day, now, all_ids)[:10]

    last_by_device = db.last_uploaded_at_by_device()
    devices_out = [
        {
            "device_id": d["device_id"],
            "device_name": d["device_name"],
            "record_count": d["record_count"],
            "last_uploaded_at": last_by_device.get(d["device_id"]),
        }
        for d in all_devices
    ]

    return jsonify({
        "now": now,
        "today": usage["today"],
        "last_7d": usage["last_7d"],
        "last_30d": usage["last_30d"],
        "quota_latest": quota_latest,
        "projects_top": projects_top,
        "devices": devices_out,
        # 全部明细的最新一次上传时间（无数据为 null）
        "last_synced_at": db.max_uploaded_at(),
    })


@app.post("/device/revoke")
def revoke():
    """撤销设备（删设备记录 + 明细）。master token 鉴权。"""
    data = request.get_json(force=True)
    err = require_master_token(data)
    if err:
        return err
    device_id = data.get("device_id", "")
    if not device_id:
        return jsonify({"error": "device_id 缺失"}), 400
    devs, recs = db.revoke_device(device_id)
    return jsonify({"devices_deleted": devs, "records_deleted": recs})


@app.post("/device/merge")
def merge():
    """合并设备：把来源设备数据并入目标设备，再删除来源。master token 鉴权。"""
    data = request.get_json(force=True)
    err = require_master_token(data)
    if err:
        return err
    source = (data.get("source_device_id") or "").strip()
    target = (data.get("target_device_id") or "").strip()
    if not source or not target:
        return jsonify({"error": "source_device_id / target_device_id 缺失"}), 400
    try:
        recs, snaps = db.merge_devices(source, target)
    except ValueError as e:
        return jsonify({"error": str(e)}), 400
    return jsonify({"records_moved": recs, "snapshots_moved": snaps})


@app.post("/device/rename")
def rename():
    """修改设备显示名。master token 鉴权。"""
    data = request.get_json(force=True)
    err = require_master_token(data)
    if err:
        return err
    device_id = (data.get("device_id") or "").strip()
    name = (data.get("device_name") or "").strip()
    if not device_id:
        return jsonify({"error": "device_id 缺失"}), 400
    if not name:
        return jsonify({"error": "设备名称不能为空"}), 400
    if len(name) > 32:
        return jsonify({"error": "设备名称过长（最多 32 字符）"}), 400
    updated = db.rename_device(device_id, name)
    return jsonify({"updated": updated})


# ===== 清理接口 =====

def _load_cleanup_config():
    """读取自动清理配置（解析失败用默认值）。"""
    raw = db.get_config(CLEANUP_CONFIG_KEY)
    if raw:
        try:
            cfg = json.loads(raw)
            return {
                "auto_enabled": bool(cfg.get("auto_enabled", False)),
                "auto_days": int(cfg.get("auto_days", 0)),
            }
        except (json.JSONDecodeError, ValueError):
            pass
    return {"auto_enabled": False, "auto_days": 0}


def _save_cleanup_config(cfg):
    db.set_config(CLEANUP_CONFIG_KEY, json.dumps(cfg))


def _spawn_auto_cleanup():
    """启动自动清理后台线程：每 24 小时检查一次。"""

    def worker():
        while True:
            cfg = _load_cleanup_config()
            if cfg["auto_enabled"] and cfg["auto_days"] > 0:
                cutoff = db.now_ms() - cfg["auto_days"] * 86_400_000
                n = db.delete_before(cutoff)
                if n > 0:
                    print(
                        f"[zbar-sync] 自动清理：删除 {n} 条 "
                        f"{cfg['auto_days']} 天前的数据"
                    )
            time.sleep(24 * 3600)

    threading.Thread(target=worker, daemon=True).start()


@app.post("/cleanup")
def cleanup():
    """执行清理（master token 鉴权）。"""
    data = request.get_json(force=True)
    err = require_master_token(data)
    if err:
        return err

    action = data.get("action", "")
    if action == "device":
        device_id = data.get("device_id", "")
        if not device_id:
            return jsonify({"error": "device_id 缺失"}), 400
        n = db.delete_device_records(device_id)
        return jsonify({"action": "device", "records_deleted": n, "devices_deleted": None})

    if action == "before":
        days = data.get("days", 0)
        if not days or days <= 0:
            return jsonify({"error": "days 必须大于 0"}), 400
        cutoff = db.now_ms() - int(days) * 86_400_000
        n = db.delete_before(cutoff)
        return jsonify({"action": "before", "records_deleted": n, "devices_deleted": None})

    if action == "all":
        n = db.delete_all_usage()
        return jsonify({"action": "all", "records_deleted": n, "devices_deleted": None})

    if action == "reset":
        u, d = db.reset_all()
        return jsonify({"action": "reset", "records_deleted": u, "devices_deleted": d})

    return (
        jsonify({"error": f"未知 action: {action}（应为 device/before/all/reset）"}),
        400,
    )


@app.get("/cleanup/status")
def cleanup_status():
    """查询清理状态（device_token 鉴权）。"""
    err, _ = require_device_token()
    if err:
        return err
    return jsonify(
        {
            "total_records": db.total_records(),
            "devices": db.list_devices(),
            "auto_config": _load_cleanup_config(),
        }
    )


@app.post("/cleanup/config")
def cleanup_config():
    """设置自动清理配置（master token 鉴权）。"""
    data = request.get_json(force=True)
    err = require_master_token(data)
    if err:
        return err
    cfg = {
        "auto_enabled": bool(data.get("auto_enabled", False)),
        "auto_days": int(data.get("auto_days", 0)),
    }
    _save_cleanup_config(cfg)
    return jsonify(cfg)


# ===== 健康检查 =====

@app.get("/health")
def health():
    return "ok"


# ===== 启动入口 =====

def main():
    global MASTER_TOKEN

    # 1. 初始化数据库（自动建库建表）
    db.init_db()

    # 2. master token：读取或生成
    MASTER_TOKEN = load_or_create_master_token()

    # 3. 打印启动信息
    print("[zbar-sync] 初始化完成")
    print(f"[zbar-sync] MASTER_TOKEN: {MASTER_TOKEN}")
    print("[zbar-sync]   ↑ 复制此 token 到客户端「同步设置」注册设备")

    # view token：首次启动生成并打印（手机端查看页面只读凭证）
    view_token = _load_or_create_view_token()
    if view_token:
        print(f"[zbar-sync] VIEW_TOKEN: {view_token}")
        print("[zbar-sync]   ↑ 手机端查看页面访问令牌（浏览器打开 /static/index.html 时输入）")

    print(f"[zbar-sync] 监听端口: {PORT}")

    # 4. 启动自动清理后台线程
    _spawn_auto_cleanup()

    # 5. 启动 Flask（threaded=True 支持并发请求）
    app.run(host=HOST, port=PORT, threaded=True)


if __name__ == "__main__":
    main()
