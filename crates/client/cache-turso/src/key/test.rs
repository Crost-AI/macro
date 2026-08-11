use super::*;

#[test]
fn checked_entity_key_round_trips() {
    for value in [
        "ROOT_QUERY",
        "GraphqlSoupDocument:doc-1",
        "GraphqlSoupDocument:tenant:doc-1",
        "__meta:identity",
        "Thing:",
    ] {
        let entity = EntityKey(Cow::Borrowed(value));
        let pair = RecordKey::from_entity(&entity).unwrap();
        assert_eq!(pair.into_entity().unwrap().as_ref(), value);
    }
}

#[test]
fn invalid_entity_keys_and_sql_pairs_are_rejected() {
    for value in ["", "Thing", ":id", "ROOT_QUERY:"] {
        assert!(RecordKey::from_entity(&EntityKey(Cow::Borrowed(value))).is_err());
    }
    for pair in [
        RecordKey {
            typename: String::new(),
            id: "id".into(),
        },
        RecordKey {
            typename: "ROOT_QUERY".into(),
            id: String::new(),
        },
    ] {
        if pair.typename.is_empty() {
            assert!(pair.into_entity().unwrap_err().requires_physical_reset());
        } else {
            assert!(pair.into_entity().is_ok());
        }
    }
}
