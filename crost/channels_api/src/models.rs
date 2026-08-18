use serde::{Deserialize, Serialize};

/// `POST /api/v1/channels`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateChannelBody {
    pub name: String,
    pub private: bool,
}

/// `POST /api/v1/channels` → `{id}`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateChannelResponse {
    pub id: String,
}

/// `POST /api/v1/channels/{id}/members`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddMemberBody {
    pub user_or_agent_ref: String,
}

/// `POST /api/v1/channels/{id}/messages`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PostMessageBody {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
}

/// `POST /api/v1/channels/{id}/messages` → `{message_id}`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PostMessageResponse {
    pub message_id: String,
}
