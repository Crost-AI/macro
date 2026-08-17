use std::{marker::PhantomData, sync::Arc};

use async_graphql::{Context, Enum, ID, InputObject, Object};
use graphql_common::{parse_id, require_authenticated_user};
use macro_user_id::user_id::MacroUserIdStr;
use notification::domain::{
    models::{
        UserNotificationRow,
        request::{
            NotificationEntityRef, NotificationItemType, NotificationItemUpdate, NotificationStatus,
        },
    },
    service::NotificationReader,
};
use rootcause::Report;
use uuid::Uuid;

use crate::objects::GraphqlNotification;

#[cfg(test)]
mod test;

/// Domain-facing capability required by notification status mutations.
pub trait NotificationMutationService: Send + Sync + 'static {
    /// Update user-owned notifications and return their authoritative active rows.
    fn update_notifications(
        &self,
        user_id: MacroUserIdStr<'static>,
        notification_ids: Vec<Uuid>,
        status: NotificationStatus,
    ) -> impl Future<Output = Result<Vec<UserNotificationRow<serde_json::Value>>, Report>> + Send;

    /// Update active notifications matching user-facing item references.
    fn update_item_notifications(
        &self,
        user_id: MacroUserIdStr<'static>,
        items: Vec<NotificationEntityRef>,
        operation: NotificationItemUpdate,
    ) -> impl Future<Output = Result<Vec<UserNotificationRow<serde_json::Value>>, Report>> + Send;
}

impl<S> NotificationMutationService for S
where
    S: NotificationReader,
{
    async fn update_notifications(
        &self,
        user_id: MacroUserIdStr<'static>,
        notification_ids: Vec<Uuid>,
        status: NotificationStatus,
    ) -> Result<Vec<UserNotificationRow<serde_json::Value>>, Report> {
        self.update_notifications_and_return(
            notification::domain::models::request::UpdateNotificationsRequest {
                user_id,
                notification_ids: &notification_ids,
                status,
            },
        )
        .await
    }

    async fn update_item_notifications(
        &self,
        user_id: MacroUserIdStr<'static>,
        items: Vec<NotificationEntityRef>,
        operation: NotificationItemUpdate,
    ) -> Result<Vec<UserNotificationRow<serde_json::Value>>, Report> {
        self.update_notifications_for_items(
            notification::domain::models::request::UpdateNotificationsForItemsRequest {
                user_id,
                items: &items,
                operation,
            },
        )
        .await
    }
}

/// Schema-only notification mutation service.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoOpNotificationMutationService;

impl NotificationMutationService for NoOpNotificationMutationService {
    async fn update_notifications(
        &self,
        _user_id: MacroUserIdStr<'static>,
        _notification_ids: Vec<Uuid>,
        _status: NotificationStatus,
    ) -> Result<Vec<UserNotificationRow<serde_json::Value>>, Report> {
        Err(rootcause::report!(
            "notification mutations are not configured"
        ))
    }

    async fn update_item_notifications(
        &self,
        _user_id: MacroUserIdStr<'static>,
        _items: Vec<NotificationEntityRef>,
        _operation: NotificationItemUpdate,
    ) -> Result<Vec<UserNotificationRow<serde_json::Value>>, Report> {
        Err(rootcause::report!(
            "notification mutations are not configured"
        ))
    }
}

/// Root GraphQL adapter for notification mutations.
pub struct NotificationMutationRoot<S>(PhantomData<S>);

impl<S> NotificationMutationRoot<S> {
    /// Construct a notification mutation root.
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<S> Default for NotificationMutationRoot<S> {
    fn default() -> Self {
        Self::new()
    }
}

/// Explicit notification status operation exposed by GraphQL.
#[derive(Clone, Copy, Debug, Enum, Eq, PartialEq)]
#[graphql(name = "NotificationUpdateOperation")]
pub enum GraphqlNotificationUpdateOperation {
    /// Mark notifications as seen.
    #[graphql(name = "MARK_SEEN")]
    MarkSeen,
    /// Mark notifications as done.
    #[graphql(name = "MARK_DONE")]
    MarkDone,
    /// Mark notifications as not done.
    #[graphql(name = "MARK_UNDONE")]
    MarkUndone,
}

impl From<GraphqlNotificationUpdateOperation> for NotificationStatus {
    fn from(value: GraphqlNotificationUpdateOperation) -> Self {
        match value {
            GraphqlNotificationUpdateOperation::MarkSeen => Self::Seen,
            GraphqlNotificationUpdateOperation::MarkDone => Self::Done(true),
            GraphqlNotificationUpdateOperation::MarkUndone => Self::Done(false),
        }
    }
}

/// Input for updating notification statuses.
#[derive(InputObject)]
pub struct UpdateNotificationsInput {
    /// User notification identifiers to update.
    pub notification_ids: Vec<ID>,
    /// Status operation applied to every requested notification.
    pub operation: GraphqlNotificationUpdateOperation,
}

/// User-facing notification item type accepted by item-scoped mutations.
#[derive(Clone, Copy, Debug, Enum, Eq, PartialEq)]
#[graphql(name = "NotificationItemType")]
pub enum GraphqlNotificationItemType {
    /// Email thread.
    Email,
    /// Channel message or thread.
    Message,
    /// Channel.
    Channel,
    /// Non-task document.
    Document,
    /// Project.
    Project,
    /// Chat.
    Chat,
    /// Call.
    Call,
    /// Task-backed document.
    Task,
    /// GitHub foreign entity.
    Github,
    /// Reminder.
    Reminder,
    /// Calendar event.
    Calendar,
}

impl From<GraphqlNotificationItemType> for NotificationItemType {
    fn from(value: GraphqlNotificationItemType) -> Self {
        match value {
            GraphqlNotificationItemType::Email => Self::Email,
            GraphqlNotificationItemType::Message => Self::Message,
            GraphqlNotificationItemType::Channel => Self::Channel,
            GraphqlNotificationItemType::Document => Self::Document,
            GraphqlNotificationItemType::Project => Self::Project,
            GraphqlNotificationItemType::Chat => Self::Chat,
            GraphqlNotificationItemType::Call => Self::Call,
            GraphqlNotificationItemType::Task => Self::Task,
            GraphqlNotificationItemType::Github => Self::Github,
            GraphqlNotificationItemType::Reminder => Self::Reminder,
            GraphqlNotificationItemType::Calendar => Self::Calendar,
        }
    }
}

/// One item whose notifications should be updated.
#[derive(InputObject)]
pub struct NotificationItemRefInput {
    /// User-facing item category.
    pub item_type: GraphqlNotificationItemType,
    /// Item identifier.
    #[graphql(validator(min_length = 1))]
    pub item_id: ID,
}

impl From<NotificationItemRefInput> for NotificationEntityRef {
    fn from(value: NotificationItemRefInput) -> Self {
        Self {
            entity_type: value.item_type.into(),
            id: value.item_id.to_string(),
        }
    }
}

/// Status operations supported by item-scoped mutations.
#[derive(Clone, Copy, Debug, Enum, Eq, PartialEq)]
#[graphql(name = "NotificationItemUpdateOperation")]
pub enum GraphqlNotificationItemUpdateOperation {
    /// Mark matching active unseen notifications as seen.
    #[graphql(name = "MARK_SEEN")]
    MarkSeen,
    /// Mark matching active notifications as done.
    #[graphql(name = "MARK_DONE")]
    MarkDone,
}

impl From<GraphqlNotificationItemUpdateOperation> for NotificationItemUpdate {
    fn from(value: GraphqlNotificationItemUpdateOperation) -> Self {
        match value {
            GraphqlNotificationItemUpdateOperation::MarkSeen => Self::Seen,
            GraphqlNotificationItemUpdateOperation::MarkDone => Self::Done,
        }
    }
}

/// Input for updating every active notification matching one or more items.
#[derive(InputObject)]
pub struct UpdateItemNotificationsInput {
    /// Items whose matching notifications should be updated.
    #[graphql(validator(max_items = 100))]
    pub items: Vec<NotificationItemRefInput>,
    /// Status operation applied to matching notifications.
    pub operation: GraphqlNotificationItemUpdateOperation,
}

/// GraphQL notification mutations.
#[Object]
impl<S> NotificationMutationRoot<S>
where
    S: NotificationMutationService,
{
    /// Update authenticated user-owned notifications and return authoritative rows.
    async fn update_notifications(
        &self,
        ctx: &Context<'_>,
        input: UpdateNotificationsInput,
    ) -> async_graphql::Result<Vec<GraphqlNotification>> {
        let user_id = require_authenticated_user(ctx)?;
        let notification_ids = input
            .notification_ids
            .into_iter()
            .map(|id| parse_id(id, "notificationIds"))
            .collect::<async_graphql::Result<Vec<_>>>()?;
        let service = ctx.data::<Arc<S>>()?;
        let notifications = service
            .update_notifications(user_id, notification_ids, input.operation.into())
            .await
            .map_err(|error| async_graphql::Error::new(error.to_string()))?;

        to_graphql_notifications(notifications)
    }

    /// Update every matching active notification owned by the authenticated user.
    async fn update_item_notifications(
        &self,
        ctx: &Context<'_>,
        input: UpdateItemNotificationsInput,
    ) -> async_graphql::Result<Vec<GraphqlNotification>> {
        let user_id = require_authenticated_user(ctx)?;
        if input.items.len() > 100 {
            return Err(async_graphql::Error::new(
                "at most 100 notification items may be updated at once",
            ));
        }
        if input
            .items
            .iter()
            .any(|item| item.item_id.as_ref().trim().is_empty())
        {
            return Err(async_graphql::Error::new(
                "notification item IDs must not be empty",
            ));
        }
        let items = input.items.into_iter().map(Into::into).collect();
        let service = ctx.data::<Arc<S>>()?;
        let notifications = service
            .update_item_notifications(user_id, items, input.operation.into())
            .await
            .map_err(|error| async_graphql::Error::new(error.to_string()))?;

        to_graphql_notifications(notifications)
    }
}

fn to_graphql_notifications(
    notifications: Vec<UserNotificationRow<serde_json::Value>>,
) -> async_graphql::Result<Vec<GraphqlNotification>> {
    notifications
        .into_iter()
        .map(GraphqlNotification::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            tracing::error!(
                error = ?error,
                "failed to deserialize notification metadata"
            );
            async_graphql::Error::new("notification metadata is unavailable")
        })
}
