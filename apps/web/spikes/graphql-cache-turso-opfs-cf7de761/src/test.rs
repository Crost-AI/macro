use super::*;

#[test]
fn turso_trait_objects_use_only_send_sync_numeric_tokens() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OwnerId>();
    assert_send_sync::<SessionId>();
    assert_send_sync::<HandleId>();
    assert_send_sync::<OpfsFile>();
    assert_send_sync::<OpfsIo>();
    assert_eq!(std::mem::size_of::<HandleId>(), std::mem::size_of::<u32>());
    assert_eq!(
        std::mem::size_of::<OpfsFile>(),
        3 * std::mem::size_of::<u32>()
    );
}

#[test]
fn write_all_retries_partial_writes_to_exact_length() {
    let mut calls = Vec::new();
    let written = write_all_with(10, b"abcdef", |pos, bytes| {
        calls.push((pos, bytes.len()));
        Ok(bytes.len().min(2))
    })
    .unwrap();
    assert_eq!(written, 6);
    assert_eq!(calls, vec![(10, 6), (12, 4), (14, 2)]);
}

#[test]
fn aggregate_write_length_defines_empty_and_out_of_range_semantics() {
    let written = write_all_with(u64::MAX, &[], |_, _| panic!("empty write called backend"));
    assert_eq!(written, Ok(0));
    assert_eq!(completion_write_len(0), Ok(0));
    assert_eq!(completion_write_len(i32::MAX as usize), Ok(i32::MAX));
    assert_eq!(
        completion_write_len(i32::MAX as usize + 1),
        Err(CompletionError::ShortWrite)
    );
    assert_eq!(
        completion_write_len(usize::MAX),
        Err(CompletionError::ShortWrite)
    );
}

#[test]
fn write_all_rejects_zero_or_oversized_progress() {
    assert_eq!(
        write_all_with(0, b"x", |_, _| Ok(0)),
        Err(CompletionError::ShortWrite)
    );
    assert_eq!(
        write_all_with(0, b"x", |_, _| Ok(2)),
        Err(CompletionError::ShortWrite)
    );
}

#[test]
fn write_all_preserves_specific_write_failure() {
    let quota = CompletionError::IOError(ErrorKind::StorageFull, "quota-test");
    assert_eq!(write_all_with(0, b"x", |_, _| Err(quota)), Err(quota));
}

#[test]
fn completion_finisher_calls_once_and_preserves_error() {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(None));
    let callback_count = count.clone();
    let callback_seen = seen.clone();
    let completion = Completion::new_write(move |result| {
        callback_count.fetch_add(1, Ordering::SeqCst);
        *callback_seen.lock().unwrap() = Some(result);
    });
    let quota = CompletionError::IOError(ErrorKind::StorageFull, "quota-test");
    finish_completion(&completion, Err(LimboError::CompletionError(quota)));
    finish_completion(&completion, Ok(1));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(*seen.lock().unwrap(), Some(Err(quota)));
    assert_eq!(completion.get_error(), Some(quota));
}

#[test]
fn lifecycle_rejects_reentry_and_delete_after_poison() {
    let mut registry = HandleRegistry::default();
    let owner = registry.claim_owner().unwrap();
    let session = registry
        .start_opening(owner, SessionKind::DirectProbe)
        .unwrap();
    registry.lifecycle = Lifecycle::Active {
        owner,
        session,
        kind: SessionKind::DirectProbe,
        operation_active: false,
    };
    registry.begin_operation(owner, session).unwrap();
    assert!(registry.begin_operation(owner, session).is_err());
    registry.end_operation(owner, session);
    registry.poison(owner, "injected uncertain close".to_string(), Vec::new());
    assert!(registry
        .consume_closed_for_reset(owner, CloseToken(1))
        .is_err());
    assert!(registry.release_owner(owner).is_err());
}

#[test]
fn recreation_fault_is_matching_close_token_scoped() {
    let mut registry = HandleRegistry::default();
    let owner = registry.claim_owner().unwrap();
    let token = CloseToken(1);
    registry.lifecycle = Lifecycle::Closed {
        owner,
        token,
        kind: SessionKind::DirectProbe,
        sizes: Vec::new(),
        inject_recreation_conflict: false,
    };
    assert!(registry
        .inject_recreation_conflict(OwnerId(owner.0 + 1), token)
        .is_err());
    assert!(registry
        .inject_recreation_conflict(owner, CloseToken(token.0 + 1))
        .is_err());
    registry.inject_recreation_conflict(owner, token).unwrap();
    let (_, _, inject) = registry.consume_closed_for_reset(owner, token).unwrap();
    assert!(inject);
}

#[test]
fn open_validation_enforces_allowed_paths_flags_and_direct_mode() {
    let owner = OwnerId(1);
    let session = SessionId(1);
    let mut registry = HandleRegistry {
        lifecycle: Lifecycle::Active {
            owner,
            session,
            kind: SessionKind::Database,
            operation_active: false,
        },
        ..HandleRegistry::default()
    };
    registry.by_path.insert(DATABASE_PATH, HandleId(1));
    registry.by_path.insert(DATABASE_WAL_PATH, HandleId(2));

    assert_eq!(
        registry
            .validate_open(owner, session, DATABASE_PATH, OpenFlags::Create, true)
            .unwrap(),
        HandleId(1)
    );
    assert!(registry
        .validate_open(owner, session, DATABASE_PATH, OpenFlags::Create, false)
        .is_err());
    assert!(registry
        .validate_open(owner, session, DATABASE_PATH, OpenFlags::ReadOnly, true)
        .is_err());
    assert!(registry
        .validate_open(owner, session, DATABASE_PATH, OpenFlags::NoLock, true)
        .is_err());
    assert!(registry
        .validate_open(owner, session, "unknown.db", OpenFlags::Create, true)
        .is_err());
    assert_eq!(
        registry
            .validate_open(owner, session, DATABASE_WAL_PATH, OpenFlags::NoLock, false)
            .unwrap(),
        HandleId(2)
    );
}

#[test]
fn foreign_key_violation_shape_is_exact_and_json_safe() {
    let observation = ForeignKeyCheckObservation {
        column_count: 4,
        rows: vec![expected_foreign_key_violation()],
    };
    assert_eq!(
        foreign_key_check_json(&observation),
        "{\"column_count\":4,\"rows\":[{\"table\":\"optimistic_layers\",\"rowid\":9999999,\"parent\":\"mutation_queue\",\"fkid\":0}]}"
    );
    assert_eq!(json_string("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
}

#[test]
fn javascript_number_bounds_are_explicit() {
    assert!(validate_position(MAX_SAFE_INTEGER).is_ok());
    assert!(validate_position(MAX_SAFE_INTEGER + 1).is_err());
    assert_eq!(
        number_to_u64(MAX_SAFE_INTEGER as f64).unwrap(),
        MAX_SAFE_INTEGER
    );
    assert!(number_to_u64(f64::NAN).is_err());
    assert!(number_to_i32(i32::MAX as f64 + 1.0).is_err());
}
