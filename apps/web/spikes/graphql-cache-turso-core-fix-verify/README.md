# Turso unused-temp database fix core verification

Measured in the repository direnv shell on 2026-08-11. This is a standalone
spike; it does not approve Gate G0 or change production/shared code.

## Outcome

**The narrow fix at `cf7de761` is verified. Gate G0 remains NO-GO.**

Exact comparison:

```text
parent: 79163249538197d01dec5ea7f65519454ed792e2
head:   cf7de76172d61057007097e2dee7c47002cdc559
```

- Native parent: unused `BEGIN IMMEDIATE` and `BEGIN EXCLUSIVE` both succeed,
  but `PRAGMA database_list` reports `main,temp`, proving the unwanted temp
  database was materialized.
- Native fixed HEAD: both succeed and report only `main`.
- WASM parent: five fresh Node processes each trap for each unused transaction
  mode. Every stack contains `ensure_temp_database` and
  `std::time::Instant::now`.
- WASM fixed HEAD: five fresh processes each pass each unused mode, report only
  `main`, and show calls through the supplied custom deterministic clock/I/O.
- The fixed HEAD runs the runnable WP-04 SQL/error suite successfully in five
  fresh WASM processes. The parent traps at the first required immediate
  transaction in all five runs.

This proves that HEAD skips transaction setup only for an unopened, unused temp
slot. It does not make Turso's built-in temp storage WASM-safe.

## Source provenance and read-only handling

`prepare-sources.sh` checks that the supplied worktree is clean, that HEAD and
its first parent are exactly the commits above, runs `git diff --check`, and
exports each immutable tree with `git archive` into ignored `target/sources`.
It checks the source worktree again afterward. Cargo builds only those exported
copies.

The exact trees and binary-diff hash are committed in
[`source-provenance.json`](measurements/generated/source-provenance.json):

```text
parent tree: 27f512eb9ee8ccafcd2ab88ad268b9e267c6181f
head tree:   0adf7c52e8d139f9f24db9fdccd549afcc04a878
diff sha256: b1701c65ce75654ed1cf7dac6bfe2cd2ecf63ec424dcfcf588033b1217254c46
```

Only three fork files differ:

- `core/translate/transaction.rs`
- `core/vdbe/execute.rs`
- `sqlite/conformance/sqlite-sqltests/transactions.sqltest`

The execution guard skips a temp `Transaction` instruction only when the temp
`Database` is unopened and the program neither reads nor writes temp. The
committed normalized source-risk counts/hashes show that HEAD does not alter
clock, random, filesystem, time, thread, WASI, or worker matches. Those broad
string scans are inventory, not proof of target behavior.

There is no committed absolute path dependency. The two locked variant
manifests use relative paths into ignored exported trees. No fork checkout,
root Cargo manifest/lock, web package manifest, or production file is modified.

## Reproducible WASM build isolation

The spike owns its WASM configuration rather than inheriting the repository
root's Cargo settings. [`.cargo/config.toml`](.cargo/config.toml) pins
`getrandom_backend="wasm_js"`. `scripts/cargo-wasm.sh` repeats that cfg through
`CARGO_ENCODED_RUSTFLAGS`, disables rustc wrappers and incremental compilation,
pins `SOURCE_DATE_EPOCH`, removes workspace-specific direnv/rpath inputs, and
orders remaps from broad host roots to the specific spike root. Rust source
locations therefore become stable `spike-src`, `cargo-home`, `nix-store`, and
`rustc` virtual paths.

Cargo also includes canonical manifest paths in crate disambiguators, outside
what source remapping alone normalizes. `scripts/build-wasm.sh` consequently
copies only the required spike/source inputs into a clean, fixed
`/tmp/macro-turso-core-fix-wasm-build-v1/apps/web/spikes/...` layout, normalizes
input timestamps, builds there without repository-parent Cargo discovery, and
copies only the raw WASM back. A lock prevents concurrent use of that fixed
build root, and the root is removed on exit.

Artifact inspection scans both raw modules plus every generated Node/web WASM,
JavaScript, and declaration file. `/home`, `/Users`, `/nix/store`, temporary,
build-root, Windows user/build, and incompletely remapped broad-root matches are
fatal. The committed scan reports zero host-sensitive matches. Exact versions
and resolved derivations for Rust, Cargo, wasm-bindgen, Node, jq, Git, Jujutsu,
tar, ripgrep, and sha256sum, together with all build flags, are validated
against [`expected-toolchain.json`](measurements/expected-toolchain.json) and
recorded in [`toolchain.json`](measurements/generated/toolchain.json).

Finally, `scripts/verify-standalone-copy.sh` copies no generated source/build
inputs into a fresh temporary Jujutsu repository at a different absolute path,
with no repository-root Cargo config, and runs the complete `verify.sh`. The
external run must reproduce the exact committed runtime/structural evidence and
byte-identical artifact hashes and path/tool evidence. Its deterministic result
is committed in
[`standalone-copy.json`](measurements/standalone-copy.json).

## Production-shaped custom I/O and clock

The harness does not use built-in `MemoryIO` as its Turso-facing `IO`. Its
`ProductionLikeIo`:

- registers one fixed main path and its WAL path;
- validates every open and returns stable path-derived file IDs;
- supplies deterministic monotonic and wall clocks safe on WASM;
- wraps synchronous `File` operations for exact operation counts and controlled
  error probes; and
- delegates only the byte store to Turso's memory `File` implementation.

Native ordering may request Turso's internal `tursodb_temp_file`; that scratch
path is explicitly allowed. On WASM Turso internally backs such scratch files
with memory. This is not an OPFS adapter and does not claim OPFS lifecycle or
browser recovery coverage.

## Explicit temp behavior

Explicit temp usage is tested separately so it cannot be mistaken for the
unused-temp fix:

- Native parent and HEAD both run `BEGIN IMMEDIATE`, create/use a temp table,
  commit row `1`, roll back row `2`, and return only row `1`; `database_list`
  honestly reports `main,temp`.
- On WASM, direct `CREATE TEMP TABLE` traps at `ensure_temp_database` and
  `std::time::Instant::now` on both revisions.
- On fixed HEAD, unused `BEGIN IMMEDIATE` succeeds first, but a subsequent temp
  create still traps. The dedicated probe preserves that distinction.
- Built-in `MemoryIO` itself still traps on both revisions.

Therefore explicit temp tables remain unsupported with this core-only browser
architecture unless Turso accepts a WASM-safe temp I/O/clock or otherwise fixes
that path.

## WP-04 contract execution

The harness executes the selected core-runnable storage SQL shapes, bindings,
result shapes, transaction boundaries, and classified failures against both
revisions natively and fixed HEAD on WASM:

- schema creation inside `BEGIN IMMEDIATE`, DDL rollback, metadata insert/read,
  clean close/drop/reopen, and `IF NOT EXISTS` parity;
- `TEXT`, `BLOB`, `INTEGER`, `NULL`, empty strings, embedded colons, `i64::MAX`,
  statement reset/reuse, and checked key/numeric rejection;
- aligned record reads, compound-key upsert, affected-row counts, absent and
  duplicate deletes;
- exact `(__typename || ':' || id) COLLATE BINARY` ordering, bound dynamic
  `IN`, bound `LIMIT`, exclusive cursors, `Type`/`Type0`, root exclusion, and
  Rust-string-order comparison;
- positive/non-reused `AUTOINCREMENT` IDs and connection-local
  `last_insert_rowid`;
- queue/layer atomic insertion, `INNER JOIN` and both required `LEFT JOIN`
  consistency queries, missing/orphan detection, ascending load;
- strict-head blocking, equality runnability, claim increments, conditional
  defer, stale complete/discard rejection, current complete/discard, record
  upsert, cascade, and `clear` preserving metadata/sequence;
- deferred and immediate commit/rollback, rollback after a statement error,
  and a two-connection WAL read snapshot;
- connection-local foreign-key enablement, enabled FK rejection, and cascade;
- storage-full write, uncertain commit-sync, corrupt-read, unique-constraint,
  foreign-key, conversion, and invalid-row classifications; and
- `PRAGMA quick_check` returning exactly one `ok` row before and after reopen.

Exact reports are
[`native-parent.json`](measurements/generated/native-parent.json) and
[`native-head.json`](measurements/generated/native-head.json). The repeated
WASM records, including normalized full trap stacks, are in
[`wasm-runtime-matrix.json`](measurements/generated/wasm-runtime-matrix.json).

### Required pragma failure

`PRAGMA foreign_key_check` is a **silent no-op** at both revisions:

1. it returns no rows for a valid database; and
2. after inserting a deliberate orphan with FK enforcement disabled on a
   second connection, it still returns no rows.

The harness records this as
`pragma_foreign_key_check_silent_noop`, not success. It intentionally emits no
`full_wp04_gate_passed` boolean. The narrower
`runnable_wp04_sql_passed` field is `true` only for the exercised core SQL,
quick-check, conversion, persistence, transaction, and classified-error
operations; it explicitly excludes the failed `foreign_key_check` contract and
all not-tested integration requirements.

Every report contains a `coverage_matrix` with `tested_passed`,
`tested_failed`, or `not_tested` for each enumerated requirement. The explicit
not-tested cases are rollback-I/O failure classification, consuming-application
reset after uncertain commit/rollback, physical reset for metadata/schema/
integrity/scope mismatch, cache-core codec corruption and shared `Storage`
trait conformance, and real OPFS quota/private-mode/eviction/crash durability.
The matrix therefore cannot be interpreted as a complete WP-04 or Gate G0
pass.

The controlled I/O failures prove the Rust variants surfaced by this revision:
`CompletionError::IOError(StorageFull)`, ordinary completion I/O error,
`Corrupt`, `Constraint`, and `ForeignKeyConstraint`. They do not prove real OPFS
quota behavior or recover the durability outcome of a failed commit. A distinct
rollback-I/O failure was not reachable with the synchronous memory-backed
`File`; production must still classify any failed rollback as reset-required.
WP-04's physical-reset policy remains required.

## Hardened WASM inspection

The wasmparser inspector examines raw Cargo, Node wasm-bindgen, and browser-web
wasm-bindgen modules. Its negative WAT tests reject zero/multiple memories,
shared memory, memory64, every threads-proposal atomic operator, and any
unexpected, missing, or duplicate import. Negative tests cover all three import
set differences as well as thread/worker/WASI imports. Generated web glue is separately
checked for clocks, random/crypto, filesystem, memory/thread constructs, WASI,
and workers.

Post-wasm-bindgen web modules:

| Property | Parent | Fixed HEAD |
|---|---:|---:|
| bytes | 8,789,333 | 8,788,289 |
| initial memory | 47 pages / 3,080,192 B | 47 pages / 3,080,192 B |
| memories | 1 | 1 |
| shared / memory64 | false / false | false / false |
| maximum | unset | unset |
| atomic operators | 0 | 0 |
| `memory.grow` operators | 1 | 1 |
| exact web imports | 33 allowed, 0 unexpected/missing/duplicate | 33 allowed, 0 unexpected/missing/duplicate |
| thread / WASI / worker imports | 0 / 0 / 0 | 0 / 0 / 0 |
| filesystem imports | 0 | 0 |

Both retain one `Date.now`/clock import and six random/crypto imports. Generated
web glue contains one `Date.now`, two `new Date`, three `getRandomValues`, and
one `randomFillSync` site. Those imports are explicitly allowlisted and
recorded; they are not hidden by the custom-clock success. The failing built-in
clock probes demonstrate that reachable built-in temp paths still call
`Instant::now`. Browser web glue has no filesystem module, WASI, worker,
SharedArrayBuffer, Atomics, shared-memory option, or memory64 construct. The
Node-only test loader uses `require('fs')` solely to load its local WASM file;
that import is absent from browser glue and from the WASM module.

See [`structural-summary.json`](measurements/generated/structural-summary.json),
[`artifact-hashes.tsv`](measurements/generated/artifact-hashes.tsv),
[`artifact-path-inspection.json`](measurements/generated/artifact-path-inspection.json),
and the exact 33-entry
[`expected-web-imports.tsv`](tools/inspect-wasm/expected-web-imports.tsv).

## Reproduction

From this spike directory in the repository dev shell:

```sh
scripts/verify.sh --source-repository "$TURSO_FORK_DIR"
```

`TURSO_FORK_DIR` must name a clean read-only worktree at exact HEAD
`cf7de761`. Verification prepares immutable source exports, validates exact tool
and WASM-build configuration, formats, runs both native suites, clean-builds
both WASM variants, executes 36 fresh-process runtime actions, scans artifacts
for host paths, verifies exact committed evidence, checks the amended
revision's full parent-to-revision source boundary, runs the same complete
verification from an external clean copy, and confirms the source worktree
remains clean at exact HEAD.

Pinned tools are Rust/Cargo 1.94.0, wasm-bindgen 0.2.121, Node 24.18.0, and the
exact supporting derivations recorded in the tool evidence. External Rust
dependencies are locked independently for parent and HEAD.

## Limitations and G0 effect

- Runtime WASM evidence is Node/V8, not an actual Chromium/Firefox/Safari worker.
- This does not rerun WP-02 OPFS or WP-03 coordinator/browser recovery tests.
- Explicit temp storage still traps on WASM and is outside the enumerated
  production cache SQL exercised here.
- `PRAGMA foreign_key_check` fails the required WP-04 gate silently.
- The artifacts include dedicated failing probes and are not production size
  artifacts; no wasm-opt or approved end-to-end resource budget is claimed.
- Broad source scans inventory risk strings; target artifact parsing and exact
  import allowlisting are the structural enforcement.
- No product browser matrix, OPFS consuming lifecycle, or resource-budget
  blocker is resolved here.

The fix removes the immediate/exclusive unused-temp trap for the tested custom
I/O routes. No claim is made about routes outside the runtime and coverage
matrices. It is suitable as a candidate input for further Gate G0 work, but it
does **not** change the recorded Gate G0 NO-GO decision by itself.
