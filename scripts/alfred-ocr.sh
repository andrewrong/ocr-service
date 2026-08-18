#!/usr/bin/env bash

# Capture a selected screen area, recognize it with the local OCR service,
# copy the Markdown result to the clipboard, and print it for Alfred.

set -uo pipefail

# Alfred starts with a minimal PATH. Include Homebrew and mise shims explicitly.
export PATH="$HOME/.local/share/mise/shims:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"

# Use Alfred-specific names for connection settings. Old workflow variables such
# as OCR_PORT=29100 must not override the current local service address.
ALFRED_OCR_HOST="${ALFRED_OCR_HOST:-127.0.0.1}"
ALFRED_OCR_PORT="${ALFRED_OCR_PORT:-18100}"
OCR_ENGINE="${OCR_ENGINE:-auto}"
OCR_TIMEOUT_SECS="${OCR_TIMEOUT_SECS:-120}"
OCR_URL="http://${ALFRED_OCR_HOST}:${ALFRED_OCR_PORT}/ocr/image"

SCREENCAPTURE_BIN="${SCREENCAPTURE_BIN:-/usr/sbin/screencapture}"
PBCOPY_BIN="${PBCOPY_BIN:-/usr/bin/pbcopy}"
JQ_BIN="${JQ_BIN:-$(command -v jq || true)}"

case "$OCR_ENGINE" in
  auto | paddle | glm | qwen) ;;
  *)
    echo "❌ 不支持的 OCR 模型: $OCR_ENGINE"
    echo "可用值: auto, paddle, glm, qwen"
    exit 1
    ;;
esac

if [[ -z "$JQ_BIN" || ! -x "$JQ_BIN" ]]; then
  echo "❌ 缺少 jq，无法解析 OCR 响应"
  echo "请通过 Homebrew 或 mise 安装 jq"
  exit 1
fi

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/alfred-ocr.XXXXXX")"
response_file="$tmp_dir/response.json"

cleanup() {
  rm -rf -- "$tmp_dir"
}
trap cleanup EXIT HUP INT TERM

if [[ -n "${OCR_SOURCE_FILE:-}" ]]; then
  screenshot_file="$OCR_SOURCE_FILE"
  if [[ ! -s "$screenshot_file" ]]; then
    echo "❌ 测试图片不存在或为空: $screenshot_file"
    exit 1
  fi
else
  screenshot_file="$tmp_dir/screenshot.png"
  if ! "$SCREENCAPTURE_BIN" -i "$screenshot_file" || [[ ! -s "$screenshot_file" ]]; then
    echo "❌ 截图失败或已取消"
    exit 1
  fi
fi

file_size="$(stat -f '%z' "$screenshot_file")"
echo "📸 截图完成: $((file_size / 1024))KB"
echo "🔍 正在识别（${OCR_ENGINE}）..."

curl_exit=0
http_status="$(curl \
  --silent \
  --show-error \
  --connect-timeout 5 \
  --max-time "$OCR_TIMEOUT_SECS" \
  --output "$response_file" \
  --write-out '%{http_code}' \
  --form "file=@${screenshot_file};type=image/png" \
  --form "engine=${OCR_ENGINE}" \
  "$OCR_URL")" || curl_exit=$?

if ((curl_exit != 0)); then
  echo "❌ OCR 服务连接失败（curl exit ${curl_exit}）"
  echo "服务地址: $OCR_URL"
  exit 1
fi

if [[ "$http_status" -lt 200 || "$http_status" -ge 300 ]]; then
  error_message="$("$JQ_BIN" -r '.error // empty' "$response_file" 2>/dev/null || true)"
  echo "❌ OCR 服务返回 HTTP ${http_status}"
  if [[ -n "$error_message" ]]; then
    echo "$error_message"
  fi
  exit 1
fi

text="$("$JQ_BIN" -r '.markdown // empty' "$response_file" 2>/dev/null || true)"
used_engine="$("$JQ_BIN" -r '.engine // empty' "$response_file" 2>/dev/null || true)"
duration_ms="$("$JQ_BIN" -r '.duration_ms // empty' "$response_file" 2>/dev/null || true)"

if [[ -z "$text" ]]; then
  echo "❌ OCR 响应中没有识别结果"
  exit 1
fi

printf '%s' "$text" | "$PBCOPY_BIN"

echo "✅ 已复制到剪贴板（${used_engine:-unknown}, ${duration_ms:-0}ms）"
echo
printf '%s\n' "$text"
