# CLA operations

How the Contributor License Agreement check is wired up and maintained. For the
agreements themselves see [`CLA.md`](../CLA.md) and
[`CLA-ENTITY.md`](../CLA-ENTITY.md); for the licensing policy behind them see
[`LICENSING.md`](../LICENSING.md).

## How it works

`.github/workflows/cla.yml` (generated from
`tooling/xtask/crates/xtask_workflows/src/workflows/cla.rs`) runs
[CLA assistant lite](https://github.com/contributor-assistant/github-action) on
every pull request against `main`. If the author has not signed, the action
comments asking them to, and reports a failing status. The contributor signs by
commenting the exact sentence:

```
I have read the CLA Document and I hereby sign the CLA
```

The action then records the signature — GitHub username, user id, comment id,
timestamp, and pull request number — in `signatures/v1/cla.json` on the
`cla-signatures` branch of this repository, and re-runs the check. A signature
covers all of that contributor's future pull requests. Commenting `recheck`
re-runs the check without signing.

Signatures live on their own branch so signing does not add commits to `main`.
That branch is also why no token or second repository is involved: the default
`GITHUB_TOKEN` can commit to an unprotected branch, so there is no PAT to create
or rotate.

`CONTRIBUTING.md` also states that opening a pull request constitutes agreement
to the CLA. That is a backstop for the gap between opening a PR and the bot
recording a signature — not a replacement for the recorded signature, which is
the thing that is actually worth having.

## One-time setup

1. **Create the `cla-signatures` branch.** An orphan branch with a single commit
   is ideal, so it carries no code:

   ```sh
   git checkout --orphan cla-signatures
   git rm -rf . >/dev/null
   printf 'CLA signatures recorded by the CLA workflow. Do not edit by hand.\n' > README.md
   git add README.md && git commit -m 'chore(cla): initialize signature branch'
   git push -u origin cla-signatures
   git checkout main
   ```

   Leave it unprotected — the action commits directly to it.

2. **Make the check required.** Settings → Branches → branch protection for
   `main` → require the `CLA Assistant` status check. Without this the check is
   advisory and an unsigned PR can still be merged.

## Day-to-day

**Someone contributes on behalf of a company.** Point them at
[`CLA-ENTITY.md`](../CLA-ENTITY.md) and have them email `legal@macro.com`. Keep
the countersigned agreement and its Schedule A somewhere durable. Their people
can still sign individually on their PRs; the two grants overlap harmlessly, and
an entity grant plus an individual signature is a stronger record than either
alone.

**Macro employees.** Employee contributions are covered by their employment
agreements, but the check applies to them too and signing is a one-time comment,
so leave it that way. Allowlisting a person is a standing assertion that Macro
already holds the rights to their work, and a wrong entry there silently loses a
grant — not worth it to save one comment.

**Bots.** Covered by the `*[bot]` allowlist entry in `cla.rs`.

**A contributor asks to withdraw.** They cannot revoke the license already
granted for contributions we have merged (Section 2 of the CLA is irrevocable),
and we do not remove their signature record, since it is the evidence that the
grant happened. They can of course stop contributing. Route any actual dispute
to `legal@macro.com` rather than handling it in a PR thread.

**Changing the CLA text.** Bump the version and date at the top of `CLA.md`,
change `path-to-signatures` in `cla.rs` to a new version directory
(`signatures/v2/cla.json`), and regenerate. Contributors are then asked to sign
the new version; the old signature file stays as the record of who agreed to
what. Ship the change as a PR with the reasoning in the description — that is a
commitment in [`LICENSING.md`](../LICENSING.md#what-we-commit-to).

## Contributions merged before the CLA existed

The CLA took effect 2026-08-11. Anything merged before then was submitted under
inbound-equals-outbound AGPLv3, which is enough to keep distributing Macro under
the AGPLv3 but does **not** by itself give Macro the right to license that code
commercially.

As of the date this document was written, every author with commits since the
AGPLv3 relicense on 2026-05-31 is a member of the team — there are no merged
drive-by contributions from outside. That makes the backfill small, but not
empty, and it is worth closing before the first commercial license goes out:

1. **Confirm the team is covered.** For each person who has committed, confirm
   there is a signed employment or contractor agreement that assigns or licenses
   the IP in their work to Macro. Employees usually are covered by default;
   contractors are covered only if their contract says so, and in the absence of
   an assignment clause a contractor owns the copyright in what they wrote.
2. **Have anyone not covered sign.** An individual [CLA](../CLA.md) is enough,
   and it can be signed on any open pull request. Do this while people are still
   around and still remember the work — a former contractor who is hard to reach
   is the expensive version of this problem.
3. **Then keep it current.** Once the check is required, new contributions take
   care of themselves.

If an outside contribution does turn up in the history later, and the author
cannot be reached or declines to sign, the options are: leave the code in place
but out of any commercially licensed build, rewrite it independently, or revert
it. Which one depends on how substantial it is — a typo fix is generally not
copyrightable enough to matter, a feature is.

Do not sell a commercial license covering code from a contributor whose rights
Macro does not hold. Have counsel confirm the state of this before the first
commercial license goes out.
