#!/usr/bin/env bash
#
# generate-default-wallpaper.sh
#
# 生成 ZCode 悬浮监控应用的内置默认动态壁纸：
#   - 画面：深蓝黑底（#0B1220）上 3 个蓝 / 紫 / 青柔边光斑缓慢漂移并呼吸明暗
#   - 闭合：光斑位置与亮度全部使用周期恰等于总时长的三角函数（整数倍频 + 固定相位），
#           首尾帧画面一致，播放时可无缝循环
#   - 产物：src-tauri/wallpapers/default.mp4
#           1920x1080 / H.264 / yuv420p / 25fps / 12 秒整 / 无声 / ≤8MB / +faststart
#
# 实现要点：
#   - geq 在 480x270 小尺寸上逐像素计算，再 lanczos 放大到 1920x1080，兼顾耗时与画质
#   - 编码在临时目录中进行，全部自验通过后才 mv 原子替换目标文件（与输出同文件系统）
#   - 固定参数 + 固定 CRF + 固定 x264 线程数，同一 ffmpeg 版本下可复现出相同字节
#
# 用法：
#   ./scripts/generate-default-wallpaper.sh
#
# 环境变量：
#   FFMPEG  显式指定 ffmpeg 可执行文件路径；设置了但不可用将直接中文报错退出，不做静默回退

set -euo pipefail

# ---------- 固定参数 ----------
readonly DUR=12                            # 总时长（秒），光斑轨迹周期与其严格相等
readonly FPS=25                            # 帧率
readonly TOTAL_FRAMES=$((DUR * FPS))       # 总帧数（12s x 25fps = 300 帧）
readonly SW=480                            # geq 逐像素计算的低分辨率宽（16:9）
readonly SH=270                            # geq 逐像素计算的低分辨率高
readonly OUT_W=1920                        # 目标分辨率宽
readonly OUT_H=1080                        # 目标分辨率高
readonly MAX_BYTES=$((8 * 1024 * 1024))    # 产物体积上限：8MB
CRF=22                                     # 起始 CRF；超体积上限则逐步调大并重编码

# ---------- 错误与日志 ----------
die()  { echo "错误：$*" >&2; exit 1; }
info() { echo "[generate-default-wallpaper] $*"; }

# ---------- 定位仓库目录（不依赖当前工作目录） ----------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
readonly OUT_DIR="${ROOT_DIR}/src-tauri/wallpapers"
readonly OUT_FILE="${OUT_DIR}/default.mp4"

# ---------- ffmpeg 探测：显式 FFMPEG 优先，其次 Homebrew，最后 PATH ----------
FFMPEG_BIN=""
if [[ -n "${FFMPEG:-}" ]]; then
  # 显式指定优先；指定的路径无效时不做静默回退，直接报错
  if [[ -x "${FFMPEG}" ]]; then
    FFMPEG_BIN="${FFMPEG}"
  elif command -v "${FFMPEG}" >/dev/null 2>&1; then
    FFMPEG_BIN="$(command -v "${FFMPEG}")"
  else
    die "环境变量 FFMPEG 已设置但不是可执行文件：${FFMPEG}（不进行静默回退，请修正后重试）"
  fi
elif [[ -x /opt/homebrew/bin/ffmpeg ]]; then
  FFMPEG_BIN="/opt/homebrew/bin/ffmpeg"
elif command -v ffmpeg >/dev/null 2>&1; then
  FFMPEG_BIN="$(command -v ffmpeg)"
else
  die "未找到 ffmpeg：请先安装（例如 brew install ffmpeg），或通过环境变量 FFMPEG 指定其可执行文件路径"
fi

# ffprobe 优先取 ffmpeg 同目录（随同安装），其次 PATH
FFPROBE_BIN=""
_probe_candidate="${FFMPEG_BIN%/*}/ffprobe"
if [[ -x "${_probe_candidate}" ]]; then
  FFPROBE_BIN="${_probe_candidate}"
elif command -v ffprobe >/dev/null 2>&1; then
  FFPROBE_BIN="$(command -v ffprobe)"
else
  die "未找到 ffprobe（与 ffmpeg 配套的规格校验工具）：请安装完整 ffmpeg 后重试"
fi
unset _probe_candidate
info "使用 ffmpeg：${FFMPEG_BIN}"
info "使用 ffprobe：${FFPROBE_BIN}"

# ---------- 临时目录（与输出目标同文件系统，保证最后 mv 原子替换） ----------
mkdir -p "${OUT_DIR}"
TMP_DIR="$(mktemp -d "${OUT_DIR}/.tmp-XXXXXX")"
cleanup() { rm -rf "${TMP_DIR}"; }
trap cleanup EXIT

# ---------- 滤镜链：深蓝黑底 + 3 个柔边光斑 ----------
# 时间相位基准：ST = 2*PI*T/DUR，周期恰等于总时长。
# 所有随时间变化的项都是 ST 的整数倍频加固定相位，因此 t=0 与 t=DUR 画面完全一致。
readonly ST="(2*PI*T/${DUR})"

# 生成单个光斑对某一颜色通道的贡献表达式（高斯柔边 + 正弦亮度呼吸）。
# 参数：
#   $1    通道权重（决定该光斑在此通道呈现的色调强度）
#   $2    亮度基准      $3  亮度摆幅    $4  亮度相位
#   $5    中心X基准(相对W)   $6  中心X漂移幅度(相对W)
#   $7    中心Y基准(相对H)   $8  中心Y漂移幅度(相对H)
#   $9    光斑尺寸 sigma(相对W)
#   ${10} 位置谐波倍数（正整数，保证循环闭合）
#   ${11} 位置相位（Y 方向自动再偏移 2.1 弧度增加轨迹变化）
blob() {
  printf '%s*(%s+%s*sin(%s+%s))*exp(-(pow(X-(W*(%s+%s*sin(%s*%s+%s))),2)+pow(Y-(H*(%s+%s*cos(%s*%s+(%s)+2.1))),2))/(2*pow(%s*W,2)))' \
    "$1" "$2" "$3" "${ST}" "$4" \
    "$5" "$6" "${10}" "${ST}" "$11" \
    "$7" "$8" "${10}" "${ST}" "$11" \
    "$9"
}

# 三个光斑的公共参数：亮度基准 亮度摆幅 亮度相位 中心X基准 X漂移 中心Y基准 Y漂移 sigma 谐波倍数 位置相位
_B1=(90 25 0.8  0.30 0.10 0.38 0.08 0.16 1 0)    # 蓝色光斑：左上区域，1 倍频漂移
_B2=(80 25 2.5  0.70 0.12 0.62 0.10 0.15 1 2.1)  # 紫色光斑：右下区域，1 倍频漂移
_B3=(70 20 1.57 0.48 0.14 0.30 0.09 0.13 2 1.2)  # 青色光斑：中上区域，2 倍频漂移

# 逐通道合成：底色 #0B1220 -> R=11 G=18 B=32，叠加三个光斑的 RGB 配比贡献
GEQ_R="11+$(blob 0.16 "${_B1[@]}")+$(blob 0.62 "${_B2[@]}")+$(blob 0.18 "${_B3[@]}")"
GEQ_G="18+$(blob 0.38 "${_B1[@]}")+$(blob 0.30 "${_B2[@]}")+$(blob 0.82 "${_B3[@]}")"
GEQ_B="32+$(blob 0.92 "${_B1[@]}")+$(blob 0.88 "${_B2[@]}")+$(blob 0.95 "${_B3[@]}")"
unset _B1 _B2 _B3

# 完整滤镜链：小尺寸逐像素合成 -> lanczos 放大到 1080p -> yuv420p
CHAIN="format=gbrp,geq=r='${GEQ_R}':g='${GEQ_G}':b='${GEQ_B}',scale=${OUT_W}:${OUT_H}:flags=lanczos,format=yuv420p"

# ---------- 工具函数 ----------
file_size() { stat -f%z "$1" 2>/dev/null || stat -c%s "$1"; }

# 编码正式产物（无声 / faststart / 固定线程数保证确定性）
render_product() {
  local crf=$1
  "$FFMPEG_BIN" -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i "color=c=0x0B1220:s=${SW}x${SH}:r=${FPS}:d=${DUR}" \
    -vf "${CHAIN}" \
    -an -c:v libx264 -preset medium -crf "${crf}" -x264-params threads=4 \
    -movflags +faststart \
    "${TMP_DIR}/default.mp4"
}

# 用同一滤镜链渲染单帧（t=0 或 t=DUR，用于源级闭合校验）
render_probe_frame() {
  local setpts=$1 out=$2
  if [[ -n "${setpts}" ]]; then
    "$FFMPEG_BIN" -nostdin -hide_banner -loglevel error -y \
      -f lavfi -i "nullsrc=s=${SW}x${SH}:r=${FPS}:d=0.2" \
      -vf "setpts=${setpts},${CHAIN}" \
      -frames:v 1 -update 1 "${out}"
  else
    "$FFMPEG_BIN" -nostdin -hide_banner -loglevel error -y \
      -f lavfi -i "nullsrc=s=${SW}x${SH}:r=${FPS}:d=0.2" \
      -vf "${CHAIN}" \
      -frames:v 1 -update 1 "${out}"
  fi
}

# 从产物中抽取指定序号的帧（用于首尾帧闭合校验）
extract_frame() {
  local src=$1 n=$2 out=$3
  "$FFMPEG_BIN" -nostdin -hide_banner -loglevel error -y -i "${src}" \
    -vf "select='eq(n,${n})'" \
    -frames:v 1 -update 1 "${out}"
}

# 计算两张图片的平均 PSNR（dB）；完全一致时 ffmpeg 输出 inf
png_psnr() {
  local out val
  out=$("$FFMPEG_BIN" -nostdin -hide_banner -i "$1" -i "$2" -lavfi psnr -f null - 2>&1 || true)
  val=$(printf '%s\n' "${out}" | { grep -o 'average:[^ ]*' || true; } | tail -n1 | cut -d: -f2)
  [[ -n "${val}" ]] || die "无法从 ffmpeg 输出中解析 PSNR（对比 ${1} 与 ${2}）"
  printf '%s' "${val}"
}

# 断言 PSNR 达标（inf 视为通过）
assert_psnr() {
  local val=$1 threshold=$2 what=$3
  if [[ "${val}" != "inf" ]]; then
    awk -v v="${val}" -v t="${threshold}" 'BEGIN { exit !(v + 0 >= t) }' \
      || die "校验失败：${what}（PSNR=${val}dB，要求 ≥${threshold}dB）"
  fi
}

probe_field() { # $1=文件 $2=show_entries
  "$FFPROBE_BIN" -v error -select_streams v:0 -show_entries "$2" -of csv=p=0 "$1"
}

# ---------- 步骤 1/5：源级闭合自验（同一滤镜链重合成 t=0 与 t=DUR 两帧） ----------
info "步骤 1/5：渲染源级闭合校验帧（t=0 与 t=${DUR}s 各一帧，走完整滤镜链）"
render_probe_frame ""          "${TMP_DIR}/src_t0.png"
render_probe_frame "${DUR}/TB" "${TMP_DIR}/src_tend.png"
PS_SRC=$(png_psnr "${TMP_DIR}/src_t0.png" "${TMP_DIR}/src_tend.png")
assert_psnr "${PS_SRC}" 50 "源级闭合（t=0 vs t=${DUR}）"
info "源级闭合 PSNR = ${PS_SRC} dB（阈值 ≥50dB，inf 为完全一致）"

# ---------- 步骤 2/5：编码产物，CRF 自适应直到体积 ≤8MB ----------
info "步骤 2/5：编码产物（起始 crf=${CRF}，体积上限 ${MAX_BYTES} 字节）"
while :; do
  render_product "${CRF}"
  SIZE=$(file_size "${TMP_DIR}/default.mp4")
  if (( SIZE <= MAX_BYTES )); then
    info "体积达标：${SIZE} 字节（crf=${CRF}）"
    break
  fi
  if (( CRF >= 34 )); then
    die "即使 crf=${CRF} 体积仍为 ${SIZE} 字节，无法满足 ≤8MB，请检查滤镜链参数"
  fi
  info "体积 ${SIZE} 字节超限，提高 crf 至 $((CRF + 2)) 后重编码"
  CRF=$((CRF + 2))
done

# ---------- 步骤 3/5：产物首尾帧闭合自验 ----------
info "步骤 3/5：抽取产物首帧与末帧做闭合校验"
extract_frame "${TMP_DIR}/default.mp4" 0 "${TMP_DIR}/prod_first.png"
extract_frame "${TMP_DIR}/default.mp4" "$((TOTAL_FRAMES - 1))" "${TMP_DIR}/prod_last.png"
PS_PROD=$(png_psnr "${TMP_DIR}/prod_first.png" "${TMP_DIR}/prod_last.png")
assert_psnr "${PS_PROD}" 30 "产物首尾帧闭合"
info "产物首尾帧 PSNR = ${PS_PROD} dB（阈值 ≥30dB）"

# ---------- 步骤 4/5：ffprobe 规格自验 ----------
info "步骤 4/5：ffprobe 校验编码 / 分辨率 / 像素格式 / 时长 / 帧数 / 音频流"
PROD="${TMP_DIR}/default.mp4"
CODEC=$(probe_field "${PROD}" stream=codec_name)
PIX=$(probe_field "${PROD}" stream=pix_fmt)
WIDTH=$(probe_field "${PROD}" stream=width)
HEIGHT=$(probe_field "${PROD}" stream=height)
DURATION=$("$FFPROBE_BIN" -v error -show_entries format=duration -of csv=p=0 "${PROD}")
# nb_read_frames 需要显式 -count_frames（逐帧解码计数）才有值
NFRAMES=$("$FFPROBE_BIN" -v error -select_streams v:0 -count_frames -show_entries stream=nb_read_frames -of csv=p=0 "${PROD}")
ASTREAMS=$("$FFPROBE_BIN" -v error -select_streams a -show_entries stream=index -of csv=p=0 "${PROD}" | wc -l | tr -d ' ')

[[ "${CODEC}" == "h264" ]]                || die "规格校验失败：编码为 ${CODEC}，期望 h264"
[[ "${PIX}" == "yuv420p" ]]               || die "规格校验失败：像素格式为 ${PIX}，期望 yuv420p"
[[ "${WIDTH}" == "${OUT_W}" ]]            || die "规格校验失败：宽度为 ${WIDTH}，期望 ${OUT_W}"
[[ "${HEIGHT}" == "${OUT_H}" ]]           || die "规格校验失败：高度为 ${HEIGHT}，期望 ${OUT_H}"
[[ "${ASTREAMS}" == "0" ]]                || die "规格校验失败：产物包含 ${ASTREAMS} 路音频流，期望无声"
[[ -z "${NFRAMES}" || "${NFRAMES}" == "${TOTAL_FRAMES}" ]] \
  || die "规格校验失败：帧数为 ${NFRAMES}，期望 ${TOTAL_FRAMES}"
awk -v d="${DURATION}" -v want="${DUR}" 'BEGIN { exit !(d >= want - 0.05 && d <= want + 0.05) }' \
  || die "规格校验失败：时长为 ${DURATION}s，期望 ${DUR}s（±0.05s）"
info "规格校验通过：${CODEC}/${PIX} ${WIDTH}x${HEIGHT}，时长 ${DURATION}s，帧数 ${NFRAMES:-${TOTAL_FRAMES}}，音频流 ${ASTREAMS}"

# ---------- 步骤 5/5：全部自验通过，原子替换目标文件 ----------
info "步骤 5/5：全部自验通过，替换目标文件 ${OUT_FILE}"
mv -f "${PROD}" "${OUT_FILE}"
info "完成：${OUT_FILE}（$(file_size "${OUT_FILE}") 字节，crf=${CRF}）"
