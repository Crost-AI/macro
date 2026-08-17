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
PROJECT="${COMPOSE_PROJECT_NAME:-macro-${INSTANCE}}"
BACKUP_DIR="${1:-deploy/backups/$(date -u +%Y%m%dT%H%M%SZ)}"
POSTGRES_PORT=$((PORT_BASE + 0))

mkdir -p "$BACKUP_DIR"

log() {
  printf '[macro-backup] %s\n' "$*"
}

log "writing backups to ${BACKUP_DIR}"

# --- Postgres (macrodb) ---
PG_URL="${MACRO_DB_URL:-postgres://user:password@localhost:${POSTGRES_PORT}/macrodb}"
log "pg_dump -> ${BACKUP_DIR}/macrodb.sql"
if command -v pg_dump >/dev/null 2>&1; then
  pg_dump "$PG_URL" > "${BACKUP_DIR}/macrodb.sql"
else
  docker run --rm --network host postgres:16-alpine \
    pg_dump "$PG_URL" > "${BACKUP_DIR}/macrodb.sql"
fi

# --- Named Docker volumes for this instance ---
volume_names() {
  local suffix="$1"
  if [[ "$INSTANCE" == "macro" ]]; then
    echo "$suffix"
  else
    echo "${suffix}_${INSTANCE}"
  fi
}

for vol in \
  "$(volume_names macro_postgres_data)" \
  "$(volume_names macro_redis_data)" \
  "$(volume_names macro_opensearch_data)" \
  "$(volume_names macro_kafka_data)" \
  "$(volume_names fusionauth_db_data)" \
  "$(volume_names fusionauth_config)"; do
  full="${PROJECT}_${vol}"
  if docker volume inspect "$full" >/dev/null 2>&1; then
    out="${BACKUP_DIR}/${vol}.tar.gz"
    log "volume ${full} -> ${out}"
    docker run --rm \
      -v "${full}:/data:ro" \
      -v "${BACKUP_DIR}:/backup" \
      alpine:3.20 \
      tar -czf "/backup/${vol}.tar.gz" -C /data .
  else
    log "skip missing volume ${full}"
  fi
done

cat > "${BACKUP_DIR}/manifest.txt" <<EOF
instance=${INSTANCE}
port_base=${PORT_BASE}
project=${PROJECT}
timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)
postgres_url=${PG_URL}
EOF

log "backup complete: ${BACKUP_DIR}"
