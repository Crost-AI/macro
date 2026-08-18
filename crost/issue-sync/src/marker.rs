//! Loop-protection markers for bidirectional sync.

const MARKER_PREFIX: &str = "<!--macro-sync:";
const MARKER_SUFFIX: &str = "-->";
pub const MACRO_METADATA_KEY: &str = "crost_sync_origin";

/// Embed a sync origin marker in a GitHub issue body.
pub fn embed_github_marker(body: &str, origin_id: &str) -> String {
    let marker = format!("{MARKER_PREFIX}{origin_id}{MARKER_SUFFIX}");
    if body_contains_marker(body, origin_id) {
        body.to_string()
    } else if body.trim().is_empty() {
        marker
    } else {
        format!("{body}\n\n{marker}")
    }
}

/// Strip all macro-sync markers from a GitHub body for display/hash.
pub fn strip_github_markers(body: &str) -> String {
    let mut out = String::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(MARKER_PREFIX) && trimmed.ends_with(MARKER_SUFFIX) {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    out.trim().to_string()
}

/// Returns true when the body carries our own origin marker.
pub fn body_has_origin(body: &str, origin_id: &str) -> bool {
    body_contains_marker(body, origin_id)
}

/// Returns any origin id embedded in the body, if present.
pub fn parse_github_origin(body: &str) -> Option<String> {
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(inner) = trimmed
            .strip_prefix(MARKER_PREFIX)
            .and_then(|s| s.strip_suffix(MARKER_SUFFIX))
        {
            return Some(inner.to_string());
        }
    }
    None
}

fn body_contains_marker(body: &str, origin_id: &str) -> bool {
    body.contains(&format!("{MARKER_PREFIX}{origin_id}{MARKER_SUFFIX}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_and_strip_round_trip() {
        let origin = "sync-abc123";
        let embedded = embed_github_marker("hello", origin);
        assert!(body_has_origin(&embedded, origin));
        assert_eq!(strip_github_markers(&embedded), "hello");
    }

    #[test]
    fn parse_origin_from_body() {
        let body = "Title\n\n<!--macro-sync:evt-1-->";
        assert_eq!(parse_github_origin(body).as_deref(), Some("evt-1"));
    }
}
