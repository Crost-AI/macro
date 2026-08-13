# Contributing to Macro

Thanks for your interest in contributing! Macro is fully open source under the
[AGPLv3](LICENSE.txt), and we welcome outside contributions. This guide covers
how to get a change from idea to merged PR.

One piece of paperwork before your first PR merges: we need you to sign the
[Contributor License Agreement](CLA.md). It takes one comment on your pull
request. See [Contributor License Agreement](#contributor-license-agreement)
below for what it says and why we ask.

## Start with an issue

Open an issue before putting up a PR. This applies to both features and fixes. It lets us confirm the change is wanted and agree on an approach before you invest time into it. PRs that show up without a linked issue may be closed.

## AI-assisted contributions

Useful contributions require human effort. You may use whatever tools best
serve you including AI tools, but if you don't understand the work you're
doing it's probably not useful.

You should:
- Understand the changes and decisions you made well enough to answer questions in review.
- See your change working in a local development environment

Unreviewed AI output submitted as-is wastes reviewer time and will be closed.

## Conventions

We use semantic (Conventional Commits) naming for branches and PR titles:

```
feat(chat): add dev observability
fix(email): handle empty thread subjects
```

- **PR title:** `type(scope): short description` this becomes the commit
  message on merge, so make it accurate.
- **Branch name:** same idea, slash-separated, e.g. `feat/chat-dev-observability`.

Common types: `feat`, `fix`, `chore`

## PR bodies

Keep PR bodies concise and write them yourself. A few sentences covering what
changed and why, a link to the issue, and anything a reviewer needs to know.
No generated boilerplate, no exhaustive file-by-file change lists.

## Development setup

See [docs/RUNNING_LOCALLY.md](docs/RUNNING_LOCALLY.md) for running the full app
locally. The short version: use the Nix shell, then `just run_local`.

## Before you push

- Follow the [style guide](docs/STYLE_GUIDE.md).
- Format and lint: `cargo fmt` and `just clippy` for Rust changes.
- Run the tests for the crates you touched: `cargo test -p <crate>`.
- If you changed SQL queries or migrations, run `just prepare_db` from the
  repository root to refresh the sqlx cache.

## Contributor License Agreement

Macro is dual licensed: everyone gets it under the [AGPLv3](LICENSE.txt), and
companies that can't work within the AGPLv3's terms can buy a commercial
license. Selling that commercial license means licensing the whole codebase —
including your contribution — under terms other than the AGPLv3, and we can only
do that with your permission. So we ask contributors to sign a CLA:

- **[CLA.md](CLA.md)** if you're contributing as an individual.
- **[CLA-ENTITY.md](CLA-ENTITY.md)** if your employer owns your work, or you're
  contributing on a company's behalf. Email
  [legal@macro.com](mailto:legal@macro.com) to get one countersigned.

**How to sign.** Open your pull request as usual. Our CLA bot comments on it
with a link and instructions; you accept by leaving one comment. It's a one-time
thing that covers all your future contributions, and the check turns green
within a minute or so. Signatures are recorded on this repository's
[`cla-signatures` branch](https://github.com/macro-inc/macro/tree/cla-signatures).

**What it does and doesn't do.** You keep the copyright in your work and can
reuse it anywhere, under any license, without asking us — it's a license grant,
not an assignment. What you're granting is the right to relicense your
contribution, including under proprietary terms. We'd rather say that plainly
than hide it. What we commit to in return — Macro stays AGPLv3, and nothing gets
moved into a proprietary edition — is written down in
[LICENSING.md](LICENSING.md#what-we-commit-to), along with how commercial
revenue gets reinvested in the project and community.

By opening a pull request against this repository you agree to the terms of the
CLA for the contributions in it, whether or not the bot has recorded your
signature yet.

Not everything needs a CLA. Bug reports, reproductions, design feedback, and
discussion don't — only contributions of copyrightable work do.

Already contributed before we introduced this? We'll ask you to sign on your
next PR, and we may reach out about earlier ones. Questions:
[legal@macro.com](mailto:legal@macro.com).
