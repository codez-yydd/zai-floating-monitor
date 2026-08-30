#!/usr/bin/env bash
# ============================================================================
# 本地直发 Gitee：正式版 release + latest 更新源（国内网络直连，秒级完成）
#
# 背景：CI（GitHub 海外 runner）调 Gitee 附件接口对大文件偶发持续挂起
# （v0.5.0 实测同一文件 6 次重试全超时），而本机直连 Gitee 秒传。本脚本
# 作为 Gitee 侧发布/补发的主力通道：GitHub Release 由流水线发布，产物
# 从 GitHub 下载后直传 Gitee。
#
# 用法：scripts/publish_gitee.sh <版本号> [发行说明]
#   例：scripts/publish_gitee.sh 0.5.0
#   例：scripts/publish_gitee.sh 0.7.0 "ZBar v0.7.0：新增……"
#   发行说明（第二个参数）可选，缺省用"自动更新通道"默认文案；
#   与 GitHub Release 说明保持一致时传入相同文本
#
# 令牌来源（按优先级，绝不写入仓库文件）：
#   1. 环境变量 GITEE_TOKEN
#   2. macOS 钥匙串：security find-generic-password -s zbar-gitee-token -w
#   GH_TOKEN（可选）用于 GitHub API 认证，避免代理出口 IP 匿名限流；缺省匿名访问，行为不变
#
# GitHub 下载走代理（默认 http://127.0.0.1:33210，可用 GH_PROXY 覆盖，
# 置空则直连）；Gitee 始终直连。
#
# 流程（原 release.yml Gitee 步骤的同款逻辑，幂等可重跑，5 步）：
#   下载 GitHub Release 产物 → 创建/复用正式版 release（tag=v版本，上传
#   全部程序附件）→ 删 Gitee 旧 latest release（仅 404 视为不存在）→
#   创建新 latest release → payload 先传（失败查服务端、已存在跳过、
#   退避重试）→ 完整性校验 → latest.json 最后提交 → 回读确认
#
# 正式版 release 永不删除，累积保留版本历史；latest 为滚动更新源，删旧
# 重建不触碰版本 tag。背景：脚本原逻辑只维护一个滚动复用的 latest release
# （应用内更新源，URL releases/download/latest/latest.json 被已发布的应用
# 写死依赖，必须保持），删旧建新——导致 Gitee 上从无正式版 release、无
# 版本 tag 历史，老版本无法回溯；自 v0.8.1 起改造为每次发布同时创建正式版
# release（tag=v版本），任何 release 均不删除。
# ============================================================================
set -euo pipefail

VERSION="${1:-}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "用法: $0 <版本号> [发行说明]，例: $0 0.5.0"; exit 1; }
# Gitee release 的发行说明：第二个参数可选，缺省沿用默认文案；
# 与 GitHub Release 说明保持一致时传入相同文本
RELEASE_BODY="${2:-ZBar v${VERSION} 自动更新通道，保持最新版即可。}"

GH_REPO="codez-yydd/zai-floating-monitor"
GITEE_REPO="codezwx/zai-floating-monitor"
GH_PROXY="${GH_PROXY-http://127.0.0.1:33210}"   # 置 GH_PROXY="" 可禁用代理
API="https://gitee.com/api/v5/repos/$GITEE_REPO/releases"
command -v jq >/dev/null 2>&1 || { echo '错误：缺少 jq（brew install jq）'; exit 1; }

# ---------------------------------------------------------------------------
# 令牌获取：环境变量 → macOS 钥匙串
# ---------------------------------------------------------------------------
if [[ -n "${GITEE_TOKEN:-}" ]]; then
  TOKEN="$GITEE_TOKEN"
elif command -v security >/dev/null 2>&1 && security find-generic-password -s zbar-gitee-token -w >/dev/null 2>&1; then
  TOKEN=$(security find-generic-password -s zbar-gitee-token -w)
else
  echo '错误：未找到 Gitee 令牌（环境变量 GITEE_TOKEN 或钥匙串 zbar-gitee-token）'
  exit 1
fi
TOKEN_PARAM="access_token=$TOKEN"

gh_curl() { # GitHub 下载走代理，Gitee 直连；GH_TOKEN 非空时附加认证头
  # GH_TOKEN（可选）：api.github.com 匿名请求受代理出口 IP 限流（HTTP 403），
  # 有令牌则附加 Bearer 认证提额；该头仅随本函数发往 github.com，
  # Gitee 侧均为裸 curl 调用，绝不会带 GitHub 令牌
  if [[ -n "${GH_TOKEN:-}" ]]; then
    set -- -H "Authorization: Bearer $GH_TOKEN" "$@" # 前插到参数最前
  fi
  if [[ -n "$GH_PROXY" ]]; then
    curl -sS --proxy "$GH_PROXY" --connect-timeout 20 "$@"
  else
    curl -sS --connect-timeout 20 "$@"
  fi
}

WORK=$(mktemp -d "${TMPDIR:-/tmp}/zbar-gitee.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

# ---------------------------------------------------------------------------
# 附件辅助函数：均以 release id 为第一个参数，正式版与 latest 两处复用
# ---------------------------------------------------------------------------
attach_list() { # $1=release id
  curl -sf --connect-timeout 20 --max-time 60 "$API/$1/attach_files?$TOKEN_PARAM" 2>/dev/null || echo '[]'
}
attachment_exists() { # $1=release id $2=附件名
  attach_list "$1" | jq -e --arg n "$2" 'map(.name) | index($n) != null' >/dev/null
}

upload() { # $1=release id $2=本地文件 $3=附件名
  local rid="$1" file="$WORK/$2" name="$3" rc code resp i d max_time size upath
  size=$(wc -c < "$file")
  max_time=$(( size / 51200 + 90 )); (( max_time < 180 )) && max_time=180
  # Git Bash 的 mingw64 curl 是原生 Windows 程序，读不了 MSYS 的 /tmp 路径
  # （curl 错误 26 Failed to open/read local data）；有 cygpath 时转成
  # Windows 真实路径，macOS/Linux 无 cygpath 则原样
  if command -v cygpath >/dev/null 2>&1; then upath=$(cygpath -w "$file"); else upath="$file"; fi
  for i in 1 2 3 4; do
    echo "  上传 ${name}（第 $i/4 次，超时 ${max_time}s）"
    resp=$(curl -sS --connect-timeout 20 --max-time "$max_time" -w '\n%{http_code}' \
      -X POST "$API/$rid/attach_files" \
      -F "$TOKEN_PARAM" \
      -F "file=@$upath;filename=$name") && rc=0 || rc=$?
    code="${resp##*$'\n'}"
    if [[ $rc -eq 0 && "$code" == 2* ]]; then echo "  $name 上传成功"; return 0; fi
    echo "  $name 未确认（rc=$rc HTTP=${code:-无}），查询服务端"
    if attachment_exists "$rid" "$name"; then echo "  服务端已存在 ${name}，判定成功"; return 0; fi
    case $i in 1) d=5 ;; 2) d=10 ;; 3) d=20 ;; *) d=0 ;; esac
    [[ $i -lt 4 ]] && { echo "  ${d}s 后重试"; sleep "$d"; }
  done
  echo "错误：上传失败 $name"; return 1
}

# ---------------------------------------------------------------------------
# 1. 从 GitHub Release 下载产物（payload 8 个 + gitee-latest.json）
# ---------------------------------------------------------------------------
echo "[1/5] 下载 GitHub Release v$VERSION 产物"
DL_BASE="https://github.com/$GH_REPO/releases/download/v$VERSION"
ASSETS=$(gh_curl -fsSL --max-time 120 "https://api.github.com/repos/$GH_REPO/releases/tags/v$VERSION" \
  | jq -r '.assets[].name' | grep -E '(-setup\.exe(\.sig)?|aarch64.*\.app\.tar\.gz(\.sig)?|x64\.app\.tar\.gz(\.sig)?|\.dmg|^gitee-latest\.json$)' || true)
[[ -n "$ASSETS" ]] || { echo '错误：GitHub Release 上未找到产物资产'; exit 1; }

PAYLOAD=()
while IFS= read -r name; do
  [[ -n "$name" ]] || continue
  echo "  下载 $name"
  gh_curl -fSL --max-time 300 -o "$WORK/$name" "$DL_BASE/$name"
  [[ "$name" == gitee-latest.json ]] || PAYLOAD+=("$name")
done <<< "$ASSETS"
[[ ${#PAYLOAD[@]} -eq 8 ]] || echo "警告：payload 数量 ${#PAYLOAD[@]}（预期 8），请核对"
[[ ${#PAYLOAD[@]} -gt 0 ]] || { echo '错误：未下载到任何程序附件'; exit 1; }
[[ -f "$WORK/gitee-latest.json" ]] || { echo '错误：缺少 gitee-latest.json（由 Release 流水线生成）'; exit 1; }

# ---------------------------------------------------------------------------
# 2. 创建/复用正式版 release（tag=v版本）：正式版永不删除，累积保留版本
#    历史；latest.json 是更新器专用文件，不上传到正式版
# ---------------------------------------------------------------------------
echo "[2/5] 创建/复用正式版 release（tag=v${VERSION}）"
OFFICIAL_ID=''
OFF_RESP=$(curl -sS --connect-timeout 20 --max-time 60 -w '\n%{http_code}' "$API/tags/v$VERSION?$TOKEN_PARAM") || true
OFF_CODE="${OFF_RESP##*$'\n'}"
OFF_BODY="${OFF_RESP%$'\n'*}"
if [[ "$OFF_CODE" == 2* ]]; then
  OFFICIAL_ID=$(echo "$OFF_BODY" | jq -r '.id // empty')
  if [[ -n "$OFFICIAL_ID" ]]; then
    echo "  正式版 release 已存在，复用补传 (id=$OFFICIAL_ID)"
  fi
  # Gitee 对"tag 存在但未挂 release"返回 200 + body null，与 404 一样走创建
elif [[ "$OFF_CODE" != 404 ]]; then
  echo "错误：查询正式版 release 失败 HTTP ${OFF_CODE:-网络错误}"; exit 1
fi
if [[ -z "$OFFICIAL_ID" ]]; then
  # 创建请求体走 UTF-8 JSON 文件 + jq 自检（同 latest release 的既有模式）。
  # body 先经 jq -Rs 转义成合法 JSON 字符串（自带引号与 \n 转义）再拼接：
  # 多行 Markdown 说明若原样拼进 heredoc 会产生非法 JSON（v1.1.0 发版踩坑）；
  # 走 stdin 字节流也顺带规避 Windows 命令行中文的 GBK 转码问题
  BODY_JSON=$(printf '%s' "$RELEASE_BODY" | jq -Rs .)
  cat > "$WORK/official.json" <<EOF
{"tag_name":"v${VERSION}","target_commitish":"master","name":"ZBar v${VERSION}","body":${BODY_JSON}}
EOF
  jq -e . "$WORK/official.json" >/dev/null
  OFF_CREATE=$(curl -sS --connect-timeout 20 --max-time 120 -w '\n%{http_code}' -X POST "$API?$TOKEN_PARAM" \
    -H "Content-Type: application/json" \
    --data-binary @"$WORK/official.json")
  OFF_CREATE_CODE="${OFF_CREATE##*$'\n'}"
  OFF_CREATE_BODY="${OFF_CREATE%$'\n'*}"
  [[ "$OFF_CREATE_CODE" == 2* ]] || { echo "错误：创建正式版 release 失败 HTTP ${OFF_CREATE_CODE}：$(echo "$OFF_CREATE_BODY" | head -c 300)"; exit 1; }
  OFFICIAL_ID=$(echo "$OFF_CREATE_BODY" | jq -r '.id // empty')
  [[ -n "$OFFICIAL_ID" ]] || { echo '错误：未解析到正式版 release id'; exit 1; }
  echo "  正式版 release 已创建 (id=$OFFICIAL_ID)"
fi
for name in "${PAYLOAD[@]}"; do
  upload "$OFFICIAL_ID" "$name" "$name" || exit 1
done

# ---------------------------------------------------------------------------
# 3. 删除旧 latest release（仅 404 视为不存在）
# ---------------------------------------------------------------------------
echo '[3/5] 清理 Gitee 旧 latest release'
OLD_RESP=$(curl -sS --connect-timeout 20 --max-time 60 -w '\n%{http_code}' "$API/tags/latest?$TOKEN_PARAM") || true
OLD_CODE="${OLD_RESP##*$'\n'}"
OLD_BODY="${OLD_RESP%$'\n'*}"
if [[ "$OLD_CODE" == 404 ]]; then
  echo '  无旧 latest release，跳过删除'
elif [[ "$OLD_CODE" == 2* ]]; then
  OLD_ID=$(echo "$OLD_BODY" | jq -r '.id // empty')
  if [[ -z "$OLD_ID" ]]; then
    # Gitee 对"tag 存在但未挂 release"返回 200 + body null，视为不存在
    echo '  无旧 latest release（tag 未挂载 release），跳过删除'
  else
    curl -sf --connect-timeout 20 --max-time 60 -X DELETE "$API/$OLD_ID?$TOKEN_PARAM" >/dev/null \
      && echo "  已删除旧 release (id=$OLD_ID)" \
      || { echo "错误：删除旧 release 失败（id=${OLD_ID}）"; exit 1; }
  fi
else
  echo "错误：查询旧 release 失败 HTTP ${OLD_CODE:-网络错误}"; exit 1
fi

# ---------------------------------------------------------------------------
# 4. 创建新 latest release 并上传 payload（业务级重试）
# ---------------------------------------------------------------------------
echo '[4/5] 创建新 latest release 并上传附件'
# 创建请求体走 UTF-8 JSON 文件：Windows 的原生 curl 会把命令行里的中文按本地
# 码页（GBK）转码发出，Gitee 报 invalid byte sequence in UTF-8；文件字节直传
# 三端（macOS/Linux/Git Bash）行为一致，jq 解析兼作 UTF-8 编码自检。
# body 同样经 jq -Rs 转义（多行说明的换行必须转成 \n，见上方正式版处的踩坑说明）
BODY_JSON=$(printf '%s' "$RELEASE_BODY" | jq -Rs .)
cat > "$WORK/release.json" <<EOF
{"tag_name":"latest","target_commitish":"master","name":"ZBar v${VERSION}（应用内更新源）","body":${BODY_JSON}}
EOF
jq -e . "$WORK/release.json" >/dev/null
RESP=$(curl -sS --connect-timeout 20 --max-time 120 -w '\n%{http_code}' -X POST "$API?$TOKEN_PARAM" \
  -H "Content-Type: application/json" \
  --data-binary @"$WORK/release.json")
CODE="${RESP##*$'\n'}"
BODY="${RESP%$'\n'*}"
[[ "$CODE" == 2* ]] || { echo "错误：创建 release 失败 HTTP ${CODE}：$(echo "$BODY" | head -c 300)"; exit 1; }
LATEST_ID=$(echo "$BODY" | jq -r '.id // empty')
[[ -n "$LATEST_ID" ]] || { echo '错误：未解析到 latest release id'; exit 1; }
echo "  latest release 已创建 (id=$LATEST_ID)"

for name in "${PAYLOAD[@]}"; do
  upload "$LATEST_ID" "$name" "$name" || exit 1
done

# ---------------------------------------------------------------------------
# 5. 完整性校验 → latest.json 最后提交 → 回读确认
# ---------------------------------------------------------------------------
echo '[5/5] 完整性校验并提交 latest.json'
FINAL_LIST=''
for i in 1 2 3; do
  if FINAL_LIST=$(curl -sf --connect-timeout 20 --max-time 60 "$API/$LATEST_ID/attach_files?$TOKEN_PARAM" 2>/dev/null); then
    break
  fi
  FINAL_LIST=''
  [[ $i -eq 3 ]] || { echo "  列表查询失败（第 $i/3 次），10s 后重试"; sleep 10; }
done
[[ -n "$FINAL_LIST" ]] || { echo '错误：附件列表查询失败，无法校验'; exit 1; }
MISSING=0
for name in "${PAYLOAD[@]}"; do
  jq -e --arg n "$name" 'map(.name) | index($n) != null' <<<"$FINAL_LIST" >/dev/null \
    || { echo "  缺失: $name"; MISSING=1; }
done
[[ $MISSING -eq 0 ]] || { echo '错误：完整性校验失败，不提交 latest.json（重跑本脚本恢复）'; exit 1; }
echo "  校验通过：${#PAYLOAD[@]} 个程序附件全部就位"

upload "$LATEST_ID" gitee-latest.json latest.json || { echo '错误：latest.json 上传失败'; exit 1; }
attachment_exists "$LATEST_ID" latest.json || { echo '错误：latest.json 未确认'; exit 1; }

echo ''
echo "发布成功：v$VERSION"
echo "  正式版 release：id=$OFFICIAL_ID / https://gitee.com/$GITEE_REPO/releases/tag/v$VERSION"
echo "  latest 更新源：id=$LATEST_ID / 附件 $(attach_list "$LATEST_ID" | jq 'length') 个"
attach_list "$LATEST_ID" | jq -r '.[].name' | sed 's/^/    /'
echo '更新源：https://gitee.com/codezwx/zai-floating-monitor/releases/download/latest/latest.json'
