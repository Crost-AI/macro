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
fn rejects_import_outside_explicit_allowlist() {
    let bytes = wat::parse_str(include_str!("../fixtures/import.wat")).unwrap();
    let allowed = BTreeSet::from(["browser\tallowed\tfunction".to_string()]);
    let inspection =
        inspect_bytes(PathBuf::from("import.wat"), &bytes, None, Some(&allowed)).unwrap();
    assert!(!inspection.browser_contract_compliant);
    assert_eq!(
        inspection.unexpected_imports,
        vec!["browser\tunexpected\tfunction"]
    );
}
