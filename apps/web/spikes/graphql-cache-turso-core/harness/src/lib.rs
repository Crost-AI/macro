//! Reproducible Turso-core WASM feasibility harness for GraphQL cache storage.
//!
//! This is deliberately a standalone spike. It opens Turso core with its Rust
//! in-memory I/O implementation, exercises the proposed browser-cache schema,
//! bindings, row stepping, transactions, foreign keys, and explicit close, and
//! exports that same path through wasm-bindgen.

#![deny(missing_docs)]

use std::{
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use turso_core::{
    Clock, Connection, Database, File, LimboError, MemoryIO, MonotonicInstant, OpenFlags,
    OpenOptions, Result, Row, SqliteDialect, Statement, StepResult, Value, WallClockInstant, IO,
};
use wasm_bindgen::prelude::*;

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
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
"#;

/// Measurements and semantic checks returned by one SQL exercise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpikeSummary {
    /// Journal mode reported by the in-memory database.
    pub journal_mode: String,
    /// Numeric synchronous mode reported after setting `NORMAL`.
    pub synchronous: i64,
    /// Number of records after committed and rolled-back writes.
    pub record_count: i64,
    /// First auto-incremented mutation identifier.
    pub mutation_id: i64,
    /// Whether an orphan optimistic layer was rejected by foreign keys.
    pub foreign_key_rejected: bool,
    /// Whether deleting a mutation cascaded to its optimistic layer.
    pub cascade_deleted_layer: bool,
    /// Whether an explicit rollback discarded its record write.
    pub rollback_discarded_record: bool,
    /// Whether compound-key ordering and colon-containing IDs round-tripped.
    pub compound_key_scan_ok: bool,
    /// Whether strict-head selection, runnable checking, and lease fencing passed.
    pub strict_head_claim_ok: bool,
    /// Whether a competing stale deferred transaction was rejected or made busy.
    pub competing_connection_fenced: bool,
    /// Exact outcome observed when the stale deferred transaction tried to write.
    pub competing_connection_result: String,
}

impl SpikeSummary {
    fn as_report_line(&self) -> String {
        format!(
            "ok journal_mode={} synchronous={} record_count={} mutation_id={} foreign_key_rejected={} cascade_deleted_layer={} rollback_discarded_record={} compound_key_scan_ok={} strict_head_claim_ok={} competing_connection_fenced={} competing_connection_result={}",
            self.journal_mode,
            self.synchronous,
            self.record_count,
            self.mutation_id,
            self.foreign_key_rejected,
            self.cascade_deleted_layer,
            self.rollback_discarded_record,
            self.compound_key_scan_ok,
            self.strict_head_claim_ok,
            self.competing_connection_fenced,
            self.competing_connection_result,
        )
    }
}

/// In-memory Turso I/O with a WASM-safe deterministic clock.
///
/// Turso's built-in [`MemoryIO`] delegates to `std::time::Instant::now`, which
/// panics on `wasm32-unknown-unknown`. The browser OPFS adapter will need to
/// supply its own [`Clock`] implementation for the same reason.
struct SpikeMemoryIo {
    files: MemoryIO,
    monotonic_tick: AtomicU64,
}

impl SpikeMemoryIo {
    fn new() -> Self {
        Self {
            files: MemoryIO::new(),
            monotonic_tick: AtomicU64::new(0),
        }
    }
}

impl Clock for SpikeMemoryIo {
    fn current_time_monotonic(&self) -> MonotonicInstant {
        MonotonicInstant::from_nanos(self.monotonic_tick.fetch_add(1, Ordering::Relaxed) as u128)
    }

    fn current_time_wall_clock(&self) -> WallClockInstant {
        WallClockInstant {
            secs: 1_700_000_000,
            micros: 0,
        }
    }
}

impl IO for SpikeMemoryIo {
    fn open_file(&self, path: &str, flags: OpenFlags, direct: bool) -> Result<Arc<dyn File>> {
        self.files.open_file(path, flags, direct)
    }

    fn remove_file(&self, path: &str) -> Result<()> {
        self.files.remove_file(path)
    }

    fn file_id(&self, path: &str) -> Result<turso_core::io::FileId> {
        self.files.file_id(path)
    }

    fn supports_shared_wal_coordination(&self) -> bool {
        false
    }
}

fn open_database() -> Result<Arc<Database>> {
    let io: Arc<dyn IO> = Arc::new(SpikeMemoryIo::new());
    Database::open(io, ":memory:", OpenOptions::new(Arc::new(SqliteDialect)))
}

fn open_connection() -> Result<Arc<Connection>> {
    open_database()?.connect()
}

/// Open, connect, and explicitly close an in-memory Turso database.
pub fn exercise_open_close() -> Result<()> {
    open_connection()?.close()
}

/// Run the complete in-memory Turso schema and SQL exercise.
pub fn exercise_required_sql() -> Result<SpikeSummary> {
    let connection = open_connection()?;

    let exercise_result = exercise_connection(&connection);
    let close_result = connection.close();
    let mut summary = match (exercise_result, close_result) {
        (Ok(summary), Ok(())) => summary,
        (Err(error), _) | (Ok(_), Err(error)) => return Err(error),
    };
    summary.competing_connection_result = exercise_competing_connections()?;
    summary.competing_connection_fenced = true;
    Ok(summary)
}

fn exercise_connection(connection: &Arc<Connection>) -> Result<SpikeSummary> {
    connection.execute(SCHEMA)?;

    let foreign_keys = query_i64(connection, "PRAGMA foreign_keys")?;
    require(
        foreign_keys == 1,
        "PRAGMA foreign_keys did not remain enabled",
    )?;
    let journal_mode = query_string(connection, "PRAGMA journal_mode")?;
    let synchronous = query_i64(connection, "PRAGMA synchronous")?;
    require(synchronous == 1, "PRAGMA synchronous is not NORMAL")?;

    // BEGIN IMMEDIATE remains an unresolved blocker: at this revision it opens
    // an internal temp Database with built-in MemoryIO, whose std::time clock
    // panics on wasm32-unknown-unknown. Deferred transactions are tested only
    // under the production single-connection, serialized-call invariant.
    connection.execute("BEGIN")?;
    execute_bound(
        connection,
        "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        vec![Value::from_text("scope"), Value::from_text("anonymous-scope")],
    )?;
    execute_bound(
        connection,
        "INSERT INTO records (__typename, id, value) VALUES (?1, ?2, ?3) ON CONFLICT(__typename, id) DO UPDATE SET value = excluded.value",
        vec![
            Value::from_text("Document"),
            Value::from_text("doc:with:colons"),
            Value::from_blob(vec![1, 2, 3, 4]),
        ],
    )?;
    execute_bound(
        connection,
        "INSERT INTO records (__typename, id, value) VALUES (?1, ?2, ?3) ON CONFLICT(__typename, id) DO UPDATE SET value = excluded.value",
        vec![
            Value::from_text("ROOT_QUERY"),
            Value::from_text(""),
            Value::from_blob(vec![9, 8, 7]),
        ],
    )?;
    connection.execute("COMMIT")?;

    let scanned = query_bound(
        connection,
        "SELECT __typename, id, value FROM records WHERE (__typename > ?1 OR (__typename = ?1 AND id > ?2)) AND __typename IN (?3, ?4) ORDER BY __typename ASC, id ASC LIMIT ?5",
        vec![
            Value::from_text(""),
            Value::from_text(""),
            Value::from_text("Document"),
            Value::from_text("ROOT_QUERY"),
            Value::from_i64(10),
        ],
        |row| {
            Ok((
                row.get::<String>(0)?,
                row.get::<String>(1)?,
                row.get_value(2)
                    .to_blob()
                    .ok_or(LimboError::InvalidColumnType)?
                    .to_vec(),
            ))
        },
    )?;
    let compound_key_scan_ok = scanned
        == vec![
            (
                "Document".to_string(),
                "doc:with:colons".to_string(),
                vec![1, 2, 3, 4],
            ),
            ("ROOT_QUERY".to_string(), "".to_string(), vec![9, 8, 7]),
        ];
    require(compound_key_scan_ok, "compound-key scan mismatch")?;

    connection.execute("BEGIN")?;
    execute_bound(
        connection,
        "INSERT INTO mutation_queue (query, operation_name, variables_json, identity, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
        vec![
            Value::from_text("mutation UpdateDocument { updateDocument { id } }") ,
            Value::Null,
            Value::from_text("{}"),
            Value::from_text("opaque-session"),
            Value::from_i64(1_700_000_000_000),
        ],
    )?;
    let mutation_id = connection.last_insert_rowid();
    execute_bound(
        connection,
        "INSERT INTO optimistic_layers (mutation_id, optimistic_data_json, normalized_updates) VALUES (?1, ?2, ?3)",
        vec![
            Value::from_i64(mutation_id),
            Value::from_text("{\"updateDocument\":{\"id\":\"doc:with:colons\"}}"),
            Value::from_blob(vec![5, 6, 7]),
        ],
    )?;
    connection.execute("COMMIT")?;
    require(mutation_id == 1, "unexpected first AUTOINCREMENT id")?;

    let joined_queue = query_bound(
        connection,
        "SELECT m.id, o.optimistic_data_json, o.normalized_updates FROM mutation_queue m INNER JOIN optimistic_layers o ON o.mutation_id = m.id ORDER BY m.id ASC LIMIT ?1",
        vec![Value::from_i64(1)],
        |row| {
            Ok((
                row.get::<i64>(0)?,
                row.get::<String>(1)?,
                row.get_value(2)
                    .to_blob()
                    .ok_or(LimboError::InvalidColumnType)?
                    .to_vec(),
            ))
        },
    )?;
    require(
        joined_queue
            == vec![(
                mutation_id,
                "{\"updateDocument\":{\"id\":\"doc:with:colons\"}}".to_string(),
                vec![5, 6, 7],
            )],
        "queue INNER JOIN did not round-trip",
    )?;

    execute_bound(
        connection,
        "INSERT INTO mutation_queue (query, operation_name, variables_json, identity, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
        vec![
            Value::from_text("mutation LaterMutation { laterMutation { id } }"),
            Value::from_text("LaterMutation"),
            Value::from_text("{}"),
            Value::from_text("opaque-session"),
            Value::from_i64(1_700_000_000_001),
        ],
    )?;
    let later_mutation_id = connection.last_insert_rowid();
    execute_bound(
        connection,
        "INSERT INTO optimistic_layers (mutation_id, optimistic_data_json, normalized_updates) VALUES (?1, ?2, ?3)",
        vec![
            Value::from_i64(later_mutation_id),
            Value::from_text("{}"),
            Value::from_blob(vec![8]),
        ],
    )?;

    // This is the exact claim transaction required by TursoStorage: select the
    // strict queue head, check that it is runnable, then update it with both a
    // generation fence and a repeated strict-head/runnable SQL predicate.
    let claim_now_ms = 1_700_000_000_100;
    connection.execute("BEGIN")?;
    let head = select_strict_head(connection)?.ok_or_else(|| {
        LimboError::InternalError("strict-head SELECT returned no mutation".to_string())
    })?;
    require(
        head.id == mutation_id,
        "strict-head SELECT skipped the head",
    )?;
    require(
        claim_head_is_runnable(&head, claim_now_ms),
        "unclaimed strict head was not runnable",
    )?;
    let claimed = fenced_claim(
        connection,
        &head,
        "worker-1",
        claim_now_ms,
        1_700_000_060_000,
    )?;
    require(claimed == 1, "fenced claim UPDATE did not change one row")?;
    connection.execute("COMMIT")?;

    // A leased strict head blocks the later runnable mutation; selection never
    // scans forward looking for another candidate.
    connection.execute("BEGIN")?;
    let blocked_head = select_strict_head(connection)?.ok_or_else(|| {
        LimboError::InternalError("strict-head recheck returned no mutation".to_string())
    })?;
    require(blocked_head.id == mutation_id, "strict-head order changed")?;
    require(
        !claim_head_is_runnable(&blocked_head, claim_now_ms + 1),
        "active lease was considered runnable",
    )?;
    let later_runnable = query_bound(
        connection,
        "SELECT COUNT(*) FROM mutation_queue WHERE id = ?1 AND (next_attempt_at_ms IS NULL OR next_attempt_at_ms <= ?2) AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms <= ?2)",
        vec![
            Value::from_i64(later_mutation_id),
            Value::from_i64(claim_now_ms + 1),
        ],
        |row| row.get::<i64>(0),
    )? == vec![1];
    require(
        later_runnable,
        "later mutation was not independently runnable",
    )?;
    connection.execute("COMMIT")?;
    let strict_head_claim_ok = true;

    connection.execute("BEGIN")?;
    execute_bound(
        connection,
        "INSERT INTO records (__typename, id, value) VALUES (?1, ?2, ?3)",
        vec![
            Value::from_text("Document"),
            Value::from_text("rolled-back"),
            Value::from_blob(vec![0]),
        ],
    )?;
    connection.execute("ROLLBACK")?;
    let rollback_discarded_record = query_bound(
        connection,
        "SELECT COUNT(*) FROM records WHERE __typename = ?1 AND id = ?2",
        vec![
            Value::from_text("Document"),
            Value::from_text("rolled-back"),
        ],
        |row| row.get::<i64>(0),
    )? == vec![0];
    require(rollback_discarded_record, "ROLLBACK retained a record")?;

    let foreign_key_rejected = execute_bound(
        connection,
        "INSERT INTO optimistic_layers (mutation_id, optimistic_data_json, normalized_updates) VALUES (?1, ?2, ?3)",
        vec![
            Value::from_i64(9_999),
            Value::from_text("{}"),
            Value::from_blob(vec![0]),
        ],
    )
    .is_err();
    require(foreign_key_rejected, "foreign key accepted an orphan layer")?;

    execute_bound(
        connection,
        "DELETE FROM mutation_queue WHERE id = ?1",
        vec![Value::from_i64(mutation_id)],
    )?;
    let cascade_deleted_layer = query_bound(
        connection,
        "SELECT COUNT(*) FROM optimistic_layers WHERE mutation_id = ?1",
        vec![Value::from_i64(mutation_id)],
        |row| row.get::<i64>(0),
    )? == vec![0];
    require(cascade_deleted_layer, "ON DELETE CASCADE retained a layer")?;
    execute_bound(
        connection,
        "DELETE FROM mutation_queue WHERE id = ?1",
        vec![Value::from_i64(later_mutation_id)],
    )?;

    let record_count = query_i64(connection, "SELECT COUNT(*) FROM records")?;
    require(record_count == 2, "unexpected final record count")?;

    Ok(SpikeSummary {
        journal_mode,
        synchronous,
        record_count,
        mutation_id,
        foreign_key_rejected,
        cascade_deleted_layer,
        rollback_discarded_record,
        compound_key_scan_ok,
        strict_head_claim_ok,
        competing_connection_fenced: false,
        competing_connection_result: String::new(),
    })
}

#[derive(Debug)]
struct ClaimHead {
    id: i64,
    query: String,
    operation_name: Option<String>,
    variables_json: String,
    identity: Option<String>,
    attempt_count: i64,
    next_attempt_at_ms: Option<i64>,
    lease_owner: Option<String>,
    lease_generation: i64,
    lease_expires_at_ms: Option<i64>,
    last_error: Option<String>,
    created_at_ms: i64,
}

fn select_strict_head(connection: &Arc<Connection>) -> Result<Option<ClaimHead>> {
    let rows = query_bound(
        connection,
        "SELECT id, query, operation_name, variables_json, identity, attempt_count, next_attempt_at_ms, lease_owner, lease_generation, lease_expires_at_ms, last_error, created_at_ms FROM mutation_queue ORDER BY id ASC LIMIT 1",
        Vec::new(),
        |row| {
            Ok(ClaimHead {
                id: row.get(0)?,
                query: row.get(1)?,
                operation_name: nullable_string(row, 2)?,
                variables_json: row.get(3)?,
                identity: nullable_string(row, 4)?,
                attempt_count: row.get(5)?,
                next_attempt_at_ms: nullable_i64(row, 6)?,
                lease_owner: nullable_string(row, 7)?,
                lease_generation: row.get(8)?,
                lease_expires_at_ms: nullable_i64(row, 9)?,
                last_error: nullable_string(row, 10)?,
                created_at_ms: row.get(11)?,
            })
        },
    )?;
    Ok(rows.into_iter().next())
}

fn claim_head_is_runnable(head: &ClaimHead, now_ms: i64) -> bool {
    head.next_attempt_at_ms.is_none_or(|next| next <= now_ms)
        && head
            .lease_expires_at_ms
            .is_none_or(|expiry| expiry <= now_ms)
}

fn fenced_claim(
    connection: &Arc<Connection>,
    head: &ClaimHead,
    owner: &str,
    now_ms: i64,
    lease_expires_at_ms: i64,
) -> Result<i64> {
    // Touch all selected fields so this probe also validates the complete row
    // shape that production maps into StoredMutation before issuing the claim.
    require(!head.query.is_empty(), "strict head query was empty")?;
    require(
        !head.variables_json.is_empty(),
        "strict head variables were empty",
    )?;
    let _selected_metadata = (
        &head.operation_name,
        &head.identity,
        &head.lease_owner,
        &head.last_error,
        head.created_at_ms,
    );

    execute_bound(
        connection,
        "UPDATE mutation_queue SET attempt_count = ?2, next_attempt_at_ms = NULL, lease_owner = ?3, lease_generation = ?4, lease_expires_at_ms = ?5 WHERE id = ?1 AND lease_generation = ?6 AND id = (SELECT id FROM mutation_queue ORDER BY id ASC LIMIT 1) AND (next_attempt_at_ms IS NULL OR next_attempt_at_ms <= ?7) AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms <= ?7)",
        vec![
            Value::from_i64(head.id),
            Value::from_i64(head.attempt_count + 1),
            Value::from_text(owner.to_owned()),
            Value::from_i64(head.lease_generation + 1),
            Value::from_i64(lease_expires_at_ms),
            Value::from_i64(head.lease_generation),
            Value::from_i64(now_ms),
        ],
    )
}

fn exercise_competing_connections() -> Result<String> {
    let database = open_database()?;
    let first = database.connect()?;
    let second = database.connect()?;
    first.execute(SCHEMA)?;
    execute_bound(
        &first,
        "INSERT INTO mutation_queue (query, operation_name, variables_json, identity, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
        vec![
            Value::from_text("mutation CompetingClaim { competingClaim { id } }"),
            Value::Null,
            Value::from_text("{}"),
            Value::Null,
            Value::from_i64(1_700_000_000_000),
        ],
    )?;

    // Two deferred readers can both observe generation zero. The first writer
    // wins; the stale writer must either receive Busy while upgrading its
    // snapshot or affect zero rows because of the generation/runnable fence.
    first.execute("BEGIN")?;
    second.execute("BEGIN")?;
    let first_head = select_strict_head(&first)?.ok_or_else(|| {
        LimboError::InternalError("first competing reader saw no head".to_string())
    })?;
    let second_head = select_strict_head(&second)?.ok_or_else(|| {
        LimboError::InternalError("second competing reader saw no head".to_string())
    })?;
    require(
        first_head.lease_generation == 0 && second_head.lease_generation == 0,
        "competing readers did not observe the same generation",
    )?;
    require(
        fenced_claim(&first, &first_head, "winner", 10, 100)? == 1,
        "first competing claim did not win",
    )?;
    first.execute("COMMIT")?;

    let stale_result = match fenced_claim(&second, &second_head, "stale", 10, 100) {
        Ok(0) => {
            second.execute("COMMIT")?;
            "zero_rows"
        }
        Ok(_) => {
            second.execute("ROLLBACK")?;
            return Err(LimboError::InternalError(
                "stale competing claim unexpectedly succeeded".to_string(),
            ));
        }
        Err(LimboError::Busy) => {
            second.execute("ROLLBACK")?;
            "busy"
        }
        Err(LimboError::BusySnapshot) => {
            second.execute("ROLLBACK")?;
            "busy_snapshot"
        }
        Err(error) => return Err(error),
    };
    let final_lease = query_bound(
        &first,
        "SELECT lease_owner, lease_generation, attempt_count FROM mutation_queue ORDER BY id ASC LIMIT 1",
        Vec::new(),
        |row| {
            Ok((
                nullable_string(row, 0)?,
                row.get::<i64>(1)?,
                row.get::<i64>(2)?,
            ))
        },
    )?;
    let winner_preserved = final_lease == vec![(Some("winner".to_string()), 1, 1)];

    first.close()?;
    second.close()?;
    require(
        winner_preserved,
        "stale claimant replaced the winning lease",
    )?;
    Ok(stale_result.to_string())
}

fn nullable_i64(row: &Row, index: usize) -> Result<Option<i64>> {
    match row.get_value(index) {
        Value::Null => Ok(None),
        value => value
            .as_int()
            .map(Some)
            .ok_or(LimboError::InvalidColumnType),
    }
}

fn nullable_string(row: &Row, index: usize) -> Result<Option<String>> {
    match row.get_value(index) {
        Value::Null => Ok(None),
        value => value
            .to_text()
            .map(|text| Some(text.to_owned()))
            .ok_or(LimboError::InvalidColumnType),
    }
}

fn execute_bound(connection: &Arc<Connection>, sql: &str, values: Vec<Value>) -> Result<i64> {
    let mut statement = connection.prepare(sql)?;
    bind_all(&mut statement, values)?;
    drive_statement(&mut statement, |_| Ok(()))?;
    Ok(statement.n_change())
}

fn query_bound<T>(
    connection: &Arc<Connection>,
    sql: &str,
    values: Vec<Value>,
    mut map: impl FnMut(&Row) -> Result<T>,
) -> Result<Vec<T>> {
    let mut statement = connection.prepare(sql)?;
    bind_all(&mut statement, values)?;
    let mut rows = Vec::new();
    drive_statement(&mut statement, |row| {
        rows.push(map(row)?);
        Ok(())
    })?;
    Ok(rows)
}

fn bind_all(statement: &mut Statement, values: Vec<Value>) -> Result<()> {
    require(
        statement.parameters_count() == values.len(),
        "parameter count mismatch",
    )?;
    for (offset, value) in values.into_iter().enumerate() {
        statement.bind_at(NonZeroUsize::new(offset + 1).unwrap(), value)?;
    }
    Ok(())
}

fn drive_statement(
    statement: &mut Statement,
    mut on_row: impl FnMut(&Row) -> Result<()>,
) -> Result<()> {
    loop {
        match statement.step()? {
            StepResult::Done => return Ok(()),
            StepResult::Row => on_row(statement.row().ok_or_else(|| {
                LimboError::InternalError("StepResult::Row without a row".to_string())
            })?)?,
            StepResult::IO | StepResult::Yield => statement._io().step()?,
            StepResult::Busy => return Err(LimboError::Busy),
            StepResult::Interrupt => return Err(LimboError::Interrupt),
        }
    }
}

fn query_i64(connection: &Arc<Connection>, sql: &str) -> Result<i64> {
    let rows = query_bound(connection, sql, Vec::new(), |row| row.get::<i64>(0))?;
    rows.into_iter()
        .next()
        .ok_or_else(|| LimboError::InternalError(format!("query returned no rows: {sql}")))
}

fn query_string(connection: &Arc<Connection>, sql: &str) -> Result<String> {
    let rows = query_bound(connection, sql, Vec::new(), |row| row.get::<String>(0))?;
    rows.into_iter()
        .next()
        .ok_or_else(|| LimboError::InternalError(format!("query returned no rows: {sql}")))
}

fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(LimboError::InternalError(message.to_string()))
    }
}

/// Open and close Turso core inside the generated WASM module.
#[wasm_bindgen]
pub fn run_open_close_spike() -> std::result::Result<(), JsValue> {
    exercise_open_close().map_err(|error| JsValue::from_str(&error.to_string()))
}

/// Reproduce the built-in `MemoryIO` clock panic on wasm32-unknown-unknown.
#[cfg(feature = "failing-runtime-probes")]
#[wasm_bindgen]
pub fn run_builtin_memory_io_probe() -> std::result::Result<(), JsValue> {
    let io: Arc<dyn IO> = Arc::new(MemoryIO::new());
    let database = Database::open(io, ":memory:", OpenOptions::new(Arc::new(SqliteDialect)))
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    database
        .connect()
        .and_then(|connection| connection.close())
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

/// Reproduce the internal temp-`MemoryIO` clock panic from `BEGIN IMMEDIATE`.
#[cfg(feature = "failing-runtime-probes")]
#[wasm_bindgen]
pub fn run_begin_immediate_probe() -> std::result::Result<(), JsValue> {
    let connection = open_connection().map_err(|error| JsValue::from_str(&error.to_string()))?;
    connection
        .execute("BEGIN IMMEDIATE")
        .and_then(|()| connection.execute("ROLLBACK"))
        .and_then(|()| connection.close())
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

/// Execute the SQL spike inside the generated WASM module.
#[wasm_bindgen]
pub fn run_sql_spike() -> std::result::Result<String, JsValue> {
    exercise_required_sql()
        .map(|summary| summary.as_report_line())
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

/// Return the current unshared WASM linear-memory allocation in bytes.
#[wasm_bindgen]
pub fn linear_memory_bytes() -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        core::arch::wasm32::memory_size(0) * 65_536
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0
    }
}

#[cfg(test)]
mod test;
