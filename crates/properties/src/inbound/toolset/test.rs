use super::get_entity_properties::ToolPropertyTargetEntityType;
#[allow(unused_imports)]
use super::*;
use crate::PropertiesServiceImpl;
use crate::domain::model::EntityPropertyOptionSelection;
use crate::domain::ports::{MockNotificationService, MockPermissionService, MockPropertiesRepo};
use ai_toolset::schema::generate_validated_input_schema;
use ai_toolset::{AsyncTool, RequestContext, ServiceContext};
use entity_access::domain::models::{
    AccessError, AccessLevel, BotAccessScope, BotId, CallChannelInfo, EntityAccessReceipt,
    EntityPermission, EntityType as AccessEntityType, RequiredPermission, TeamRole, UserTeamInfo,
};
use entity_access::domain::ports::EntityAccessService;
use macro_user_id::{
    lowercased::Lowercase,
    user_id::{MacroUserId, MacroUserIdStr},
};
use models_properties::service::property_definition::PropertyDefinition;
use models_properties::{DataType, PropertyOwner};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[test]
fn test_get_entity_properties_schema_validation() {
    let result = generate_validated_input_schema::<GetEntityProperties>();
    assert!(result.is_ok(), "{:?}", result);

    let validated = result.unwrap();
    assert_eq!(validated.name, "GetEntityProperties");
    assert!(
        validated.description.contains("Get all properties"),
        "Description should contain expected text"
    );
}

#[test]
fn test_set_entity_property_schema_validation() {
    let result = generate_validated_input_schema::<SetEntityProperty>();
    assert!(result.is_ok(), "{:?}", result);

    let validated = result.unwrap();
    assert_eq!(validated.name, "SetEntityProperty");
    assert!(
        validated.description.contains("Set or update a property"),
        "Description should contain expected text"
    );
}

#[test]
fn test_set_entity_property_schema_documents_delta_options() {
    let validated = generate_validated_input_schema::<SetEntityProperty>().unwrap();
    let schema_json = serde_json::to_string(&validated.schema).unwrap();
    assert!(
        schema_json.contains("add_option_ids"),
        "schema should expose add_option_ids"
    );
    assert!(
        schema_json.contains("remove_option_ids"),
        "schema should expose remove_option_ids"
    );
    assert!(
        validated.description.contains("atomically"),
        "description should steer to atomic add/remove over full replace"
    );
}

#[test]
fn test_bulk_set_entity_property_options_schema_validation() {
    let result = generate_validated_input_schema::<BulkSetEntityPropertyOptions>();
    assert!(result.is_ok(), "{:?}", result);

    let validated = result.unwrap();
    assert_eq!(validated.name, "BulkSetEntityPropertyOptions");
    assert!(
        validated.description.contains("many entities"),
        "Description should explain the multi-entity apply"
    );

    let schema_json = serde_json::to_string(&validated.schema).unwrap();
    assert!(
        schema_json.contains("entities")
            && schema_json.contains("add_option_ids")
            && schema_json.contains("remove_option_ids"),
        "schema should expose entities and the add/remove option deltas"
    );
}

#[test]
fn test_list_tags_schema_validation() {
    let result = generate_validated_input_schema::<ListTags>();
    assert!(result.is_ok(), "{:?}", result);

    let validated = result.unwrap();
    assert_eq!(validated.name, "ListTags");
    assert!(
        validated.description.contains("personal tag set"),
        "Description should explain the personal/team tag sets"
    );
    assert!(
        validated.description.contains("SetEntityProperty"),
        "Description should point at SetEntityProperty for applying tags"
    );
}

#[test]
fn test_create_tag_schema_validation() {
    let result = generate_validated_input_schema::<CreateTag>();
    assert!(result.is_ok(), "{:?}", result);

    let validated = result.unwrap();
    assert_eq!(validated.name, "CreateTag");
    assert!(
        validated.description.contains("Create a new tag"),
        "Description should explain that it creates a new tag"
    );

    let schema_json = serde_json::to_string(&validated.schema).unwrap();
    assert!(
        schema_json.contains("label") && schema_json.contains("color"),
        "schema should expose label and color"
    );
    assert!(
        schema_json.contains("scope"),
        "schema should expose the personal/team scope"
    );
}

#[test]
fn test_edit_tag_schema_validation() {
    let result = generate_validated_input_schema::<EditTag>();
    assert!(result.is_ok(), "{:?}", result);

    let validated = result.unwrap();
    assert_eq!(validated.name, "EditTag");
    assert!(
        validated.description.contains("Rename or recolor"),
        "Description should explain rename/recolor"
    );

    let schema_json = serde_json::to_string(&validated.schema).unwrap();
    assert!(
        schema_json.contains("label") && schema_json.contains("color"),
        "schema should expose label and color"
    );
    assert!(
        schema_json.contains("property_definition_id"),
        "schema should require the tag set's property_definition_id"
    );
}

#[test]
fn test_delete_tag_schema_validation() {
    let result = generate_validated_input_schema::<DeleteTag>();
    assert!(result.is_ok(), "{:?}", result);

    let validated = result.unwrap();
    assert_eq!(validated.name, "DeleteTag");
    assert!(
        validated.description.contains("Permanently delete a tag"),
        "Description should explain the destructive delete"
    );

    let schema_json = serde_json::to_string(&validated.schema).unwrap();
    assert!(
        schema_json.contains("property_definition_id"),
        "schema should require the tag set's property_definition_id"
    );
}

#[test]
fn property_target_accepts_email_and_legacy_thread_as_email_thread() {
    // ListEntities surfaces email threads as `email`; the property tools must
    // accept that spelling (and keep the legacy `thread` alias) and map both
    // to the canonical EmailThread entity type.
    for spelling in ["\"email\"", "\"thread\""] {
        let parsed: ToolPropertyTargetEntityType = serde_json::from_str(spelling)
            .unwrap_or_else(|e| panic!("property target should deserialize from {spelling}: {e}"));
        assert!(
            matches!(
                model_entity::EntityType::from(parsed),
                model_entity::EntityType::EmailThread
            ),
            "{spelling} should map to the canonical EmailThread type"
        );
    }
}

#[test]
fn property_target_schema_advertises_email_not_thread() {
    let schema = serde_json::to_value(schemars::schema_for!(ToolPropertyTargetEntityType))
        .expect("schema should serialize");
    let values: Vec<&str> = schema["enum"]
        .as_array()
        .expect("target entity type should be a plain string enum")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        values.contains(&"email"),
        "schema must advertise email: {values:?}"
    );
    assert!(
        !values.contains(&"thread"),
        "the legacy thread alias must not be advertised: {values:?}"
    );
}

#[test]
fn property_tool_inputs_deserialize_with_email_target() {
    let get: Result<GetEntityProperties, _> = serde_json::from_value(serde_json::json!({
        "entity_id": "9f0f4be7-1b2c-4a52-9c30-1f6f4bb1a111",
        "entity_type": "email",
    }));
    assert!(
        get.is_ok(),
        "GetEntityProperties should accept email: {get:?}"
    );

    let set: Result<SetEntityProperty, _> = serde_json::from_value(serde_json::json!({
        "entity_id": "9f0f4be7-1b2c-4a52-9c30-1f6f4bb1a111",
        "entity_type": "email",
        "property_definition_id": "00000001-0000-0000-0000-000000000001",
        "add_option_ids": ["00000001-0000-0000-0002-000000000001"],
    }));
    assert!(
        set.is_ok(),
        "SetEntityProperty should accept email: {set:?}"
    );

    let bulk: Result<BulkSetEntityPropertyOptions, _> = serde_json::from_value(serde_json::json!({
        "entities": [{"entity_type": "email", "entity_id": "9f0f4be7-1b2c-4a52-9c30-1f6f4bb1a111"}],
        "property_definition_id": "00000001-0000-0000-0000-000000000001",
        "add_option_ids": ["00000001-0000-0000-0002-000000000001"],
    }));
    assert!(
        bulk.is_ok(),
        "BulkSetEntityPropertyOptions should accept email: {bulk:?}"
    );
}

const CALLER: &str = "macro|caller@example.com";

fn caller_user_id() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from(CALLER.to_string()).expect("test caller id should be valid")
}

/// Records receipt requests and mints a receipt for any entity, standing in
/// for an inbox owner who can edit their own email threads.
#[derive(Clone, Default)]
struct FakeEntityAccessService {
    receipt_requests: Arc<Mutex<Vec<(String, AccessEntityType)>>>,
}

impl FakeEntityAccessService {
    fn receipt_requests(&self) -> Vec<(String, AccessEntityType)> {
        self.receipt_requests
            .lock()
            .expect("receipt lock poisoned")
            .clone()
    }
}

impl EntityAccessService for FakeEntityAccessService {
    async fn generate_entity_access_receipt<T: RequiredPermission>(
        &self,
        user_id: &MacroUserId<Lowercase<'_>>,
        _user_org_id: Option<i64>,
        entity_id: &str,
        entity_type: AccessEntityType,
    ) -> Result<EntityAccessReceipt<T>, AccessError> {
        self.receipt_requests
            .lock()
            .expect("receipt lock poisoned")
            .push((entity_id.to_string(), entity_type));
        let user_id = MacroUserIdStr::try_from(user_id.as_ref().to_string())
            .expect("authorized test user id should be valid");
        Ok(EntityAccessReceipt::dangerously_assert_authenticated_user(
            user_id,
            entity_id,
            entity_type,
        ))
    }

    async fn generate_bot_entity_access_receipt<T: RequiredPermission>(
        &self,
        _bot_id: BotId,
        _scope: BotAccessScope,
        _entity_id: &str,
        _entity_type: AccessEntityType,
    ) -> Result<EntityAccessReceipt<T>, AccessError> {
        panic!("unexpected bot receipt request")
    }

    async fn get_access_level(
        &self,
        _user_id: Option<&MacroUserId<Lowercase<'_>>>,
        _entity_id: &str,
        _entity_type: AccessEntityType,
    ) -> Result<Option<AccessLevel>, AccessError> {
        panic!("unexpected access-level request")
    }

    async fn check_access(
        &self,
        _user_id: Option<&MacroUserId<Lowercase<'_>>>,
        _entity_id: &str,
        _entity_type: AccessEntityType,
        _required_level: AccessLevel,
    ) -> Result<AccessLevel, AccessError> {
        panic!("unexpected access check")
    }

    async fn check_public_access(
        &self,
        _entity_id: &str,
        _entity_type: AccessEntityType,
        _required_level: AccessLevel,
    ) -> Result<AccessLevel, AccessError> {
        panic!("unexpected public access check")
    }

    async fn get_entity_permission(
        &self,
        _user_id: Option<&MacroUserId<Lowercase<'_>>>,
        _entity_id: &str,
        _entity_type: AccessEntityType,
        _user_org_id: Option<i64>,
    ) -> Result<EntityPermission, AccessError> {
        panic!("unexpected entity-permission request")
    }

    async fn get_crm_entity_permission_with_team(
        &self,
        _user_id: Option<&MacroUserId<Lowercase<'_>>>,
        _entity_id: &str,
        _entity_type: AccessEntityType,
    ) -> Result<(EntityPermission, Uuid, TeamRole), AccessError> {
        panic!("unexpected CRM permission request")
    }

    async fn get_users_by_entity(
        &self,
        _entity_id: &str,
        _entity_type: AccessEntityType,
    ) -> Result<Vec<MacroUserIdStr<'static>>, AccessError> {
        panic!("unexpected entity-users request")
    }

    async fn get_call_channel(
        &self,
        _call_id: &Uuid,
    ) -> Result<Option<CallChannelInfo>, AccessError> {
        panic!("unexpected call-channel request")
    }

    async fn get_call_channel_by_channel_id(
        &self,
        _channel_id: &Uuid,
    ) -> Result<Option<CallChannelInfo>, AccessError> {
        panic!("unexpected channel-call request")
    }

    async fn get_user_team(
        &self,
        _user_id: &MacroUserId<Lowercase<'_>>,
    ) -> Result<Option<UserTeamInfo>, AccessError> {
        panic!("unexpected user-team request")
    }
}

fn tag_definition(id: Uuid) -> PropertyDefinition {
    PropertyDefinition {
        id,
        owner: PropertyOwner::System,
        display_name: "Tags".to_string(),
        data_type: DataType::Tag,
        is_multi_select: true,
        specific_entity_type: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        is_system: false,
        is_metadata: false,
    }
}

#[tokio::test]
async fn bulk_tool_tags_an_email_thread_via_email_entity_type() {
    // The incident chain end to end: entity_type "email" deserializes, the
    // tool mints an edit receipt for the canonical EmailThread type, and the
    // write reaches the repository under the storage Thread representation.
    let thread_id = Uuid::new_v4().to_string();
    let def_id = Uuid::from_u128(0xA1);
    let add_id = Uuid::from_u128(0xB2);

    let mut repo = MockPropertiesRepo::new();
    repo.expect_get_property_definition()
        .returning(move |_| Box::pin(async move { Ok(Some(tag_definition(def_id))) }));
    repo.expect_count_valid_property_options()
        .returning(|_, _| Box::pin(async { Ok(1) }));
    repo.expect_bulk_update_entity_property_options()
        .withf(|_, entity_type, _| *entity_type == models_properties::EntityType::Thread)
        .times(1)
        .returning(move |_, _, _| {
            Box::pin(async move {
                Ok(vec![EntityPropertyOptionSelection {
                    property_definition_id: def_id,
                    option_ids: vec![add_id],
                    mutation: None,
                }])
            })
        });

    let access = FakeEntityAccessService::default();
    let service = PropertiesServiceImpl::new(
        repo,
        None::<MockPermissionService>,
        None::<MockNotificationService>,
    );
    let context = ServiceContext(PropertiesToolContext::new(service, access.clone()));

    let tool: BulkSetEntityPropertyOptions = serde_json::from_value(serde_json::json!({
        "entities": [{"entity_type": "email", "entity_id": thread_id}],
        "property_definition_id": def_id,
        "add_option_ids": [add_id],
    }))
    .expect("bulk input with an email target should deserialize");

    let response = tool
        .call(context, RequestContext::new(caller_user_id()))
        .await
        .expect("tagging an owned email thread should succeed");

    assert_eq!(
        access.receipt_requests(),
        vec![(thread_id.clone(), AccessEntityType::EmailThread)],
        "the edit receipt must be requested for the canonical EmailThread type"
    );
    assert_eq!(response.results.len(), 1);
    assert_eq!(response.results[0].status, "applied");
    assert_eq!(response.results[0].entity_type, "email");
    assert_eq!(response.results[0].entity_id, thread_id);
}

// run `cargo test -p properties inbound::toolset::test::print_get_input_schema -- --nocapture --include-ignored`
#[test]
#[ignore = "prints the input schema"]
fn print_get_input_schema() {
    let schema = generate_validated_input_schema::<GetEntityProperties>()
        .unwrap()
        .schema;
    println!("{}", serde_json::to_string_pretty(&schema).unwrap());
}

// run `cargo test -p properties inbound::toolset::test::print_set_input_schema -- --nocapture --include-ignored`
#[test]
#[ignore = "prints the input schema"]
fn print_set_input_schema() {
    let schema = generate_validated_input_schema::<SetEntityProperty>()
        .unwrap()
        .schema;
    println!("{}", serde_json::to_string_pretty(&schema).unwrap());
}

// run `cargo test -p properties inbound::toolset::test::print_get_output_schema -- --nocapture --include-ignored`
#[test]
#[ignore = "prints the output schema"]
fn print_get_output_schema() {
    let schema = schemars::schema_for!(GetEntityPropertiesResponse);
    println!("{}", serde_json::to_string_pretty(&schema).unwrap());
}

// run `cargo test -p properties inbound::toolset::test::print_set_output_schema -- --nocapture --include-ignored`
#[test]
#[ignore = "prints the output schema"]
fn print_set_output_schema() {
    let schema = schemars::schema_for!(SetEntityPropertyResponse);
    println!("{}", serde_json::to_string_pretty(&schema).unwrap());
}
