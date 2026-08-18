# Crost self-host Macro

Run a full Macro stack on a single machine with pinned ports, stubbed cloud integrations, object storage (LocalStack), Postgres, and a seeded admin world for first login.

This path closes the W0.1 gate-3 gaps: deterministic ports, bundled env stubs, LocalStack for S3, and an automated first-run seed.

## Prerequisites

1. **Docker Engine** with the Compose v2 plugin (`docker compose version`).
2. **Git** — clone this fork:
   ```bash
   git clone https://github.com/Crost-AI/macro.git
   cd macro
   ```
3. **Disk & RAM** — allow ~20 GB free disk. Give Docker Desktop **≥12 GB RAM** for the compose runner (first boot cross-compiles Rust services and may build the frontend inside the container).

The compose runner image ships Rust, zigbuild, bun, and `just`. First boot compiles service binaries and can take 15–30 minutes.

**Optional (avoids in-runner frontend OOM):** if you have [bun](https://bun.sh) on the host, build the static bundle before `docker compose up` — the orchestrator reuses `apps/web/dist` when present:

```bash
cd apps/web && bun install && \
  MODE=development NODE_ENV=production VITE_LOCAL_SERVERS=ALL \
  VITE_LOCAL_BACKEND_ORIGIN=same-origin VITE_AI_EDITING_WORKER_URL=/ai-editing \
  bun run --bun build
```

### macOS notes

- Install [Docker Desktop](https://docs.docker.com/desktop/setup/install/mac-install/) or OrbStack.
- Default port base `31000` avoids macOS conflicts on 8080/8090 (see `research/macro-trial.md`).
- On Apple Silicon, `deploy/.env.example` documents an optional `CFLAGS` override for librdkafka cross-builds; the bootstrap script sets it automatically when Homebrew curl headers are present (Linux runner images use Debian multiarch include paths).

## Configure

```bash
cp deploy/.env.example deploy/.env
```

Edit `deploy/.env` only if you need a different `MACRO_PORT_BASE`, want to disable auto-seed (`MACRO_SELFHOST_SEED=false`), or are enabling real integration keys.

## Start

From the repository root:

```bash
docker compose -f deploy/docker-compose.selfhost.yml up
```

The outer compose project is `crost-selfhost-runner` (the bootstrap container only). The Macro stack itself runs under the inner project `macro-selfhost` (instance `selfhost`).

First boot takes several minutes (toolchain setup, Rust cross-build, frontend bundle, infra init). When startup finishes, the orchestrator prints URLs:

| Service | URL |
| --- | --- |
| Macro app | `http://localhost:31009/app/` |
| Mailpit (login codes) | `http://localhost:31009/mailpit/` |
| FusionAuth | `http://localhost:31005` |
| LocalStack (S3) | `http://localhost:31006` |
| Postgres | `localhost:31000` (`macrodb`, user `user`, password `password`) |

Verify:

```bash
curl -fsS "http://localhost:31009/auth/health"
```

### Login (first-run admin)

With `MACRO_SELFHOST_SEED=true` (default), the `team-perms` scenario creates persona accounts. After seeding, the orchestrator log includes login links such as:

`http://alice.localhost:31009/app/login?email=alice@seed.macro.local`

Open a link, request a code, and read the OTP from Mailpit.

To re-print links or check seed status (runner image has `just`):

```bash
docker compose -f deploy/docker-compose.selfhost.yml run --rm --no-deps macro \
  just seed-scenario --instance selfhost --port-base 31000 \
  status --file seed/scenarios/team-perms.json
```

If the runner container is still attached (`up` without `-d`):

```bash
docker compose -f deploy/docker-compose.selfhost.yml exec macro \
  just seed-scenario --instance selfhost --port-base 31000 \
  status --file seed/scenarios/team-perms.json
```

### Stop

Stop the bootstrap container:

```bash
docker compose -f deploy/docker-compose.selfhost.yml down
```

Tear down the Macro stack (containers + volumes):

```bash
docker compose -f deploy/docker-compose.selfhost.yml run --rm --no-deps macro \
  just stack down --instance selfhost --port-base 31000
```

Or from the host when the inner project name is known:

```bash
docker compose -p macro-selfhost down -v
```

## Backup

```bash
./deploy/backup.sh
# or: ./deploy/backup.sh deploy/backups/manual
```

Writes `macrodb.sql` (via the running Postgres container) plus tarballs for the six external volumes xtask creates (`macro_postgres_data_<instance>`, etc.). Requires the stack to be running.

## Health check

```bash
docker compose -f deploy/docker-compose.selfhost.yml run --rm --no-deps macro \
  just stack status --json --instance selfhost --port-base 31000
curl -fsS "http://localhost:31009/auth/health"
curl -fsS "http://localhost:31009/app/" | head
```

## Upstream sync

See `UPSTREAM.md`. Rebase Crost patches onto a new upstream tag:

```bash
./scripts/sync-upstream.sh v2026.8.14.0
```

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| Port already in use | Change `MACRO_PORT_BASE` in `deploy/.env` (use a free 1000-port window). |
| `vite: command not found` on first boot | Wait for `bun install` in the orchestrator log, or run `cd apps/web && bun install` on the host and restart. |
| Seed failed but stack is healthy | Run the `seed-scenario apply` command from the Login section. |
| Stale containers from a prior run | `docker compose -p macro-selfhost down -v` then `docker compose -f deploy/docker-compose.selfhost.yml up` again. |
| `sccache: Compiler killed by signal 9` on first boot | Docker VM ran out of RAM during zigbuild. Give Docker Desktop ≥12 GB RAM, or set `CARGO_BUILD_JOBS=1` in `deploy/.env` and retry. |
| Mixed macOS/Linux `target/` after host `just stack up` | `rm -rf target/aarch64-unknown-linux-gnu target/zig-cache` then retry compose. |
| Runner exits immediately | Expected when keepalive is off; inner stack keeps running. Use `curl` / `docker compose -p macro-selfhost ps` to verify. |

Pull requests run `.github/workflows/ci.yml` (Rust workspace check + SolidJS production build).
