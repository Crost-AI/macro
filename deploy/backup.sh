#!/usr/bin/env bash
# Backup Macro self-host data: Postgres dump + Docker volume archives.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

ENV_FILE="${MACRO_ENV_FILE:-deploy/.env}"
if [[ -f "$ENV_FILE" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

INSTANCE="${MACRO_INSTANCE:-selfhost}"
PORT_BASE="${MACRO_PORT_BASE:-31000}"
STACK_PROJECT="${COMPOSE_PROJECT_NAME:-macro-${INSTANCE}}"
BACKUP_DIR="${1:-deploy/backups/$(date -u +%Y%m%dT%H%M%SZ)}"

mkdir -p "$BACKUP_DIR"

log() {
  printf '[macro-backup] %s\n' "$*"
}

# xtask creates external volumes named macro_<kind>_<instance> (no compose prefix).
volume_name() {
  local base="$1"
  if [[ "$INSTANCE" == "macro" ]]; then
    echo "$base"
  else
    echo "${base}_${INSTANCE}"
  fi
}

postgres_container() {
  local id
  id="$(docker ps \
    --filter "label=com.docker.compose.project=${STACK_PROJECT}" \
    --filter "label=com.docker.compose.service=postgres" \
    -q | head -1)"
  if [[ -z "$id" ]]; then
    id="$(docker ps --filter "name=${STACK_PROJECT}-postgres" -q | head -1)"
  fi
  echo "$id"
}

log "writing backups to ${BACKUP_DIR}"

# --- Postgres (macrodb) via the running instance container ---
PG_CONTAINER="$(postgres_container)"
if [[ -z "$PG_CONTAINER" ]]; then
  echo "error: postgres container not found for project ${STACK_PROJECT}" >&2
  exit 1
fi

log "pg_dump via container ${PG_CONTAINER} -> ${BACKUP_DIR}/macrodb.sql"
docker exec "$PG_CONTAINER" pg_dump -U user macrodb > "${BACKUP_DIR}/macrodb.sql"

# --- External Docker volumes (xtask Instance::volume_*) ---
ARCHIVED=0
for vol in \
  "$(volume_name macro_postgres_data)" \
  "$(volume_name macro_redis_data)" \
  "$(volume_name macro_opensearch_data)" \
  "$(volume_name macro_kafka_data)" \
  "$(volume_name fusionauth_db_data)" \
  "$(volume_name fusionauth_config)"; do
  if docker volume inspect "$vol" >/dev/null 2>&1; then
    out="${BACKUP_DIR}/${vol}.tar.gz"
    log "volume ${vol} -> ${out}"
    docker run --rm \
      -v "${vol}:/data:ro" \
      -v "${BACKUP_DIR}:/backup" \
      alpine:3.20 \
      tar -czf "/backup/${vol}.tar.gz" -C /data .
    ARCHIVED=$((ARCHIVED + 1))
  else
    log "skip missing volume ${vol}"
  fi
done

cat > "${BACKUP_DIR}/manifest.txt" <<EOF
instance=${INSTANCE}
port_base=${PORT_BASE}
stack_project=${STACK_PROJECT}
timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)
postgres_container=${PG_CONTAINER}
volumes_archived=${ARCHIVED}
EOF

log "backup complete: ${BACKUP_DIR} (${ARCHIVED} volume archives + macrodb.sql)"
