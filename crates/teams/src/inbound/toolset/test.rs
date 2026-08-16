use super::TeamToolContext;
use super::list_team_members::ListTeamMembers;
use crate::domain::model::{TeamError, TeamMembers};
use crate::domain::team_repo::TeamMembersService;
use ai_toolset::schema::generate_validated_input_schema;
use ai_toolset::{AsyncTool, RequestContext, ServiceContext};
use entity_access::domain::models::{EntityAccessReceipt, MemberTeamRole};
use entity_access::domain::ports::NoOpEntityAccessService;
use macro_user_id::user_id::MacroUserIdStr;

#[test]
fn test_list_team_members_schema_validation() {
    let result = generate_validated_input_schema::<ListTeamMembers>();
    assert!(result.is_ok(), "{:?}", result);

    let validated = result.unwrap();
    assert_eq!(
        validated.name, "ListTeamMembers",
        "Tool name should match the schemars title"
    );
    assert!(
        validated.description.contains("List"),
        "Description should contain expected text"
    );
    assert!(
        validated.description.contains("inTeam=false"),
        "Description should explain the successful no-team result"
    );
}

/// A caller with no team must never reach the team service.
#[derive(Clone)]
struct NeverCalledTeamMembersService;

impl TeamMembersService for NeverCalledTeamMembersService {
    async fn list_team_members(
        &self,
        _entity_access_receipt: EntityAccessReceipt<MemberTeamRole>,
    ) -> Result<TeamMembers, TeamError> {
        panic!("list_team_members must not be called for a caller with no team")
    }
}

#[tokio::test]
async fn no_team_is_a_successful_explicit_result() {
    // NoOpEntityAccessService reports no team membership; the tool must treat
    // that as normal data (inTeam=false), not as a ToolCallError.
    let context = ServiceContext(TeamToolContext::new(
        NeverCalledTeamMembersService,
        NoOpEntityAccessService,
    ));
    let user_id = MacroUserIdStr::try_from("macro|solo@example.com".to_string())
        .expect("test user id should be valid");

    let response = ListTeamMembers::default()
        .call(context, RequestContext::new(user_id))
        .await
        .expect("a caller with no team must get a successful result");

    assert!(!response.in_team);
    assert!(response.members.is_empty());
    assert!(response.invited.is_empty());
}
