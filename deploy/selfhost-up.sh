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
STACK_PROJECT="${MACRO_STACK_PROJECT:-${COMPOSE_PROJECT_NAME:-macro-${INSTANCE}}}"

log() {
  printf '[macro-selfhost] %s\n' "$*"
}

export COMPOSE_PROJECT_NAME="$STACK_PROJECT"
export MACRO_ENV_FILE="${MACRO_ENV_FILE:-${REPO_ROOT}/deploy/.env}"
export MACRO_REPO_ROOT="$REPO_ROOT"

# docker compose run macro just … passes a command; do not re-run full stack up.
if [[ $# -gt 0 ]]; then
  exec "$@"
fi

# xtask always reads binaries from ./target/<triple>/debug (not CARGO_TARGET_DIR).
# When the repo is bind-mounted from macOS, stale host metadata in that tree breaks
# in-container zigbuild — drop only the linux cross-target dirs inside the runner.
if [[ -f /.dockerenv ]]; then
  # xtask probes curl http://localhost:<port-base+N>; stack ports publish on the host daemon.
  host_ip="$(getent ahostsv4 host.docker.internal 2>/dev/null | awk 'NR==1 {print $1}')"
  if [[ -n "$host_ip" ]]; then
    log "forwarding localhost:$PORT_BASE-$((PORT_BASE + 29)) -> ${host_ip} (compose runner)"
    for offset in $(seq 0 29); do
      port=$((PORT_BASE + offset))
      socat "TCP-LISTEN:${port},bind=127.0.0.1,reuseaddr,fork" "TCP:${host_ip}:${port}" &
    done
  fi
  # xtask zigbuild defaults to all CPUs + RUSTC_WRAPPER=sccache; both OOM small Docker VMs.
  export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
  runner_bin="/tmp/macro-runner-bin"
  mkdir -p "$runner_bin"
  cat > "${runner_bin}/sccache" <<'EOF'
#!/bin/sh
# Passthrough compile invocations; delegate sccache CLI to the real binary for doctor-local.
case "$1" in
  --version|-V|--show-stats|--start-server|--stop-server|-h|--help)
    exec /root/.cargo/bin/sccache "$@"
    ;;
  *)
    exec "$@"
    ;;
esac
EOF
  chmod +x "${runner_bin}/sccache"
  export PATH="${runner_bin}:${PATH}"
fi

# W0.1 gate-3: librdkafka needs curl headers even with WITH_CURL=0 (OAuth OIDC ifdef).
# Use -idirafter (not -I) so zig's libc++ headers stay ahead of system includes.
if [[ -z "${CFLAGS:-}" ]]; then
  case "$(uname -m)" in
    aarch64) curl_inc="/usr/include/aarch64-linux-gnu" ;;
    x86_64) curl_inc="/usr/include/x86_64-linux-gnu" ;;
    *) curl_inc="" ;;
  esac
  if [[ -n "$curl_inc" && -f "${curl_inc}/curl/curl.h" ]]; then
    export CFLAGS="-idirafter ${curl_inc}"
    export CXXFLAGS="-idirafter ${curl_inc}"
  elif [[ "$(uname -s)" == "Darwin" && -d /opt/homebrew/opt/curl/include ]]; then
    export CFLAGS="-idirafter /opt/homebrew/opt/curl/include"
    export CXXFLAGS="-idirafter /opt/homebrew/opt/curl/include"
  fi
fi

run_nix() {
  if command -v nix >/dev/null 2>&1; then
    nix develop --extra-experimental-features nix-command --extra-experimental-features flakes --command "$@"
  else
    "$@"
  fi
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
stack_args=(--no-doppler --instance "$INSTANCE" --port-base "$PORT_BASE")
if [[ -f "$ENV_FILE" ]]; then
  # Sourcing makes values available to this wrapper; --env-file is also required
  # because xtask intentionally ignores process-env keys it does not already know.
  stack_args+=(--env-file "$ENV_FILE")
fi
if [[ -f "${REPO_ROOT}/apps/web/dist/index.html" ]]; then
  log "using prebuilt frontend at apps/web/dist"
  stack_args+=(--frontend-dist "${REPO_ROOT}/apps/web/dist")
fi
run_nix just stack up "${stack_args[@]}"

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
