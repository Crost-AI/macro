//! Read-only regression harness for Turso's unused temporary-database fix.
//!
//! The same source is compiled against the exact parent and fixed commit. It
//! uses a fixed-path synchronous I/O adapter with a deterministic clock, runs
//! the WP-04 storage SQL contract, and exports focused WASM probes.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod io;
mod sql;

use io::IoEvidence;
use serde::Serialize;
#[cfg(feature = "failing-runtime-probes")]
use std::sync::Arc;
#[cfg(feature = "failing-runtime-probes")]
use turso_core::{Database, MemoryIO, OpenOptions, SqliteDialect, IO};
use turso_core::{LimboError, Result};
use wasm_bindgen::prelude::*;

#[cfg(all(feature = "parent-revision", feature = "head-revision"))]
compile_error!("select exactly one revision feature");
#[cfg(not(any(feature = "parent-revision", feature = "head-revision")))]
compile_error!("select one revision feature");

/// Exact parent commit tested by this spike.
pub const PARENT_COMMIT: &str = "79163249538197d01dec5ea7f65519454ed792e2";
/// Exact fixed commit tested by this spike.
pub const HEAD_COMMIT: &str = "cf7de76172d61057007097e2dee7c47002cdc559";

/// Evidence from an unused immediate or exclusive transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnusedTempReport {
    /// Compiled source variant.
    pub revision: String,
    /// SQL transaction mode.
    pub mode: String,
    /// Names returned by `PRAGMA database_list` while the transaction is open.
    pub database_names: Vec<String>,
    /// Whether Turso unnecessarily materialized the `temp` database.
    pub temp_database_listed: bool,
    /// Evidence that main/WAL operations and timing used the supplied adapter.
    pub io: IoEvidence,
}

/// Native-only evidence that deliberately used temporary storage remains valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExplicitTempReport {
    /// Compiled source variant.
    pub revision: String,
    /// Rows preserved after commit followed by a rolled-back insert.
    pub committed_rows: Vec<i64>,
    /// Names returned after explicit temp initialization.
    pub database_names: Vec<String>,
    /// Evidence for the supplied main-database adapter.
    pub io: IoEvidence,
}

/// One classified SQL or I/O error observed by the contract harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorEvidence {
    /// Test scenario producing the error.
    pub scenario: String,
    /// Stable Rust enum classification available to the caller.
    pub class: String,
    /// Exact display text from this Turso revision.
    pub message: String,
    /// Whether WP-04 requires a physical reset for this outcome.
    pub reset_required: bool,
}

/// Verification status for one explicitly enumerated WP-04 requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Wp04CoverageStatus {
    /// The harness exercised the requirement and it passed.
    TestedPassed,
    /// The harness exercised the requirement and it failed.
    TestedFailed,
    /// This spike did not exercise the requirement.
    NotTested,
}

/// One entry in the explicit WP-04 coverage matrix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Wp04CoverageItem {
    /// Stable requirement identifier.
    pub requirement: String,
    /// Whether the requirement passed, failed, or was not tested.
    pub status: Wp04CoverageStatus,
    /// Scope and evidence for the recorded status.
    pub evidence: String,
}

/// Result of executing the WP-04 SQL, pragma, transaction, and error contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Wp04Report {
    /// Compiled source variant.
    pub revision: String,
    /// Effective journal mode returned by Turso.
    pub journal_mode: String,
    /// Whether the schema, metadata, binding, and DML shapes passed.
    pub ddl_dml_passed: bool,
    /// Whether canonical compound-key order and exclusive cursors passed.
    pub canonical_scan_passed: bool,
    /// Whether queue joins, strict-head claims, fences, settlement, and clear passed.
    pub queue_contract_passed: bool,
    /// Whether deferred and immediate commit/rollback/snapshot behavior passed.
    pub transaction_contract_passed: bool,
    /// Whether close/drop/reopen preserved schema, metadata, and valid integrity.
    pub clean_reopen_passed: bool,
    /// Exact valid-database `quick_check` rows.
    pub quick_check_rows: Vec<String>,
    /// Whether `foreign_key_check` is implemented by this core revision.
    pub foreign_key_check_supported: bool,
    /// Unsupported/error result from `foreign_key_check`, if any.
    pub foreign_key_check_error: Option<ErrorEvidence>,
    /// Whether foreign keys were proved connection-local and enforced when enabled.
    pub foreign_keys_connection_local: bool,
    /// Whether checked key/numeric conversion rejection passed.
    pub conversion_contract_passed: bool,
    /// Classified constraint, storage-full, commit-sync, and corruption evidence.
    pub errors: Vec<ErrorEvidence>,
    /// Evidence that work used the supplied custom I/O and deterministic clock.
    pub io: IoEvidence,
    /// Whether every runnable SQL/pragma operation except the separately reported
    /// unsupported `foreign_key_check` contract passed in this core harness.
    pub runnable_wp04_sql_passed: bool,
    /// Explicit status for each covered, failed, or untested WP-04 requirement.
    pub coverage_matrix: Vec<Wp04CoverageItem>,
    /// Explicitly recorded limitations rather than inferred success.
    pub limitations: Vec<String>,
}

/// Native report for one exact source revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeReport {
    /// Compiled source variant.
    pub revision: String,
    /// Unused `BEGIN IMMEDIATE` evidence.
    pub immediate: UnusedTempReport,
    /// Unused `BEGIN EXCLUSIVE` evidence.
    pub exclusive: UnusedTempReport,
    /// Deliberate temporary-table behavior.
    pub explicit_temp: ExplicitTempReport,
    /// Complete WP-04 execution report.
    pub wp04: Wp04Report,
}

/// Return the exact commit represented by this compiled variant.
pub fn revision() -> &'static str {
    #[cfg(feature = "parent-revision")]
    {
        PARENT_COMMIT
    }
    #[cfg(feature = "head-revision")]
    {
        HEAD_COMMIT
    }
}

/// Run `BEGIN IMMEDIATE` or `BEGIN EXCLUSIVE` without touching temp objects.
pub fn exercise_unused_temp_transaction(mode: &str) -> Result<UnusedTempReport> {
    let mode = match mode {
        "IMMEDIATE" => "IMMEDIATE",
        "EXCLUSIVE" => "EXCLUSIVE",
        _ => {
            return Err(LimboError::InvalidArgument(format!(
                "unsupported transaction mode: {mode}"
            )))
        }
    };
    let (database, connection, io) = sql::open("unused-temp.db")?;
    connection.execute(format!("BEGIN {mode}"))?;
    let database_names = sql::query(&connection, "PRAGMA database_list", Vec::new(), |row| {
        row.get::<String>(1)
    })?;
    connection.execute("ROLLBACK")?;
    connection.close()?;
    drop(connection);
    drop(database);
    let temp_database_listed = database_names.iter().any(|name| name == "temp");
    Ok(UnusedTempReport {
        revision: revision().to_owned(),
        mode: mode.to_owned(),
        database_names,
        temp_database_listed,
        io: io.evidence(),
    })
}

/// Deliberately create and transact against a temp table.
pub fn exercise_explicit_temp_native() -> Result<ExplicitTempReport> {
    let (database, connection, io) = sql::open("explicit-temp.db")?;
    connection.execute("BEGIN IMMEDIATE")?;
    connection.execute("CREATE TEMP TABLE used_temp(v INTEGER)")?;
    connection.execute("INSERT INTO used_temp VALUES(1)")?;
    connection.execute("COMMIT")?;
    connection.execute("BEGIN EXCLUSIVE")?;
    connection.execute("INSERT INTO used_temp VALUES(2)")?;
    connection.execute("ROLLBACK")?;
    let committed_rows = sql::query(
        &connection,
        "SELECT v FROM used_temp ORDER BY v",
        Vec::new(),
        |row| row.get::<i64>(0),
    )?;
    let database_names = sql::query(&connection, "PRAGMA database_list", Vec::new(), |row| {
        row.get::<String>(1)
    })?;
    connection.close()?;
    drop(connection);
    drop(database);
    Ok(ExplicitTempReport {
        revision: revision().to_owned(),
        committed_rows,
        database_names,
        io: io.evidence(),
    })
}

/// Execute all runnable WP-04 contract cases and record unsupported gates.
pub fn exercise_wp04_contract() -> Result<Wp04Report> {
    sql::exercise_wp04_contract()
}

/// Produce the complete native report for this exact source revision.
pub fn native_report() -> Result<NativeReport> {
    Ok(NativeReport {
        revision: revision().to_owned(),
        immediate: exercise_unused_temp_transaction("IMMEDIATE")?,
        exclusive: exercise_unused_temp_transaction("EXCLUSIVE")?,
        explicit_temp: exercise_explicit_temp_native()?,
        wp04: exercise_wp04_contract()?,
    })
}

fn to_js<T: Serialize>(result: Result<T>) -> std::result::Result<String, JsValue> {
    result
        .and_then(|value| {
            serde_json::to_string(&value)
                .map_err(|error| LimboError::InternalError(error.to_string()))
        })
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

/// WASM export for an unused `BEGIN IMMEDIATE` transaction.
#[wasm_bindgen]
pub fn run_unused_immediate_probe() -> std::result::Result<String, JsValue> {
    to_js(exercise_unused_temp_transaction("IMMEDIATE"))
}

/// WASM export for an unused `BEGIN EXCLUSIVE` transaction.
#[wasm_bindgen]
pub fn run_unused_exclusive_probe() -> std::result::Result<String, JsValue> {
    to_js(exercise_unused_temp_transaction("EXCLUSIVE"))
}

/// WASM export for the complete runnable WP-04 contract.
#[wasm_bindgen]
pub fn run_wp04_contract() -> std::result::Result<String, JsValue> {
    to_js(exercise_wp04_contract())
}

/// WASM probe that honestly requests explicit temp storage.
///
/// Turso still constructs built-in `MemoryIO` for explicit temp storage at
/// both revisions, so this is expected to trap at its unsupported WASM clock.
#[cfg(feature = "failing-runtime-probes")]
#[wasm_bindgen]
pub fn run_explicit_temp_create_probe() -> std::result::Result<(), JsValue> {
    let (_database, connection, _io) = sql::open("explicit-temp-wasm.db")
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    connection
        .execute("CREATE TEMP TABLE explicit_temp(v INTEGER)")
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

/// WASM probe for temp use after `BEGIN IMMEDIATE`.
#[cfg(feature = "failing-runtime-probes")]
#[wasm_bindgen]
pub fn run_temp_after_immediate_probe() -> std::result::Result<(), JsValue> {
    let (_database, connection, _io) = sql::open("temp-after-immediate.db")
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    connection
        .execute("BEGIN IMMEDIATE")
        .and_then(|()| connection.execute("CREATE TEMP TABLE explicit_temp(v INTEGER)"))
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

/// WASM probe confirming built-in `MemoryIO` itself still uses unsupported time.
#[cfg(feature = "failing-runtime-probes")]
#[wasm_bindgen]
pub fn run_builtin_memory_io_probe() -> std::result::Result<(), JsValue> {
    let io: Arc<dyn IO> = Arc::new(MemoryIO::new());
    let database = Database::open(io, "builtin.db", OpenOptions::new(Arc::new(SqliteDialect)))
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    database
        .connect()
        .and_then(|connection| connection.close())
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

/// Return current WASM linear-memory allocation in bytes.
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
