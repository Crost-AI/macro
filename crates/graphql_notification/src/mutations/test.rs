use std::sync::{Arc, Mutex};

use async_graphql::{EmptySubscription, Object, Schema};
use chrono::Utc;
use model_entity::EntityType;
use model_notifications::{ChannelMessageSendMetadata, ChannelType, CommonChannelMetadata};
use notification::domain::models::request::{
    NotificationEntityRef, NotificationItemType, NotificationItemUpdate, NotificationStatus,
};

use super::*;

struct QueryRoot;

#[Object]
impl QueryRoot {
    async fn health(&self) -> bool {
        true
    }
}

#[derive(Default)]
struct CapturingNotificationService {
    calls: Mutex<Vec<(String, Vec<Uuid>, &'static str)>>,
    item_calls: Mutex<Vec<(String, Vec<NotificationEntityRef>, NotificationItemUpdate)>>,
}

impl NotificationMutationService for CapturingNotificationService {
    async fn update_notifications(
        &self,
        user_id: MacroUserIdStr<'static>,
        notification_ids: Vec<Uuid>,
        status: NotificationStatus,
    ) -> Result<Vec<UserNotificationRow<serde_json::Value>>, Report> {
        let operation = match status {
            NotificationStatus::Seen => "seen",
            NotificationStatus::Done(true) => "done",
            NotificationStatus::Done(false) => "undone",
        };
        self.calls
            .lock()
            .unwrap()
            .push((user_id.to_string(), notification_ids.clone(), operation));
        let now = Utc::now();
        Ok(notification_ids
            .into_iter()
            .map(|notification_id| notification_row(user_id.clone(), notification_id, now))
            .collect())
    }

    async fn update_item_notifications(
        &self,
        user_id: MacroUserIdStr<'static>,
        items: Vec<NotificationEntityRef>,
        operation: NotificationItemUpdate,
    ) -> Result<Vec<UserNotificationRow<serde_json::Value>>, Report> {
        self.item_calls
            .lock()
            .unwrap()
            .push((user_id.to_string(), items, operation));
        let now = Utc::now();
        Ok(vec![notification_row(user_id, Uuid::nil(), now)])
    }
}

fn notification_row(
    user_id: MacroUserIdStr<'static>,
    notification_id: Uuid,
    now: chrono::DateTime<Utc>,
) -> UserNotificationRow<serde_json::Value> {
    UserNotificationRow {
        owner_id: user_id,
        notification_id,
        notification_event_type: "channel_message_send".to_string(),
        entity: EntityType::Channel.with_entity_string("channel-1".to_string()),
        sent: true,
        done: false,
        created_at: now,
        viewed_at: Some(now),
        updated_at: now,
        deleted_at: None,
        notification_metadata: serde_json::to_value(ChannelMessageSendMetadata {
            sender: None,
            sender_display_name: None,
            message_content: "Test message".to_string(),
            message_id: "message-1".to_string(),
            has_attachments: false,
            common: CommonChannelMetadata {
                channel_type: ChannelType::Public,
                channel_name: "Test channel".to_string(),
            },
            sender_profile_picture_url: None,
        })
        .unwrap(),
        sender_id: None,
    }
}

#[test]
fn notification_operations_map_to_explicit_domain_statuses() {
    assert!(matches!(
        NotificationStatus::from(GraphqlNotificationUpdateOperation::MarkSeen),
        NotificationStatus::Seen
    ));
    assert!(matches!(
        NotificationStatus::from(GraphqlNotificationUpdateOperation::MarkDone),
        NotificationStatus::Done(true)
    ));
    assert!(matches!(
        NotificationStatus::from(GraphqlNotificationUpdateOperation::MarkUndone),
        NotificationStatus::Done(false)
    ));
}

#[tokio::test]
async fn update_notifications_maps_operation_and_returns_normalized_rows_in_order() {
    let service = Arc::new(CapturingNotificationService::default());
    let user = MacroUserIdStr::try_from_email("user@example.com").unwrap();
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let schema = Schema::build(
        QueryRoot,
        NotificationMutationRoot::<CapturingNotificationService>::new(),
        EmptySubscription,
    )
    .data(service.clone())
    .data(user)
    .finish();

    let response = schema
        .execute(format!(
            r#"mutation {{ updateNotifications(input: {{ notificationIds: ["{second}", "{first}"], operation: MARK_SEEN }}) {{ __typename id seen viewedAt metadata {{ __typename }} }} }}"#
        ))
        .await;

    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(
        data["updateNotifications"][0]["__typename"],
        "GraphqlNotification"
    );
    assert_eq!(
        data["updateNotifications"][0]["metadata"]["__typename"],
        "GraphqlChannelMessageSendMetadata"
    );
    assert_eq!(data["updateNotifications"][0]["id"], second.to_string());
    assert_eq!(data["updateNotifications"][1]["id"], first.to_string());
    assert_eq!(
        service.calls.lock().unwrap().as_slice(),
        [(
            "macro|user@example.com".to_string(),
            vec![second, first],
            "seen",
        ),]
    );
}

#[tokio::test]
async fn update_item_notifications_maps_item_refs_and_done_operation() {
    let service = Arc::new(CapturingNotificationService::default());
    let user = MacroUserIdStr::try_from_email("user@example.com").unwrap();
    let schema = Schema::build(
        QueryRoot,
        NotificationMutationRoot::<CapturingNotificationService>::new(),
        EmptySubscription,
    )
    .data(service.clone())
    .data(user)
    .finish();

    let response = schema
        .execute(
            r#"mutation {
                updateItemNotifications(input: {
                    items: [
                        { itemType: DOCUMENT, itemId: "doc-1" },
                        { itemType: MESSAGE, itemId: "message-1" }
                    ],
                    operation: MARK_DONE
                }) {
                    __typename
                    id
                    done
                }
            }"#,
        )
        .await;

    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(
        data["updateItemNotifications"][0]["__typename"],
        "GraphqlNotification"
    );
    assert_eq!(
        data["updateItemNotifications"][0]["id"],
        Uuid::nil().to_string()
    );

    let calls = service.item_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "macro|user@example.com");
    assert_eq!(calls[0].2, NotificationItemUpdate::Done);
    assert_eq!(
        calls[0].1,
        vec![
            NotificationEntityRef {
                entity_type: NotificationItemType::Document,
                id: "doc-1".to_string(),
            },
            NotificationEntityRef {
                entity_type: NotificationItemType::Message,
                id: "message-1".to_string(),
            },
        ]
    );
}

#[tokio::test]
async fn update_item_notifications_rejects_empty_item_id() {
    let service = Arc::new(CapturingNotificationService::default());
    let user = MacroUserIdStr::try_from_email("user@example.com").unwrap();
    let schema = Schema::build(
        QueryRoot,
        NotificationMutationRoot::<CapturingNotificationService>::new(),
        EmptySubscription,
    )
    .data(service.clone())
    .data(user)
    .finish();

    let response = schema
        .execute(
            r#"mutation {
                updateItemNotifications(input: {
                    items: [{ itemType: DOCUMENT, itemId: "   " }],
                    operation: MARK_SEEN
                }) { id }
            }"#,
        )
        .await;

    assert!(!response.errors.is_empty());
    assert!(service.item_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn update_item_notifications_rejects_oversized_batch() {
    let service = Arc::new(CapturingNotificationService::default());
    let user = MacroUserIdStr::try_from_email("user@example.com").unwrap();
    let schema = Schema::build(
        QueryRoot,
        NotificationMutationRoot::<CapturingNotificationService>::new(),
        EmptySubscription,
    )
    .data(service.clone())
    .data(user)
    .finish();
    let items = (0..101)
        .map(|index| format!(r#"{{ itemType: DOCUMENT, itemId: "doc-{index}" }}"#))
        .collect::<Vec<_>>()
        .join(",");

    let response = schema
        .execute(format!(
            r#"mutation {{
                updateItemNotifications(input: {{
                    items: [{items}],
                    operation: MARK_DONE
                }}) {{ id }}
            }}"#
        ))
        .await;

    assert!(!response.errors.is_empty());
    assert!(service.item_calls.lock().unwrap().is_empty());
}
