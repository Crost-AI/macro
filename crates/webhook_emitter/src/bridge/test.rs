use chrono::Utc;
use models_properties::{EntityType, service::property_value::PropertyValue};
use properties::domain::events::{EntityPropertyUpdatedMetadata, PropertyTopicEvent};
use system_properties::SystemPropertyKey;
use uuid::Uuid;

use crate::events::TASK_UPDATED;

#[test]
fn maps_task_status_property_update_with_status_field() {
    let status_id = Uuid::parse_str("00000001-0000-0000-0000-000000000002").expect("status");
    let previous_status_id = Uuid::parse_str("00000001-0000-0000-0000-000000000001").expect("prev");

    let envelope = super::map_property_event(PropertyTopicEvent::EntityPropertyUpdated(
        EntityPropertyUpdatedMetadata {
            entity_property_id: Uuid::now_v7(),
            entity_id: "task-42".into(),
            entity_type: EntityType::Task,
            property_definition_id: SystemPropertyKey::STATUS_UUID,
            actor_user_id: None,
            value: Some(PropertyValue::SelectOption(vec![status_id])),
            previous_value: Some(PropertyValue::SelectOption(vec![previous_status_id])),
            updated_at: Utc::now(),
        },
    ))
    .expect("mapped");

    assert_eq!(envelope.event_type, TASK_UPDATED);
    assert_eq!(envelope.metadata["task_id"], "task-42");
    assert_eq!(envelope.metadata["status"], status_id.to_string());
    assert_eq!(
        envelope.metadata["previous_status"],
        previous_status_id.to_string()
    );
}

#[test]
fn ignores_non_task_property_updates() {
    let envelope = super::map_property_event(PropertyTopicEvent::EntityPropertyUpdated(
        EntityPropertyUpdatedMetadata {
            entity_property_id: Uuid::now_v7(),
            entity_id: "doc-1".into(),
            entity_type: EntityType::Document,
            property_definition_id: SystemPropertyKey::STATUS_UUID,
            actor_user_id: None,
            value: None,
            previous_value: None,
            updated_at: Utc::now(),
        },
    ));
    assert!(envelope.is_none());
}
