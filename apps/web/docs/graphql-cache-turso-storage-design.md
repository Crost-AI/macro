# GraphQL cache Turso storage contract

Status: **Wave-0 WP-04 design; production implementation is gated on WP-01 and
Gate G0**

This document specifies the browser `cache-turso` implementation of
[`cache_core::store::Storage`](../../../crates/client/cache-core/src/store.rs).
It refines the storage portion of the
[worker migration plan](./graphql-cache-turso-worker-migration-plan.md). The
native [`cache-sqlite`](../../../crates/client/cache-sqlite/src/lib.rs) backend
is a semantic reference, not code to share, and remains unchanged.

## 1. Scope and invariants

The Turso database contains normalized records, the durable mutation queue,
optimistic layers, and storage metadata. All of it is disposable. A reset
never preserves queue rows or optimistic state.

This document covers:

- schema creation and compatibility validation;
- checked conversion between `EntityKey` and a compound SQL key;
- every `Storage` method's SQL, bindings, result conversion, ordering, and
  transaction boundary;
- full browser-database reset behavior; and
- the storage conformance suite required of WP-06.

It intentionally does **not** design IndexedDB handoff, mutation preservation
across reset, cache/API racing, npm integration, fallback backends, worker
election, or OPFS implementation details. The coordinator and OPFS packages
invoke the initialization/reset contract defined here.

The implementation has one Turso connection, owned by one elected dedicated
worker. That ownership rule does not replace SQL transactions: transactions
are still required for atomicity, crash behavior, and lease fencing.

## 2. Schema

`BROWSER_STORAGE_SCHEMA_VERSION` is a new integer constant owned by
`cache-turso`. It is independent of
`cache_core::codec::CACHE_FORMAT_VERSION` and
`CACHE_SCHEMA_COMPATIBILITY_EPOCH`.

The required schema is:

```sql
CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE records (
  __typename TEXT NOT NULL,
  id TEXT NOT NULL,
  value BLOB NOT NULL,
  PRIMARY KEY (__typename, id)
);

CREATE TABLE mutation_queue (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  query TEXT NOT NULL,
  operation_name TEXT,
  variables_json TEXT NOT NULL,
  identity TEXT,
  attempt_count INTEGER NOT NULL DEFAULT 0,
  next_attempt_at_ms INTEGER,
  lease_owner TEXT,
  lease_generation INTEGER NOT NULL DEFAULT 0,
  lease_expires_at_ms INTEGER,
  last_error TEXT,
  created_at_ms INTEGER NOT NULL
);

CREATE TABLE optimistic_layers (
  mutation_id INTEGER PRIMARY KEY,
  optimistic_data_json TEXT NOT NULL,
  normalized_updates BLOB NOT NULL,
  FOREIGN KEY (mutation_id) REFERENCES mutation_queue(id) ON DELETE CASCADE
);
```

No extra record index is required. The compound primary key supports exact
record lookup and a future exact-typename lookup. It does **not** by itself
provide canonical `EntityKey` order across multiple typenames; Section 4
specifies the required scan expression.

Required metadata rows are:

| `key` | `value` binding |
|---|---|
| `scope` | the opaque anonymous cache scope supplied by the owner |
| `namespace` | `cache_core::codec::cache_namespace(scope)` |
| `storage_schema_version` | decimal `BROWSER_STORAGE_SCHEMA_VERSION` |

The scope must not be logged or placed in telemetry. A future clean-shutdown
marker may add a metadata row, but is not part of this storage contract.

Records and `normalized_updates` use the existing postcard codecs
`encode_record`/`decode_record` and
`encode_record_updates`/`decode_record_updates`. Queue request fields and the
optimistic source remain text exactly as represented by `cache-core`.

### 2.1 SQL scalar conversions

All values are bound; payload values are never interpolated into SQL.

| Rust value | SQL value | Check on bind/read |
|---|---|---|
| strings | `TEXT` | valid Rust UTF-8; no content logging |
| postcard bytes | `BLOB` | decode failure is corrupt local state and requests full reset |
| `MutationId` | `INTEGER` | `1..=i64::MAX`; reject zero, negative rows, and overflow |
| `attempt_count: u32` | `INTEGER` | checked non-negative `u32` conversion |
| `lease_generation: u64` | `INTEGER` | checked `0..=i64::MAX` conversion |
| timestamps | `INTEGER` | direct `i64` conversion |
| `Option<T>` | value or `NULL` | preserve nullability exactly |

`AUTOINCREMENT` supplies positive, monotonically increasing queue IDs and does
not reuse an ID after `Storage::clear`. A physical full reset creates a new
database and may restart IDs at one; no command or claim from the old database
may be applied to the replacement engine.

## 3. Checked `EntityKey` conversion

The SQL key is `RecordKey { typename: String, id: String }`. Conversion must be
centralized in private helpers and used by every record method before a write
transaction starts.

### 3.1 `EntityKey` to SQL pair

1. The exact string `ROOT_QUERY` maps to
   `(__typename = "ROOT_QUERY", id = "")`.
2. Every other key must contain a colon and have a non-empty prefix.
3. Split with `split_once(':')`, never with an unrestricted split. The prefix
   is `__typename`; the entire suffix, including any further colons, is `id`.
4. Reconstruct the pair with the inverse conversion and require byte-for-byte
   equality with the input. Reject a key that fails this round trip.

Examples:

| Canonical `EntityKey` | `__typename` | `id` |
|---|---|---|
| `ROOT_QUERY` | `ROOT_QUERY` | empty string |
| `GraphqlSoupDocument:doc-1` | `GraphqlSoupDocument` | `doc-1` |
| `GraphqlSoupDocument:tenant:doc-1` | `GraphqlSoupDocument` | `tenant:doc-1` |
| `__meta:identity` | `__meta` | `identity` |

An empty ID suffix such as `Thing:` remains representable and round-trips; the
storage layer does not add a GraphQL-ID policy that `EntityKey` does not have.
The empty key, a non-root key without a colon, a key with an empty typename,
and `ROOT_QUERY:` are rejected. `ROOT_QUERY:` would otherwise collide with the
root pair and reconstruct as `ROOT_QUERY`.

### 3.2 SQL pair to `EntityKey`

1. Reject an empty `__typename`.
2. `("ROOT_QUERY", "")` reconstructs as `EntityKey::root()`.
3. Every other pair reconstructs as `format!("{__typename}:{id}")`.
4. Convert the result forward again and require the exact original pair.

This validates rows read from the database rather than trusting local bytes.
A malformed pair is corrupt local state and requests a full reset; it is not
returned as a different entity.

## 4. Canonical scan order and cursor semantics

`EntityKey` derives Rust `Ord`, so the contract is ascending bytewise order of
the complete canonical key string. SQLite/Turso `BINARY` collation over UTF-8
text provides that order.

`ORDER BY __typename, id` is **incorrect**. For example, tuple ordering puts
`Type:9` before `Type0:1`, while canonical key ordering puts `Type0:1` before
`Type:9` because `0` sorts before `:`. The current SQLite backend avoids this
because it orders one complete text key. The current IDB implementation sorts
typenames and then scans one prefix range at a time; that usually agrees, but
has the same `Type`/`Type0` edge case. The trait's complete-`EntityKey` order is
authoritative, so Turso must not copy IDB's per-typename concatenation.

Root storage is excluded from record scans, matching current `cache-core`,
IndexedDB, and SQLite behavior. For every non-root row, the canonical sort key
is:

```sql
(__typename || ':' || id) COLLATE BINARY
```

For two deduplicated requested typenames, a scan without a cursor is exactly:

```sql
SELECT __typename, id, value
FROM records
WHERE __typename IN (?1, ?2)
  AND NOT (__typename = 'ROOT_QUERY' AND id = '')
ORDER BY (__typename || ':' || id) COLLATE BINARY ASC
LIMIT ?3;
```

Bindings are `?1` and `?2` = requested typenames as `TEXT`, sorted and
deduplicated in Rust; `?3` = checked `limit` as `INTEGER`.

With an exclusive cursor it is:

```sql
SELECT __typename, id, value
FROM records
WHERE __typename IN (?1, ?2)
  AND NOT (__typename = 'ROOT_QUERY' AND id = '')
  AND ((__typename || ':' || id) COLLATE BINARY) > ?3
ORDER BY (__typename || ':' || id) COLLATE BINARY ASC
LIMIT ?4;
```

`?3` is the validated cursor's complete canonical `EntityKey` string and `?4`
is the checked limit. The implementation generates one placeholder per unique
typename; payload text is never interpolated. For no typenames or `limit == 0`,
it returns an empty vector without preparing SQL.

The same expression is used for both `>` and `ORDER BY`. This gives one global
exclusive cursor across all requested typenames, including the boundary
between typenames, with no duplicate or skipped row. Each result pair is
checked by the inverse conversion before its `value` is decoded.

## 5. Transaction driver

Private Turso helpers must bind `TEXT`, `BLOB`, `INTEGER`, and `NULL`, drive a
statement through all I/O/row states, and finalize/reset it before another
statement or transaction boundary is driven.

Write methods use:

```sql
BEGIN IMMEDIATE;
-- method statements
COMMIT;
```

A deterministic stale-claim or not-runnable result commits a transaction that
made no changes. Any statement, conversion, codec, or commit error attempts
`ROLLBACK`. A failed rollback, failed commit with uncertain outcome, or
unexpected Turso I/O state requests full reset.

Read methods that issue multiple statements use `BEGIN` and `COMMIT` for one
snapshot. A single `scan_records` statement already has a single statement
snapshot and needs no explicit transaction.

Key validation, queue-number conversion, and postcard encoding should happen
before `BEGIN IMMEDIATE` where possible. Postcard decoding happens while the
read snapshot is held. Empty write batches are successful no-ops.

## 6. `Storage` method mapping

### 6.1 `get_batch`

Boundary: one read transaction for the complete input batch.

Prepare once and execute once per validated pair:

```sql
SELECT value
FROM records
WHERE __typename = ?1 AND id = ?2;
```

Bindings: `?1` = typename `TEXT`, `?2` = ID `TEXT`.

The result vector has exactly the input length and order, including duplicate
keys. No row produces `None`; one row produces `Some(decode_record(value))`.
More than one row is impossible under the primary key and is an invariant
error.

### 6.2 `put_batch`

Boundary: one write transaction for the whole input vector.

Prepare once and execute in input order:

```sql
INSERT INTO records (__typename, id, value)
VALUES (?1, ?2, ?3)
ON CONFLICT (__typename, id) DO UPDATE SET value = excluded.value;
```

Bindings: checked typename and ID as `TEXT`; `encode_record(record)` as `BLOB`.
The last occurrence wins when an input batch repeats a key, matching current
backends. Any failure rolls back every upsert. Result is `()` after commit.

### 6.3 `delete_batch`

Boundary: one write transaction for the whole input slice.

Prepare once and execute in input order:

```sql
DELETE FROM records
WHERE __typename = ?1 AND id = ?2;
```

Bindings are the checked compound key. Missing and duplicate keys are ignored.
Any failure rolls back every deletion. Result is `()` after commit.

### 6.4 `scan_records`

Boundary: one read statement; no explicit transaction. SQL and bindings are
specified in Section 4.

Rows are returned as `(checked EntityKey, decode_record(value))` in global
ascending canonical-key order, after the exclusive cursor, capped at `limit`.
The input typename order and duplicates do not affect results.

### 6.5 `enqueue_mutation`

Boundary: one write transaction containing the queue row and optimistic row.

```sql
INSERT INTO mutation_queue (
  query, operation_name, variables_json, identity,
  attempt_count, next_attempt_at_ms, lease_owner,
  lease_generation, lease_expires_at_ms, last_error, created_at_ms
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11);
```

Bindings correspond in order to `StoredMutation`: four request fields,
checked `attempt_count`, retry time, lease owner, checked lease generation,
lease expiry, last error, and creation time. Optional values bind `NULL`.

After the insert, read the connection-local `last_insert_rowid` while the
transaction is active, check it is in `1..=i64::MAX`, then execute:

```sql
INSERT INTO optimistic_layers (
  mutation_id, optimistic_data_json, normalized_updates
) VALUES (?1, ?2, ?3);
```

Bindings: generated ID `INTEGER`, optimistic source `TEXT`, and
`encode_record_updates` bytes `BLOB`. Return the checked ID only after commit.
A failed layer insert leaves neither row.

### 6.6 `load_mutation_queue`

Boundary: one read transaction, because queue consistency uses two statements.

```sql
SELECT
  m.id, m.query, m.operation_name, m.variables_json, m.identity,
  m.attempt_count, m.next_attempt_at_ms, m.lease_owner,
  m.lease_generation, m.lease_expires_at_ms, m.last_error,
  m.created_at_ms, o.optimistic_data_json, o.normalized_updates
FROM mutation_queue AS m
LEFT JOIN optimistic_layers AS o ON o.mutation_id = m.id
ORDER BY m.id ASC;
```

A missing joined optimistic value is an inconsistent queue and requests full
reset. To detect an orphan layer as well:

```sql
SELECT o.mutation_id
FROM optimistic_layers AS o
LEFT JOIN mutation_queue AS m ON m.id = o.mutation_id
WHERE m.id IS NULL
LIMIT 1;
```

Any row is an inconsistent queue. Convert all IDs/counts/generations with the
checks in Section 2.1 and decode every updates BLOB. Return complete
`QueuedMutation` values in ascending ID order.

### 6.7 `claim_next_mutation`

Boundary: one write transaction containing strict-head selection, runnable
check, lease update, and optimistic-layer read.

Select the queue head without allowing a missing layer to hide it:

```sql
SELECT
  m.id, m.query, m.operation_name, m.variables_json, m.identity,
  m.attempt_count, m.next_attempt_at_ms, m.lease_owner,
  m.lease_generation, m.lease_expires_at_ms, m.last_error,
  m.created_at_ms, o.optimistic_data_json, o.normalized_updates
FROM mutation_queue AS m
LEFT JOIN optimistic_layers AS o ON o.mutation_id = m.id
ORDER BY m.id ASC
LIMIT 1;
```

If no queue row exists, confirm that no orphan optimistic row exists with:

```sql
SELECT mutation_id FROM optimistic_layers LIMIT 1;
```

No rows in either table returns `None`. A missing/orphan layer is an invariant
error and requests full reset.

Only the selected head is considered. Return `None` without updating when
`next_attempt_at_ms > request.now_ms` or
`lease_expires_at_ms > request.now_ms`. Equality is runnable. Never query for
a later runnable row.

Increment `attempt_count` with Rust `saturating_add(1)` and
`lease_generation` with Rust `saturating_add(1)`, then perform their checked
SQL integer conversions. Bind the request owner and expiry:

```sql
UPDATE mutation_queue SET
  attempt_count = ?2,
  next_attempt_at_ms = NULL,
  lease_owner = ?3,
  lease_generation = ?4,
  lease_expires_at_ms = ?5
WHERE id = ?1;
```

Exactly one affected row is required. `last_error` is retained. Commit and
return `Some(ClaimedMutation)` containing the updated mutation, the joined
optimistic layer, and the new generation.

### 6.8 `defer_mutation`

Boundary: one write transaction. Claim validation and update are one
conditional statement:

```sql
UPDATE mutation_queue SET
  next_attempt_at_ms = ?4,
  lease_owner = NULL,
  lease_expires_at_ms = NULL,
  last_error = ?5
WHERE id = ?1
  AND lease_owner = ?2
  AND lease_generation = ?3;
```

Bindings: checked ID, claim owner, checked generation, next-attempt timestamp,
and error text. One affected row commits and returns `true`; zero commits and
returns `false` for an absent or stale claim. Attempt count, generation,
request, and optimistic layer are unchanged. More than one affected row is an
invariant error.

### 6.9 `complete_mutation`

Boundary: one write transaction containing claim validation, every real record
upsert, and queue/layer removal.

First read the claim:

```sql
SELECT lease_owner, lease_generation
FROM mutation_queue
WHERE id = ?1;
```

No row or a non-matching owner/generation commits without writes and returns
`false`. For a current claim, require its optimistic layer:

```sql
SELECT 1
FROM optimistic_layers
WHERE mutation_id = ?1;
```

A missing layer is an invariant error and requests full reset. Execute the
same compound-key upsert as `put_batch` once per prevalidated/preencoded entry,
then:

```sql
DELETE FROM mutation_queue WHERE id = ?1;
```

Exactly one queue row must be deleted. With required foreign-key enforcement,
the optimistic row is deleted by `ON DELETE CASCADE`. Commit and return
`true`. A stale claim must never write response records; any later failure
rolls back both record writes and settlement.

### 6.10 `discard_mutation`

Boundary: one write transaction containing claim validation and queue/layer
removal.

Use the same claim query as `complete_mutation`. An absent/stale claim commits
and returns `false`. For a current claim, require the optimistic row and then
execute:

```sql
DELETE FROM mutation_queue WHERE id = ?1;
```

Exactly one deletion is required; the optimistic row cascades. Commit and
return `true`. No records change.

### 6.11 `clear`

Boundary: one write transaction covering all browser cache data tables.

```sql
DELETE FROM optimistic_layers;
DELETE FROM mutation_queue;
DELETE FROM records;
```

Delete the child table first even though cascading is enabled. Metadata and
SQLite's `sqlite_sequence` remain intact, so the open database stays
compatible and queue IDs are not reused. Result is `()` after commit.

`clear` is not the physical-reset procedure below. Both operations remove the
entire queue; neither preserves user mutations or optimistic layers.

## 7. Initialization and full reset

### 7.1 Fresh database

The OPFS owner tells `cache-turso` whether it created a fresh physical database.
On a fresh database:

1. Enable and read back foreign-key enforcement before beginning a transaction.
2. `BEGIN IMMEDIATE`.
3. Execute the four schema statements from Section 2.
4. Insert the three required metadata rows with separate bound statements:

   ```sql
   INSERT INTO meta (key, value) VALUES ('scope', ?1);
   INSERT INTO meta (key, value) VALUES ('namespace', ?1);
   INSERT INTO meta (key, value) VALUES ('storage_schema_version', ?1);
   ```

5. Commit, validate foreign keys, and expose `TursoStorage` only after all
   statements complete.

A crash before commit leaves no accepted partially initialized database. A
file that exists but is empty/partial on the next open is treated as
incompatible and physically reset.

### 7.2 Existing database

Before constructing the engine:

1. Enable and read back foreign-key enforcement.
2. Run the approved integrity check and require its sole success result.
3. Read required metadata:

   ```sql
   SELECT key, value
   FROM meta
   WHERE key IN ('scope', 'namespace', 'storage_schema_version')
   ORDER BY key ASC;
   ```

4. Require all three exact values for the current anonymous scope,
   `cache_namespace(scope)`, and storage schema version.
5. Run the foreign-key consistency check and require no violation rows.

A missing table/row, wrong value/type, schema SQL error, failed integrity check,
or foreign-key violation does not receive an in-place migration. It requests a
physical full reset. A valid clean reopen preserves records and the complete
queue.

### 7.3 Physical full reset

Full reset is an owner-level operation, not a SQL transaction:

1. stop accepting engine requests and reject affected in-flight requests;
2. drop/finalize every statement and close the Turso connection;
3. close every OPFS sync access handle while retaining exclusive ownership;
4. remove the main database and WAL files;
5. recreate/pre-register fresh files;
6. open Turso, initialize the schema and all metadata as a fresh database; and
7. construct a new empty engine before requests resume.

If close, removal, recreation, or initialization fails, surface a storage
initialization error. Do not retain or copy any rows, switch to IDB, or choose a
fallback backend.

Full reset is required for:

- storage schema, scope, or cache namespace mismatch;
- structural corruption, invalid compound-key rows, invalid queue numeric
  state, queue/layer inconsistency, or corrupt postcard payloads;
- an integrity or foreign-key check failure;
- quota/full-storage and OPFS failures that leave durability uncertain;
- a failed commit/rollback or unexpected Turso I/O state;
- logout/identity-directed wipe;
- abrupt or uncertain owner failover; and
- explicit test/debug reset.

Abrupt failover always deletes the old main and WAL files; it does not rely on
WAL recovery to preserve state. A graceful drain/close may reopen the valid
existing database without reset.

## 8. Existing backend semantics and required Turso SQL

The target keeps the `Storage` trait semantics, not either backend's physical
layout:

| Aspect | Native SQLite now | Browser IDB now | Browser Turso contract |
|---|---|---|---|
| record key | one canonical text key | one canonical IDB text key | checked compound `(__typename, id)` |
| record payload | postcard BLOB | postcard `Uint8Array` | same postcard BLOB |
| queue payload | relational columns; updates BLOB | `StoredMutation` and layer postcard values | relational columns; updates BLOB |
| queue ID range | checked SQLite signed integer | checked positive JS safe integer | checked positive SQLite signed integer |
| queue order/fencing | ascending ID, strict head, owner + generation | same | same |
| record scan | one global text-key range/order | sorted per-typename prefix scans | one global canonical-key expression/order |
| namespace mismatch | clears records, retains queue | clears records, retains queue | physical reset clears records and queue |
| scope mismatch / clear | clears all data tables, retains metadata | clears all data stores, retains metadata | clears all data tables; physical reset also replaces files |

The queue-preserving namespace behavior is deliberately **not** carried to the
disposable browser Turso database. Method-level behavior such as aligned batch
reads, atomic writes, strict-head blocking, lease fencing, and stale-claim
`false` results is carried over.

WP-01 must execute, not merely prepare, the required subset against the exact
pinned `turso_core` revision. WP-06 must not assume rusqlite behavior that the
spike did not prove.

### 8.1 Required SQL and core API behavior

| Current `cache-sqlite` usage | Turso contract / WP-01 proof |
|---|---|
| `CREATE TABLE`, `IF NOT EXISTS`, `TEXT`, `BLOB`, `INTEGER`, `NULL`, defaults | Execute the Section 2 shape; prove DDL transaction rollback and reopen. `IF NOT EXISTS` may be tested for API parity but fresh creation does not depend on it. |
| `INTEGER PRIMARY KEY AUTOINCREMENT` | Prove positive monotonic IDs, deletion/non-reuse, and checked connection-local `last_insert_rowid`. |
| foreign key with `ON DELETE CASCADE` | Required. Prove enforcement is connection-local, layer cascade works, and violations fail. |
| numbered positional bindings | Prove text/blob/integer/null binding, embedded colons, empty strings, large allowed integers, and statement reuse/reset. |
| `BEGIN`/transactions through rusqlite | Prove `BEGIN`, `BEGIN IMMEDIATE`, `COMMIT`, `ROLLBACK`, read snapshots, rollback after statement failure, and commit-error reporting. |
| record `INSERT ... ON CONFLICT ... DO UPDATE SET ... = excluded...` | Required with the compound `(__typename, id)` conflict target. |
| `SELECT`, `INSERT`, `UPDATE`, `DELETE` | Required, including reliable affected-row counts for fenced updates/deletes. |
| `ORDER BY ... ASC`, `LIMIT ?`, comparisons, `IN (...)` | Required for queue order and record scans. Prove a bound integer `LIMIT`. |
| `INNER JOIN` | Retained for ordinary relational checks. |
| new `LEFT JOIN` consistency queries | Required so a missing child cannot hide the strict queue head. |
| current single-key typename ranges (`key >= 'Type:' AND key < 'Type;'`) | Not retained. Compound records use bound `IN` filtering. |
| current `ORDER BY key` | Replaced by `(__typename || ':' || id) COLLATE BINARY`; prove exact Rust `String` order, including prefix typenames and colon IDs. |
| current metadata upsert | Not required for initialization. Incompatible metadata causes physical reset rather than in-place update. |
| rusqlite `prepare_cached`, `OptionalExtension`, and mutex | Library conveniences, not SQL requirements. Turso helpers may prepare/reuse statements but must preserve the stated results. |

WP-01 must also prove that stepping and finalizing statements in these
transactions does not require a nested worker, threads, or shared memory.

### 8.2 Required correctness pragmas/checks

These are correctness gates, not tuning:

```sql
PRAGMA foreign_keys = ON;
PRAGMA foreign_keys;
PRAGMA quick_check;
PRAGMA foreign_key_check;
```

WP-01 must verify the exact result shapes through `turso_core`: foreign keys
read back as enabled, `quick_check` returns exactly `ok` for a valid database,
and `foreign_key_check` returns no rows for a valid database and violation rows
for a deliberately invalid one. If the pinned revision does not implement one
of these checks, WP-01/WP-00 must approve and document an equally strong core
API before the schema is frozen; WP-06 must not silently omit validation.

### 8.3 Optional tuning pragmas

Native `cache-sqlite` currently sets:

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
```

They are **not** copied into the required Turso storage SQL by this design.
Turso's journal behavior, OPFS file model, flush guarantees, and worker-kill
tests must determine them in WP-01/WP-02 and Gate G0. If WP-00 chooses either
setting, initialization must read back the effective value and kill/reopen
tests must justify it.

Other tuning such as `busy_timeout`, cache size, temp storage, or automatic
checkpointing is also optional and outside the conformance contract. The
single-owner rule must not depend on `busy_timeout`.

## 9. Conformance suite

WP-06 should expose a backend factory so the same semantic tests run against
`InMemoryStorage`, `SqliteStorage`, and `TursoStorage` where the backend can
support the setup. Turso-only tests cover schema/reset and fault injection.
Every test starts from an isolated scope/database and must avoid logging
payloads.

### 9.1 Entity keys and records

- Round-trip `ROOT_QUERY`, an ordinary entity, `__meta:identity`, an empty ID,
  and IDs with one and multiple additional colons.
- Reject empty/missing typename forms, a non-root colonless key, and
  `ROOT_QUERY:`; reject malformed SQL pairs on read.
- Verify `get_batch` alignment with leading/middle/trailing misses and duplicate
  keys.
- Verify upsert overwrite and last-duplicate-wins behavior.
- Verify deleting absent/duplicate keys succeeds.
- Inject failure after the first item of multi-item put/delete and prove no
  partial batch is observable (or that uncertain I/O causes a full reset).
- Insert corrupt postcard bytes and require reset signaling, never a panic or
  partial result.

### 9.2 Scans

- Empty typename input and zero limit return empty.
- Input typename order and duplicates do not affect output.
- Non-selected types, `ROOT_QUERY`, and `__meta:identity` are excluded from a
  concrete-type scan.
- Ordinary and colon-containing IDs sort by the complete canonical key.
- Use prefix typenames such as `Type` and `Type0` to prove compound tuple order
  is not being used.
- Page before, within, and after a typename boundary with limits one and two;
  every cursor is exclusive and concatenated pages have no duplicate/gap.
- A cursor after all selected rows returns empty; a cursor belonging to an
  unselected typename still applies globally.
- Verify SQL `BINARY` results against a Rust-sorted `Vec<EntityKey>` containing
  the same rows.

### 9.3 Queue and leases

- Enqueue stores every nullable/non-null field and one optimistic layer; IDs
  are positive, increasing, and ordered after reopen.
- Failure between queue and layer insertion leaves neither row.
- Loading is ascending by ID and detects missing or orphan layers.
- Empty claim returns `None`; the oldest row is the only candidate.
- An actively leased or future-deferred head blocks all later rows.
- Retry/lease equality is runnable; an expired lease can be reclaimed.
- Each claim increments attempt count and generation and returns the updated
  row/layer; a previous owner/generation becomes stale.
- Defer with the current claim sets retry/error, clears owner/expiry, preserves
  generation/layer, and returns `true`; absent/stale claims return `false`
  without mutation.
- Complete with the current claim atomically upserts all real records and
  removes queue plus optimism. A stale claim writes no records.
- Discard with the current claim removes queue plus optimism and changes no
  records. A stale claim changes nothing.
- A commit/statement failure in complete/discard exposes neither partial record
  state nor half settlement; uncertain storage signals reset.
- Checked conversion rejects zero/negative IDs, oversized Rust IDs/generations,
  negative or oversized attempt counts, and arithmetic beyond the SQL integer
  range.

### 9.4 Clear, initialization, and reset

- `clear` atomically removes records, all queue rows, and all optimistic layers
  while preserving valid metadata; the next queue ID is not reused.
- Fresh initialization creates exactly the required schema/metadata and has
  foreign keys enabled.
- Clean close/reopen preserves records, queue order, lease state, and layers.
- Scope, namespace, and browser storage schema mismatches each cause a physical
  reset that removes records **and** queue state.
- Missing/partial metadata, malformed schema, failed integrity check,
  foreign-key inconsistency, and corrupt postcard data request full reset.
- Logout/test reset remove main and WAL files and recreate an empty database.
- Quota/full-storage, flush/commit uncertainty, and deletion/recreation failure
  produce deterministic classified errors; no fallback opens.
- Graceful close/reopen preserves state; abrupt owner loss starts empty and no
  old claim/RPC can settle against the new database.

### 9.5 Engine integration

Run `cache-core`'s engine tests over Turso for query write/read, identity
binding, cold record selection, optimistic hydration, queue retry, complete,
and discard. In particular, namespace/scope reset must hydrate zero optimistic
layers rather than attempting to re-normalize old queued source.

## 10. Gate G0 / integration questions

The schema is ready to freeze only after WP-01 answers:

1. Does the pinned Turso revision execute every required DDL/DML statement and
   return reliable affected-row and `last_insert_rowid` values?
2. Does `BEGIN IMMEDIATE` work through the intended async statement driver, and
   what outcome is reported when commit/rollback I/O fails?
3. Are foreign-key enablement, cascade, `quick_check`, and
   `foreign_key_check` implemented with the required result shapes?
4. Does `(__typename || ':' || id) COLLATE BINARY` exactly match Rust UTF-8
   `String` ordering in WASM, including prefix typenames and embedded colons?
5. Are dynamic `IN` bindings and bound `LIMIT` supported without a low variable
   limit that affects the maximum concrete-type set?
6. What journal mode does Turso core actually use over the approved OPFS
   adapter, and should WP-00 explicitly set WAL or synchronous behavior after
   kill/reopen tests?
7. Which Turso/core error codes identify corruption, full/quota storage, and
   uncertain commit/rollback strongly enough to request full reset without
   inspecting payload data?

Until those answers are recorded, required SQL marked above is a feasibility
contract, not evidence that the pinned core revision supports it.
