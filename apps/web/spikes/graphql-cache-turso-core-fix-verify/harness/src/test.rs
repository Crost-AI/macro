use super::*;

#[test]
fn unused_temp_behavior_matches_revision() {
    for mode in ["IMMEDIATE", "EXCLUSIVE"] {
        let report = exercise_unused_temp_transaction(mode).unwrap();
        #[cfg(feature = "parent-revision")]
        assert!(
            report.temp_database_listed,
            "parent did not open temp for {mode}"
        );
        #[cfg(feature = "head-revision")]
        assert!(
            !report.temp_database_listed,
            "fixed head still opened unused temp for {mode}"
        );
        assert!(report.io.open_calls > 0);
        assert!(report.io.monotonic_clock_calls > 0);
    }
}

#[test]
fn explicit_temp_behavior_is_not_conflated_with_unused_temp() {
    let report = exercise_explicit_temp_native().unwrap();
    assert_eq!(report.committed_rows, vec![1]);
    assert!(report.database_names.iter().any(|name| name == "temp"));
}

#[test]
fn wp04_contract_executes_and_records_unsupported_foreign_key_check() {
    let report = exercise_wp04_contract().unwrap();
    assert!(report.ddl_dml_passed);
    assert!(report.canonical_scan_passed);
    assert!(report.queue_contract_passed);
    assert!(report.transaction_contract_passed);
    assert!(report.clean_reopen_passed);
    assert_eq!(report.quick_check_rows, vec!["ok"]);
    assert!(!report.foreign_key_check_supported);
    assert!(report.runnable_wp04_sql_passed);
    assert!(report.foreign_key_check_error.is_some());
    let foreign_key_check = report
        .coverage_matrix
        .iter()
        .find(|item| item.requirement == "foreign_key_check_valid_and_invalid_result_shape")
        .expect("foreign_key_check coverage entry");
    assert_eq!(foreign_key_check.status, Wp04CoverageStatus::TestedFailed);
    let untested = report
        .coverage_matrix
        .iter()
        .filter(|item| item.status == Wp04CoverageStatus::NotTested)
        .map(|item| item.requirement.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        untested,
        vec![
            "rollback_io_failure_classification",
            "application_reset_after_uncertain_commit_or_rollback",
            "physical_reset_for_metadata_schema_integrity_and_scope_mismatch",
            "cache_core_codec_corruption_and_storage_trait_conformance",
            "real_opfs_quota_private_mode_eviction_and_crash_durability",
        ]
    );
    assert!(report
        .errors
        .iter()
        .any(|error| error.class == "ForeignKeyConstraint"));
    assert!(report
        .errors
        .iter()
        .any(|error| error.class == "CompletionError::IOError(StorageFull)"));
    assert!(report
        .errors
        .iter()
        .any(|error| error.scenario == "commit_sync_uncertain"));
    assert!(report
        .errors
        .iter()
        .any(|error| error.scenario == "corrupt_header_reopen"));
    assert!(report.io.open_calls > 0);
    assert!(report.io.monotonic_clock_calls > 0);
}
