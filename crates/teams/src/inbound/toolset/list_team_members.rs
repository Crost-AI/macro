//! ListTeamMembers tool for reading the caller's team members.

use super::TeamToolContext;
use crate::domain::{
    model::{TeamInviteDetails, TeamMember},
    team_repo::TeamMembersService,
};
use ai_toolset::{AsyncTool, RequestContext, ServiceContext, ToolCallError, ToolResult};
use async_trait::async_trait;
use entity_access::domain::{
    models::{
        AccessError, Entity, EntityAccessReceipt, EntityPermission, EntityType, MemberTeamRole,
    },
    ports::EntityAccessService,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A current team member returned by [`ListTeamMembers`].
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolTeamMember {
    /// The user's Macro user id.
    pub user_id: String,
    /// The user's workspace permission role (owner/admin/member). An app
    /// permission level, not a job title or evidence of company ownership.
    pub role: String,
}

impl From<TeamMember<'static>> for ToolTeamMember {
    fn from(member: TeamMember<'static>) -> Self {
        Self {
            user_id: member.user_id.to_string(),
            role: member.role.to_string(),
        }
    }
}

/// A pending team invite returned by [`ListTeamMembers`].
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolTeamInvite {
    /// The invited email address.
    pub email: String,
    /// The workspace permission role (owner/admin/member) the invited user
    /// will receive. An app permission level, not a job title.
    pub role: String,
}

impl From<TeamInviteDetails> for ToolTeamInvite {
    fn from(invite: TeamInviteDetails) -> Self {
        Self {
            email: invite.email,
            role: invite.team_role.to_string(),
        }
    }
}

/// Response from [`ListTeamMembers`].
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListTeamMembersResponse {
    /// Whether the caller belongs to a team. `false` means the caller works
    /// solo — a normal state, not an error — and `members` and `invited` are
    /// empty. Defaulted so historical responses without it still parse.
    #[serde(default)]
    pub in_team: bool,
    /// Current accepted team members.
    pub members: Vec<ToolTeamMember>,
    /// Pending team invites.
    pub invited: Vec<ToolTeamInvite>,
}

impl ListTeamMembersResponse {
    /// The successful result for a caller who belongs to no team.
    fn no_team() -> Self {
        Self {
            in_team: false,
            members: Vec::new(),
            invited: Vec::new(),
        }
    }
}

/// List current and invited members of the caller's team.
#[derive(Debug, Deserialize, JsonSchema, Clone, Default)]
#[schemars(
    title = "ListTeamMembers",
    description = "List the current members and pending invites for the authenticated user's team. A caller who belongs to no team gets a successful result with inTeam=false and empty members/invited — working solo is normal, so don't treat it as a failure or retry. The returned roles (owner/admin/member) are app permission levels only, not job titles — they say nothing about the org chart. Never infer that someone is a founder, an executive, or the company's owner from their workspace role."
)]
#[allow(unused)]
// empty structs can't be deserialized;
pub struct ListTeamMembers {}

#[async_trait]
impl<TSvc, ESvc> AsyncTool<TeamToolContext<TSvc, ESvc>> for ListTeamMembers
where
    TSvc: TeamMembersService,
    ESvc: EntityAccessService,
{
    type Output = ListTeamMembersResponse;

    #[tracing::instrument(skip_all, fields(user_id=?request_context.user_id), err)]
    async fn call(
        &self,
        service_context: ServiceContext<TeamToolContext<TSvc, ESvc>>,
        request_context: RequestContext,
    ) -> ToolResult<Self::Output> {
        tracing::info!("List team members");

        // No team is the normal solo-user state: report it as data the model
        // can act on rather than as a tool error.
        let Some(entity_access_receipt) =
            team_member_receipt(&service_context, request_context).await?
        else {
            return Ok(ListTeamMembersResponse::no_team());
        };

        let team_members = service_context
            .service
            .list_team_members(entity_access_receipt)
            .await
            .map_err(|e| ToolCallError {
                description: "unable to list team members".to_string(),
                internal_error: e.into(),
            })?;

        Ok(ListTeamMembersResponse {
            in_team: true,
            members: team_members.members.into_iter().map(Into::into).collect(),
            invited: team_members.invited.into_iter().map(Into::into).collect(),
        })
    }
}

/// Mint the caller's team-membership receipt, or `None` when they have no team.
async fn team_member_receipt<TSvc, ESvc>(
    service_context: &ServiceContext<TeamToolContext<TSvc, ESvc>>,
    request_context: RequestContext,
) -> Result<Option<EntityAccessReceipt<MemberTeamRole>>, ToolCallError>
where
    TSvc: TeamMembersService,
    ESvc: EntityAccessService,
{
    let team_info = service_context
        .entity_access_service
        .get_user_team(&request_context.user_id)
        .await
        .map_err(team_access_error)?;

    let Some(team_info) = team_info else {
        return Ok(None);
    };

    EntityAccessReceipt::try_new_authenticated_user(
        request_context.user_id,
        Entity {
            entity_id: team_info.team_id.to_string(),
            entity_type: EntityType::Team,
        },
        EntityPermission::TeamRole {
            role: team_info.role,
        },
    )
    .map(Some)
    .map_err(team_access_error)
}

fn team_access_error(err: AccessError) -> ToolCallError {
    let description = match err {
        AccessError::Unauthorized | AccessError::UnauthorizedWithMessage(_) => {
            "user is not a member of a team"
        }
        AccessError::NotFound(_) => "team not found",
        AccessError::BadRequest(_) => "invalid team membership",
        AccessError::DatabaseError(_) | AccessError::Internal => "failed to verify team membership",
    };

    ToolCallError {
        description: description.to_string(),
        internal_error: err.into(),
    }
}
