# Browser Turso cache rollout and rollback runbook

Status: **candidate local subset only; exposure 0%/off**

The source of truth is the validated provider-neutral specification
[`../ops/graphql-cache-wp12-dashboard.json`](../ops/graphql-cache-wp12-dashboard.json)
and complete Section 10 inventory
[`../ops/graphql-cache-wp12-test-matrix.json`](../ops/graphql-cache-wp12-test-matrix.json).
Neither file proves provider deployment or an executable PostHog integration.

## Preconditions

Do not begin internal exposure until all are true:

- every required Section 10 matrix entry is verified on its required target;
- current fresh Chromium/Firefox evidence passes the report validator;
- latest stable Safari passes on a real macOS external runner;
- live S3/CloudFront delivery and expected production URLs are verified;
- numeric budgets are accepted by the product owner;
- provider queries/alerts are deployed from the neutral formulas and verified;
- an approved external executor can set the PostHog kill switch;
- human rollout approval is recorded.

Current candidate does not satisfy these preconditions.

## Flags

| Purpose | PostHog | Build override | Default |
|---|---|---|---|
| Browser exposure | `enable-browser-turso-cache` | `VITE_ENABLE_BROWSER_TURSO_CACHE` | off |
| Emergency stop | `disable-browser-turso-cache` | `VITE_DISABLE_BROWSER_TURSO_CACHE` | not tripped |

The existing `enable-graphql-soup` transport gate remains a prerequisite. A
true disable source always wins before activation; undefined browser flags fail
closed. PostHog's enable flag remains Boolean so percentage/cohort targeting is
handled by PostHog. Tauri does not evaluate these browser-cache flags, is not
session-latched by this rollout, and retains dynamic `ENABLE_GRAPHQL_SOUP`
behavior.

## Session-safe emergency behavior

The browser Turso activation decision is latched when its client/host is
constructed. Changing either remote browser flag does not dispose that live
host. This is required
because an admitted/claimed mutation may have completed durably while its
settlement is still in flight; hot retirement could duplicate the API call or
replay queued work.

Therefore:

1. Setting `disable-browser-turso-cache=true` immediately prevents new page
   sessions from activating the browser cache.
2. Existing active page sessions continue until reload/navigation or normal
   page close.
3. Reload/navigation applies the kill switch and uses the uncached client.
4. If incident command requires faster termination, terminate the application
   session separately and treat it as abrupt owner loss; replacement recovery
   wipes uncertain storage before ready proof.

The bounded delay is one active page-session lifetime. Do not describe this as
instant hot disablement. Do not add a feature-flag listener that retires a live
host.

## Staged rollout

| Stage | Maximum | Minimum clean soak |
|---|---:|---:|
| Internal | 1% | 24 h |
| Canary | 5% | 48 h |
| General | 100% | 168 h |

Promotion requires all formulas below warning for the complete window, all
matrix gates required for that stage, accepted budgets, and human approval.
General exposure additionally requires live S3/CloudFront evidence. Retain at
least two prior application releases for at least 168 hours.

## Provider-neutral alerts

The JSON specification defines concrete formulas, units, windows, minimum
logical counts, and thresholds for:

- engine DB-ready error rate;
- reset/wipe per DB-ready;
- transaction error rate;
- abrupt owner loss/replacement per DB-ready;
- host end-to-end request p95;
- treatment-minus-control navigation p95;
- WASM linear-memory p95;
- origin-wide storage pressure p95.

Rate formulas count raw `sampleRate=1` errors once and combine them with
count-weighted **success** aggregates identified by `aggregatedEventName`.
Matching pending successes flush before each raw error, so the denominator is
current; errors are never delayed aggregates. Latency percentiles exclude aggregate
means and use individual `sampleRate` weights. Origin storage pressure is
sampled at readiness and a bounded periodic interval from
`navigator.storage.estimate()` and must not be called OPFS-only quota. Its timer
is cleared on disposal. WASM linear memory is not total worker memory.

Queue diagnostics explicitly report `available`/`unavailable`. A payload-free
aggregate storage query refreshes only during initialization and serialized,
once-per-60-second mutation checkpoints; a 250 ms observation timeout/error is
telemetry-only and preserves the latest successful snapshot. Heartbeat and
drain perform no query: they emit cached depth and recalculate age from the
cached oldest timestamp, and drain cancels a hanging observation. The
`created_at_ms` covering index prevents a wide payload scan and has a 10,000-row
query-plan test. `COUNT(*)` remains O(n), so never describe the query as free;
the rate limit and timeout are part of its budget. The provider spec contains
depth/age queries and a queue-health panel. Total
DedicatedWorker JS/native memory remains pending: the tested unisolated worker
contexts expose neither a cross-browser `performance.memory` nor
`measureUserAgentSpecificMemory`, the latter requires cross-origin isolation
and lacks Firefox worker support. Only WASM linear memory is alerted; the
worker-total alert remains disabled and no origin/process estimate may be
relabeled. The provider translation must preserve these formulas and pass the
repository threshold evaluator tests.

## Rollback decision and execution

Any validated critical alert produces `trip-kill-switch` for
`disable-browser-turso-cache=true`. The repository performs no credentialed
mutation. An approved external executor must:

1. confirm the alert met its minimum logical count and complete window;
2. set the Boolean disable flag true;
3. verify new/reloaded treatment and control probes remain uncached;
4. leave existing active sessions latched unless incident command separately
   orders application-session termination;
5. record provider query output, flag audit evidence, app/browser buckets, and
   timestamps.

If executor/deployment is unavailable, keep exposure at zero. Never claim a
live mutation from the repository evaluator.

## Re-enable

Never automatically clear the kill switch. Re-enable only after:

1. root cause and affected release are recorded;
2. the fix passes the full required matrix and fresh evidence validation;
3. one complete clean soak window is observed;
4. product/incident owner approves;
5. prior release retention remains available.

Restart from internal exposure.

## Storage lifecycle

- **Normal page close/controlled owner handoff:** stop admission, drain admitted
  work, close cleanly, preserve main/WAL, then transfer ownership.
- **Explicit clear/identity change:** emit `logical_reset`; do not count it as
  a physical wipe execution.
- **Abrupt/uncertain owner loss:** reject old-epoch requests and remove
  main/WAL under one owner lock. A typed engine fatal code reaches the
  coordinator, which alone emits `storage_reset_required`; recovery open never
  repeats it. Accepted engine-ready wipe proof then emits one `logical_reset`
  and one `reset_wipe`.
- **Internal incompatible/corrupt open:** the bounded open outcome reaches the
  coordinator, which emits exactly one event for each uncertainty, logical
  reset, and completed physical wipe phase.
- **Recovery activation failure:** emit `reset_wipe` failure with typed coarse
  `resetAttempt='wipe-before-open'`, not success.
- **Uncertain admitted optimistic enqueue:** quarantine the scope and never
  infer retry safety.
- **Identity/logout/namespace transition:** wipe all records, queue rows, and
  optimistic layers before rebinding.

## Evidence commands

```sh
just report-cache-wp12
bun scripts/cache-wp12/report.ts
```

Before each run, global setup deletes both per-project evidence files. WP-12
runs serially per project; each successful test contributes only its own
scenarios, and Playwright `afterAll` writes evidence only after the complete
required non-eviction inventory passed. The treatment override is required and
cannot skip. The validator requires this exact finalizer record, so a prior
failure cannot leave promotable partial or stale evidence.

The report validator requires fresh same-revision evidence, a deterministic
path-plus-bytes digest of the complete tracked web/cache/observability runtime,
all tracked `crates/**` files as a conservative local dependency closure,
build/lockfile, operations-spec, and evidence-tooling input set (excluding
generated measurements, dist, and docs), exact executable and Playwright
versions, an executable SHA-256 recomputed from an existing file,
matching browser/user-agent majors, exact local production origin and worker
URLs, the expected hashed WASM URL, and a SHA-256 matching WP-11 inspection.
The amendment-stable change ID is insufficient alone; the source digest binds
the measured inputs without a final-commit cycle. The query-parameter control
remains harness-only evidence, while separate production Chromium/Firefox tests
exercise the actual default-off GraphQL selector, a genuine standby browser
page, logout cache reset, deterministic pre-core termination for every
persistent mutating RPC, and lock-safe incompatible/corrupt reopen controls.
The fault hook exists only in the browser-test worker wrapper; it does not add a
production coordinator/page message. The termination harness uses same-page
iframes, waits for replacement readiness plus a distinctive request-admission
barrier, and derives zero unexpected mutating admissions from observed events.
The destructive storage controls exist only in a separately named WASM built
with the `browser-test-hooks` Cargo feature and loaded by a browser-test
DedicatedWorker after graceful close. Matrix/evidence label these recovery
cases real-browser test-artifact, not exact-production-artifact. The default
production inspector requires both exports absent, and the production deploy
still permits one external cache WASM. The selector treatment test initializes the
actual analytics PostHog singleton with a dummy local key, disables every
supported capture/persistence/external-loading/flags-network path, applies only
supported `featureFlags.overrideFeatureFlags`, and verifies exact lazy cache
resources, same-session latching, next-navigation emergency disable, and zero
PostHog requests. No test-only selector is substituted. Archive Safari/live CDN/provider artifacts
separately; do not edit local evidence to imply those runs occurred.
