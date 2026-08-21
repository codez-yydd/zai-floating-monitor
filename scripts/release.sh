#!/usr/bin/env bash
# ============================================================================
# ZBar 发布脚本：版本号同步 → 签名构建 → 生成双源 latest.json → 上传双仓库
#
# 用法:
#   scripts/release.sh 0.2.0                 # 构建并上传 GitHub + Gitee
#   scripts/release.sh 0.2.0 --no-upload     # 只构建不上传（本地验证产物）
#   scripts/release.sh 0.2.0 --notes "修复XX" # 指定更新说明（默认取参数后全部）
#
# 必需环境:
#   ~/.tauri/zbar-updater.key   更新签名私钥（首次用
#                               `npx tauri signer generate -w ~/.tauri/zbar-updater.key`
#                               生成，公钥已写入 tauri.conf.json。私钥丢失将无法
#                               再推送更新，务必备份！）
#   gh                          GitHub CLI（已登录），或导出 GITHUB_TOKEN
#   GITEE_TOKEN                 Gitee 私人令牌（设置→私人令牌，勾选 projects）
#
# 双仓库更新机制:
#   应用内 updater 的 endpoints 依次尝试 GitHub latest.json 与 Gitee 固定 tag
#   的 latest.json；两份 latest.json 内容一致，仅安装包下载 url 指向各自仓库，
#   任一仓库可达即可完成更新。
# ============================================================================
set -euo pipefail

# ---------- 参数 ----------
VERSION="${1:-}"
NOTES=""
UPLOAD=1
shift_done=0
if [[ -z "$VERSION" ]]; then
  echo "用法: scripts/release.sh <版本号> [--no-upload] [--notes \"更新说明\"]" >&2
  exit 1
fi
shift || true
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-upload) UPLOAD=0 ;;
    --notes) shift; NOTES="${1:-}" ;;
    *) NOTES="$NOTES$1 " ;;
  esac
  shift || true
done
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "版本号格式应为 x.y.z: $VERSION" >&2; exit 1; }

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
GITHUB_REPO="codez-yydd/zai-floating-monitor"
GITEE_REPO="codezwx/zai-floating-monitor"
KEY_PATH="${TAURI_SIGNING_PRIVATE_KEY_PATH:-$HOME/.tauri/zbar-updater.key}"
BUNDLE_DIR="$ROOT/src-tauri/target/release/bundle"

[[ -f "$KEY_PATH" ]] || { echo "未找到更新签名私钥: $KEY_PATH（见脚本头部说明）" >&2; exit 1; }

echo "==> 1/6 同步版本号到 $VERSION"
# 三处版本号：package.json / tauri.conf.json / Cargo.toml（tauri 打包以 tauri.conf.json 为准）
# 用 perl 替换而非 sed：BSD/GNU sed 的 -i 与 0,/addr/ 语法在 macOS 上不兼容
perl -pi -e 's/"version": "[^"]*"/"version": "'"$VERSION"'"/' package.json src-tauri/tauri.conf.json
perl -pi -e 'if (!$done && /^version = /) { s/version = "[^"]*"/version = "'"$VERSION"'"/; $done = 1 }' src-tauri/Cargo.toml

echo "==> 2/6 安装依赖"
npm install --silent

echo "==> 3/6 签名构建（产物含 .sig 签名文件）"
# TAURI_SIGNING_PRIVATE_KEY 可直接传私钥文件路径（tauri CLI 自动识别）
export TAURI_SIGNING_PRIVATE_KEY="$KEY_PATH"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
npm run tauri build

# ---------- 收集产物 ----------
NSIS_EXE=$(ls "$BUNDLE_DIR/nsis/"*x64-setup.exe 2>/dev/null | head -1 || true)
MSI=$(ls "$BUNDLE_DIR/msi/"*.msi 2>/dev/null | head -1 || true)
MAC_TAR=$(ls "$BUNDLE_DIR/macos/"*.app.tar.gz 2>/dev/null | head -1 || true)
# Windows 构建必有 NSIS；在 mac 上构建时只有 mac 产物（用于补传 macOS 更新包）
if [[ -z "$NSIS_EXE" && -z "$MAC_TAR" ]]; then
  echo "未找到任何更新产物（NSIS 或 app.tar.gz），请检查 $BUNDLE_DIR" >&2
  exit 1
fi

echo "==> 4/6 生成 latest.json（双源）"
GH_BASE="https://github.com/$GITHUB_REPO/releases/download/v$VERSION"
GITEE_BASE="https://gitee.com/$GITEE_REPO/releases/download/latest"
PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# 组装 latest.json：有 NSIS 则含 windows；macOS 产物存在（在 mac 上构建）才纳入
build_latest() { # $1 = 下载 base url
  local base="$1"
  local win_part="" mac_part=""
  if [[ -n "$NSIS_EXE" && -f "$NSIS_EXE.sig" ]]; then
    local nsis_name nsis_sig
    nsis_name="$(basename "$NSIS_EXE")"
    nsis_sig=$(cat "$NSIS_EXE.sig")
    win_part="\"windows-x86_64\": {\"signature\": \"$nsis_sig\", \"url\": \"$base/$nsis_name\"}"
  fi
  if [[ -n "$MAC_TAR" && -f "$MAC_TAR.sig" ]]; then
    local mac_name mac_sig
    mac_name="$(basename "$MAC_TAR")"
    mac_sig=$(cat "$MAC_TAR.sig")
    [[ -n "$win_part" ]] && win_part="$win_part,"
    win_part="$win_part\"darwin-aarch64\": {\"signature\": \"$mac_sig\", \"url\": \"$base/$mac_name\"}"
  fi
  cat <<EOF
{
  "version": "$VERSION",
  "notes": $(json_escape "${NOTES:-ZBar $VERSION}"),
  "pub_date": "$PUB_DATE",
  "platforms": { $win_part }
}
EOF
}

# JSON 字符串转义（反斜杠/引号/换行）
json_escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e ':a;N;$!ba;s/\n/\\n/g' | sed 's/^/"/;s/$/"/'
}

build_latest "$GH_BASE" > "$BUNDLE_DIR/latest.json"
build_latest "$GITEE_BASE" > "$BUNDLE_DIR/gitee-latest.json"
echo "    已生成 latest.json / gitee-latest.json"
if [[ "$UPLOAD" != "1" ]]; then
  echo "==> --no-upload：跳过上传。产物位置："
  ls -la "$BUNDLE_DIR/latest.json" "$BUNDLE_DIR/gitee-latest.json" || true
  exit 0
fi

# ---------- GitHub ----------
echo "==> 5/6 上传 GitHub Release（tag v$VERSION，自动标记 latest）"
GH_FILES=("$BUNDLE_DIR/latest.json")
[[ -n "$NSIS_EXE" ]] && GH_FILES+=("$NSIS_EXE" "$NSIS_EXE.sig")
[[ -n "$MSI" ]] && GH_FILES+=("$MSI")
[[ -n "$MAC_TAR" ]] && GH_FILES+=("$MAC_TAR" "$MAC_TAR.sig")
gh release create "v$VERSION" "${GH_FILES[@]}" \
  --repo "$GITHUB_REPO" --title "ZBar v$VERSION" --notes "${NOTES:-ZBar v$VERSION}" \
  --latest

# ---------- Gitee（固定 tag latest，endpoint 用静态 URL）----------
echo "==> 6/6 上传 Gitee Release（固定 tag latest）"
: "${GITEE_TOKEN:?需要导出 GITEE_TOKEN（Gitee 设置→私人令牌，勾选 projects）}"
GITEE_API="https://gitee.com/api/v5/repos/$GITEE_REPO/releases"

# 删除旧 latest release（tag 保留复用；无则首次创建）
OLD_ID=$(curl -sf "$GITEE_API/tags/latest?access_token=$GITEE_TOKEN" | grep -o '"id":[0-9]*' | head -1 | cut -d: -f2 || true)
if [[ -n "${OLD_ID:-}" ]]; then
  curl -sf -X DELETE "$GITEE_API/$OLD_ID?access_token=$GITEE_TOKEN" >/dev/null && echo "    已删除旧 latest release"
fi

# 附件先上传拿 file_path（不加 head -1：pipefail 下 head 提前关管道会让 grep 收
# SIGPIPE 返回非零被误判失败；响应本身单条，直接整条解析）
attach() { # $1 = 文件路径 $2 = 上传后的文件名（Gitee 以上传名为准）
  curl -sf -X POST "$GITEE_API/attach_files?access_token=$GITEE_TOKEN" \
    -F "file=@$1;filename=$2" | grep -o '"path":"[^"]*"' | cut -d'"' -f4
}
FILES=""
if [[ -n "$NSIS_EXE" ]]; then
  for f in "$NSIS_EXE" "$NSIS_EXE.sig"; do
    p=$(attach "$f" "$(basename "$f")"); FILES="$FILES$p,"
  done
  [[ -n "$MSI" ]] && { p=$(attach "$MSI" "$(basename "$MSI")"); FILES="$FILES$p,"; }
fi
[[ -n "$MAC_TAR" ]] && { p=$(attach "$MAC_TAR" "$(basename "$MAC_TAR")"); FILES="$FILES$p,"; }
p=$(attach "$BUNDLE_DIR/gitee-latest.json" "latest.json"); FILES="$FILES$p,"   # Gitee 侧文件名必须是 latest.json
FILES="${FILES%,}"

curl -sf -X POST "$GITEE_API" \
  -d "access_token=$GITEE_TOKEN" \
  -d "tag_name=latest" \
  -d "name=ZBar v$VERSION（应用内更新源）" \
  -d "body=${NOTES:-ZBar $VERSION 自动更新通道，保持最新版即可。}" \
  -d "files=$FILES" >/dev/null && echo "    Gitee release 已创建"

echo ""
echo "✅ 发布完成。请验证两个更新源可达："
echo "   https://github.com/$GITHUB_REPO/releases/latest/download/latest.json"
echo "   https://gitee.com/$GITEE_REPO/releases/download/latest/latest.json"
echo "别忘了提交代码并推送双仓库（git push / git push gitee main:master）。"
