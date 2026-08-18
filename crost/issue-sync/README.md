# Crost GitHub Issues 2-way sync (W2.9)

Bidirectional sync between GitHub Issues and Macro tasks for linked projects.

## Features

- Per-project config: Macro `project_id` ↔ GitHub `owner/repo`
- Sync directions: issue↔task create, title/body, state (open/closed ↔ configured status), labels, comments
- Loop protection: hash/timestamp echo detection (standing `<!--macro-sync:…-->` markers do not block the other direction)
- Conflicts: last-writer-wins per field using stored timestamps
- Persistent sync-state SQLite database (`gh_issue` ↔ task id, field hashes)
- `backfill` command imports existing open GitHub issues

## Configuration

Copy `config.example.json` and set:

| Field | Purpose |
| --- | --- |
| `state_db_path` | SQLite database for sync links |
| `macro_base_url` | Macro document-storage base URL |
| `macro_service_token` | Bearer token for `/api/v1/tasks` (W2.4 contract) |
| `github_token` | GitHub REST token |
| `projects[]` | Project ↔ repo links and status mapping |

Environment: `CROST_ISSUE_SYNC_CONFIG=/path/to/config.json`

## Run

```bash
# Webhook ingress (GitHub + Macro outgoing webhooks from W2.7)
cargo run -p crost-issue-sync -- serve --config crost/issue-sync/config.example.json

# Initial import of open GitHub issues
cargo run -p crost-issue-sync -- backfill --config crost/issue-sync/config.example.json
```

Webhook endpoints:

- `POST /webhooks/github` — GitHub `issues` / `issue_comment` events
- `POST /webhooks/macro` — Macro `task.created`, `task.updated`, `task.comment`

## Demo

```bash
./crost/issue-sync/scripts/demo-issue-sync.sh
```

Runs an in-process integration test proving create/edit/close/comment on both sides converges without duplicates or echo loops, plus backfill import.

## Integration points

Upstream Macro files are untouched. This module is additive under `crost/issue-sync/` and consumes:

- Macro REST `/api/v1/tasks` (W2.4 client contract) — see `DEFERRALS.md` for server surface / GH→Macro field updates
- Macro outgoing webhooks (W2.7)
- GitHub Issues REST + webhooks
