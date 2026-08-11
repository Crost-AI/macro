use super::*;

fn inspect_fixture(name: &str, wat: &str) -> Inspection {
    let bytes = wat::parse_str(wat).unwrap_or_else(|error| panic!("parse {name}: {error}"));
    inspect_bytes(PathBuf::from(name), &bytes, None, None)
        .unwrap_or_else(|error| panic!("inspect {name}: {error}"))
}

#[test]
fn accepts_exactly_one_unshared_32_bit_memory() {
    let inspection = inspect_fixture("valid.wat", include_str!("../fixtures/valid.wat"));
    assert!(inspection.browser_contract_compliant);
    assert!(inspection.contract_violations.is_empty());
}

#[test]
fn rejects_zero_memories() {
    let inspection = inspect_fixture(
        "zero-memory.wat",
        include_str!("../fixtures/zero-memory.wat"),
    );
    assert!(!inspection.browser_contract_compliant);
    assert_eq!(inspection.memories.len(), 0);
}

#[test]
fn rejects_multiple_memories() {
    let inspection = inspect_fixture(
        "multiple-memories.wat",
        include_str!("../fixtures/multiple-memories.wat"),
    );
    assert!(!inspection.browser_contract_compliant);
    assert_eq!(inspection.memories.len(), 2);
}

#[test]
fn rejects_memory64() {
    let inspection = inspect_fixture("memory64.wat", include_str!("../fixtures/memory64.wat"));
    assert!(!inspection.browser_contract_compliant);
    assert!(inspection.memories[0].memory64);
}

#[test]
fn rejects_shared_memory() {
    let inspection = inspect_fixture(
        "shared-memory.wat",
        include_str!("../fixtures/shared-memory.wat"),
    );
    assert!(!inspection.browser_contract_compliant);
    assert!(inspection.memories[0].shared);
}

#[test]
fn rejects_atomic_instructions() {
    let inspection = inspect_fixture("atomic.wat", include_str!("../fixtures/atomic.wat"));
    assert!(!inspection.browser_contract_compliant);
    assert_eq!(inspection.atomic_operator_count, 1);
}

#[test]
fn accepts_an_exact_import_set() {
    let bytes = wat::parse_str(include_str!("../fixtures/import.wat")).unwrap();
    let allowed = BTreeSet::from(["browser\tunexpected\tfunction".to_string()]);
    let inspection =
        inspect_bytes(PathBuf::from("import.wat"), &bytes, None, Some(&allowed)).unwrap();
    assert!(inspection.browser_contract_compliant);
    assert_eq!(inspection.imports_allowed, Some(true));
}

#[test]
fn rejects_unexpected_and_missing_imports() {
    let bytes = wat::parse_str(include_str!("../fixtures/import.wat")).unwrap();
    let allowed = BTreeSet::from([
        "browser\tallowed\tfunction".to_string(),
        "browser\tmissing\tfunction".to_string(),
    ]);
    let inspection =
        inspect_bytes(PathBuf::from("import.wat"), &bytes, None, Some(&allowed)).unwrap();
    assert!(!inspection.browser_contract_compliant);
    assert_eq!(
        inspection.unexpected_imports,
        vec!["browser\tunexpected\tfunction"]
    );
    assert_eq!(
        inspection.missing_imports,
        vec!["browser\tallowed\tfunction", "browser\tmissing\tfunction"]
    );
    assert!(inspection.duplicate_imports.is_empty());
}

#[test]
fn rejects_duplicate_actual_imports() {
    let bytes = wat::parse_str(
        r#"(module
          (import "browser" "duplicate" (func))
          (import "browser" "duplicate" (func))
          (memory 1))"#,
    )
    .unwrap();
    let allowed = BTreeSet::from(["browser\tduplicate\tfunction".to_string()]);
    let inspection = inspect_bytes(
        PathBuf::from("duplicate-import.wat"),
        &bytes,
        None,
        Some(&allowed),
    )
    .unwrap();
    assert!(!inspection.browser_contract_compliant);
    assert_eq!(inspection.imports_allowed, Some(false));
    assert_eq!(
        inspection.duplicate_imports,
        vec!["browser\tduplicate\tfunction"]
    );
    assert!(inspection.unexpected_imports.is_empty());
    assert!(inspection.missing_imports.is_empty());
}

#[test]
fn rejects_duplicate_allowlist_entries() {
    let path = std::env::temp_dir().join(format!(
        "inspect-turso-wasm-duplicate-allowlist-{}.tsv",
        std::process::id()
    ));
    std::fs::write(
        &path,
        "browser\tduplicate\tfunction\nbrowser\tduplicate\tfunction\n",
    )
    .unwrap();
    let error = load_expected_imports(&path).expect_err("duplicate allowlist was accepted");
    std::fs::remove_file(&path).unwrap();
    assert!(error.contains("duplicate import allowlist entry"));
}

#[test]
fn rejects_wasi_thread_and_worker_imports() {
    for (name, wat) in [
        (
            "wasi",
            r#"(module (import "wasi_snapshot_preview1" "clock_time_get" (func)) (memory 1))"#,
        ),
        (
            "thread",
            r#"(module (import "env" "thread_spawn" (func)) (memory 1))"#,
        ),
        (
            "worker",
            r#"(module (import "browser" "new_Worker" (func)) (memory 1))"#,
        ),
    ] {
        let inspection = inspect_fixture(name, wat);
        assert!(!inspection.browser_contract_compliant, "accepted {name}");
    }
}

#[test]
fn inventories_clock_random_and_filesystem_imports() {
    let inspection = inspect_fixture(
        "platform-imports",
        r#"(module
          (import "browser" "Date_now" (func))
          (import "browser" "getRandomValues" (func))
          (import "browser" "wasi:filesystem/read" (func))
          (memory 1))"#,
    );
    assert_eq!(inspection.clock_time_imports.len(), 1);
    assert_eq!(inspection.random_crypto_imports.len(), 1);
    assert_eq!(inspection.filesystem_imports.len(), 1);
    assert_eq!(inspection.wasi_imports.len(), 1);
    assert!(!inspection.browser_contract_compliant);
}
