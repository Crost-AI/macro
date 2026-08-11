#!/usr/bin/env bash
set -euo pipefail

SPIKE_ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
OUTPUT="$SPIKE_ROOT/target/evidence/standalone-copy.json"
usage() {
  echo "usage: scripts/verify-standalone-copy.sh --source-repository <read-only-turso-worktree>" >&2
  exit 2
}
source_repository=
while (($#)); do
  case "$1" in
    --source-repository)
      (($# >= 2)) || usage
      source_repository=$2
      shift 2
      ;;
    *) usage ;;
  esac
done
[[ -n "$source_repository" ]] || usage
source_repository="$(builtin cd "$source_repository" && pwd -P)"

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
repository="$scratch/standalone-repository"
copy="$repository/apps/web/spikes/graphql-cache-turso-core-fix-verify"
mkdir -p "$copy"
tar -C "$SPIKE_ROOT" \
  --exclude='./target' \
  --exclude='./variants/parent/target' \
  --exclude='./variants/head/target' \
  --exclude='./measurements/standalone-copy.actual.json' \
  -cf - . | tar -xf - -C "$copy"

[[ "$copy" != "$SPIKE_ROOT" ]]
[[ ! "$copy" =~ ^$SPIKE_ROOT(/|$) ]]
[[ ! -e "$repository/.cargo/config.toml" ]]
[[ -f "$copy/.cargo/config.toml" ]]
for absent in target variants/parent/target variants/head/target; do
  [[ ! -e "$copy/$absent" ]] || {
    echo "standalone copy retained generated path: $absent" >&2
    exit 1
  }
done

jj git init --colocate "$repository" > /dev/null
jj -R "$repository" commit -m "standalone core verification input" > /dev/null
[[ -z "$(jj -R "$repository" diff --name-only)" ]]

before_head="$(GIT_OPTIONAL_LOCKS=0 git -C "$source_repository" rev-parse HEAD)"
before_status="$(GIT_OPTIONAL_LOCKS=0 git -C "$source_repository" status --porcelain=v1 --untracked-files=all)"
(
  builtin cd "$copy"
  CORE_FIX_VERIFY_STANDALONE_CHILD=1 \
    scripts/verify.sh --source-repository "$source_repository"
)
after_head="$(GIT_OPTIONAL_LOCKS=0 git -C "$source_repository" rev-parse HEAD)"
after_status="$(GIT_OPTIONAL_LOCKS=0 git -C "$source_repository" status --porcelain=v1 --untracked-files=all)"
[[ "$before_head" == "$after_head" && "$before_status" == "$after_status" ]]
[[ -z "$(jj -R "$repository" diff --name-only)" ]]

cmp "$copy/target/evidence/artifact-hashes.tsv" "$SPIKE_ROOT/target/evidence/artifact-hashes.tsv"
cmp "$copy/target/evidence/artifact-path-inspection.json" \
  "$SPIKE_ROOT/target/evidence/artifact-path-inspection.json"
cmp "$copy/target/evidence/toolchain.json" "$SPIKE_ROOT/target/evidence/toolchain.json"
jq -e '.hostSensitiveAbsolutePathFree == true and .hostSensitiveMatches == []' \
  "$copy/target/evidence/artifact-path-inspection.json" > /dev/null

mkdir -p "$(dirname "$OUTPUT")"
jq -n --arg revision "$after_head" '{
  cleanCopy: true,
  differentAbsolutePath: true,
  outsideRepositoryParentCargoConfig: true,
  generatedBuildInputsAbsent: true,
  spikeLocalCargoConfigPresent: true,
  cleanStandaloneJjRevision: true,
  fullVerifyScriptPassed: true,
  exactSourceBoundaryPassed: true,
  lockedNativeTestsPassed: true,
  lockedWasmBuildAndRuntimePassed: true,
  exactGeneratedEvidencePassed: true,
  artifactHashesMatchPrimaryBuild: true,
  artifactPathEvidenceMatchesPrimaryBuild: true,
  exactToolEvidenceMatchesPrimaryBuild: true,
  artifactHostSensitiveAbsolutePathFree: true,
  getrandomBackend: "wasm_js",
  deterministicRemapWrapperApplied: true,
  tursoRevision: $revision,
  tursoCheckoutUnmodified: true
}' > "$OUTPUT"

printf 'full verify.sh passed in a clean standalone copy outside repository Cargo config\n'
