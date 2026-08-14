# zbar-sync · ZBar Multi-Device Sync Server

The ZBar client uploads local ZCode usage data incrementally to this service, enabling aggregated viewing across multiple machines (office / home).

**Self-hosted, single-user**: you deploy it yourself, data stays on your own server. Supports HTTP / HTTPS / bare IP.

Tech stack: **Python 3 + Flask + stdlib sqlite3** — zero compilation, runs immediately.

---

## Quick Start

### 1. Upload files to your server

Upload **these files** from the `server/` folder (do NOT upload `.venv/` or `zbar-data/`):

```
app.py          # Main app (all endpoints)
db.py           # Database ops (auto-creates DB & tables)
auth.py         # Auth (master token / device token)
config.py       # Config (port, data dir)
start.sh        # Launch script
requirements.txt
```

### 2. Install dependencies

**Run in the terminal** (avoid the BT Panel "Python Project Manager" — it may throw pip errors):

```bash
cd /your/upload/path/zbar-sync
pip3 install flask
```

> Only Flask is needed. Python 3.8+ required; sqlite3 is part of the standard library.
> If `pip3` errors out, fix it first: `python3 -m ensurepip --upgrade`, or use `python3 -m pip install flask`.

### 3. Start

```bash
./start.sh
```

Or directly:

```bash
python3 app.py
```

On first launch it automatically:
- Creates the `zbar-data/` directory
- Creates the SQLite database (`zbar-data/usage.db`)
- Generates a Master Token and prints it to the log

Output looks like:

```
[zbar-sync] Initialized
[zbar-sync] MASTER_TOKEN: 9f3a7c2e8b1d4a6f...
[zbar-sync]   ↑ Copy this token to the client "Sync Settings" to register a device
[zbar-sync] Listening on port: 3838
```

**Copy this Master Token** (you can always view it later with `cat zbar-data/master.token`).

### 4. Open the port

Open port **3838** (TCP) in your firewall / security group.
Cloud providers (AWS / Azure / etc.) also need the port opened in their **security group** settings.

### 5. Connect the client

ZBar panel → click **⇅** → fill in:
- Server URL: `http://your-server-ip:3838`
- Credential: paste the Master Token
- Device name: e.g. `work` / `home`

Click "Connect & Register" to finish.

---

## Keep it running (process supervisor)

The process exits when you close the terminal. Use a process manager to keep it alive:

**systemd (Linux):**

```bash
cat > /etc/systemd/system/zbar-sync.service <<'EOF'
[Unit]
Description=ZBar Sync Server
After=network.target

[Service]
Type=simple
WorkingDirectory=/your/upload/path/zbar-sync
ExecStart=/usr/bin/python3 /your/upload/path/zbar-sync/app.py
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

systemctl enable --now zbar-sync
```

**BT Panel**: install the "Process Supervisor Manager" plugin → add a supervisor process:
- Name: `zbar-sync`
- User: `root`
- Working dir: `/your/upload/path/zbar-sync`
- Start command: `python3 /your/upload/path/zbar-sync/app.py`

---

## Custom port

Set the `PORT` environment variable at startup:

```bash
PORT=8080 python3 app.py
```

---

## Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `3838` | Listen port |
| `DATA_DIR` | `./zbar-data` | Data directory (DB + master.token) |
| `HOST` | `0.0.0.0` | Listen address (0.0.0.0 = accessible externally) |

---

## API Overview

| Endpoint | Auth | Purpose |
|----------|------|---------|
| `POST /register` | Master Token | Register a device, returns Device Token |
| `POST /sync` | Device Token | Incremental upload of usage records (each record may carry source, defaults to zcode) |
| `GET /usage` | Device Token | Aggregated query (overall + by_model + trend, optional source filter) |
| `GET /devices` | Device Token | Device list |
| `POST /device/revoke` | Master Token | Revoke a device |
| `POST /cleanup` | Master Token | Data cleanup (by device / by time / all / reset) |
| `GET /cleanup/status` | Device Token | Data volume + auto-cleanup config |
| `POST /cleanup/config` | Master Token | Configure scheduled auto-cleanup |
| `GET /health` | None | Health check |

---

## Codex / Claude Data (source dimension)

Newer clients also upload Codex CLI and Claude Code usage records in addition to ZCode usage. Sources are distinguished by the `source` field (`zcode` / `codex` / `claude`): the same device and the same `local_rowid` never conflict across sources (each source has its own independent rowid sequence), and their upload/query cursors are independent as well. The server schema does not constrain source values, so future sources need no server changes.

**Upgrade**: pull the new code and restart the service. On first launch the `usage_records` table structure is migrated automatically (a `source` column is added, primary key becomes `(device_id, source, local_rowid)`); all existing data is kept intact and marked as `zcode`, and indexes are rebuilt. Upgrading the server before the clients is recommended.

**Upgrade-order protection**: before uploading Codex / Claude data, new clients probe the server protocol version (the `/sync` response now includes a `proto: 2` field). Old servers do not return it, so the client neither uploads such data nor advances the cursor (the sync log shows "server version too old"); after the server is upgraded, uploading resumes automatically — no data is lost even if clients are upgraded first. Old clients are unaffected.

**API changes** (all backward compatible):

- `POST /sync`: each record gains a `source` field, defaulting to `zcode` (old clients that omit it are treated as zcode). Each batch contains a single source; `last_rowid` / `max_rowid` count within that source's own rowid sequence. Re-sending the same primary key with a larger `computed_total_tokens` overwrites the old row (Claude Code sessions stream to disk, so the client re-uploads final values to correct previously uploaded intermediate ones; zcode/codex records are immutable and re-sends are no-ops due to the overwrite guard).
- `GET /usage`: new optional query parameter `source` (`zcode` / `codex` / `claude`); omitting it merges all sources. Every group in `by_model` and `trend.by_model` gains a `source` field for frontend display.
- `POST /period_detail`: the body also accepts an optional `source` field.

---

## Data Cleanup

Operate via the client "Sync Settings → Data Management", or call the API directly.

| Action | Description | Irreversible |
|--------|-------------|--------------|
| By device | Delete all records of a device | ✅ |
| By time | Delete data older than N days (shortens trend history) | ✅ |
| All | Clear all usage data, keep device registrations | ✅ |
| Reset | Clear everything including devices, back to initial state | ✅ |

> **Cleanup is deletion**: cleaned data is NOT re-uploaded from the client.

---

## Data Storage

```
zbar-data/
├── master.token     # Master Token (persisted, not regenerated on restart)
├── usage.db         # SQLite database (auto-created)
└── usage.db-wal     # WAL log (managed by SQLite)
```

Backup: just copy the `zbar-data/` directory.

---

## Security Notes

- **HTTP warning**: when using HTTP or bare IP, tokens and usage data travel in plaintext over the network. Recommended for intranet use, or set up HTTPS via an Nginx reverse proxy.
- **Auth**: Master Token for registration admission, Device Token for daily auth (server stores only hashes).
- **Privacy**: synced data contains only model names, token counts, and timestamps — **no code or conversation content**.
