#!/usr/bin/env bash
# Gate 3: self-host viability — stack status + endpoint health.
set -euo pipefail
cd "$(dirname "$0")/../.."
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

INSTANCE="${INSTANCE:-crost-trial}"
PORT_BASE="${PORT_BASE:-31000}"

echo "=== gate3: doctor-local ==="
just doctor-local --instance "$INSTANCE" --port-base "$PORT_BASE"

echo
echo "=== gate3: stack status ==="
just stack status --json --instance "$INSTANCE" --port-base "$PORT_BASE"

echo
echo "=== gate3: endpoint probes ==="
for url in \
  "http://localhost:$((PORT_BASE + 9))/auth/health" \
  "http://localhost:$((PORT_BASE + 15))/health" \
  "http://localhost:$((PORT_BASE + 9))/app/" \
  "http://localhost:$((PORT_BASE + 8))/"; do
  code=$(curl -s -o /dev/null -w '%{http_code}' "$url" || true)
  echo "$code $url"
done
