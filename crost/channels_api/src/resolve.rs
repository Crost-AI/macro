use crate::error::ApiError;
use macro_db_client::user::get::get_user_by_email::get_user_by_email;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;

/// Resolve a Crost `user_or_agent_ref` to a Macro user id.
///
/// Accepted forms (W2.4 / W2.8 contract):
/// - `macro|user@example.com` — canonical Macro user id
/// - `user@example.com` — email shorthand (must exist in `User` table)
/// - `agent-claude`, `team-acme`, … — slug shorthand → `macro|{slug}@agents.crost.local`
pub async fn resolve_user_ref(
    db: &PgPool,
    user_ref: &str,
) -> Result<MacroUserIdStr<'static>, ApiError> {
    let trimmed = user_ref.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request("user_or_agent_ref must not be empty"));
    }

    let macro_id = if trimmed.contains('|') {
        trimmed.to_string()
    } else if trimmed.contains('@') {
        format!("macro|{trimmed}")
    } else {
        format!("macro|{trimmed}@agents.crost.local")
    };

    let user_id = MacroUserIdStr::try_from(macro_id)
        .map_err(|_| ApiError::bad_request("invalid user_or_agent_ref"))?;
    ensure_user_exists(db, user_id.as_ref()).await?;
    Ok(user_id)
}

async fn ensure_user_exists(db: &PgPool, macro_user_id: &str) -> Result<(), ApiError> {
    let email = macro_user_id
        .strip_prefix("macro|")
        .ok_or_else(|| ApiError::bad_request("invalid macro user id"))?;

    match get_user_by_email(db.clone(), email).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(ApiError::not_found("user not found")),
        Err(err) => {
            tracing::error!(error=?err, "user lookup failed");
            Err(ApiError::internal("user lookup failed"))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn macro_id_passthrough_format() {
        let id = "macro|alice@seed.macro.local";
        assert!(id.contains('|'));
        assert!(id.starts_with("macro|"));
    }

    #[test]
    fn email_shorthand_format() {
        let email = "bob@seed.macro.local";
        let macro_id = format!("macro|{email}");
        assert_eq!(macro_id, "macro|bob@seed.macro.local");
    }

    #[test]
    fn agent_slug_format() {
        let slug = "agent-claude";
        let macro_id = format!("macro|{slug}@agents.crost.local");
        assert_eq!(macro_id, "macro|agent-claude@agents.crost.local");
    }
}
