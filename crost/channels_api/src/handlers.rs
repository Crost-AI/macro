use crate::{
    error::ApiError,
    models::{
        AddMemberBody, CreateChannelBody, CreateChannelResponse, PostMessageBody,
        PostMessageResponse,
    },
    resolve::resolve_user_ref,
    router::CrostChannelsRouterState,
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use channel_sender::ChannelSender;
use channels::domain::{
        models::{
        AddParticipantsRequest, ChannelType, CreateChannelRequest, PostMessageNotificationPolicy,
        PostMessageRequest, RemoveParticipantsRequest,
    },
    ports::{ChannelMutationErr, ChannelService},
};
use macro_user_id::user_id::MacroUserIdStr;
use std::collections::HashSet;
use uuid::Uuid;

/// Macro internal service actor for Crost-authenticated mutations.
pub const SERVICE_ACTOR_USER_ID: &str = "macro|INTERNAL@macro.com";

fn service_actor() -> ChannelSender<'static> {
    ChannelSender::new_from_user(
        MacroUserIdStr::parse_from_str(SERVICE_ACTOR_USER_ID).expect("valid internal user id"),
    )
}

pub async fn create_channel<Svc>(
    State(state): State<CrostChannelsRouterState<Svc>>,
    Json(body): Json<CreateChannelBody>,
) -> Result<impl IntoResponse, ApiError>
where
    Svc: ChannelService,
{
    if body.name.trim().is_empty() {
        return Err(ApiError::bad_request("name must not be empty"));
    }

    let channel_type = if body.private {
        ChannelType::Private
    } else {
        ChannelType::Public
    };

    let req = CreateChannelRequest {
        name: Some(body.name),
        channel_type,
        team_id: None,
        auto_join_team: false,
        participants: HashSet::new(),
    };

    let res = state
        .service
        .create_channel(service_actor(), None, req)
        .await
        .map_err(map_mutation_err)?;

    Ok((
        StatusCode::CREATED,
        Json(CreateChannelResponse { id: res.id }),
    ))
}

pub async fn archive_channel<Svc>(
    State(state): State<CrostChannelsRouterState<Svc>>,
    Path(channel_id): Path<Uuid>,
) -> Result<StatusCode, ApiError>
where
    Svc: ChannelService,
{
    ensure_channel_exists(&state, channel_id).await?;

    state
        .service
        .delete_channel(service_actor(), channel_id)
        .await
        .map_err(|err| match err {
            ChannelMutationErr::NotFound(_) => ApiError::not_found("channel not found"),
            other => map_mutation_err(other),
        })?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn add_member<Svc>(
    State(state): State<CrostChannelsRouterState<Svc>>,
    Path(channel_id): Path<Uuid>,
    Json(body): Json<AddMemberBody>,
) -> Result<StatusCode, ApiError>
where
    Svc: ChannelService,
{
    ensure_channel_exists(&state, channel_id).await?;

    let user_id = resolve_user_ref(&state.db, &body.user_or_agent_ref).await?;

    let participants = state
        .service
        .get_channel_participants(channel_id)
        .await
        .map_err(map_messages_err)?;

    if participants.iter().any(|p| p.user_id == user_id.as_ref()) {
        return Err(ApiError::conflict("member already exists"));
    }

    let req = AddParticipantsRequest {
        participants: HashSet::from([user_id]),
    };

    state
        .service
        .add_participants(service_actor(), channel_id, req)
        .await
        .map_err(map_mutation_err)?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_member<Svc>(
    State(state): State<CrostChannelsRouterState<Svc>>,
    Path((channel_id, member_ref)): Path<(Uuid, String)>,
) -> Result<StatusCode, ApiError>
where
    Svc: ChannelService,
{
    ensure_channel_exists(&state, channel_id).await?;

    let user_id = resolve_user_ref(&state.db, &member_ref).await?;

    let participants = state
        .service
        .get_channel_participants(channel_id)
        .await
        .map_err(map_messages_err)?;

    if !participants.iter().any(|p| p.user_id == user_id.as_ref()) {
        return Err(ApiError::not_found("member not found"));
    }

    let req = RemoveParticipantsRequest {
        participants: vec![user_id.to_string()],
    };

    state
        .service
        .remove_participants(service_actor(), channel_id, req)
        .await
        .map_err(map_mutation_err)?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn post_message<Svc>(
    State(state): State<CrostChannelsRouterState<Svc>>,
    Path(channel_id): Path<Uuid>,
    Json(body): Json<PostMessageBody>,
) -> Result<impl IntoResponse, ApiError>
where
    Svc: ChannelService,
{
    ensure_channel_exists(&state, channel_id).await?;

    let thread_id = match body.thread {
        None => None,
        Some(ref raw) if raw.is_empty() => None,
        Some(ref raw) => Some(
            Uuid::parse_str(raw)
                .map_err(|_| ApiError::bad_request("invalid thread id"))?,
        ),
    };

    let req = PostMessageRequest {
        content: body.text,
        mentions: Vec::new(),
        thread_id,
        attachments: Vec::new(),
        nonce: None,
        notification_policy: PostMessageNotificationPolicy::Default,
        triggered_by: None,
    };

    let res = state
        .service
        .post_message(service_actor(), channel_id, req)
        .await
        .map_err(map_mutation_err)?;

    Ok((
        StatusCode::CREATED,
        Json(PostMessageResponse {
            message_id: res.id,
        }),
    ))
}

async fn ensure_channel_exists<Svc>(
    state: &CrostChannelsRouterState<Svc>,
    channel_id: Uuid,
) -> Result<(), ApiError>
where
    Svc: ChannelService,
{
    match state.service.get_channel_participants(channel_id).await {
        Ok(_) => Ok(()),
        Err(err) => Err(map_messages_err(err)),
    }
}

fn map_mutation_err(err: ChannelMutationErr) -> ApiError {
    match err {
        ChannelMutationErr::NotFound(message) => ApiError::not_found(message),
        ChannelMutationErr::BadRequest(message) => ApiError::bad_request(message),
        ChannelMutationErr::Unauthorized(message) => ApiError::bad_request(message),
        ChannelMutationErr::Forbidden(message) => ApiError::bad_request(message),
        other => {
            tracing::error!(error=?other, "channel mutation failed");
            ApiError::internal("channel mutation failed")
        }
    }
}

fn map_messages_err(err: channels::domain::ports::ChannelMessagesErr) -> ApiError {
    match err {
        channels::domain::ports::ChannelMessagesErr::MessageNotFound(_) => {
            ApiError::not_found("channel not found")
        }
        channels::domain::ports::ChannelMessagesErr::Repo(repo_err) => {
            let message = repo_err.to_string();
            if message.contains("not found") || message.contains("does not exist") {
                ApiError::not_found("channel not found")
            } else {
                tracing::error!(error=?repo_err, "channel read failed");
                ApiError::internal("channel read failed")
            }
        }
    }
}
