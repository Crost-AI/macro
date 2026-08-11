#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
builtin cd "$SPIKE_ROOT"
scripts/generate-evidence.sh "$@"
scripts/collect-evidence.sh measurements/generated
scripts/verify-standalone-copy.sh "$@"
cp target/evidence/standalone-copy.json measurements/standalone-copy.json
printf 'updated committed verification and standalone-copy evidence\n'
