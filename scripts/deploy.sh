#!/usr/bin/env bash
# Deploy a commit of this repository, or roll back to the one before, and append one line per
# attempt to a record. See the Deploy section of README.md.
set -uo pipefail

usage() {
  cat >&2 <<'EOF'
usage:
  OCR_DEPLOY_SERVICE_DIR=<dir> OCR_DEPLOY_RECORD=<file> scripts/deploy.sh [<commit>]
  OCR_DEPLOY_SERVICE_DIR=<dir> OCR_DEPLOY_RECORD=<file> scripts/deploy.sh --rollback

  OCR_DEPLOY_SERVICE_DIR      directory with the Compose file of the service (required)
  OCR_DEPLOY_RECORD           file the record lines are appended to (required)
  OCR_DEPLOY_HEALTH_TIMEOUT   seconds to wait for the health check (default 180)
EOF
  exit 2
}

IMAGE=ocr-service
UNKNOWN=0000000
SERVICE_DIR=${OCR_DEPLOY_SERVICE_DIR:-}
RECORD=${OCR_DEPLOY_RECORD:-}
TIMEOUT=${OCR_DEPLOY_HEALTH_TIMEOUT:-180}

[[ -n $SERVICE_DIR && -n $RECORD ]] || usage
[[ $TIMEOUT =~ ^[0-9]+$ ]] || usage
TIMEOUT=$((10#$TIMEOUT))

ACTION=deploy
ARG=
case $# in
  0) ;;
  1)
    if [[ $1 == --rollback ]]; then
      ACTION=rollback
    elif [[ $1 == -* ]]; then
      usage
    else
      ARG=$1
    fi
    ;;
  *) usage ;;
esac

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
HEX='^[0-9a-f]{7,40}$'
FROM=$UNKNOWN
TO=$UNKNOWN

finish() {
  local result=$1 now
  now=$(date +%Y-%m-%dT%H:%M:%S%z | sed 's/\([0-9][0-9]\)$/:\1/')
  echo "deploy: $ACTION $result (from $FROM to $TO)" >&2
  if ! printf '%s %s %s %s %s\n' "$now" "$ACTION" "$FROM" "$TO" "$result" >>"$RECORD"; then
    echo "deploy: cannot append to the record $RECORD" >&2
    exit 1
  fi
  [[ $result == ok ]] && exit 0
  exit 1
}

# The commit running now: the revision label of ocr-service:local.
IMAGE_EXISTS=0
if docker image inspect "$IMAGE:local" >/dev/null 2>&1; then
  IMAGE_EXISTS=1
  label=$(docker image inspect --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' "$IMAGE:local" 2>/dev/null) || label=
  [[ $label =~ $HEX ]] && FROM=$label
fi

# Point ocr-service:local at an image, start it and wait for the health check.
start_and_wait() {
  (cd "$SERVICE_DIR" && docker compose up -d --no-build) || finish up-failed
  local deadline=$((SECONDS + TIMEOUT)) ids id status healthy
  while :; do
    ids=$(cd "$SERVICE_DIR" && docker compose ps -q 2>/dev/null) || ids=
    healthy=0
    if [[ -n $ids ]]; then
      healthy=1
      for id in $ids; do
        status=$(docker inspect --format '{{ .State.Health.Status }}' "$id" 2>/dev/null) || status=
        [[ $status == healthy ]] || healthy=0
      done
    fi
    [[ $healthy == 1 ]] && finish ok
    ((SECONDS >= deadline)) && finish unhealthy
    sleep 1
  done
}

if [[ $ACTION == rollback ]]; then
  target=
  if [[ -f $RECORD ]]; then
    target=$(awk '$2 == "deploy" && $5 == "ok" { t = $3 } END { print t }' "$RECORD")
  fi
  [[ $target =~ $HEX ]] || finish no-rollback-target
  TO=$target
  docker image inspect "$IMAGE:$target" >/dev/null 2>&1 || finish image-missing
  docker tag "$IMAGE:$target" "$IMAGE:local" || finish image-missing
  start_and_wait
fi

# Deploy. The commit asked for is recorded as given when it is plain hexadecimal.
[[ $ARG =~ $HEX ]] && TO=$ARG
git -C "$ROOT" fetch origin >&2 || finish fetch-failed
commit=$(git -C "$ROOT" rev-parse --verify --quiet "${ARG:-origin/master}^{commit}") || commit=
[[ $commit =~ ^[0-9a-f]{40}$ ]] || finish unknown-commit
TO=$commit

if [[ $IMAGE_EXISTS == 1 ]]; then
  docker tag "$IMAGE:local" "$IMAGE:$FROM" || finish build-failed
fi

# The build context is the archive of the commit, never the working tree.
git -C "$ROOT" archive "$commit" |
  docker build --label "org.opencontainers.image.revision=$commit" -t "$IMAGE:$commit" - ||
  finish build-failed
docker tag "$IMAGE:$commit" "$IMAGE:local" || finish build-failed
start_and_wait
