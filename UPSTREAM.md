# Upstream sync policy (Crost fork)

This repository is a **rebase-style fork** of [macro-inc/macro](https://github.com/macro-inc/macro). We track upstream release tags and keep Crost changes as a **minimal additive patch series** on top.

## Rules

1. **Rebase, never merge upstream.** Each sync rebases the patch series onto the target upstream tag. No merge commits from `macro-inc/macro` into `main`.
2. **Minimal additive delta.** Prefer small, isolated commits that add Crost-specific files (`deploy/`, `scripts/`, `.github/workflows/ci.yml`, `research/`, etc.) or extend upstream surfaces without rewriting upstream modules. Feature work that belongs upstream should go to macro-inc first.
3. **Sync on release tags.** Upstream publishes tags like `v2026.8.14.0`. Sync to those tags, not arbitrary `main` SHAs, except for emergency hotfixes agreed in an issue.
4. **One patch series on `main`.** `main` = `<upstream-base>` + Crost commits. The current upstream base is recorded in `.upstream-base` (release tag when available, otherwise the upstream commit SHA — today `a69c3a1eb` because `v2026.8.14.0` is older than the fork point).
5. **Conflicts stop the sync.** If `scripts/sync-upstream.sh` cannot rebase cleanly, it aborts and leaves the working tree unchanged. Resolve conflicts manually, then re-run.

## Remotes

```bash
git remote add upstream https://github.com/macro-inc/macro.git   # once
# sync-upstream.sh fetches only the tag you pass; no full upstream branch fetch needed
```

`origin` is `Crost-AI/macro`. `upstream` is `macro-inc/macro`.

## Sync workflow

```bash
# List recent upstream release tags
git ls-remote --tags upstream 'v*' | tail

# Rebase the Crost patch series onto a tag (prints range-diff on success)
./scripts/sync-upstream.sh v2026.8.14.0
```

On success:

- Patch commits are replayed onto the tag.
- `git range-diff` compares the old patch series (`OLD_BASE..OLD_HEAD`) against the rebased series (`NEW_BASE..HEAD`).
- `.upstream-base` is updated to the new tag.

On conflict:

- The script runs `git rebase --abort`.
- Fix conflicts locally if needed, complete the rebase, update `.upstream-base`, and push.

## Standing deltas (do not resurrect on sync)

### GitHub Actions workflow slimming

Upstream ships ~35 workflow files under `.github/workflows/` (Fly deploys, Pulumi, FusionAuth, desktop builds, etc.). **This fork keeps only `ci.yml` (Crost CI).** All other upstream workflow files are intentionally deleted — disabling them in the GitHub UI is not durable; an upstream tag sync would restore them.

| Keep | Remove (standing delete) |
| --- | --- |
| `.github/workflows/ci.yml` | Every other file under `.github/workflows/` |
| Local actions Crost CI calls (`setup-nix`, `setup-nix-dev-shell`, `setup-reqs-web`, …) | Upstream deploy/preview/build workflows listed in the CROS-58 issue |

`scripts/sync-upstream.sh` aborts after a successful rebase if any disallowed workflow file is present. Re-apply the slimming commit or update this table before accepting new upstream workflows.

After merge to `main`, ensure Crost CI is enabled: `gh workflow enable "Crost CI"`.

## What belongs in the patch series

| In scope (Crost fork) | Out of scope (upstream or later waves) |
| --- | --- |
| `deploy/` self-host compose, backup, README | GitHub Issues 2-way sync (W2.7–W2.9) |
| `scripts/sync-upstream.sh`, `UPSTREAM.md` | Broker integration / webhooks glue |
| `research/` verification probes (W0.1) | Product feature changes without upstream path |
| `.github/workflows/ci.yml` (Crost CI gate) | Rewriting upstream service internals |
| Workflow slimming (delete upstream `.github/workflows/*` except `ci.yml`) | Restoring Fly/Pulumi/deploy workflows without a human decision |

## Review checklist

- [ ] `./scripts/sync-upstream.sh <tag>` completes without conflict
- [ ] `git range-diff` reviewed — no accidental upstream drift
- [ ] Crost CI (`.github/workflows/ci.yml`) green on the rebased branch
- [ ] `deploy/README.md` smoke path still valid after infra/env changes
