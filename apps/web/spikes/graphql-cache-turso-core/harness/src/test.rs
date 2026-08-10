use super::*;

#[test]
fn required_cache_schema_and_sql_work() {
    let summary = exercise_required_sql().unwrap();
    assert_eq!(summary.journal_mode, "wal");
    assert_eq!(summary.synchronous, 1);
    assert_eq!(summary.record_count, 2);
    assert_eq!(summary.mutation_id, 1);
    assert!(summary.foreign_key_rejected);
    assert!(summary.cascade_deleted_layer);
    assert!(summary.rollback_discarded_record);
    assert!(summary.compound_key_scan_ok);
    assert!(summary.strict_head_claim_ok);
    assert!(summary.competing_connection_fenced);
    assert_eq!(summary.competing_connection_result, "busy_snapshot");
}
