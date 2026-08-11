use crate::{
    io::{Fault, ProductionLikeIo},
    revision, ErrorEvidence, Wp04CoverageItem, Wp04CoverageStatus, Wp04Report,
};
use std::{num::NonZeroUsize, sync::Arc};
use turso_core::{
    CompletionError, Connection, Database, LimboError, OpenOptions, Result, Row, SqliteDialect,
    Statement, StepResult, Value, IO,
};

const SCHEMA: [&str; 4] = [
    "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    "CREATE TABLE records (__typename TEXT NOT NULL, id TEXT NOT NULL, value BLOB NOT NULL, PRIMARY KEY (__typename, id))",
    "CREATE TABLE mutation_queue (id INTEGER PRIMARY KEY AUTOINCREMENT, query TEXT NOT NULL, operation_name TEXT, variables_json TEXT NOT NULL, identity TEXT, attempt_count INTEGER NOT NULL DEFAULT 0, next_attempt_at_ms INTEGER, lease_owner TEXT, lease_generation INTEGER NOT NULL DEFAULT 0, lease_expires_at_ms INTEGER, last_error TEXT, created_at_ms INTEGER NOT NULL)",
    "CREATE TABLE optimistic_layers (mutation_id INTEGER PRIMARY KEY, optimistic_data_json TEXT NOT NULL, normalized_updates BLOB NOT NULL, FOREIGN KEY (mutation_id) REFERENCES mutation_queue(id) ON DELETE CASCADE)",
];

pub(crate) fn open(path: &str) -> Result<(Arc<Database>, Arc<Connection>, Arc<ProductionLikeIo>)> {
    let io = ProductionLikeIo::new(path);
    let dyn_io: Arc<dyn IO> = io.clone();
    let database = Database::open(
        dyn_io,
        io.database_path(),
        OpenOptions::new(Arc::new(SqliteDialect)),
    )?;
    let connection = database.connect()?;
    Ok((database, connection, io))
}

fn reopen(io: Arc<ProductionLikeIo>) -> Result<(Arc<Database>, Arc<Connection>)> {
    let path = io.database_path().to_owned();
    let dyn_io: Arc<dyn IO> = io;
    let database = Database::open(dyn_io, &path, OpenOptions::new(Arc::new(SqliteDialect)))?;
    let connection = database.connect()?;
    Ok((database, connection))
}

fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(LimboError::InternalError(message.to_owned()))
    }
}

pub(crate) fn execute(connection: &Arc<Connection>, sql: &str, values: Vec<Value>) -> Result<i64> {
    let mut statement = connection.prepare(sql)?;
    bind(&mut statement, values)?;
    drive(&mut statement, |_| Ok(()))?;
    Ok(statement.n_change())
}

pub(crate) fn query<T>(
    connection: &Arc<Connection>,
    sql: &str,
    values: Vec<Value>,
    mut map: impl FnMut(&Row) -> Result<T>,
) -> Result<Vec<T>> {
    let mut statement = connection.prepare(sql)?;
    bind(&mut statement, values)?;
    let mut rows = Vec::new();
    drive(&mut statement, |row| {
        rows.push(map(row)?);
        Ok(())
    })?;
    Ok(rows)
}

fn bind(statement: &mut Statement, values: Vec<Value>) -> Result<()> {
    require(
        statement.parameters_count() == values.len(),
        "parameter count mismatch",
    )?;
    for (offset, value) in values.into_iter().enumerate() {
        statement.bind_at(
            NonZeroUsize::new(offset + 1)
                .ok_or_else(|| LimboError::InternalError("zero parameter index".to_owned()))?,
            value,
        )?;
    }
    Ok(())
}

fn drive(statement: &mut Statement, mut on_row: impl FnMut(&Row) -> Result<()>) -> Result<()> {
    loop {
        match statement.step()? {
            StepResult::Done => return Ok(()),
            StepResult::Row => on_row(
                statement
                    .row()
                    .ok_or_else(|| LimboError::InternalError("row step had no row".to_owned()))?,
            )?,
            StepResult::IO | StepResult::Yield => statement._io().step()?,
            StepResult::Busy => return Err(LimboError::Busy),
            StepResult::Interrupt => return Err(LimboError::Interrupt),
        }
    }
}

fn scalar_i64(connection: &Arc<Connection>, sql: &str) -> Result<i64> {
    query(connection, sql, Vec::new(), |row| row.get::<i64>(0))?
        .into_iter()
        .next()
        .ok_or_else(|| LimboError::InternalError(format!("query returned no row: {sql}")))
}

fn scalar_string(connection: &Arc<Connection>, sql: &str) -> Result<String> {
    query(connection, sql, Vec::new(), |row| row.get::<String>(0))?
        .into_iter()
        .next()
        .ok_or_else(|| LimboError::InternalError(format!("query returned no row: {sql}")))
}

fn nullable_string(row: &Row, index: usize) -> Result<Option<String>> {
    match row.get_value(index) {
        Value::Null => Ok(None),
        value => value
            .to_text()
            .map(|value| Some(value.to_owned()))
            .ok_or(LimboError::InvalidColumnType),
    }
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

fn classify(scenario: &str, error: &LimboError, reset_required: bool) -> ErrorEvidence {
    let class = match error {
        LimboError::Corrupt(_) => "Corrupt",
        LimboError::NotADB => "NotADB",
        LimboError::DatabaseFull(_) => "DatabaseFull",
        LimboError::ParseError(_) | LimboError::LexerError(_) => "ParseError",
        LimboError::ConversionError(_) => "ConversionError",
        LimboError::Constraint(_) => "Constraint",
        LimboError::ForeignKeyConstraint(_) => "ForeignKeyConstraint",
        LimboError::CompletionError(CompletionError::IOError(kind, _))
            if *kind == std::io::ErrorKind::StorageFull =>
        {
            "CompletionError::IOError(StorageFull)"
        }
        LimboError::CompletionError(CompletionError::IOError(_, _)) => "CompletionError::IOError",
        LimboError::Busy => "Busy",
        LimboError::BusySnapshot => "BusySnapshot",
        LimboError::InvalidColumnType => "InvalidColumnType",
        LimboError::InternalError(_) => "InternalError",
        _ => "Other",
    };
    ErrorEvidence {
        scenario: scenario.to_owned(),
        class: class.to_owned(),
        message: error.to_string(),
        reset_required,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordKey {
    typename: String,
    id: String,
}

fn parse_key(key: &str) -> Result<RecordKey> {
    let parsed = if key == "ROOT_QUERY" {
        RecordKey {
            typename: "ROOT_QUERY".to_owned(),
            id: String::new(),
        }
    } else {
        let (typename, id) = key.split_once(':').ok_or_else(|| {
            LimboError::ConversionError("non-root EntityKey has no colon".to_owned())
        })?;
        require(!typename.is_empty(), "EntityKey typename is empty")?;
        RecordKey {
            typename: typename.to_owned(),
            id: id.to_owned(),
        }
    };
    require(format_key(&parsed)? == key, "EntityKey did not round trip")?;
    Ok(parsed)
}

fn format_key(key: &RecordKey) -> Result<String> {
    require(!key.typename.is_empty(), "SQL typename is empty")?;
    let value = if key.typename == "ROOT_QUERY" && key.id.is_empty() {
        "ROOT_QUERY".to_owned()
    } else {
        format!("{}:{}", key.typename, key.id)
    };
    let reparsed = if value == "ROOT_QUERY" {
        RecordKey {
            typename: "ROOT_QUERY".to_owned(),
            id: String::new(),
        }
    } else {
        let (typename, id) = value
            .split_once(':')
            .ok_or_else(|| LimboError::ConversionError("formatted key has no colon".to_owned()))?;
        RecordKey {
            typename: typename.to_owned(),
            id: id.to_owned(),
        }
    };
    require(&reparsed == key, "SQL key pair did not round trip")?;
    Ok(value)
}

fn check_conversions(errors: &mut Vec<ErrorEvidence>) -> Result<()> {
    for key in [
        "ROOT_QUERY",
        "Document:doc-1",
        "Document:tenant:doc-1",
        "__meta:identity",
        "Thing:",
    ] {
        let pair = parse_key(key)?;
        require(
            format_key(&pair)? == key,
            "valid EntityKey round trip failed",
        )?;
    }
    for invalid in ["", "Document", ":id", "ROOT_QUERY:"] {
        let error = parse_key(invalid).expect_err("invalid key unexpectedly accepted");
        errors.push(classify("invalid_entity_key", &error, true));
    }
    require(
        i64::try_from(u64::MAX).is_err()
            && i64::try_from(u64::MAX).is_err()
            && u32::try_from(-1_i64).is_err(),
        "checked numeric conversion accepted out-of-range value",
    )
}

fn initialize(connection: &Arc<Connection>) -> Result<String> {
    let journal_mode = scalar_string(connection, "PRAGMA journal_mode = WAL")?;
    connection.execute("PRAGMA foreign_keys = ON")?;
    require(
        scalar_i64(connection, "PRAGMA foreign_keys")? == 1,
        "foreign keys did not read back enabled",
    )?;

    connection.execute("BEGIN IMMEDIATE")?;
    connection.execute("CREATE TABLE rolled_back_ddl(v INTEGER)")?;
    connection.execute("ROLLBACK")?;
    require(
        query(
            connection,
            "SELECT name FROM sqlite_schema WHERE name = 'rolled_back_ddl'",
            Vec::new(),
            |row| row.get::<String>(0),
        )?
        .is_empty(),
        "DDL rollback retained a table",
    )?;

    connection.execute("BEGIN IMMEDIATE")?;
    for sql in SCHEMA {
        connection.execute(sql)?;
    }
    execute(
        connection,
        "INSERT INTO meta (key, value) VALUES ('scope', ?1)",
        vec![Value::from_text("opaque-scope")],
    )?;
    execute(
        connection,
        "INSERT INTO meta (key, value) VALUES ('namespace', ?1)",
        vec![Value::from_text("cache-v1:opaque-scope")],
    )?;
    execute(
        connection,
        "INSERT INTO meta (key, value) VALUES ('storage_schema_version', ?1)",
        vec![Value::from_text("1")],
    )?;
    connection.execute("COMMIT")?;
    connection
        .execute("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")?;
    Ok(journal_mode)
}

fn check_metadata_and_bindings(connection: &Arc<Connection>) -> Result<()> {
    let metadata = query(
        connection,
        "SELECT key, value FROM meta WHERE key IN ('scope', 'namespace', 'storage_schema_version') ORDER BY key ASC",
        Vec::new(),
        |row| Ok((row.get::<String>(0)?, row.get::<String>(1)?)),
    )?;
    require(
        metadata
            == vec![
                ("namespace".to_owned(), "cache-v1:opaque-scope".to_owned()),
                ("scope".to_owned(), "opaque-scope".to_owned()),
                ("storage_schema_version".to_owned(), "1".to_owned()),
            ],
        "metadata result shape mismatch",
    )?;

    let bound = query(
        connection,
        "SELECT ?1, ?2, ?3, ?4",
        vec![
            Value::from_text("id:with:colons"),
            Value::from_i64(i64::MAX),
            Value::Null,
            Value::from_blob(vec![0, 1, 2, 255]),
        ],
        |row| {
            Ok((
                row.get::<String>(0)?,
                row.get::<i64>(1)?,
                matches!(row.get_value(2), Value::Null),
                row.get_value(3)
                    .to_blob()
                    .ok_or(LimboError::InvalidColumnType)?
                    .to_vec(),
            ))
        },
    )?;
    require(
        bound
            == vec![(
                "id:with:colons".to_owned(),
                i64::MAX,
                true,
                vec![0, 1, 2, 255],
            )],
        "TEXT/INTEGER/NULL/BLOB binding mismatch",
    )
}

fn upsert_records_reused(connection: &Arc<Connection>, rows: &[(&str, &str, &[u8])]) -> Result<()> {
    let mut statement = connection.prepare(
        "INSERT INTO records (__typename, id, value) VALUES (?1, ?2, ?3) ON CONFLICT (__typename, id) DO UPDATE SET value = excluded.value",
    )?;
    for (offset, (typename, id, value)) in rows.iter().enumerate() {
        if offset > 0 {
            statement.reset()?;
            statement.clear_bindings();
        }
        bind(
            &mut statement,
            vec![
                Value::from_text((*typename).to_owned()),
                Value::from_text((*id).to_owned()),
                Value::from_blob(value.to_vec()),
            ],
        )?;
        drive(&mut statement, |_| Ok(()))?;
        require(
            statement.n_change() == 1,
            "upsert affected-row count mismatch",
        )?;
    }
    Ok(())
}

fn check_records(connection: &Arc<Connection>) -> Result<()> {
    connection.execute("BEGIN IMMEDIATE")?;
    upsert_records_reused(
        connection,
        &[
            ("ROOT_QUERY", "", &[0]),
            ("Type", "9", &[1]),
            ("Type0", "1", &[2]),
            ("Type", "a:colon", &[3]),
            ("Other", "1", &[4]),
            ("Type", "9", &[9]),
        ],
    )?;
    connection.execute("COMMIT")?;

    connection.execute("BEGIN")?;
    let requested = ["Type:9", "Type:missing", "Type:9", "ROOT_QUERY"];
    let mut values = Vec::new();
    for key in requested {
        let pair = parse_key(key)?;
        let row = query(
            connection,
            "SELECT value FROM records WHERE __typename = ?1 AND id = ?2",
            vec![Value::from_text(pair.typename), Value::from_text(pair.id)],
            |row| {
                Ok(row
                    .get_value(0)
                    .to_blob()
                    .ok_or(LimboError::InvalidColumnType)?
                    .to_vec())
            },
        )?;
        values.push(row.into_iter().next());
    }
    connection.execute("COMMIT")?;
    require(
        values == vec![Some(vec![9]), None, Some(vec![9]), Some(vec![0])],
        "get_batch alignment mismatch",
    )?;

    let scan = query(
        connection,
        "SELECT __typename, id, value FROM records WHERE __typename IN (?1, ?2) AND NOT (__typename = 'ROOT_QUERY' AND id = '') ORDER BY (__typename || ':' || id) COLLATE BINARY ASC LIMIT ?3",
        vec![
            Value::from_text("Type"),
            Value::from_text("Type0"),
            Value::from_i64(10),
        ],
        |row| {
            let key = format_key(&RecordKey {
                typename: row.get(0)?,
                id: row.get(1)?,
            })?;
            Ok(key)
        },
    )?;
    let mut rust_sorted = vec![
        "Type:9".to_owned(),
        "Type0:1".to_owned(),
        "Type:a:colon".to_owned(),
    ];
    rust_sorted.sort();
    require(
        scan == rust_sorted,
        "canonical BINARY scan differs from Rust order",
    )?;
    require(
        scan == vec!["Type0:1", "Type:9", "Type:a:colon"],
        "prefix typename edge case used tuple order",
    )?;
    let after_cursor = query(
        connection,
        "SELECT __typename, id, value FROM records WHERE __typename IN (?1, ?2) AND NOT (__typename = 'ROOT_QUERY' AND id = '') AND ((__typename || ':' || id) COLLATE BINARY) > ?3 ORDER BY (__typename || ':' || id) COLLATE BINARY ASC LIMIT ?4",
        vec![
            Value::from_text("Type"),
            Value::from_text("Type0"),
            Value::from_text("Type0:1"),
            Value::from_i64(2),
        ],
        |row| {
            format_key(&RecordKey {
                typename: row.get(0)?,
                id: row.get(1)?,
            })
        },
    )?;
    require(
        after_cursor == vec!["Type:9", "Type:a:colon"],
        "exclusive global cursor mismatch",
    )?;

    connection.execute("BEGIN IMMEDIATE")?;
    let first_delete = execute(
        connection,
        "DELETE FROM records WHERE __typename = ?1 AND id = ?2",
        vec![Value::from_text("Other"), Value::from_text("1")],
    )?;
    let missing_delete = execute(
        connection,
        "DELETE FROM records WHERE __typename = ?1 AND id = ?2",
        vec![Value::from_text("Other"), Value::from_text("missing")],
    )?;
    let duplicate_delete = execute(
        connection,
        "DELETE FROM records WHERE __typename = ?1 AND id = ?2",
        vec![Value::from_text("Other"), Value::from_text("1")],
    )?;
    connection.execute("COMMIT")?;
    require(
        (first_delete, missing_delete, duplicate_delete) == (1, 0, 0),
        "delete affected-row counts mismatch",
    )
}

fn enqueue(
    connection: &Arc<Connection>,
    operation_name: Option<&str>,
    next_attempt_at_ms: Option<i64>,
) -> Result<i64> {
    let values = vec![
        Value::from_text("mutation Verify { verify { id } }"),
        operation_name.map_or(Value::Null, |value| Value::from_text(value.to_owned())),
        Value::from_text("{}"),
        Value::Null,
        Value::from_i64(0),
        next_attempt_at_ms.map_or(Value::Null, Value::from_i64),
        Value::Null,
        Value::from_i64(0),
        Value::Null,
        Value::Null,
        Value::from_i64(1_700_000_000_000),
    ];
    execute(
        connection,
        "INSERT INTO mutation_queue (query, operation_name, variables_json, identity, attempt_count, next_attempt_at_ms, lease_owner, lease_generation, lease_expires_at_ms, last_error, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        values,
    )?;
    let id = connection.last_insert_rowid();
    require(id > 0, "last_insert_rowid was not positive")?;
    execute(
        connection,
        "INSERT INTO optimistic_layers (mutation_id, optimistic_data_json, normalized_updates) VALUES (?1, ?2, ?3)",
        vec![
            Value::from_i64(id),
            Value::from_text("{}"),
            Value::from_blob(vec![id as u8]),
        ],
    )?;
    Ok(id)
}

#[derive(Debug)]
struct Head {
    id: i64,
    attempt_count: i64,
    next_attempt_at_ms: Option<i64>,
    lease_owner: Option<String>,
    lease_generation: i64,
    lease_expires_at_ms: Option<i64>,
    optimistic_data: Option<String>,
}

fn queue_head(connection: &Arc<Connection>) -> Result<Option<Head>> {
    Ok(query(
        connection,
        "SELECT m.id, m.query, m.operation_name, m.variables_json, m.identity, m.attempt_count, m.next_attempt_at_ms, m.lease_owner, m.lease_generation, m.lease_expires_at_ms, m.last_error, m.created_at_ms, o.optimistic_data_json, o.normalized_updates FROM mutation_queue AS m LEFT JOIN optimistic_layers AS o ON o.mutation_id = m.id ORDER BY m.id ASC LIMIT 1",
        Vec::new(),
        |row| {
            let _: String = row.get(1)?;
            let _ = nullable_string(row, 2)?;
            let _: String = row.get(3)?;
            let _ = nullable_string(row, 4)?;
            let _ = nullable_string(row, 10)?;
            let _: i64 = row.get(11)?;
            if !matches!(row.get_value(13), Value::Null) {
                row.get_value(13)
                    .to_blob()
                    .ok_or(LimboError::InvalidColumnType)?;
            }
            Ok(Head {
                id: row.get(0)?,
                attempt_count: row.get(5)?,
                next_attempt_at_ms: nullable_i64(row, 6)?,
                lease_owner: nullable_string(row, 7)?,
                lease_generation: row.get(8)?,
                lease_expires_at_ms: nullable_i64(row, 9)?,
                optimistic_data: nullable_string(row, 12)?,
            })
        },
    )?
    .into_iter()
    .next())
}

fn runnable(head: &Head, now_ms: i64) -> bool {
    head.next_attempt_at_ms.is_none_or(|next| next <= now_ms)
        && head
            .lease_expires_at_ms
            .is_none_or(|expires| expires <= now_ms)
}

fn claim(connection: &Arc<Connection>, now_ms: i64, owner: &str) -> Result<Option<(i64, i64)>> {
    connection.execute("BEGIN IMMEDIATE")?;
    let Some(head) = queue_head(connection)? else {
        let orphan = query(
            connection,
            "SELECT mutation_id FROM optimistic_layers LIMIT 1",
            Vec::new(),
            |row| row.get::<i64>(0),
        )?;
        require(orphan.is_empty(), "orphan layer with empty queue")?;
        connection.execute("COMMIT")?;
        return Ok(None);
    };
    require(
        head.optimistic_data.is_some(),
        "queue head is missing layer",
    )?;
    if !runnable(&head, now_ms) {
        connection.execute("COMMIT")?;
        return Ok(None);
    }
    let _previous_owner = &head.lease_owner;
    let attempt_count = head.attempt_count.saturating_add(1);
    let generation = head.lease_generation.saturating_add(1);
    let changed = execute(
        connection,
        "UPDATE mutation_queue SET attempt_count = ?2, next_attempt_at_ms = NULL, lease_owner = ?3, lease_generation = ?4, lease_expires_at_ms = ?5 WHERE id = ?1",
        vec![
            Value::from_i64(head.id),
            Value::from_i64(attempt_count),
            Value::from_text(owner.to_owned()),
            Value::from_i64(generation),
            Value::from_i64(now_ms + 100),
        ],
    )?;
    require(changed == 1, "claim affected-row count mismatch")?;
    connection.execute("COMMIT")?;
    Ok(Some((head.id, generation)))
}

fn claim_matches(
    connection: &Arc<Connection>,
    id: i64,
    owner: &str,
    generation: i64,
) -> Result<bool> {
    let rows = query(
        connection,
        "SELECT lease_owner, lease_generation FROM mutation_queue WHERE id = ?1",
        vec![Value::from_i64(id)],
        |row| Ok((nullable_string(row, 0)?, row.get::<i64>(1)?)),
    )?;
    Ok(rows == vec![(Some(owner.to_owned()), generation)])
}

fn require_layer(connection: &Arc<Connection>, id: i64) -> Result<()> {
    require(
        query(
            connection,
            "SELECT 1 FROM optimistic_layers WHERE mutation_id = ?1",
            vec![Value::from_i64(id)],
            |row| row.get::<i64>(0),
        )? == vec![1],
        "current mutation has no optimistic layer",
    )
}

fn settle(
    connection: &Arc<Connection>,
    id: i64,
    owner: &str,
    generation: i64,
    complete: bool,
) -> Result<bool> {
    connection.execute("BEGIN IMMEDIATE")?;
    if !claim_matches(connection, id, owner, generation)? {
        connection.execute("COMMIT")?;
        return Ok(false);
    }
    require_layer(connection, id)?;
    if complete {
        upsert_records_reused(connection, &[("Completed", "result", &[7, 7])])?;
    }
    require(
        execute(
            connection,
            "DELETE FROM mutation_queue WHERE id = ?1",
            vec![Value::from_i64(id)],
        )? == 1,
        "settlement delete count mismatch",
    )?;
    connection.execute("COMMIT")?;
    Ok(true)
}

fn check_queue(connection: &Arc<Connection>, second: &Arc<Connection>) -> Result<()> {
    let second_last_insert_rowid = second.last_insert_rowid();
    connection.execute("BEGIN IMMEDIATE")?;
    let first = enqueue(connection, Some("Verify"), Some(100))?;
    let second_id = enqueue(connection, None, None)?;
    connection.execute("COMMIT")?;
    require(second_id > first, "AUTOINCREMENT did not increase")?;

    connection.execute("BEGIN")?;
    let loaded = query(
        connection,
        "SELECT m.id, m.query, m.operation_name, m.variables_json, m.identity, m.attempt_count, m.next_attempt_at_ms, m.lease_owner, m.lease_generation, m.lease_expires_at_ms, m.last_error, m.created_at_ms, o.optimistic_data_json, o.normalized_updates FROM mutation_queue AS m LEFT JOIN optimistic_layers AS o ON o.mutation_id = m.id ORDER BY m.id ASC",
        Vec::new(),
        |row| {
            require(nullable_string(row, 12)?.is_some(), "load found missing layer")?;
            row.get::<i64>(0)
        },
    )?;
    let orphan = query(
        connection,
        "SELECT o.mutation_id FROM optimistic_layers AS o LEFT JOIN mutation_queue AS m ON m.id = o.mutation_id WHERE m.id IS NULL LIMIT 1",
        Vec::new(),
        |row| row.get::<i64>(0),
    )?;
    connection.execute("COMMIT")?;
    require(
        loaded == vec![first, second_id] && orphan.is_empty(),
        "queue load mismatch",
    )?;
    let inner_join = query(
        connection,
        "SELECT m.id FROM mutation_queue AS m INNER JOIN optimistic_layers AS o ON o.mutation_id = m.id ORDER BY m.id ASC LIMIT ?1",
        vec![Value::from_i64(2)],
        |row| row.get::<i64>(0),
    )?;
    require(
        inner_join == vec![first, second_id],
        "queue INNER JOIN or bound LIMIT mismatch",
    )?;
    require(
        second.last_insert_rowid() == second_last_insert_rowid,
        "last_insert_rowid changed on the other connection",
    )?;

    require(
        claim(connection, 99, "owner-a")?.is_none(),
        "future head did not block",
    )?;
    let (claimed_id, generation) = claim(connection, 100, "owner-a")?
        .ok_or_else(|| LimboError::InternalError("equality claim was not runnable".to_owned()))?;
    require(
        claimed_id == first && generation == 1,
        "claim result mismatch",
    )?;

    connection.execute("BEGIN IMMEDIATE")?;
    let stale_defer = execute(
        connection,
        "UPDATE mutation_queue SET next_attempt_at_ms = ?4, lease_owner = NULL, lease_expires_at_ms = NULL, last_error = ?5 WHERE id = ?1 AND lease_owner = ?2 AND lease_generation = ?3",
        vec![
            Value::from_i64(first),
            Value::from_text("stale-owner"),
            Value::from_i64(generation),
            Value::from_i64(200),
            Value::from_text("retry"),
        ],
    )?;
    let current_defer = execute(
        connection,
        "UPDATE mutation_queue SET next_attempt_at_ms = ?4, lease_owner = NULL, lease_expires_at_ms = NULL, last_error = ?5 WHERE id = ?1 AND lease_owner = ?2 AND lease_generation = ?3",
        vec![
            Value::from_i64(first),
            Value::from_text("owner-a"),
            Value::from_i64(generation),
            Value::from_i64(200),
            Value::from_text("retry"),
        ],
    )?;
    connection.execute("COMMIT")?;
    require(
        (stale_defer, current_defer) == (0, 1),
        "defer fence mismatch",
    )?;
    let deferred_state = query(
        connection,
        "SELECT attempt_count, next_attempt_at_ms, lease_owner, lease_generation, lease_expires_at_ms, last_error FROM mutation_queue WHERE id = ?1",
        vec![Value::from_i64(first)],
        |row| {
            Ok((
                row.get::<i64>(0)?,
                nullable_i64(row, 1)?,
                nullable_string(row, 2)?,
                row.get::<i64>(3)?,
                nullable_i64(row, 4)?,
                nullable_string(row, 5)?,
            ))
        },
    )?;
    require(
        deferred_state == vec![(1, Some(200), None, 1, None, Some("retry".to_owned()))],
        "defer did not preserve generation/error or clear lease fields",
    )?;

    require(
        claim(connection, 199, "owner-b")?.is_none(),
        "deferred head did not block",
    )?;
    let (_, generation2) = claim(connection, 200, "owner-b")?
        .ok_or_else(|| LimboError::InternalError("retry equality was not runnable".to_owned()))?;
    require(generation2 == 2, "lease generation did not increment")?;
    require(
        !settle(connection, first, "owner-a", generation, true)?,
        "stale complete unexpectedly succeeded",
    )?;
    require(
        scalar_i64(
            connection,
            "SELECT COUNT(*) FROM records WHERE __typename = 'Completed'",
        )? == 0,
        "stale complete wrote records",
    )?;
    require(
        settle(connection, first, "owner-b", generation2, true)?,
        "current complete failed",
    )?;
    require(
        query(
            connection,
            "SELECT COUNT(*) FROM optimistic_layers WHERE mutation_id = ?1",
            vec![Value::from_i64(first)],
            |row| row.get::<i64>(0),
        )? == vec![0],
        "cascade retained completed optimistic layer",
    )?;

    require(
        !settle(connection, second_id, "stale-owner", 0, false)?,
        "stale discard unexpectedly succeeded",
    )?;
    let (discard_id, discard_generation) = claim(connection, 200, "owner-c")?
        .ok_or_else(|| LimboError::InternalError("second mutation was not claimable".to_owned()))?;
    require(discard_id == second_id, "strict queue order changed")?;
    require(
        settle(connection, discard_id, "owner-c", discard_generation, false)?,
        "current discard failed",
    )?;

    connection.execute("BEGIN IMMEDIATE")?;
    execute(
        connection,
        "INSERT INTO mutation_queue (query, variables_json, created_at_ms) VALUES (?1, ?2, ?3)",
        vec![
            Value::from_text("mutation MissingLayer { x }"),
            Value::from_text("{}"),
            Value::from_i64(0),
        ],
    )?;
    let missing = queue_head(connection)?.is_some_and(|head| head.optimistic_data.is_none());
    connection.execute("ROLLBACK")?;
    require(missing, "LEFT JOIN did not expose missing layer")?;

    second.execute("PRAGMA foreign_keys = OFF")?;
    execute(
        second,
        "INSERT INTO optimistic_layers (mutation_id, optimistic_data_json, normalized_updates) VALUES (?1, ?2, ?3)",
        vec![
            Value::from_i64(99_999),
            Value::from_text("{}"),
            Value::from_blob(vec![0]),
        ],
    )?;
    require(
        query(
            connection,
            "SELECT o.mutation_id FROM optimistic_layers AS o LEFT JOIN mutation_queue AS m ON m.id = o.mutation_id WHERE m.id IS NULL LIMIT 1",
            Vec::new(),
            |row| row.get::<i64>(0),
        )? == vec![99_999],
        "orphan LEFT JOIN did not expose invalid layer",
    )?;
    execute(
        second,
        "DELETE FROM optimistic_layers WHERE mutation_id = ?1",
        vec![Value::from_i64(99_999)],
    )?;

    connection.execute("BEGIN IMMEDIATE")?;
    let before_clear = enqueue(connection, Some("BeforeClear"), None)?;
    upsert_records_reused(connection, &[("Clear", "record", &[1])])?;
    connection.execute("COMMIT")?;
    connection.execute("BEGIN IMMEDIATE")?;
    connection.execute("DELETE FROM optimistic_layers")?;
    connection.execute("DELETE FROM mutation_queue")?;
    connection.execute("DELETE FROM records")?;
    connection.execute("COMMIT")?;
    require(
        scalar_i64(connection, "SELECT COUNT(*) FROM meta")? == 3,
        "clear removed metadata",
    )?;
    require(
        scalar_i64(connection, "SELECT COUNT(*) FROM records")? == 0
            && scalar_i64(connection, "SELECT COUNT(*) FROM mutation_queue")? == 0
            && scalar_i64(connection, "SELECT COUNT(*) FROM optimistic_layers")? == 0,
        "clear retained cache data",
    )?;
    connection.execute("BEGIN IMMEDIATE")?;
    let after_clear = enqueue(connection, Some("AfterClear"), None)?;
    connection.execute("COMMIT")?;
    require(after_clear > before_clear, "clear reused AUTOINCREMENT id")?;
    connection.execute("BEGIN IMMEDIATE")?;
    connection.execute("DELETE FROM optimistic_layers")?;
    connection.execute("DELETE FROM mutation_queue")?;
    connection.execute("COMMIT")
}

fn seed_reopen_state(connection: &Arc<Connection>) -> Result<i64> {
    connection.execute("BEGIN IMMEDIATE")?;
    let id = enqueue(connection, Some("PreserveAcrossReopen"), None)?;
    require(
        execute(
            connection,
            "UPDATE mutation_queue SET attempt_count = ?2, next_attempt_at_ms = NULL, lease_owner = ?3, lease_generation = ?4, lease_expires_at_ms = ?5 WHERE id = ?1",
            vec![
                Value::from_i64(id),
                Value::from_i64(3),
                Value::from_text("reopen-owner"),
                Value::from_i64(7),
                Value::from_i64(9_999),
            ],
        )? == 1,
        "reopen lease seed update failed",
    )?;
    upsert_records_reused(connection, &[("Reopen", "record", &[4, 2])])?;
    connection.execute("COMMIT")?;
    Ok(id)
}

fn check_transactions(
    connection: &Arc<Connection>,
    second: &Arc<Connection>,
    errors: &mut Vec<ErrorEvidence>,
) -> Result<()> {
    connection.execute("BEGIN")?;
    execute(
        connection,
        "INSERT INTO records (__typename, id, value) VALUES (?1, ?2, ?3)",
        vec![
            Value::from_text("Rollback"),
            Value::from_text("record"),
            Value::from_blob(vec![1]),
        ],
    )?;
    connection.execute("ROLLBACK")?;
    require(
        scalar_i64(
            connection,
            "SELECT COUNT(*) FROM records WHERE __typename = 'Rollback'",
        )? == 0,
        "deferred rollback retained a row",
    )?;

    connection.execute("BEGIN IMMEDIATE")?;
    execute(
        connection,
        "INSERT INTO records (__typename, id, value) VALUES (?1, ?2, ?3)",
        vec![
            Value::from_text("RollbackFailure"),
            Value::from_text("record"),
            Value::from_blob(vec![1]),
        ],
    )?;
    let constraint = execute(
        connection,
        "INSERT INTO meta (key, value) VALUES ('scope', ?1)",
        vec![Value::from_text("duplicate")],
    )
    .expect_err("duplicate metadata key unexpectedly succeeded");
    errors.push(classify(
        "statement_constraint_then_rollback",
        &constraint,
        false,
    ));
    connection.execute("ROLLBACK")?;
    require(
        scalar_i64(
            connection,
            "SELECT COUNT(*) FROM records WHERE __typename = 'RollbackFailure'",
        )? == 0,
        "rollback after statement failure retained a row",
    )?;

    let before = scalar_i64(connection, "SELECT COUNT(*) FROM records")?;
    connection.execute("BEGIN")?;
    let snapshot_before = scalar_i64(connection, "SELECT COUNT(*) FROM records")?;
    second.execute("BEGIN IMMEDIATE")?;
    execute(
        second,
        "INSERT INTO records (__typename, id, value) VALUES (?1, ?2, ?3)",
        vec![
            Value::from_text("Snapshot"),
            Value::from_text("new"),
            Value::from_blob(vec![1]),
        ],
    )?;
    second.execute("COMMIT")?;
    let snapshot_after = scalar_i64(connection, "SELECT COUNT(*) FROM records")?;
    connection.execute("COMMIT")?;
    let after = scalar_i64(connection, "SELECT COUNT(*) FROM records")?;
    require(
        snapshot_before == before && snapshot_after == before && after == before + 1,
        "read transaction did not preserve one snapshot",
    )?;

    let foreign_key_error = execute(
        connection,
        "INSERT INTO optimistic_layers (mutation_id, optimistic_data_json, normalized_updates) VALUES (?1, ?2, ?3)",
        vec![
            Value::from_i64(88_888),
            Value::from_text("{}"),
            Value::from_blob(vec![1]),
        ],
    )
    .expect_err("enabled foreign key accepted orphan");
    errors.push(classify("foreign_key_violation", &foreign_key_error, false));
    Ok(())
}

fn check_pragmas(
    connection: &Arc<Connection>,
    second: &Arc<Connection>,
) -> Result<(Vec<String>, bool, Option<ErrorEvidence>)> {
    let quick_check = query(connection, "PRAGMA quick_check", Vec::new(), |row| {
        row.get::<String>(0)
    })?;
    require(
        quick_check == vec!["ok"],
        "quick_check result shape mismatch",
    )?;
    let valid_rows = match query(connection, "PRAGMA foreign_key_check", Vec::new(), |row| {
        Ok((
            row.get_value(0).clone(),
            row.get_value(1).clone(),
            row.get_value(2).clone(),
            row.get_value(3).clone(),
        ))
    }) {
        Ok(rows) => rows,
        Err(error) => {
            return Ok((
                quick_check,
                false,
                Some(classify(
                    "pragma_foreign_key_check_unsupported",
                    &error,
                    true,
                )),
            ));
        }
    };
    require(
        valid_rows.is_empty(),
        "valid database had foreign-key violations",
    )?;

    second.execute("PRAGMA foreign_keys = OFF")?;
    execute(
        second,
        "INSERT INTO optimistic_layers (mutation_id, optimistic_data_json, normalized_updates) VALUES (?1, ?2, ?3)",
        vec![
            Value::from_i64(77_777),
            Value::from_text("{}"),
            Value::from_blob(vec![0]),
        ],
    )?;
    let invalid_rows = query(connection, "PRAGMA foreign_key_check", Vec::new(), |row| {
        Ok((
            row.get_value(0).clone(),
            row.get_value(1).clone(),
            row.get_value(2).clone(),
            row.get_value(3).clone(),
        ))
    })?;
    execute(
        second,
        "DELETE FROM optimistic_layers WHERE mutation_id = ?1",
        vec![Value::from_i64(77_777)],
    )?;
    if invalid_rows.is_empty() {
        let error = LimboError::InternalError(
            "PRAGMA foreign_key_check returned no rows for a deliberate violation".to_owned(),
        );
        Ok((
            quick_check,
            false,
            Some(classify(
                "pragma_foreign_key_check_silent_noop",
                &error,
                true,
            )),
        ))
    } else {
        Ok((quick_check, true, None))
    }
}

fn probe_faults() -> Result<Vec<ErrorEvidence>> {
    let mut errors = Vec::new();

    let (_database, connection, io) = open("storage-full.db")?;
    connection.execute("PRAGMA journal_mode = WAL")?;
    connection.execute("BEGIN IMMEDIATE")?;
    io.arm(Fault::WriteStorageFull);
    let error = connection
        .execute("CREATE TABLE storage_full_probe(v BLOB)")
        .and_then(|()| connection.execute("COMMIT"))
        .expect_err("injected storage-full write unexpectedly succeeded");
    errors.push(classify("storage_full_write", &error, true));

    let (_database, connection, io) = open("commit-sync.db")?;
    connection.execute("PRAGMA journal_mode = WAL")?;
    connection.execute("CREATE TABLE commit_sync_probe(v INTEGER)")?;
    connection.execute("BEGIN IMMEDIATE")?;
    connection.execute("INSERT INTO commit_sync_probe VALUES(1)")?;
    io.arm(Fault::SyncOther);
    let error = connection
        .execute("COMMIT")
        .expect_err("injected commit sync error unexpectedly succeeded");
    errors.push(classify("commit_sync_uncertain", &error, true));

    let (database, connection, io) = open("corrupt-header.db")?;
    connection.execute("PRAGMA journal_mode = WAL")?;
    connection.execute("CREATE TABLE corruption_probe(v INTEGER)")?;
    connection.close()?;
    drop(connection);
    drop(database);
    io.arm(Fault::CorruptMainHeaderRead);
    let error = match reopen(io) {
        Err(error) => error,
        Ok((_database, connection)) => connection
            .execute("PRAGMA quick_check")
            .expect_err("corrupt header read unexpectedly passed"),
    };
    errors.push(classify("corrupt_header_reopen", &error, true));
    Ok(errors)
}

pub(crate) fn exercise_wp04_contract() -> Result<Wp04Report> {
    let (database, connection, io) = open("wp04-contract.db")?;
    let journal_mode = initialize(&connection)?;
    check_metadata_and_bindings(&connection)?;

    let second = database.connect()?;
    let second_default_fk = scalar_i64(&second, "PRAGMA foreign_keys")?;
    require(
        second_default_fk == 0,
        "foreign_keys was not connection-local",
    )?;
    second.execute("PRAGMA foreign_keys = ON")?;
    require(
        scalar_i64(&second, "PRAGMA foreign_keys")? == 1,
        "second connection could not enable foreign keys",
    )?;

    let mut errors = Vec::new();
    check_conversions(&mut errors)?;
    check_records(&connection)?;
    check_transactions(&connection, &second, &mut errors)?;
    check_queue(&connection, &second)?;
    let (quick_check_rows, foreign_key_check_supported, foreign_key_check_error) =
        check_pragmas(&connection, &second)?;
    let reopen_id = seed_reopen_state(&connection)?;
    errors.extend(probe_faults()?);

    connection.close()?;
    second.close()?;
    drop(connection);
    drop(second);
    drop(database);

    let (reopened_database, reopened) = reopen(io.clone())?;
    reopened.execute("PRAGMA foreign_keys = ON")?;
    require(
        scalar_i64(&reopened, "PRAGMA foreign_keys")? == 1,
        "foreign keys did not re-enable after reopen",
    )?;
    check_metadata_and_bindings(&reopened)?;
    require(
        query(&reopened, "PRAGMA quick_check", Vec::new(), |row| {
            row.get::<String>(0)
        })? == vec!["ok"],
        "quick_check failed after clean reopen",
    )?;
    let reopened_record = query(
        &reopened,
        "SELECT value FROM records WHERE __typename = ?1 AND id = ?2",
        vec![Value::from_text("Reopen"), Value::from_text("record")],
        |row| {
            Ok(row
                .get_value(0)
                .to_blob()
                .ok_or(LimboError::InvalidColumnType)?
                .to_vec())
        },
    )?;
    let reopened_queue = query(
        &reopened,
        "SELECT m.id, m.attempt_count, m.lease_owner, m.lease_generation, m.lease_expires_at_ms, o.optimistic_data_json, o.normalized_updates FROM mutation_queue AS m LEFT JOIN optimistic_layers AS o ON o.mutation_id = m.id ORDER BY m.id ASC",
        Vec::new(),
        |row| {
            Ok((
                row.get::<i64>(0)?,
                row.get::<i64>(1)?,
                nullable_string(row, 2)?,
                row.get::<i64>(3)?,
                nullable_i64(row, 4)?,
                nullable_string(row, 5)?,
                row.get_value(6)
                    .to_blob()
                    .ok_or(LimboError::InvalidColumnType)?
                    .to_vec(),
            ))
        },
    )?;
    require(
        reopened_record == vec![vec![4, 2]]
            && reopened_queue
                == vec![(
                    reopen_id,
                    3,
                    Some("reopen-owner".to_owned()),
                    7,
                    Some(9_999),
                    Some("{}".to_owned()),
                    vec![reopen_id as u8],
                )],
        "clean reopen did not preserve records, queue, lease, and layer",
    )?;
    reopened.close()?;
    drop(reopened);
    drop(reopened_database);

    let ddl_dml_passed = true;
    let canonical_scan_passed = true;
    let queue_contract_passed = true;
    let transaction_contract_passed = true;
    let clean_reopen_passed = true;
    let foreign_keys_connection_local = true;
    let conversion_contract_passed = true;
    let quick_check_passed = quick_check_rows == ["ok"];
    let classified_error_contract_passed = [
        ("statement_constraint_then_rollback", "Constraint"),
        ("foreign_key_violation", "ForeignKeyConstraint"),
        (
            "storage_full_write",
            "CompletionError::IOError(StorageFull)",
        ),
        ("commit_sync_uncertain", "CompletionError::IOError"),
        ("corrupt_header_reopen", "Corrupt"),
    ]
    .iter()
    .all(|(scenario, class)| {
        errors
            .iter()
            .any(|error| error.scenario == *scenario && error.class == *class)
    });
    require(
        classified_error_contract_passed,
        "classified error coverage was incomplete",
    )?;

    // This intentionally excludes the separately reported unsupported
    // foreign_key_check contract and every not-tested integration requirement.
    // It is not a full WP-04 or Gate G0 result.
    let runnable_wp04_sql_passed = ddl_dml_passed
        && canonical_scan_passed
        && queue_contract_passed
        && transaction_contract_passed
        && clean_reopen_passed
        && quick_check_passed
        && foreign_keys_connection_local
        && conversion_contract_passed
        && classified_error_contract_passed;

    let coverage_matrix = vec![
        Wp04CoverageItem {
            requirement: "schema_metadata_bindings_and_dml".to_owned(),
            status: Wp04CoverageStatus::TestedPassed,
            evidence: "schema, metadata, bound values, affected rows, AUTOINCREMENT, and last_insert_rowid executed".to_owned(),
        },
        Wp04CoverageItem {
            requirement: "canonical_key_scan_and_cursor".to_owned(),
            status: Wp04CoverageStatus::TestedPassed,
            evidence: "binary canonical ordering, dynamic IN, bound LIMIT, prefix typenames, embedded colon, and exclusive cursor matched Rust ordering".to_owned(),
        },
        Wp04CoverageItem {
            requirement: "queue_layer_order_claim_fencing_and_clear".to_owned(),
            status: Wp04CoverageStatus::TestedPassed,
            evidence: "joins, orphan checks, strict head, claim/defer/complete/discard fencing, cascade, and atomic clear executed".to_owned(),
        },
        Wp04CoverageItem {
            requirement: "transaction_commit_rollback_and_snapshot".to_owned(),
            status: Wp04CoverageStatus::TestedPassed,
            evidence: "deferred and immediate commit/rollback, statement-error rollback, DDL rollback, and two-connection WAL snapshot executed".to_owned(),
        },
        Wp04CoverageItem {
            requirement: "clean_close_and_reopen_persistence".to_owned(),
            status: Wp04CoverageStatus::TestedPassed,
            evidence: "schema, metadata, records, queue lease state, layer, and quick_check survived clean reopen".to_owned(),
        },
        Wp04CoverageItem {
            requirement: "quick_check_valid_database_shape".to_owned(),
            status: Wp04CoverageStatus::TestedPassed,
            evidence: "PRAGMA quick_check returned exactly one ok row before and after reopen".to_owned(),
        },
        Wp04CoverageItem {
            requirement: "foreign_keys_connection_local_enforcement_and_cascade".to_owned(),
            status: Wp04CoverageStatus::TestedPassed,
            evidence: "foreign_keys readback was connection-local; enabled violation rejection and ON DELETE CASCADE executed".to_owned(),
        },
        Wp04CoverageItem {
            requirement: "checked_conversion_and_classified_core_errors".to_owned(),
            status: Wp04CoverageStatus::TestedPassed,
            evidence: "key/numeric conversion, constraint, foreign-key, storage-full, uncertain commit-sync, and corrupt-read variants were classified".to_owned(),
        },
        Wp04CoverageItem {
            requirement: "foreign_key_check_valid_and_invalid_result_shape".to_owned(),
            status: if foreign_key_check_supported {
                Wp04CoverageStatus::TestedPassed
            } else {
                Wp04CoverageStatus::TestedFailed
            },
            evidence: if foreign_key_check_supported {
                "valid database returned no rows and deliberate orphan returned a four-column violation row".to_owned()
            } else {
                "PRAGMA foreign_key_check returned no rows for a deliberate orphan and is a silent no-op".to_owned()
            },
        },
        Wp04CoverageItem {
            requirement: "rollback_io_failure_classification".to_owned(),
            status: Wp04CoverageStatus::NotTested,
            evidence: "a distinct rollback-I/O failure was not reachable with the synchronous memory-backed File".to_owned(),
        },
        Wp04CoverageItem {
            requirement: "application_reset_after_uncertain_commit_or_rollback".to_owned(),
            status: Wp04CoverageStatus::NotTested,
            evidence: "fault injection proves surfaced core variants, not the consuming application's physical-reset decision".to_owned(),
        },
        Wp04CoverageItem {
            requirement: "physical_reset_for_metadata_schema_integrity_and_scope_mismatch".to_owned(),
            status: Wp04CoverageStatus::NotTested,
            evidence: "this core-only harness does not implement or invoke the future consuming storage reset policy".to_owned(),
        },
        Wp04CoverageItem {
            requirement: "cache_core_codec_corruption_and_storage_trait_conformance".to_owned(),
            status: Wp04CoverageStatus::NotTested,
            evidence: "cache-core codecs and the future shared Storage trait are outside this standalone Turso core spike".to_owned(),
        },
        Wp04CoverageItem {
            requirement: "real_opfs_quota_private_mode_eviction_and_crash_durability".to_owned(),
            status: Wp04CoverageStatus::NotTested,
            evidence: "custom deterministic memory files are not browser OPFS and cannot establish browser storage behavior".to_owned(),
        },
    ];

    let mut limitations = Vec::new();
    if !foreign_key_check_supported {
        limitations.push(
            "PRAGMA foreign_key_check is not implemented by this Turso revision; the required valid/invalid result-shape gate fails"
                .to_owned(),
        );
    }
    limitations.extend(
        coverage_matrix
            .iter()
            .filter(|item| item.status == Wp04CoverageStatus::NotTested)
            .map(|item| format!("not tested: {} — {}", item.requirement, item.evidence)),
    );

    Ok(Wp04Report {
        revision: revision().to_owned(),
        journal_mode,
        ddl_dml_passed,
        canonical_scan_passed,
        queue_contract_passed,
        transaction_contract_passed,
        clean_reopen_passed,
        quick_check_rows,
        foreign_key_check_supported,
        foreign_key_check_error,
        foreign_keys_connection_local,
        conversion_contract_passed,
        errors,
        io: io.evidence(),
        runnable_wp04_sql_passed,
        coverage_matrix,
        limitations,
    })
}
