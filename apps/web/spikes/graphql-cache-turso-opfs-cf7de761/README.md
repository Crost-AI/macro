# Turso fork `cf7de761` OPFS verification spike

Measured on 2026-08-11 in the repository direnv shell. This is a new,
self-contained adaptation of the real WP-02 Rust `turso_core::IO`/`File` OPFS
DedicatedWorker harness. It changes no prior spike, shared manifest, package
file, production source, or G0 decision document.

## Outcome

**The fork fixes the `BEGIN IMMEDIATE`/`BEGIN EXCLUSIVE` WASM trap in the
actually-run Chromium and Firefox builds, but Gate G0 remains NO-GO.**

The enumerated OPFS/lifecycle/recovery routes listed below completed in every
recorded Chromium and Firefox run with zero production-classified WASM
environment traps or unhandled failures. This is not a claim about unenumerated
SQL, VFS, or browser routes. The selected WP-04 SQL probe found a separate
conformance failure: after foreign keys were
disabled solely to insert a deliberate orphan, `PRAGMA foreign_key_check`
returned zero rows instead of the required violation row. The harness therefore
records `operationalPass: true`, but `pass: false` and `ConformanceFailure` for
each HEAD Chromium/Firefox run.

Actually run:

| Engine | Exact version | Repetitions | Result |
|---|---:|---:|---|
| Playwright Chromium | 145.0.7632.6 | 2 fresh-context cold + 2 same-context warm | OPFS/transaction/recovery operational pass; full SQL gate fails only deliberate `foreign_key_check` detection |
| Playwright Firefox | 146.0.1 | 2 fresh-context cold + 2 same-context warm | OPFS/transaction/recovery operational pass; same SQL gate failure |
| Playwright WebKit WPE | 26.0 | 1 honest cold run | `navigator.storage.getDirectory()` absent in its DedicatedWorker; OPFS suite cannot start |

WebKit WPE is not Safari, WKWebView, iOS Simulator, or a physical iOS target.
Its Safari-like user agent does not make it Safari evidence. No Safari or
WKWebView target was available or run.

## Exact source provenance and fork immutability

Read-only source checkout supplied for this verification:

```text
checkout: /home/sean/dev/turso-unused-temp-db-fix
branch:   fix/avoid-unused-temp-db-init
remote:   https://github.com/tursodatabase/turso.git
HEAD:     cf7de76172d61057007097e2dee7c47002cdc559
tree:     0adf7c52e8d139f9f24db9fdccd549afcc04a878
parent:   79163249538197d01dec5ea7f65519454ed792e2
parent tree: 27f512eb9ee8ccafcd2ab88ad268b9e267c6181f
```

The checkout was clean before and after all builds and browser runs and remained
at exact HEAD. Scripts use only read-only Git operations against it.
`measurements/generated/provenance.json` records the commit/tree/branch/remote
and clean-worktree result.

There is no committed absolute Cargo path. `Cargo.toml` depends on the relative,
ignored `.turso-source/core`. `scripts/materialize-turso.sh` requires
`TURSO_FORK`, checks exact clean HEAD and parent, then exports the selected
commit into that generated location. Parent differential builds overwrite only
that generated copy and restore HEAD afterward. The fork itself is never
checked out, patched, formatted, built in place, or otherwise modified.

The fork commit is not present on the recorded upstream remote, so reproduction
requires a checkout containing exact commit `cf7de761`; the source snapshot is
not vendored into this spike. This is an explicit reproducibility limitation,
not an implicit Cargo path dependency.

The spike itself pins `getrandom_backend="wasm_js"` in its own
`.cargo/config.toml`; scripted WASM builds validate and repeat that cfg while
adding `--remap-path-prefix` entries for the spike, Cargo home, host home, Nix
store, and rustc paths. `measurements/expected-toolchain.json` records these
settings. Artifact inspection scans the raw and wasm-bindgen WASM plus generated
JS/declarations: no host-sensitive `/home`, `/Users`, `/nix/store`, temporary,
or Windows build paths remain. Reproducible Cargo/rustc virtual paths are
inventoried separately rather than mislabeled as host leakage.

`scripts/verify-standalone-copy.sh` copies only source/committed evidence to a
fresh temporary directory outside the repository hierarchy, proves generated
source/build/package inputs are absent, materializes exact Turso source, and
runs locked native tests, WASM check/build, structural inspection, route
inspection, and artifact path scanning without inheriting repository-parent
Cargo config. Its deterministic result is committed as
`measurements/generated/standalone-copy.json`.

## Transaction fix and parent differential

HEAD executed both modes through a real Turso connection over the OPFS adapter:

1. `BEGIN IMMEDIATE`, insert, `COMMIT`, verify one row;
2. `BEGIN IMMEDIATE`, delete, `ROLLBACK`, verify the committed row remains;
3. the same commit/rollback sequence with `BEGIN EXCLUSIVE`.

Every one of the four Chromium and four Firefox HEAD runs passed both modes.
Worker and page trap accounting remained zero.

The same harness and artifact settings were then rebuilt against the exact
parent `79163249538197d01dec5ea7f65519454ed792e2`. Chromium and Firefox each
reproduced a `WebAssembly.RuntimeError` for both `IMMEDIATE` and `EXCLUSIVE`.
All four parent stacks retain:

```text
std::time::Instant::now
... Database::open_file_with_flags
... Connection::ensure_temp_database
... op_transaction
```

This differentially confirms the one-commit fork change removes the former
eager unopened-temp-database path for these exact transaction routes in the
recorded Chromium/Firefox builds. It does not establish safety for other SQL
that may open temp storage. Raw stacks are committed in
`measurements/generated/parent-differential.json`.

## Full cache SQL probe

The HEAD browser probe runs the WP-04 SQL families on the real OPFS main/WAL
files, not only parser preparation:

- all four cache tables and three metadata rows;
- `BEGIN IMMEDIATE`, `BEGIN EXCLUSIVE`, commit, rollback, DDL rollback, and
  rollback after a statement failure;
- numbered bound `TEXT`, `BLOB`, `INTEGER`, and `NULL` values;
- compound-key upsert/delete and affected-row checks;
- connection-local `last_insert_rowid()`, positive increasing `AUTOINCREMENT`
  IDs, and non-reuse after logical clear;
- canonical `(__typename || ':' || id) COLLATE BINARY` ordering, including
  prefix typenames (`Type`/`Type0`), an embedded-colon ID, a bound `IN` set,
  bound `LIMIT`, and an exclusive cursor;
- queue/layer `LEFT JOIN` consistency and orphan queries;
- strict-head selection, claim/defer fencing, complete/discard, and
  `ON DELETE CASCADE` cleanup;
- atomic record/queue/layer clear while preserving metadata;
- foreign-key enable/readback and direct FK constraint rejection;
- `PRAGMA quick_check` returning `ok`;
- `PRAGMA foreign_key_check` returning zero rows for a valid database; and
- a deliberate orphan to verify the non-empty `foreign_key_check` result shape.

Everything above passed except the last item. The harness now prepares and
decodes the exact four-column SQLite result contract—`table TEXT`, `rowid
INTEGER`, `parent TEXT`, `fkid INTEGER`—and requires exactly
`{table: "optimistic_layers", rowid: 9999999, parent: "mutation_queue", fkid:
0}`. With FK enforcement temporarily disabled, insertion of that one orphan
succeeded, but this Turso revision exposed zero result columns and returned
**0**, not **1**, decoded violation rows in every recorded Chromium/Firefox
run. Cleanup then removed the orphan and re-enabled enforcement. This remains a
real G0 failure: production corruption detection cannot silently accept an
empty result for an invalid database.

The probe exercises the complete selected SQL shape directly but is not a
production `cache-turso` implementation and does not run `cache-core` codecs or
the future shared `Storage` trait conformance suite. Payload BLOBs are sentinel
bytes, and error-taxonomy integration remains unimplemented.

## Persistence, kill, recovery, reset, and failures

Every recorded Chromium/Firefox run also completed these enumerated routes:

- same-worker close/reopen of the full cache SQL rows;
- clean shutdown and cross-worker reopen preserving three metadata and three
  canonical record rows, plus the independent `persisted-v1` marker;
- warm page reload observing the prior run's `recovered-fresh` marker before
  its explicit reset;
- exactly one exclusive Web Lock owner and a denied contender;
- serialized concurrent page RPCs;
- direct read/write/partial-write retry/empty write/short read/EOF/flush/
  truncate callback semantics;
- injected zero-progress, ordinary write, and storage-full errors with exact
  completion preservation;
- uncertain close poisoning with delete/reopen rejection;
- real non-empty-directory `removeEntry()` failure;
- real file-vs-directory recreation conflict;
- an active finite 10,000-commit BLOB loop terminated after the first committed
  event while its RPC was still pending;
- recovery observing `1 <= committed_rows < 10,000` and non-empty main/WAL;
- bounded Web Lock + pre-open + SQL count + close + delete + recreate recovery;
- deletion and zero-byte recreation of both main and WAL; and
- a fresh SQL database with zero old rows.

The first-commit event records main/WAL sizes and the actively killed worker's
production-trap, expected-negative-trap, and unhandled-failure counters before
the page terminates it. Every RPC response and worker event records those
counters, including lifecycle poison, actual `removeEntry`, recreation,
excluded-owner, recovery-candidate, and fresh-worker paths. Browser assertions
require every recorded production/control error to be non-trap and every
production counter to remain zero. Exact attempts, timings, rows, sizes, error
records, and per-worker observations are in `browser-matrix.json`.

## WASM, glue, import, and thread inspection

Current post-wasm-bindgen release artifact:

```text
bytes:                 8,823,799
sha256:                000c187e7f6247127603d57cdd8055ad8ae5ed5a5d6395cdd1a002708e11dffc
memory count:          1
initial memory:        47 pages / 3,080,192 bytes
maximum:               unset
shared:                false
memory64:              false
atomic operators:      0
web imports:           75 exact
missing/unexpected:    0 / 0
duplicate imports:     0
```

Inspection now requires exact equality with the committed import set; missing
imports fail just like unexpected or duplicate imports. Negative WAT fixtures
prove rejection of zero/multiple/shared/memory64 memories, atomics, and import
set differences. Suspicious thread, pthread, Emscripten, WASI, worker, and
shared-memory imports fail.

Generated glue and worker-source inspection rejects direct and dynamic Worker
construction, SharedWorker, `importScripts`, `SharedArrayBuffer`, Atomics,
`WebAssembly.Memory`, worker_threads, pthread markers, dynamic code, Node
runtime imports, and shared-memory options. Source inspection permits exactly
one lazy page-side DedicatedWorker constructor and requires the serialized
queue, transaction/full-SQL routes, finite active kill, bounded recovery,
failure probes, warm persistence, parent mode, and runtime trap accounting.
Runtime confirms no nested worker and no COOP/COEP or cross-origin isolation.

The binary still contains `time not implemented on this platform` / `not
implemented on this platform`, because Turso retains non-browser-safe clock code
for built-in temp `MemoryIO`. A negative-only `CREATE TEMP TABLE` export is run
in a separate disposable worker after all enumerated production routes. It
reliably traps through `std::time::Instant::now`, `ensure_temp_database`, and
`open_file_with_flags`; its dedicated expected-negative counter is exactly one
while production and unhandled counters remain zero. The worker is immediately
terminated and never reused. Thus only the enumerated HEAD routes have zero-trap
evidence; explicit temp is a demonstrated retained failure outside them.

## Remaining limitations

1. `PRAGMA foreign_key_check` failed to expose a deliberate FK violation.
2. WebKit WPE 26 lacks worker OPFS; Safari and WKWebView remain unrun.
3. The artifact is a spike module, not combined `cache-core` + adapter +
   production shell; no numeric download/startup/active-memory budget is
   approved.
4. Turso's `File` trait still has no close operation and OPFS deletion remains
   asynchronous; the consuming production lifecycle API is not frozen.
5. Real browser quota exhaustion, private mode, eviction, corrupted page bytes,
   Safari process loss, and device-level crash durability were not exercised.
6. Explicit temp storage demonstrably traps through built-in `MemoryIO` and
   `std::time::Instant`; no unenumerated SQL/temp route is approved.
7. Parent differential is intentionally limited to the transaction regression;
   the full HEAD matrix was not redundantly run against the known-failing
   parent.

## Gate G0 impact

The fork materially resolves the former `BEGIN IMMEDIATE` blocker and passes
the enumerated `BEGIN EXCLUSIVE` route in the recorded Chromium/Firefox runs.
The specifically listed WP-04 SQL shapes now have direct evidence; this does not
extend to unlisted SQL/temp/VFS routes or other browser versions.

**G0 remains STOP / NO-GO.** The newly discovered `foreign_key_check`
conformance failure is itself sufficient to block the required integrity/reset
contract. The approved Safari/WKWebView matrix, WebKit WPE capability, numeric
combined resource budgets, and frozen consuming lifecycle API also remain
open. This spike does not authorize WP-05 through WP-12 or any production
cutover.

## Reproduction

From the repository root:

```sh
TURSO_FORK=/home/sean/dev/turso-unused-temp-db-fix \
  direnv exec . bash -lc '\cd apps/web/spikes/graphql-cache-turso-opfs-cf7de761 && scripts/verify.sh --revision vswnnmxn'
apps/web/spikes/graphql-cache-turso-opfs-cf7de761/scripts/source-boundary.sh \
  --revision vswnnmxn
```

`verify.sh` also accepts `SOURCE_BOUNDARY_REVISION`. Without either override it
checks `latest(ancestors(@) & ~empty())`, so a clean empty Jujutsu child checks
its nearest non-empty committed ancestor. Supplying `vswnnmxn` makes the check
independent of unrelated pending integration-workspace changes.

Regenerate committed evidence:

```sh
TURSO_FORK=/home/sean/dev/turso-unused-temp-db-fix \
  direnv exec . bash -lc '\cd apps/web/spikes/graphql-cache-turso-opfs-cf7de761 && scripts/update-measurements.sh'
```

`scripts/verify.sh` also executes the clean standalone-copy proof. Dynamic raw
browser timestamps/timings are committed for provenance but are not
byte-compared. Deterministic provenance, artifact hash, artifact-path scan,
standalone proof, structural inspections, toolchain/build flags, and invariant
summary are regenerated and compared exactly.
