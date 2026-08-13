//! `CLA` — blocks a pull request until its author has accepted the Contributor
//! License Agreement.
//!
//! Contributors sign by commenting on their PR; the action records the
//! signature (GitHub username, comment id, timestamp, CLA version) on the
//! orphan `cla-signatures` branch of this repository, which keeps signature
//! commits out of `main` without needing a second repository or a PAT — the
//! default `GITHUB_TOKEN` can push to an unprotected branch. See `CLA.md`,
//! `CLA-ENTITY.md`, and `LICENSING.md` for the documents themselves, and
//! `docs/CLA_OPERATIONS.md` for setup and day-to-day handling.
//!
//! Uses `pull_request_target` rather than `pull_request` because a fork PR needs
//! write access to comment and to set a status. Nothing here checks out or runs
//! PR code, so the elevated context is not exposed to it — keep it that way.

use gh_workflow::{
    Event, Expression, IssueComment, IssueCommentType, Job, Level, Permissions, PullRequestTarget,
    PullRequestType, Step, Workflow,
};

use crate::workflows::runners;

/// `contributor-assistant/github-action` v2.6.1 ("CLA assistant lite").
const CLA_ACTION_SHA: &str = "ca4a40a7d1004f18d9960b404b97e5f30a505a08";

/// The exact sentence a contributor comments to sign. Kept in sync with the
/// "How to sign" section of `CLA.md`.
const SIGN_SENTENCE: &str = "I have read the CLA Document and I hereby sign the CLA";

/// Branch holding the signature file. Must stay unprotected so the default
/// `GITHUB_TOKEN` can commit signatures to it.
const SIGNATURES_BRANCH: &str = "cla-signatures";

/// Build the workflow.
pub fn cla() -> Workflow {
    Workflow::new("CLA")
        .on(Event::default()
            .issue_comment(IssueComment::default().add_type(IssueCommentType::Created))
            .pull_request_target(
                PullRequestTarget::default()
                    .add_type(PullRequestType::Opened)
                    .add_type(PullRequestType::Reopened)
                    .add_type(PullRequestType::Synchronize)
                    .add_branch("main"),
            ))
        .add_job("cla", cla_job())
}

fn cla_job() -> Job {
    Job::default()
        .name("CLA Assistant")
        .runs_on(runners::Runner::TinyNoCache.to_string())
        // Only react to a signature comment (or an explicit `recheck`) on a PR,
        // not to every comment on every issue.
        .cond(Expression::new(format!(
            "(github.event.issue.pull_request \
             && (github.event.comment.body == 'recheck' \
             || contains(github.event.comment.body, '{SIGN_SENTENCE}'))) \
             || github.event_name == 'pull_request_target'"
        )))
        .permissions(Permissions {
            // `actions: write` lets the action re-run itself after a signature.
            actions: Some(Level::Write),
            // Commits the signature file to the `cla-signatures` branch.
            contents: Some(Level::Write),
            issues: Some(Level::Write),
            pull_requests: Some(Level::Write),
            statuses: Some(Level::Write),
            ..Default::default()
        })
        .add_step(cla_step())
}

fn cla_step() -> Step<gh_workflow::Use> {
    Step::new("Check CLA signature")
        .uses("contributor-assistant", "github-action", CLA_ACTION_SHA)
        .add_env(("GITHUB_TOKEN", "${{ secrets.GITHUB_TOKEN }}"))
        .add_with(("path-to-signatures", "signatures/v1/cla.json"))
        .add_with((
            "path-to-document",
            "https://github.com/macro-inc/macro/blob/main/CLA.md",
        ))
        .add_with(("branch", SIGNATURES_BRANCH))
        // Bots cannot agree to anything.
        .add_with(("allowlist", "*[bot]"))
        .add_with(("custom-pr-sign-comment", SIGN_SENTENCE))
        // Contributors keep their own copyright, so leave merged PRs unlocked.
        .add_with(("lock-pullrequest-aftermerge", false))
        .add_with(("custom-notsigned-prcomment", notsigned_comment()))
        .add_with((
            "custom-allsigned-prcomment",
            "All contributors have signed the CLA. Thanks — on to the review.",
        ))
}

/// First comment a new contributor sees. Explains the ask before making it.
fn notsigned_comment() -> String {
    indoc::indoc! {"
        Thanks for the pull request! Before we can merge it, we need you to sign
        our [Contributor License Agreement](https://github.com/macro-inc/macro/blob/main/CLA.md).

        Macro is dual licensed: everyone gets it under the AGPLv3, and companies
        that cannot work within the AGPLv3's terms can buy a commercial license.
        The CLA is what lets us include your contribution in both. **You keep the
        copyright in your work** and can reuse it anywhere — it is a license
        grant, not an assignment. What we commit to in return is written down in
        [LICENSING.md](https://github.com/macro-inc/macro/blob/main/LICENSING.md#what-we-commit-to).

        If your employer owns what you write, or you are contributing on a
        company's behalf, email legal@macro.com for the
        [Entity CLA](https://github.com/macro-inc/macro/blob/main/CLA-ENTITY.md)
        instead of signing below.

        To sign, post a comment on this pull request with exactly:
    "}
    .to_string()
}
