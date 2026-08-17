# Macro verification trial (W0.1)

Trial date: 2026-08-17  
Upstream revision: `macro-inc/macro` @ `main` (cloned locally)  
Stack instance: `crost-trial` with `--port-base 31000` (`just stack up --no-doppler`)

## Summary

| Gate | Verdict | Fork delta required |
| --- | --- | --- |
| G1 — 2-way GitHub **Issues** sync | **FAIL** (upstream: PR sync only) | **yes** — new Issues entity sync + `issues` webhook handling + outbound REST |
| G2 — Outgoing webhooks | **PASS** (with host-listener caveat) | **no** for basic delivery; **yes** for Crost broker ergonomics (stable public URL, validation on create) |
| G3 — Self-host viability | **PASS** (with manual host fixes) | **no** for core stack; **yes** for turnkey Crost compose (documented friction below) |
| G4 — Channel create/archive API | **PASS** | **no** — REST API exists; auth via user JWT |

Probe scripts and captured logs: `research/probes/` (run from repo root after stack is up).

---

## G1 — 2-way GitHub Issues sync

**Verdict:** FAIL on upstream Macro. GitHub integration is a **PR-centric sync app** (`pull_request`, `issue_comment` on PRs, reviews, `check_run`, `installation`). There is no handler for standalone `issues` events and no bidirectional Issues entity.

### Evidence

- Handled webhook types (`crates/github/src/domain/models/sync.rs`): `PullRequest`, `IssueComment`, `PullRequestReview`, `PullRequestReviewComment`, `CheckRun`, `Installation`, `Unknown`.
- Simulated `X-GitHub-Event: issues` POST to `POST /github/webhook` returned **HTTP 200** but is classified as `Unknown` — no Macro task/issue mirror.
- `--no-doppler` stubs all GitHub OAuth/app credentials; `/auth/link/github/status` → **404** `no github link found` for seeded user.
- No code path creates or mutates GitHub Issues from Macro tasks (only PR foreign entities + PR comments).

### Reproduction

```bash
# after stack + seed (see G3)
./research/probes/gate1-github-issues.sh | tee research/probes/out/gate1.log
```

### Gap (feeds W2.7–W2.9)

- Ingest `issues`, `issue_comment` (non-PR), `projects_v2_item` (if needed) webhooks.
- Map GitHub Issue ↔ Macro task (or dedicated issue entity) with loop protection (`crost-sync` marker).
- Outbound: create/update/close/comment via GitHub REST from Macro mutations.
- Real `GITHUB_*` + GitHub App install required (not available in `--no-doppler`).

**Fork delta required: yes** — full Issues sync is net-new on top of upstream PR sync.

---

## G2 — Outgoing webhooks

**Verdict:** PASS. Macro exposes webhook CRUD at `POST /webhook/webhooks` on document-storage. Local stack includes `sdk-webhook-relay` + SSH reverse tunnel to a host listener.

### Evidence

- Created webhook (`HTTP 201`) with `endpoint_url: http://sdk-webhook-relay:8787/macro-events`.
- `POST /webhook/webhooks/{id}/validate` → `is_valid: true`, `response_status: 200`.
- Host listener on `port-base+24` (31024) received signed `webhook.validation.test` delivery (see `research/probes/out/gate2.log`).
- **Caveat:** `just stack up` starts the SSH tunnel before any host listener binds; validation/delivery fail until the listener is up and the tunnel is restarted (probe script automates this).

### Reproduction

```bash
./research/probes/gate2-webhooks.sh | tee research/probes/out/gate2.log
```

**Fork delta required: no** for mechanism; **yes (scope notes)** for Crost broker: auto-start listener with stack, document relay URL, optional HMAC verify helper aligned with `crost-core` event kinds.

---

## G3 — Self-host viability

**Verdict:** PASS — full local stack boots and serves API after documented host workarounds.

### Manual fixes required (this trial host: macOS arm64, no Nix shell)

1. Install toolchain outside `nix develop`: `just`, `cargo-zigbuild`, `sqlx-cli`, `zig`, `sccache`, `rustup` + `aarch64-unknown-linux-gnu` target.
2. **rdkafka / librdkafka cross-compile:** `cargo zigbuild` failed with `curl/curl.h` missing until `export CFLAGS="-I/opt/homebrew/opt/curl/include"` (librdkafka `#ifdef WITH_OAUTHBEARER_OIDC` includes curl even when disabled).
3. **Frontend:** `bun install` in `apps/web/` before first `stack up` (`vite: command not found` otherwise).
4. **Ports:** use `--instance crost-trial --port-base 31000` to avoid macOS conflicts on 8080/8090.
5. **Seed:** `just seed-scenario --instance crost-trial --port-base 31000 apply --file seed/scenarios/team-perms.json` (sync_service 404 warnings during doc seed are benign locally).

### Boot result

- `just stack status --json` → `"backend_healthy": true`, 27 containers, proxy `http://localhost:31009`, storage `http://localhost:31015`.
- Auth via passwordless login; OTP returned inline in local env (`code` field in `POST /auth/login/passwordless` response).

### Reproduction

```bash
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
export CFLAGS="-I/opt/homebrew/opt/curl/include"
just doctor-local --instance crost-trial --port-base 31000
just stack up --no-doppler --instance crost-trial --port-base 31000
just seed-scenario --instance crost-trial --port-base 31000 apply --file seed/scenarios/team-perms.json
./research/probes/gate3-selfhost.sh | tee research/probes/out/gate3.log
```

**Fork delta required: no** for upstream self-host path; **yes (scope notes)** for Crost `crost-config` compose: pin port-base, bake curl/zigbuild deps, one-shot `crost macro up` wrapper.

---

## G4 — Channel create / archive via API

**Verdict:** PASS using user JWT (passwordless session access token) against document-storage REST.

### Evidence

- `POST /channels` → **200** `{"id":"<uuid>"}` (requires `participants` as `macro|<email>` list).
- `GET /channels/{id}` → **200** with participants.
- `DELETE /channels/{id}` → **200** `channel successfully deleted`.
- `GET` after delete → **404** `Channel not found`.
- `GET /auth/jwt/macro_api_token` returned **500** `unable to encode macro-api-token` on this stack (JWT path used instead).

### Reproduction

```bash
./research/probes/gate4-channels.sh | tee research/probes/out/gate4.log
```

**Fork delta required: no** — API contract sufficient for Crost broker; may want service-account token minting fix on local if `macro_api_token` is required by automation.

---

## References

- Local runbook: `docs/RUNNING_LOCALLY.md`
- GitHub sync router: `crates/github/src/inbound/github_sync_router/mod.rs` (`POST /webhook`)
- Outgoing webhooks OpenAPI: `packages/sdk/specs/storage.json` (`/webhook/webhooks`)
- Channels OpenAPI: `POST /channels`, `DELETE /channels/{channel_id}`
- SDK webhook relay: `tooling/xtask/crates/xtask_local/src/local/sdk_webhook.rs`
