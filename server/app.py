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


# ===== 鉴权辅助 =====

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


# ===== 同步接口 =====

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

    body 字段（snapshots 可选，向后兼容旧客户端）：
    - records: 用量明细数组
    - last_rowid: 本批记录游标
    - snapshots: 额度快照数组（可选）
    - last_snapshot_ts: 快照游标（可选）
    """
    err, device_id = require_device_token()
    if err:
        return err

    data = request.get_json(force=True)
    records = data.get("records", [])
    snapshots = data.get("snapshots", [])

    now = db.now_ms()

    # 明细
    accepted = 0
    if records:
        accepted = db.insert_usage_records(device_id, records, now)
    last_rowid = data.get("last_rowid")
    if last_rowid is None:
        # 兼容不传 last_rowid 的旧客户端。当前客户端恒传 last_rowid（sync.rs），
        # 此分支实际不会命中。注意：不能用 max_rowid_of(device_id) 作为回退——
        # 设备合并后该值会因合并记录的大偏移 rowid 而膨胀到 20 亿+，旧客户端会
        # 把它当游标写回，导致后续上传被永久跳过（静默丢数据）。返回 0 让旧客户端
        # 全量重传（INSERT OR IGNORE 幂等去重，不丢数据），最坏只是多一次冗余上传。
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

    return jsonify({
        "accepted": accepted,
        "max_rowid": max_rowid,
        "accepted_snapshots": accepted_snaps,
        "max_snapshot_ts": max_snapshot_ts,
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
    """聚合查询：返回指定设备集合在时间范围内的 overall + by_model + trend。"""
    err, _ = require_device_token()
    if err:
        return err

    from_ms = int(request.args.get("from_ms", "0"))
    to_ms = int(request.args.get("to_ms", str(db.now_ms())))
    bucket = request.args.get("bucket", "day")
    if bucket not in ("hour", "day"):
        return jsonify({"error": "bucket 必须是 hour 或 day"}), 400

    # 取所有 device_id 作为筛选池
    all_devices = db.list_devices()
    all_ids = [d["device_id"] for d in all_devices]
    filter_ids, has_filter_param = _resolve_device_filter(request.args, all_ids)

    # 边界：显式指定了 devices/exclude_device 但过滤后为空，返回空结果而非全部
    if has_filter_param and not filter_ids:
        return jsonify(db.empty_usage_result(from_ms, to_ms))

    result = db.query_usage(from_ms, to_ms, bucket, filter_ids)
    return jsonify(result)


@app.get("/snapshots")
def snapshots():
    """额度快照查询：返回指定设备集合在时间范围内的快照（带 device_id）。

    复用 /usage 的 devices/exclude_device 过滤语义。
    用于对比页/报告页的跨设备周额度周期解析。
    """
    err, _ = require_device_token()
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


@app.post("/period_detail")
def period_detail():
    """按一组周期 [start, end) 返回远端逐条用量明细。

    供客户端用本地 peak 配置折算消耗（服务端无 peak 配置）。
    body: {periods: [[start,end],...], devices?, exclude_device?}
    """
    err, _ = require_device_token()
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

    buckets = db.query_period_detail(periods, filter_ids)
    return jsonify({"buckets": buckets})


@app.get("/devices")
def devices():
    """列出所有设备（附各设备记录数）。"""
    err, _ = require_device_token()
    if err:
        return err
    return jsonify(db.list_devices())


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
    print(f"[zbar-sync] 监听端口: {PORT}")

    # 4. 启动自动清理后台线程
    _spawn_auto_cleanup()

    # 5. 启动 Flask（threaded=True 支持并发请求）
    app.run(host=HOST, port=PORT, threaded=True)


if __name__ == "__main__":
    main()
