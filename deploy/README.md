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
3. **Disk & RAM** — allow ~20 GB free disk and 8 GB RAM for the full stack.

The compose runner installs **Nix** inside its container on first boot. You do not need Nix on the host.

### macOS notes

- Install [Docker Desktop](https://docs.docker.com/desktop/setup/install/mac-install/) or OrbStack.
- Default port base `31000` avoids macOS conflicts on 8080/8090 (see `research/macro-trial.md`).
- On Apple Silicon, `deploy/.env.example` documents an optional `CFLAGS` override for librdkafka cross-builds; the bootstrap script sets it automatically when Homebrew curl headers are present.

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

First boot takes several minutes (Nix shell, Rust cross-build, frontend bundle, infra init). When startup finishes, the orchestrator prints URLs:

| Service | URL |
| --- | --- |
| Macro app | `http://localhost:31009/app/` |
| Mailpit (login codes) | `http://localhost:31009/mailpit/` |
| FusionAuth | `http://localhost:31005` |
| LocalStack (S3) | `http://localhost:31006` |
| Postgres | `localhost:31000` (`macrodb`, user `user`, password `password`) |

### Login (first-run admin)

With `MACRO_SELFHOST_SEED=true` (default), the `team-perms` scenario creates persona accounts. After seeding, the orchestrator log includes login links such as:

`http://alice.localhost:31009/app/login?email=alice@seed.macro.local`

Open a link, request a code, and read the OTP from Mailpit.

To re-print links or check seed status:

```bash
nix develop --command just seed-scenario \
  --instance selfhost --port-base 31000 \
  status --file seed/scenarios/team-perms.json
```

### Stop

```bash
docker compose -f deploy/docker-compose.selfhost.yml down
```

Stack containers are managed by the inner orchestrator. To tear down the Macro service containers and volumes:

```bash
nix develop --command just stack down --instance selfhost --port-base 31000
```

## Backup

```bash
./deploy/backup.sh
# or: ./deploy/backup.sh /path/to/backup-dir
```

Writes:

- `macrodb.sql` — Postgres dump
- `*.tar.gz` — archives of Postgres, Redis, OpenSearch, Kafka, and FusionAuth volumes
- `manifest.txt` — instance metadata

## Health check

```bash
nix develop --command just stack status --json --instance selfhost --port-base 31000
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
| Stale containers from a prior run | `just stack down --instance selfhost --port-base 31000` then `docker compose -f deploy/docker-compose.selfhost.yml up` again. |

## CI

Pull requests run `.github/workflows/ci.yml` (Rust workspace build + SolidJS production build).
