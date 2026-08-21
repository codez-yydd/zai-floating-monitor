#!/usr/bin/env python3
"""生成 Tauri updater 的 latest.json（Windows + macOS 双架构）。

用法: python3 scripts/gen_latest.py <下载base_url> <输出文件>

从 dist/ 目录收集三平台安装包与配套 .sig 签名文件，组装成 updater
所需的 latest.json。版本号取环境变量 VERSION（缺省回退 tauri.conf.json）。
两份 latest.json 内容一致，仅安装包下载 url 指向各自仓库（GitHub / Gitee）。
"""
import glob
import json
import os
import sys
from datetime import datetime, timezone

base, out = sys.argv[1], sys.argv[2]

version = os.environ.get("VERSION")
if not version:
    with open("src-tauri/tauri.conf.json", encoding="utf-8") as f:
        version = json.load(f)["version"]

nsis = glob.glob("out/*x64-setup.exe")
mac_arm = glob.glob("out/*aarch64*.app.tar.gz")
mac_x64 = glob.glob("out/*x64.app.tar.gz")
assert nsis and mac_arm and mac_x64, f"产物不全: nsis={nsis} arm={mac_arm} x64={mac_x64}"
nsis, mac_arm, mac_x64 = nsis[0], mac_arm[0], mac_x64[0]


def sig(path: str) -> str:
    with open(path + ".sig", encoding="utf-8") as f:
        return f.read().strip()


def entry(path: str) -> dict:
    return {"signature": sig(path), "url": f"{base}/{os.path.basename(path)}"}


data = {
    "version": version,
    "notes": f"ZBar {version}",
    "pub_date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "platforms": {
        "windows-x86_64": entry(nsis),
        "darwin-aarch64": entry(mac_arm),
        "darwin-x86_64": entry(mac_x64),
    },
}

with open(out, "w", encoding="utf-8") as f:
    json.dump(data, f, ensure_ascii=False, indent=2)
print(f"已生成 {out} (v{version})")
