"""Agent 额度快照同步服务端回归测试。"""

import sys
import tempfile
import unittest
from pathlib import Path


SERVER_DIR = Path(__file__).resolve().parent
if str(SERVER_DIR) not in sys.path:
    sys.path.insert(0, str(SERVER_DIR))

import app
import db
from auth import hash_token


class AgentQuotaSyncTest(unittest.TestCase):
    """覆盖 Agent 快照的数据库和 HTTP 同步行为。"""

    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        data_dir = Path(self.temp_dir.name)
        db.DATA_DIR = data_dir
        db.DB_PATH = data_dir / "usage.db"
        db.init_db()

        db.insert_device("device-a", "设备 A", hash_token("token-a"), 1)
        db.insert_device("device-b", "设备 B", hash_token("token-b"), 2)
        app.MASTER_TOKEN = "master-token"
        self.client = app.app.test_client()

    def tearDown(self):
        self.temp_dir.cleanup()

    def _headers(self, token="token-a"):
        return {"Authorization": f"Bearer {token}"}

    def _snapshot(self, source, ts, used_pct, key="weekly", plan_type="pro"):
        return {
            "source": source,
            "ts": ts,
            "plan_type": plan_type,
            "windows": [
                {
                    "key": key,
                    "used_pct": used_pct,
                    "reset_at": 2_000_000,
                }
            ],
        }

    def test_init_is_idempotent_and_schema_is_complete(self):
        db.init_db()
        db.init_db()

        conn = db.get_conn()
        try:
            columns = {
                row["name"]: row["type"]
                for row in conn.execute(
                    "PRAGMA table_info(agent_quota_snapshots)"
                ).fetchall()
            }
            self.assertEqual(
                columns,
                {
                    "device_id": "TEXT",
                    "source": "TEXT",
                    "ts": "INTEGER",
                    "plan_type": "TEXT",
                    "window_key": "TEXT",
                    "used_pct": "REAL",
                    "reset_at": "INTEGER",
                },
            )
        finally:
            conn.close()

    def test_sync_accepts_agent_snapshots_and_preserves_zai_fields(self):
        payload = {
            "records": [],
            "last_rowid": 0,
            "snapshots": [
                {
                    "ts": 100,
                    "level": "免费",
                    "weekly_pct": 12,
                    "weekly_reset": 200,
                }
            ],
            "last_snapshot_ts": 100,
            "agent_quota_snapshots": [
                {
                    "source": "codex",
                    "ts": 1_000,
                    "plan_type": "pro",
                    "windows": [
                        {"key": "hour5", "used_pct": 5.5, "reset_at": 1_500},
                        {"key": "weekly", "used_pct": 10, "reset_at": 2_000},
                    ],
                }
            ],
            "last_agent_quota_snapshot_ts": 1_000,
        }

        response = self.client.post(
            "/sync", json=payload, headers=self._headers()
        )
        body = response.get_json()
        self.assertEqual(response.status_code, 200)
        self.assertEqual(body["proto"], 5)
        self.assertEqual(body["accepted"], 0)
        self.assertEqual(body["accepted_snapshots"], 1)
        self.assertEqual(body["max_snapshot_ts"], 100)
        self.assertEqual(body["accepted_agent_quota_snapshots"], 2)
        self.assertEqual(body["max_agent_quota_snapshot_ts"], 1_000)

        duplicate = self.client.post(
            "/sync", json=payload, headers=self._headers()
        ).get_json()
        self.assertEqual(duplicate["accepted_snapshots"], 0)
        self.assertEqual(duplicate["accepted_agent_quota_snapshots"], 0)

        db.insert_agent_quota_snapshots(
            "device-a", [self._snapshot("codex", 1_000, 15)], 1_000
        )
        deduped = db.query_agent_quota_snapshots(0, 2_000, ["device-a"])
        weekly = next(
            window["used_pct"]
            for snapshot in deduped
            if snapshot["source"] == "codex"
            for window in snapshot["windows"]
            if window["key"] == "weekly"
        )
        self.assertEqual(weekly, 15.0)

        old_snapshots = self.client.get(
            "/snapshots?from_ms=0&to_ms=200", headers=self._headers()
        ).get_json()["snapshots"]
        self.assertEqual(len(old_snapshots), 1)
        self.assertEqual(old_snapshots[0]["weekly_pct"], 12)
        self.assertNotIn("source", old_snapshots[0])

    def test_sync_without_agent_fields_remains_compatible(self):
        response = self.client.post(
            "/sync",
            json={"records": [], "last_rowid": 0},
            headers=self._headers(),
        )
        body = response.get_json()
        self.assertEqual(response.status_code, 200)
        self.assertEqual(body["proto"], 5)
        self.assertEqual(body["accepted_agent_quota_snapshots"], 0)
        self.assertIsNone(body["max_agent_quota_snapshot_ts"])

    def test_invalid_agent_sources_and_percentages_are_ignored(self):
        response = self.client.post(
            "/sync",
            json={
                "records": [],
                "last_rowid": 0,
                "agent_quota_snapshots": [
                    {
                        "source": "__proto__",
                        "ts": 1_000,
                        "windows": [{"key": "weekly", "used_pct": 20}],
                    },
                    {
                        "source": "codex",
                        "ts": 1_001,
                        "windows": [
                            {"key": "weekly", "used_pct": -1},
                            {"key": "hour5", "used_pct": 101},
                        ],
                    },
                ],
            },
            headers=self._headers(),
        )
        self.assertEqual(response.status_code, 200)
        self.assertEqual(response.get_json()["accepted_agent_quota_snapshots"], 0)
        self.assertEqual(
            db.query_agent_quota_snapshots(0, 2_000, ["device-a"]), []
        )

    def test_agent_snapshot_query_groups_windows_and_filters(self):
        db.insert_agent_quota_snapshots(
            "device-a",
            [
                {
                    "source": "codex",
                    "ts": 1_000,
                    "plan_type": "pro",
                    "windows": [
                        {"key": "hour5", "used_pct": 4.5, "reset_at": 1_100},
                        {"key": "weekly", "used_pct": 8, "reset_at": 2_000},
                    ],
                },
                self._snapshot("claude", 1_200, 9),
            ],
            1_300,
        )
        db.insert_agent_quota_snapshots(
            "device-b", [self._snapshot("codex", 1_000, 12)], 1_300
        )

        response = self.client.get(
            "/agent-quota-snapshots?from_ms=1000&to_ms=1300&source=codex",
            headers=self._headers(),
        )
        snapshots = response.get_json()["snapshots"]
        self.assertEqual(response.status_code, 200)
        self.assertEqual(len(snapshots), 2)
        self.assertEqual(
            {snapshot["device_id"] for snapshot in snapshots},
            {"device-a", "device-b"},
        )
        device_a = next(
            snapshot for snapshot in snapshots if snapshot["device_id"] == "device-a"
        )
        self.assertEqual(device_a["source"], "codex")
        self.assertEqual(device_a["plan_type"], "pro")
        self.assertEqual(
            [window["key"] for window in device_a["windows"]],
            ["hour5", "weekly"],
        )

        only_a = self.client.get(
            "/agent-quota-snapshots?from_ms=0&to_ms=2000&devices=device-a",
            headers=self._headers(),
        ).get_json()["snapshots"]
        self.assertEqual({snapshot["device_id"] for snapshot in only_a}, {"device-a"})

        without_a = self.client.get(
            "/agent-quota-snapshots?from_ms=0&to_ms=2000&exclude_device=device-a",
            headers=self._headers(),
        ).get_json()["snapshots"]
        self.assertEqual(
            {snapshot["device_id"] for snapshot in without_a}, {"device-b"}
        )

    def test_cleanup_and_merge_cover_agent_snapshots(self):
        db.insert_agent_quota_snapshots(
            "device-a", [self._snapshot("codex", 10, 3)], 10
        )
        db.insert_agent_quota_snapshots(
            "device-a", [self._snapshot("codex", 20, 5)], 20
        )
        db.delete_before(20)
        self.assertEqual(
            db.query_agent_quota_snapshots(0, 100, ["device-a"])[0]["ts"], 20
        )

        db.insert_agent_quota_snapshots(
            "device-b", [self._snapshot("codex", 20, 99)], 20
        )
        db.insert_agent_quota_snapshots(
            "device-b", [self._snapshot("codex", 30, 7, key="weekly")], 30
        )
        records_moved, old_snapshots_moved = db.merge_devices(
            "device-b", "device-a"
        )
        self.assertEqual(records_moved, 0)
        self.assertEqual(old_snapshots_moved, 0)

        merged = db.query_agent_quota_snapshots(0, 100, ["device-a"])
        self.assertEqual(
            {(snapshot["ts"], snapshot["device_id"]) for snapshot in merged},
            {(20, "device-a"), (30, "device-a")},
        )
        at_conflict = next(snapshot for snapshot in merged if snapshot["ts"] == 20)
        self.assertEqual(at_conflict["windows"][0]["used_pct"], 99.0)
        self.assertEqual(
            db.query_agent_quota_snapshots(0, 100, ["device-b"]), []
        )

        db.insert_agent_quota_snapshots(
            "device-a", [self._snapshot("cursor", 40, 1)], 40
        )
        db.delete_device_records("device-a")
        self.assertEqual(
            db.query_agent_quota_snapshots(0, 100, ["device-a"]), []
        )


if __name__ == "__main__":
    unittest.main()
