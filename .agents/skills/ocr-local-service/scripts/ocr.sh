#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage:" >&2
  echo "  ocr.sh health [--json]" >&2
  echo "  ocr.sh image FILE [--engine auto|paddle|glm|qwen] [--json]" >&2
  echo "  ocr.sh pdf FILE [--engine auto|paddle|glm|qwen] [--pages N|N-M] [--json]" >&2
  exit 2
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Required command not found: $1" >&2
    exit 127
  fi
}

require_command curl
require_command jq

operation="${1:-}"
[[ -n "$operation" ]] || usage
shift

base_url="${OCR_SERVICE_URL:-http://127.0.0.1:18100}"
timeout_secs="${OCR_TIMEOUT_SECS:-900}"
engine="auto"
page_range=""
json_output=false
file=""

if [[ "$operation" != "health" ]]; then
  file="${1:-}"
  [[ -n "$file" ]] || usage
  shift
  if [[ ! -f "$file" || ! -r "$file" ]]; then
    echo "File is not readable: $file" >&2
    exit 2
  fi
fi

while (($#)); do
  case "$1" in
    --engine)
      (($# >= 2)) || usage
      engine="$2"
      shift 2
      ;;
    --pages)
      (($# >= 2)) || usage
      page_range="$2"
      shift 2
      ;;
    --json)
      json_output=true
      shift
      ;;
    *)
      usage
      ;;
  esac
done

case "$engine" in
  auto|paddle|glm|qwen) ;;
  *)
    echo "Unsupported engine: $engine" >&2
    exit 2
    ;;
esac

if [[ "$operation" != "pdf" && -n "$page_range" ]]; then
  echo "--pages is only valid for PDF OCR" >&2
  exit 2
fi

response_file="$(mktemp -t ocr-service-response.XXXXXX)"
trap 'rm -f "$response_file"' EXIT

curl_common=(
  --silent
  --show-error
  --connect-timeout 5
  --max-time "$timeout_secs"
  --output "$response_file"
  --write-out '%{http_code}'
)

request_with_legacy_fallback() {
  local versioned_path="$1"
  local legacy_path="$2"
  shift 2

  local response_status
  response_status="$(curl "${curl_common[@]}" "$@" "$base_url$versioned_path")"
  if [[ "$response_status" == "404" ]]; then
    response_status="$(curl "${curl_common[@]}" "$@" "$base_url$legacy_path")"
  fi
  printf '%s' "$response_status"
}

case "$operation" in
  health)
    status="$(request_with_legacy_fallback "/v1/ocr/health" "/ocr/health")"
    ;;
  image)
    status="$(request_with_legacy_fallback "/v1/ocr/image" "/ocr/image" \
      --form "file=@$file" \
      --form "engine=$engine")"
    ;;
  pdf)
    form_args=(--form "file=@$file" --form "engine=$engine")
    if [[ -n "$page_range" ]]; then
      form_args+=(--form "page_range=$page_range")
    fi
    status="$(request_with_legacy_fallback "/v1/ocr/pdf" "/ocr/pdf" "${form_args[@]}")"
    ;;
  *)
    usage
    ;;
esac

if [[ "$status" -lt 200 || "$status" -ge 300 ]]; then
  if [[ "$operation" == "health" ]]; then
    jq . "$response_file" 2>/dev/null || sed -n '1,20p' "$response_file"
    exit 1
  fi
  message="$(jq -r '.error // empty' "$response_file" 2>/dev/null || true)"
  if [[ -n "$message" ]]; then
    echo "OCR service returned HTTP $status: $message" >&2
  else
    echo "OCR service returned HTTP $status:" >&2
    sed -n '1,20p' "$response_file" >&2
  fi
  exit 1
fi

if [[ "$operation" == "health" ]]; then
  jq . "$response_file"
  jq -e '(if has("backend_ready") then .backend_ready else .ollama end) == true' "$response_file" >/dev/null
elif [[ "$json_output" == true ]]; then
  jq . "$response_file"
else
  jq -er '.markdown' "$response_file"
fi
