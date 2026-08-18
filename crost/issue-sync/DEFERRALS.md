# W2.9 deferrals

Pinned deferrals for CROS-49 (review round 2).

## Macro task title/body/label updates (GitHub → Macro)

W2.4 exposes `POST /api/v1/tasks`, `GET /api/v1/tasks/{ref}`, `POST .../status`, `POST .../comment`, and `GET ...?assignee=` / `?label=`. There is **no** route to mutate title, body, or labels on an existing task.

This sync module applies GitHub **state** changes via `POST /api/v1/tasks/{ref}/status` and tracks title/body/label hashes locally so echo detection and per-field LWW stay correct. Pushing those fields into Macro awaits a W2.4/W2.9 REST extension (or a follow-on issue).

## Macro `/api/v1/tasks` + `/api/v1/docs` server surface

W2.4’s package comment assigns the Macro tasks/docs REST implementation to W2.9. This crate is the **sync consumer** only (calls the W2.4 contract; demo uses in-process fakes). Serving `/api/v1/tasks` and `/api/v1/docs/{ref}` on document-storage is deferred to a dedicated follow-on (or W5.2 `fakemacro` seed).
