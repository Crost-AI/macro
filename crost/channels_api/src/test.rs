//! Router integration tests for the W2.8 channels REST contract.

use crate::{
    auth::ServiceApiToken,
    crost_channels_router,
    handlers::SERVICE_ACTOR_USER_ID,
    resolve::UserResolver,
    router::CrostChannelsRouterState,
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header::AUTHORIZATION},
};
use channel_sender::ChannelSender;
use channels::domain::{
    models::{
        AddParticipantsRequest, ChannelAttachment, ChannelMessage, ChannelMessageFilters,
        ChannelMetadata, ChannelParticipant, ChannelType, CreateChannelRequest,
        CreateChannelResponse, MessagePageDirection, ParticipantRole, PostMessageRequest,
        PostMessageResponse, RemoveParticipantsRequest,
    },
    ports::{
        ChannelAttachmentsPage, ChannelMessagesErr, ChannelMessagesQueryResult, ChannelMutationErr,
        ChannelService,
    },
};
use http_body_util::BodyExt;
use macro_user_id::user_id::MacroUserIdStr;
use models_pagination::{CreatedAt, PaginateOn, Query};
use sqlx::PgPool;
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};
use tower::ServiceExt;
use uuid::Uuid;

const TOKEN: &str = "test-service-token";

struct StubUserResolver {
    users: HashSet<String>,
}

impl UserResolver for StubUserResolver {
    fn resolve_user_ref(
        &self,
        user_ref: &str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<MacroUserIdStr<'static>, crate::error::ApiError>> + Send + '_>,
    > {
        let macro_id = if user_ref.contains('|') {
            user_ref.to_string()
        } else if user_ref.contains('@') {
            format!("macro|{user_ref}")
        } else {
            format!("macro|{user_ref}@agents.crost.local")
        };
        let users = self.users.clone();
        Box::pin(async move {
            if users.contains(&macro_id) {
                MacroUserIdStr::try_from(macro_id).map_err(|_| {
                    crate::error::ApiError::bad_request("invalid user_or_agent_ref")
                })
            } else {
                Err(crate::error::ApiError::not_found("user not found"))
            }
        })
    }
}

#[derive(Default)]
struct MockChannelService {
    existing_channels: Mutex<HashSet<Uuid>>,
    participants: Mutex<Vec<ChannelParticipant>>,
    last_create: Mutex<Option<CreateChannelRequest>>,
}

impl MockChannelService {
    fn with_channel(channel_id: Uuid) -> Self {
        let mut channels = HashSet::new();
        channels.insert(channel_id);
        Self {
            existing_channels: Mutex::new(channels),
            ..Default::default()
        }
    }
}

impl ChannelService for MockChannelService {
    async fn get_channel_messages(
        &self,
        _channel_id: Uuid,
        _query: Query<Uuid, CreatedAt, ()>,
        _direction: MessagePageDirection,
        _limit: u16,
        _filters: &ChannelMessageFilters,
        _notification_user_id: Option<MacroUserIdStr<'static>>,
    ) -> Result<ChannelMessagesQueryResult, ChannelMessagesErr> {
        Ok(ChannelMessagesQueryResult {
            page: Vec::<ChannelMessage>::new()
                .into_iter()
                .paginate_on(50, CreatedAt)
                .filter_on(())
                .into_page(),
            has_more_newer: false,
        })
    }

    async fn get_channel_attachments(
        &self,
        _channel_id: Uuid,
        _query: Query<Uuid, CreatedAt, ()>,
        _limit: u16,
        _attachment_type: Option<channels::domain::models::ChannelAttachmentType>,
    ) -> Result<ChannelAttachmentsPage, ChannelMessagesErr> {
        Ok(Vec::<ChannelAttachment>::new()
            .into_iter()
            .paginate_on(50, CreatedAt)
            .filter_on(())
            .into_page())
    }

    async fn get_channel_participants(
        &self,
        _channel_id: Uuid,
    ) -> Result<Vec<ChannelParticipant>, ChannelMessagesErr> {
        Ok(self.participants.lock().unwrap().clone())
    }

    async fn get_channel_metadata(
        &self,
        channel_id: Uuid,
        _viewer_user_id: MacroUserIdStr<'static>,
    ) -> Result<ChannelMetadata, ChannelMessagesErr> {
        if self.existing_channels.lock().unwrap().contains(&channel_id) {
            Ok(ChannelMetadata {
                channel_type: ChannelType::Private,
                channel_name: "test".into(),
            })
        } else {
            Err(ChannelMessagesErr::Repo(anyhow::anyhow!(
                "no rows returned by a query that expected to return at least one row"
            )))
        }
    }

    async fn get_attachment_references(
        &self,
        _entity_type: String,
        _entity_id: String,
        _user_id: String,
    ) -> Result<Vec<channels::domain::models::AttachmentEntityReference>, ChannelMessagesErr>
    {
        Ok(vec![])
    }

    async fn get_channel_messages_around(
        &self,
        _channel_id: Uuid,
        _message_id: Uuid,
        _limit: u16,
    ) -> Result<ChannelMessagesQueryResult, ChannelMessagesErr> {
        Ok(ChannelMessagesQueryResult {
            page: Vec::<ChannelMessage>::new()
                .into_iter()
                .paginate_on(50, CreatedAt)
                .filter_on(())
                .into_page(),
            has_more_newer: false,
        })
    }

    async fn get_thread_replies(
        &self,
        _channel_id: Uuid,
        _message_id: Uuid,
    ) -> Result<Vec<channels::domain::models::ThreadReply>, ChannelMessagesErr> {
        Ok(vec![])
    }

    async fn create_channel(
        &self,
        _actor: ChannelSender<'_>,
        _actor_org_id: Option<i64>,
        req: CreateChannelRequest,
    ) -> Result<CreateChannelResponse, ChannelMutationErr> {
        *self.last_create.lock().unwrap() = Some(req);
        let id = Uuid::new_v4();
        self.existing_channels.lock().unwrap().insert(id);
        Ok(CreateChannelResponse {
            id: id.to_string(),
        })
    }

    async fn delete_channel(
        &self,
        _actor: ChannelSender<'_>,
        channel_id: Uuid,
    ) -> Result<(), ChannelMutationErr> {
        if self.existing_channels.lock().unwrap().remove(&channel_id) {
            Ok(())
        } else {
            Err(ChannelMutationErr::Repo(anyhow::anyhow!(
                "channel not deleted, either it didn't exist or the user_id provided was not the owner"
            )))
        }
    }

    async fn add_participants(
        &self,
        _actor: ChannelSender<'_>,
        channel_id: Uuid,
        req: AddParticipantsRequest,
    ) -> Result<(), ChannelMutationErr> {
        for user_id in req.participants {
            self.participants.lock().unwrap().push(ChannelParticipant {
                channel_id,
                user_id: user_id.as_ref().to_string(),
                role: ParticipantRole::Member,
                joined_at: chrono::Utc::now(),
                left_at: None,
            });
        }
        Ok(())
    }

    async fn remove_participants(
        &self,
        _actor: ChannelSender<'_>,
        _channel_id: Uuid,
        req: RemoveParticipantsRequest,
    ) -> Result<(), ChannelMutationErr> {
        self.participants
            .lock()
            .unwrap()
            .retain(|p| !req.participants.iter().any(|id| id == &p.user_id));
        Ok(())
    }

    async fn post_message(
        &self,
        _actor: ChannelSender<'_>,
        _channel_id: Uuid,
        _req: PostMessageRequest,
    ) -> Result<PostMessageResponse, ChannelMutationErr> {
        Ok(PostMessageResponse {
            id: Uuid::new_v4().to_string(),
            nonce: None,
        })
    }
}

fn test_pool() -> PgPool {
    PgPool::connect_lazy("postgres://user:pass@localhost/unused")
        .expect("lazy pool")
}

fn test_router(service: Arc<MockChannelService>, users: HashSet<String>) -> Router {
    let state = CrostChannelsRouterState::new(
        service,
        test_pool(),
        Arc::new(StubUserResolver { users }),
        ServiceApiToken::new(TOKEN),
    );
    Router::new().nest("/api/v1/channels", crost_channels_router(state))
}

async fn send(
    app: Router,
    method: &str,
    path: &str,
    body: Option<&str>,
    token: Option<&str>,
) -> (StatusCode, String) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = builder
        .header("content-type", "application/json")
        .body(Body::from(body.unwrap_or("").to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap_or_default())
}

#[tokio::test]
async fn unauthorized_without_bearer_token() {
    let app = test_router(Arc::new(MockChannelService::default()), HashSet::new());
    let (status, body) = send(
        app,
        "POST",
        "/api/v1/channels",
        Some(r#"{"name":"x","private":true}"#),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, r#"{"error":"unauthorized"}"#);
}

#[tokio::test]
async fn unauthorized_with_wrong_bearer_token() {
    let app = test_router(Arc::new(MockChannelService::default()), HashSet::new());
    let (status, body) = send(
        app,
        "POST",
        "/api/v1/channels",
        Some(r#"{"name":"x","private":true}"#),
        Some("wrong"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, r#"{"error":"unauthorized"}"#);
}

#[tokio::test]
async fn create_private_channel_returns_id() {
    let service = Arc::new(MockChannelService::default());
    let app = test_router(service.clone(), HashSet::new());
    let (status, body) = send(
        app,
        "POST",
        "/api/v1/channels",
        Some(r#"{"name":"pr-team-a","private":true}"#),
        Some(TOKEN),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(body.contains(r#""id":"#));
    let create = service.last_create.lock().unwrap().take().expect("create req");
    assert_eq!(create.channel_type, ChannelType::Private);
    assert!(create.participants.is_empty());
}

#[tokio::test]
async fn create_public_channel_includes_service_actor_participant() {
    let service = Arc::new(MockChannelService::default());
    let app = test_router(service.clone(), HashSet::new());
    let (status, _body) = send(
        app,
        "POST",
        "/api/v1/channels",
        Some(r#"{"name":"town-square","private":false}"#),
        Some(TOKEN),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let create = service.last_create.lock().unwrap().take().expect("create req");
    assert_eq!(create.channel_type, ChannelType::Public);
    assert_eq!(create.participants.len(), 1);
    assert!(
        create
            .participants
            .contains(&MacroUserIdStr::parse_from_str(SERVICE_ACTOR_USER_ID).unwrap())
    );
}

#[tokio::test]
async fn missing_channel_returns_404_on_archive() {
    let channel_id = Uuid::new_v4();
    let app = test_router(Arc::new(MockChannelService::default()), HashSet::new());
    let (status, body) = send(
        app,
        "POST",
        &format!("/api/v1/channels/{channel_id}/archive"),
        None,
        Some(TOKEN),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, r#"{"error":"channel not found"}"#);
}

#[tokio::test]
async fn duplicate_member_returns_409() {
    let channel_id = Uuid::new_v4();
    let member = "macro|bob@seed.macro.local".to_string();
    let service = Arc::new(MockChannelService::with_channel(channel_id));
    service.participants.lock().unwrap().push(ChannelParticipant {
        channel_id,
        user_id: member.clone(),
        role: ParticipantRole::Member,
        joined_at: chrono::Utc::now(),
        left_at: None,
    });
    let mut users = HashSet::new();
    users.insert(member);
    let app = test_router(service, users);
    let (status, body) = send(
        app,
        "POST",
        &format!("/api/v1/channels/{channel_id}/members"),
        Some(r#"{"user_or_agent_ref":"bob@seed.macro.local"}"#),
        Some(TOKEN),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body, r#"{"error":"member already exists"}"#);
}

#[tokio::test]
async fn missing_user_returns_404() {
    let channel_id = Uuid::new_v4();
    let app = test_router(
        Arc::new(MockChannelService::with_channel(channel_id)),
        HashSet::new(),
    );
    let (status, body) = send(
        app,
        "POST",
        &format!("/api/v1/channels/{channel_id}/members"),
        Some(r#"{"user_or_agent_ref":"bob@seed.macro.local"}"#),
        Some(TOKEN),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, r#"{"error":"user not found"}"#);
}

#[tokio::test]
async fn missing_member_returns_404() {
    let channel_id = Uuid::new_v4();
    let member = "macro|bob@seed.macro.local".to_string();
    let mut users = HashSet::new();
    users.insert(member);
    let app = test_router(
        Arc::new(MockChannelService::with_channel(channel_id)),
        users,
    );
    let (status, body) = send(
        app,
        "DELETE",
        &format!("/api/v1/channels/{channel_id}/members/bob@seed.macro.local"),
        None,
        Some(TOKEN),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, r#"{"error":"member not found"}"#);
}

#[tokio::test]
async fn post_message_returns_message_id() {
    let channel_id = Uuid::new_v4();
    let app = test_router(
        Arc::new(MockChannelService::with_channel(channel_id)),
        HashSet::new(),
    );
    let (status, body) = send(
        app,
        "POST",
        &format!("/api/v1/channels/{channel_id}/messages"),
        Some(r#"{"text":"hello room","thread":"thread-1"}"#),
        Some(TOKEN),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(body.contains(r#""message_id":"#));
}
