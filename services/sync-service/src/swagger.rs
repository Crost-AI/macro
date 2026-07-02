use utoipa::OpenApi;

use crate::d1::PeerWithUserId;
use crate::durable_object::{
    CopyDocumentRequest, DocumentMetadata, GetSnapshotRequest, PeerResponse, VersionIndicator,
};

/// OpenAPI spec for the sync service's JSON HTTP endpoints.
///
/// The WebSocket sync protocol is bebop-encoded and lives in the bebop schema
/// (`/schema`); only the JSON control-plane endpoints are described here.
#[derive(OpenApi)]
#[openapi(
    info(title = "Sync Service", description = "Document sync service JSON control plane"),
    paths(
        crate::cf_worker::copy_route,
        crate::durable_object::exists_route,
        crate::durable_object::metadata_route,
        crate::durable_object::raw_route,
        crate::durable_object::active_peers_route,
        crate::durable_object::peer_route,
        crate::durable_object::wakeup_route,
        crate::durable_object::snapshot_route,
        crate::durable_object::initialize_route,
    ),
    components(schemas(
        CopyDocumentRequest,
        GetSnapshotRequest,
        VersionIndicator,
        DocumentMetadata,
        PeerResponse,
        PeerWithUserId,
    )),
    tags((name = "sync_service", description = "Sync service"))
)]
pub struct ApiDoc;
