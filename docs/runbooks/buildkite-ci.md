# Macro CI on Buildkite (CROS-63)

Remote CI for the Crost macro fork runs on the self-hosted Mac agent in org `crost-ai` (see `crost-config` `docs/runbooks/buildkite-agent-mac.md`).

**Pipeline:** [buildkite.com/crost-ai/macro-ci](https://buildkite.com/crost-ai/macro-ci)  
**Required GitHub check:** `buildkite/macro-ci`

## What runs

Two parallel steps (same gate as the slim Crost GitHub Actions CI):

1. `SQLX_OFFLINE=true cargo check --workspace --bins -j 2` (repo root)
2. `just build-dev` in `apps/web` (after `bun install --frozen-lockfile`)

Source of truth: `.buildkite/pipeline.yml`.

## Toolchain

**Canonical:** [Nix](https://nix.dev/install-nix) flake dev shell (`nix develop` / `nix print-dev-env`). The repo pins Rust, Bun, `just`, `wasm-pack`, and related inputs in `flake.nix` / `nix/`.

**Mac agent fallback:** when `nix` is not installed, `.buildkite/scripts/nix-dev-env.sh` requires these on `PATH`:

| Tool | Typical install on the Mac |
|------|----------------------------|
| `cargo` / `rustc` | rustup |
| `just` | Homebrew |
| `bun` | `~/.bun/bin` or Homebrew |
| `wasm-pack` | `cargo install wasm-pack` or Homebrew |

The agent launchd unit should expose `/nix/var/nix/profiles/default/bin`, `~/.cargo/bin`, and `~/.bun/bin` (see `PATH` in `.buildkite/pipeline.yml`).

## Local parity

Pre-push hooks mirror this gate (`./scripts/install-git-hooks.sh`). GitHub Actions `ci.yml` is **manual only** (`workflow_dispatch`); do not re-enable PR triggers.

## Operations

| Action | Command |
|--------|---------|
| Validate pipeline YAML | `bk pipeline validate -F .buildkite/pipeline.yml` |
| List recent builds | `bk build list --pipeline macro-ci --limit 10` |
| Re-run failed build | Buildkite UI or `bk build rebuild <build-url>` |
| Intentional fail demo | `bk build create -p macro-ci -b <branch> -e CI_FAIL_DEMO=true -m "fail demo"` |
