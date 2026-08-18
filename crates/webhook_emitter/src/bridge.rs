//! Kafka bridge: map broker topics into Crost webhook events.

#[cfg(test)]
mod test;

use channels::domain::broker_events::{ChannelMacroEvent, ChannelTopicEvent};
use document_sub_type::DocumentSubType;
use documents::domain::events::{DocumentMacroEvent, DocumentTopicEvent};
use kafka_util::{GroupName, KafkaEventConsumer};
use macro_event_broker::{
    KafkaConsumerAdapter, MacroEvent as _, MacroEventCollection as _, MacroEventConsumerService,
};
use models_properties::{EntityType, service::property_value::PropertyValue};
use properties::domain::events::{PropertyMacroEvent, PropertyTopicEvent};
use rdkafka::consumer::CommitMode;
use rdkafka::message::{BorrowedMessage, Message};
use serde_json::json;
use sqlx::PgPool;
use system_properties::SystemPropertyKey;

use crate::{
    events::{self, WebhookEnvelope},
    outbox::PgOutbox,
};

/// Consumer group for the Crost webhook emitter bridge.
struct CrostWebhookEmitterConsumerGroup;

impl GroupName for CrostWebhookEmitterConsumerGroup {
    const GROUP_NAME: &'static str = "crost-webhook-emitter";
}

macro_event_broker::declare_topics!(
    DeclaredMacroEvent: DocumentMacroEvent,
    ChannelMacroEvent,
    PropertyMacroEvent,
);

type BridgeKafkaAdapter =
    KafkaConsumerAdapter<CrostWebhookEmitterConsumerGroup, DeclaredMacroEvent>;
type BridgeKafkaConsumer = MacroEventConsumerService<DeclaredMacroEvent, BridgeKafkaAdapter>;

/// Start the Kafka bridge; returns when the consumer exits.
pub async fn run_kafka_bridge(
    brokers: &str,
    pool: PgPool,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> anyhow::Result<()> {
    let outbox = PgOutbox::new(pool);
    let consumer = KafkaEventConsumer::<CrostWebhookEmitterConsumerGroup>::from_env(brokers)?;
    let consumer = KafkaConsumerAdapter::<CrostWebhookEmitterConsumerGroup, ()>::new(consumer)
        .subscribe::<DeclaredMacroEvent>()
        .map_err(|error| {
            anyhow::anyhow!("failed to subscribe to crost webhook emitter topics: {error:?}")
        })?;
    let consumer = BridgeKafkaConsumer::new(consumer);

    tracing::info!(
        topics = ?DeclaredMacroEvent::topics(),
        group = CrostWebhookEmitterConsumerGroup::GROUP_NAME,
        "crost webhook emitter kafka bridge listening"
    );

    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!("crost webhook emitter kafka bridge shutting down");
                break;
            }
            result = consumer.recv() => {
                let message = match result {
                    Ok(message) => message,
                    Err(error) => {
                        tracing::error!(error = ?error, "crost webhook emitter kafka receive error");
                        continue;
                    }
                };
                let kafka_message = message.inner();
                match message.decode_payload() {
                    Ok(event) => {
                        if let Err(error) = handle_event(&outbox, event).await {
                            tracing::error!(error = ?error, "failed to enqueue crost webhook event");
                            return Err(error);
                        }
                    }
                    Err(error) => tracing::warn!(
                        error = ?error,
                        topic = kafka_message.topic(),
                        partition = kafka_message.partition(),
                        offset = kafka_message.offset(),
                        "skipping undecodable kafka message in crost webhook bridge"
                    ),
                }
                commit_logged(&consumer, kafka_message);
            }
        }
    }

    Ok(())
}

async fn handle_event(outbox: &PgOutbox, event: DeclaredMacroEvent) -> anyhow::Result<()> {
    let Some(envelope) = map_event(outbox, event).await? else {
        return Ok(());
    };
    outbox.enqueue(&envelope).await
}

async fn map_event(
    outbox: &PgOutbox,
    event: DeclaredMacroEvent,
) -> anyhow::Result<Option<WebhookEnvelope>> {
    match event {
        DeclaredMacroEvent::DocumentMacroEvent(event) => {
            map_document_event(outbox, event.event().event.clone()).await
        }
        DeclaredMacroEvent::ChannelMacroEvent(event) => {
            Ok(map_channel_event(event.event().event.clone()))
        }
        DeclaredMacroEvent::PropertyMacroEvent(event) => {
            Ok(map_property_event(event.event().event.clone()))
        }
    }
}

pub(crate) async fn map_document_event(
    outbox: &PgOutbox,
    event: DocumentTopicEvent,
) -> anyhow::Result<Option<WebhookEnvelope>> {
    match event {
        DocumentTopicEvent::Created(metadata) => {
            if metadata.sub_type == Some(DocumentSubType::Task) {
                return Ok(Some(WebhookEnvelope::new(
                    events::TASK_CREATED,
                    json!({
                        "task_id": metadata.document_id,
                        "title": metadata.document_name,
                        "owner": metadata.owner,
                        "project_id": metadata.project_id,
                        "created_at": metadata.created_at,
                    }),
                )));
            }
            Ok(None)
        }
        DocumentTopicEvent::Updated(metadata) => {
            if outbox.document_is_task(&metadata.document_id).await? {
                return Ok(Some(WebhookEnvelope::new(
                    events::TASK_UPDATED,
                    json!({
                        "task_id": metadata.document_id,
                        "owner": metadata.owner,
                        "actor_user_id": metadata.actor_user_id,
                        "document_name": metadata.document_name,
                        "project_id": metadata.project_id,
                        "previous_project_id": metadata.previous_project_id,
                        "share_permission_updated": metadata.share_permission_updated,
                    }),
                )));
            }
            Ok(Some(WebhookEnvelope::new(
                events::DOC_UPDATED,
                json!({
                    "document_id": metadata.document_id,
                    "owner": metadata.owner,
                    "actor_user_id": metadata.actor_user_id,
                    "document_name": metadata.document_name,
                    "project_id": metadata.project_id,
                }),
            )))
        }
        _ => Ok(None),
    }
}

pub(crate) fn map_channel_event(event: ChannelTopicEvent) -> Option<WebhookEnvelope> {
    let ChannelTopicEvent::MessagePosted(metadata) = event else {
        return None;
    };

    let author: String = metadata.sender.clone().into();
    let mentions: Vec<_> = metadata
        .mentions
        .iter()
        .map(|mention| {
            json!({
                "entity_type": mention.entity_type,
                "entity_id": mention.entity_id,
            })
        })
        .collect();

    Some(WebhookEnvelope::new(
        events::MESSAGE_POSTED,
        json!({
            "channel_id": metadata.channel_id,
            "message_id": metadata.message_id,
            "author": author,
            "text": metadata.content,
            "thread_id": metadata.thread_id,
            "mentions": mentions,
            "created_at": metadata.created_at,
        }),
    ))
}

pub(crate) fn map_property_event(event: PropertyTopicEvent) -> Option<WebhookEnvelope> {
    let PropertyTopicEvent::EntityPropertyUpdated(metadata) = event else {
        return None;
    };
    if metadata.entity_type != EntityType::Task {
        return None;
    }

    let mut body = json!({
        "task_id": metadata.entity_id,
        "property_definition_id": metadata.property_definition_id,
        "actor_user_id": metadata.actor_user_id,
        "updated_at": metadata.updated_at,
    });

    if let Some(value) = &metadata.value {
        body["value"] = serde_json::to_value(value).ok()?;
    }
    if let Some(previous_value) = &metadata.previous_value {
        body["previous_value"] = serde_json::to_value(previous_value).ok()?;
    }

    if metadata.property_definition_id == SystemPropertyKey::STATUS_UUID {
        append_status_fields(&mut body, &metadata.value, &metadata.previous_value);
    }

    Some(WebhookEnvelope::new(events::TASK_UPDATED, body))
}

fn append_status_fields(
    body: &mut serde_json::Value,
    value: &Option<PropertyValue>,
    previous_value: &Option<PropertyValue>,
) {
    if let Some(PropertyValue::SelectOption(options)) = value
        && let Some(status_id) = options.first()
    {
        body["status"] = json!(status_id.to_string());
    }
    if let Some(PropertyValue::SelectOption(options)) = previous_value
        && let Some(status_id) = options.first()
    {
        body["previous_status"] = json!(status_id.to_string());
    }
}

fn commit_logged(consumer: &BridgeKafkaConsumer, message: &BorrowedMessage<'_>) {
    if let Err(error) = consumer.inner().commit_message(message, CommitMode::Async) {
        tracing::error!(
            error = ?error,
            partition = message.partition(),
            offset = message.offset(),
            "failed to commit crost webhook bridge offset"
        );
    }
}
