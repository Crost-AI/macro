#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT="$SPIKE_ROOT/measurements/generated/standalone-copy.actual.json"
if [[ -z "${TURSO_FORK:-}" ]]; then
  echo "TURSO_FORK is required for standalone-copy verification" >&2
  exit 1
fi

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
copy="$scratch/graphql-cache-turso-opfs-cf7de761"
mkdir -p "$copy"
tar -C "$SPIKE_ROOT" \
  --exclude='./.turso-source' \
  --exclude='./.turso-source.tmp.*' \
  --exclude='./target' \
  --exclude='./target-parent' \
  --exclude='./pkg' \
  --exclude='./pkg-head' \
  --exclude='./measurements/generated/*.actual.json' \
  --exclude='./measurements/generated/*.actual.sha256' \
  -cf - . | tar -xf - -C "$copy"

for absent in .turso-source target target-parent pkg pkg-head; do
  [[ ! -e "$copy/$absent" ]] || {
    echo "standalone copy retained generated path: $absent" >&2
    exit 1
  }
done
[[ -f "$copy/.cargo/config.toml" ]]
[[ ! "$copy" =~ ^$SPIKE_ROOT(/|$) ]]

before_head="$(GIT_OPTIONAL_LOCKS=0 git -C "$TURSO_FORK" rev-parse HEAD)"
before_status="$(GIT_OPTIONAL_LOCKS=0 git -C "$TURSO_FORK" status --porcelain=v1 --untracked-files=all)"
(
  builtin cd "$copy"
  scripts/check-toolchain.sh
  scripts/materialize-turso.sh > /dev/null
  cargo test --locked
  scripts/cargo-wasm.sh check --locked --target wasm32-unknown-unknown \
    --no-default-features --features wasm-minimum
  scripts/build.sh
  scripts/inspect.sh
  node scripts/assert-runtime-routes.mjs
)
after_head="$(GIT_OPTIONAL_LOCKS=0 git -C "$TURSO_FORK" rev-parse HEAD)"
after_status="$(GIT_OPTIONAL_LOCKS=0 git -C "$TURSO_FORK" status --porcelain=v1 --untracked-files=all)"
[[ "$before_head" == "$after_head" && "$before_status" == "$after_status" ]]

artifact_scan="$(jq -c . "$copy/measurements/generated/artifact-path-inspection.actual.json")"
jq -n \
  --arg revision "$after_head" \
  --argjson artifactScan "$artifact_scan" \
  '{
    cleanCopy: true,
    outsideRepositoryParentConfig: true,
    generatedInputsAbsentBeforeBuild: true,
    spikeLocalCargoConfigPresent: true,
    lockedNativeTestsPassed: true,
    lockedWasmCheckPassed: true,
    lockedWasmBuildPassed: true,
    structuralInspectionPassed: true,
    runtimeRouteInspectionPassed: true,
    getrandomBackend: "wasm_js",
    remapPathPrefixApplied: true,
    artifactAbsolutePathScanPerformed: $artifactScan.absolutePathScanPerformed,
    artifactHostSensitiveAbsolutePathFree: $artifactScan.hostSensitiveAbsolutePathFree,
    tursoRevision: $revision,
    tursoCheckoutUnmodified: true
  }' > "$OUTPUT"

echo "clean standalone copy built and inspected without repository-parent Cargo config"
