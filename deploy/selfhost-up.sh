#!/usr/bin/env bash
# Bring up the Crost self-host Macro stack (wraps `just stack up` + seed).
# Invoked by deploy/docker-compose.selfhost.yml or directly from the repo root.
set -euo pipefail

REPO_ROOT="${MACRO_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$REPO_ROOT"

ENV_FILE="${MACRO_ENV_FILE:-${REPO_ROOT}/deploy/.env}"
if [[ -f "$ENV_FILE" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

INSTANCE="${MACRO_INSTANCE:-selfhost}"
PORT_BASE="${MACRO_PORT_BASE:-31000}"
SEED="${MACRO_SELFHOST_SEED:-true}"
SEED_FILE="${MACRO_SELFHOST_SEED_FILE:-seed/scenarios/team-perms.json}"
STACK_PROJECT="${COMPOSE_PROJECT_NAME:-macro-${INSTANCE}}"

export COMPOSE_PROJECT_NAME="$STACK_PROJECT"
export MACRO_ENV_FILE="${MACRO_ENV_FILE:-${REPO_ROOT}/deploy/.env}"
export MACRO_REPO_ROOT="$REPO_ROOT"

# W0.1 gate-3: librdkafka needs curl headers on macOS arm64 cross-builds.
if [[ -z "${CFLAGS:-}" && "$(uname -s)" == "Darwin" && -d /opt/homebrew/opt/curl/include ]]; then
  export CFLAGS="-I/opt/homebrew/opt/curl/include"
fi

run_nix() {
  if command -v nix >/dev/null 2>&1; then
    nix develop --extra-experimental-features nix-command --extra-experimental-features flakes --command "$@"
  else
    "$@"
  fi
}

log() {
  printf '[macro-selfhost] %s\n' "$*"
}

log "preflight (repo=${REPO_ROOT}, instance=${INSTANCE}, port-base=${PORT_BASE})"
run_nix just doctor-local --instance "$INSTANCE" --port-base "$PORT_BASE"

log "installing frontend deps (apps/web)"
if command -v bun >/dev/null 2>&1; then
  (cd apps/web && bun install --frozen-lockfile 2>/dev/null || bun install)
else
  run_nix bash -lc 'cd apps/web && bun install --frozen-lockfile 2>/dev/null || bun install'
fi

log "starting stack (no-doppler, headless)"
run_nix just stack up --no-doppler --instance "$INSTANCE" --port-base "$PORT_BASE"

if [[ "$SEED" == "true" || "$SEED" == "1" ]]; then
  log "seeding first-run admin scenario (${SEED_FILE})"
  if ! run_nix just seed-scenario --instance "$INSTANCE" --port-base "$PORT_BASE" apply --file "$SEED_FILE"; then
    log "warning: seed apply failed (stack may already be seeded)"
  fi
fi

PROXY_PORT=$((PORT_BASE + 9))
log "Macro is up:"
log "  app:      http://localhost:${PROXY_PORT}/app/"
log "  mailpit:  http://localhost:${PROXY_PORT}/mailpit/"
log "  status:   just stack status --json --instance ${INSTANCE} --port-base ${PORT_BASE}"

if [[ -n "${MACRO_SELFHOST_KEEPALIVE:-}" ]]; then
  log "keepalive enabled — tailing stack logs (Ctrl+C to detach; containers keep running)"
  docker compose -p "$STACK_PROJECT" logs -f proxy authentication-service document_storage_service
fi
