use crate::{
    auth::{ServiceApiToken, require_service_token},
    handlers,
};
use axum::{
    Router,
    routing::{delete, post},
};
use channels::domain::ports::ChannelService;
use sqlx::PgPool;
use std::sync::Arc;

/// Shared state for the Crost channels REST router.
pub struct CrostChannelsRouterState<Svc> {
    pub service: Arc<Svc>,
    pub db: PgPool,
    pub service_token: ServiceApiToken,
}

impl<Svc> Clone for CrostChannelsRouterState<Svc> {
    fn clone(&self) -> Self {
        Self {
            service: Arc::clone(&self.service),
            db: self.db.clone(),
            service_token: self.service_token.clone(),
        }
    }
}

impl<Svc> CrostChannelsRouterState<Svc> {
    pub fn new(service: Arc<Svc>, db: PgPool, service_token: ServiceApiToken) -> Self {
        Self {
            service,
            db,
            service_token,
        }
    }
}

/// Mount W2.8 channels REST routes under `/api/v1/channels`.
pub fn crost_channels_router<Svc>(state: CrostChannelsRouterState<Svc>) -> Router
where
    Svc: ChannelService + 'static,
{
    let token = state.service_token.clone();
    Router::new()
        .route("/", post(handlers::create_channel::<Svc>))
        .route("/{id}/archive", post(handlers::archive_channel::<Svc>))
        .route("/{id}/members", post(handlers::add_member::<Svc>))
        .route(
            "/{id}/members/{ref}",
            delete(handlers::remove_member::<Svc>),
        )
        .route("/{id}/messages", post(handlers::post_message::<Svc>))
        .layer(axum::middleware::from_fn_with_state(
            token,
            require_service_token,
        ))
        .with_state(state)
}
