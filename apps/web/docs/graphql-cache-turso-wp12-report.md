# WP-12 browser rollout and operations report

Status: **local Chromium/Firefox subset passes; browser exposure remains 0%/off**

Machine-readable evidence:

- [`../measurements/cache-wasm-wp12.json`](../measurements/cache-wasm-wp12.json)
- [`../ops/graphql-cache-wp12-test-matrix.json`](../ops/graphql-cache-wp12-test-matrix.json)
- [`../ops/graphql-cache-wp12-dashboard.json`](../ops/graphql-cache-wp12-dashboard.json)

The report validator binds fresh evidence to the measured Jujutsu change ID,
a deterministic SHA-256 over every enumerated runtime/config/report source
input, Playwright version, absolute browser executable plus its verified
SHA-256, exact executable version, production origin/worker URLs, and the
production WASM URL/SHA-256. The WASM hash must equal both inspected package
and dist hashes in the WP-11 report. A change ID is amendment-stable; the source
digest closes that gap without cyclically hashing generated evidence or the
final commit object.
Evidence with false claimed scenarios, stale/future timestamps, inconsistent
browser/project/origin, unexpected URLs, or privacy-forbidden fields fails
closed. Each project starts by deleting stale evidence, runs the WP-12 file in
serial mode, accumulates only scenarios that completed successfully, and writes
once from the Playwright `afterAll` finalizer. The validator requires the exact
finalizer marker and complete required-test inventory; any prior failure leaves
that project without valid evidence. Treatment override is required and cannot
be converted into a skip.

## Honest status and environment

The final local run records:

| Browser | Exact executable | Reported user agent |
|---|---|---|
| Chromium | 145.0.7632.6 | HeadlessChrome/145.0.0.0 |
| Firefox | 151.0 | Firefox/151.0 |
| Safari | pending latest stable macOS external runner | not recorded |

These are localhost production-origin tests, not live CDN evidence. Every
scenario represented in an evidence `scenarios` object is required to be
exactly `true`.

This is only a **candidate local subset**, because the complete Section 10
matrix still contains required pending real-browser/native coverage. The
machine inventory explicitly identifies, among other gaps:

- latest stable Safari on a real macOS external runner;
- real-browser quota denial and private-mode coverage;
- Firefox storage eviction control;
- real Tauri and iOS no-browser-resource validation;
- live S3/CloudFront delivery verification;
- product-owner numeric budget acceptance.

No Safari, CloudFront, live PostHog mutation, provider dashboard deployment, or
complete local-gate result is claimed.

## Rollout and bounded emergency semantics

Browser activation requires the existing GraphQL transport gate and the
Boolean `enable-browser-turso-cache` env/PostHog gate. The independent
`disable-browser-turso-cache` kill switch always wins before activation.
Undefined browser rollout values fail closed. Only browser Turso activation is
session-latched. Tauri continues through its existing native cache path without
reading browser rollout flags, and its existing `ENABLE_GRAPHQL_SOUP` decision
remains dynamic: disabling that gate after native activation returns the plain
client without constructing a browser resource.

Once a browser cache client/host is constructed, that choice is latched for the
page session. A later remote disable or kill-switch update blocks new
activation and takes effect on the next reload/navigation. It deliberately does
**not** hot-dispose an active host: claimed/admitted mutations may have durable
but not yet observed settlement, so live retirement could cause duplicate API
calls or queued replay. Existing active pages may therefore continue until
their page-session lifetime ends. Incident command may separately terminate an
application session, accepting normal abrupt-owner recovery semantics.

The regression test at the GraphQL Soup client/exchange seam changes the flag
while a queued mutation is active and proves one API call, successful commit,
zero queued replay, and no host disposal. Tauri/import tests prove the GraphQL
Soup path constructs no browser worker on the native path and that a later
native GraphQL-gate disable returns the plain client.

## Telemetry contract

Cache telemetry uses detached anonymous root OpenTelemetry spans. It cannot
inherit an active user trace and suppresses user enrichment for the complete
span tree. Browser versions are major-only and app releases are bounded to
major.minor. Relayed worker observations use both app version and rollout
cohort `unknown`, never the reporter page's release or cohort.

Only fixed event names, allowlisted dimensions/classifications, and bounded
numeric measurements are exported. The complete forbidden-field contract in
the provider-neutral specification includes scope, entity/user/document IDs,
GraphQL documents/variables/results, operation name, record bytes, database
filenames, query, variables, and result.

Authoritative layers are:

- physical owner lifecycle: coordinator only;
- DB-ready: engine runtime only;
- storage read hit/miss: engine core only;
- host request and host-ready: page end-to-end events with distinct names;
- origin storage pressure: page `navigator.storage.estimate()` observation at
  host readiness and a bounded periodic interval, explicitly origin-wide and
  not OPFS-specific; its timer is cleared on disposal.

High-volume successes retain `aggregatedEventName`, count-weighted mean
semantics, and per-event sampling sequences. Errors are emitted once as raw
`sampleRate=1` events and are never buffered into delayed aggregates. Before
each raw error, matching pending success aggregates are flushed so the current
denominator is observable. Error rates divide raw errors by raw errors plus
success aggregate counts; weighted
latency percentiles use individual samples and exclude aggregate means.

Engine linear memory is emitted at ready, periodically with throttling, and at
drain with current/high-water bytes. This is WASM linear memory, not total
DedicatedWorker memory. Queue diagnostics explicitly distinguish `available`
from `unavailable`; compatibility defaults can never masquerade as an
authoritative empty queue. A static Turso aggregate returns only durable queue
count and `MIN(created_at_ms)`. The frozen browser schema and SQLite parity
schema include a `created_at_ms` covering index, with 10,000-row query-plan
checks. `COUNT(*)` remains O(n), so this is not described as free: refreshes are
rate-limited to once per 60 seconds, serialized after initialization or a
mutation checkpoint, and bounded by a 250 ms observation timeout. Errors and
timeouts emit telemetry errors only, retain the latest successful snapshot,
and never latch storage health or request a wipe. Heartbeat and drain perform
no storage I/O: they emit the cached depth and recalculate oldest age from the
cached timestamp. Drain cancels an outstanding observation. OTel uses the fixed
`cache.open_outcome`, `cache.queue_depth`, and `cache.oldest_age_ms` attributes.
No queue row, ID, GraphQL text, variables, or optimistic payload crosses the
storage/WASM/telemetry boundary.

`storage_reset_required` is the uncertainty phase; `logical_reset` is the
logical state transition; and `reset_wipe` is physical execution proof/failure.
The additive WASM open API reports only `opened-existing`, `opened-new`,
`reset-incompatible`, `reset-corrupt`, or `reset-storage-uncertain`. The
coordinator is the sole reset-phase telemetry authority. Engine fatal messages
carry the bounded `storage-reset-required` code rather than requiring reason
parsing; recovery open reports its bounded outcome but never repeats the
uncertainty event. Accepted recovery readiness emits logical reset and one wipe
proof. A failed `wipe-before-open` activation emits a typed coarse
`resetAttempt='wipe-before-open'` wipe failure. Unit and production-browser
sequences require exactly uncertainty → logical reset → wipe.

No standards-based API can report total active DedicatedWorker JS/native
memory in both tested browsers without COOP/COEP: Chromium's
`measureUserAgentSpecificMemory` requires cross-origin isolation and Firefox
does not expose it in workers; `performance.memory` is non-standard and absent
in both tested worker contexts. The dashboard therefore retains only the WASM
linear-memory alert. Any total-worker-memory alert remains disabled/pending and
must not relabel origin/process/linear estimates as worker total.

## Provider-neutral operations specification

`graphql-cache-wp12-dashboard.json` is a validated provider-neutral
specification, not deployed or executable dashboard configuration. It contains
concrete selectors/formulas, aggregate/sample weighting rules, units, windows,
minimum counts, positive warning/critical thresholds, exact unique
query/panel/alert inventories, bounded rollout stages, required promotion
gates, and deterministic threshold evaluator tests. The Section 10 validator
also pins each canonical requirement to its exact evidence list.

A critical evaluation produces a machine decision to set
`disable-browser-turso-cache=true`, but the credentialed external executor is
not present or verified. Re-enablement always requires human approval after a
clean soak window. Until the executor and provider queries are deployed and
verified, exposure remains zero.

## Local browser subset

The production-artifact harness proves for Chromium and Firefox:

- its query-parameter control constructs zero cache hosts, workers, or WASM;
  this remains harness-only lazy/resource evidence;
- the actual `getGraphqlSoupClient` production path evaluates the default-off
  selector without constructing a host, SharedWorker, Worker, or WASM;
- the actual analytics singleton is initialized with a dummy local key and all
  supported capture, persistence, external loading, and flags-network paths
  disabled; supported `featureFlags.overrideFeatureFlags` treatment activates
  the exact lazy production host/SharedWorker/Worker/WASM with zero PostHog
  payload or network requests;
- treatment imports the exact production host but remains lazy until use;
- same-page disable remains session-latched, while a new navigation with both
  enable flags true and the emergency-disable flag true creates no cache host
  or resources;
- exactly one hashed WASM, engine worker URL, and coordinator worker URL;
- closing a second host in the same page does not replace the active engine;
- a genuine second browser `Page` joins as a standby without an engine; closing
  that page leaves the first page's same engine alive and able to read/write;
- offline graceful owner handoff preserves a cached query;
- identity change resets prior data;
- the production logout cache lifecycle wipes the registered host and removes
  prior cached data in both local browsers;
- abrupt engine loss rejects the old request and replacement starts empty;
- for each persistent mutating RPC (`write`, enqueue, claim, defer, commit,
  rollback, invalidate, delete-records, and clear), the browser-test worker
  wrapper arms one production runtime hook, proves admission, blocks before
  core execution, and the harness terminates the actual DedicatedWorker; the
  old request rejects, replacement records and durable queue are empty, and a
  distinctive post-replacement request-admission barrier observes zero
  unexpected mutating admissions. The count is derived from runtime events,
  not a literal replay value. The iframe-based direct harness avoids popup
  registration races. This deliberately proves the
  uncertain-admission policy boundary and does not claim mid-SQL execution;
  teardown is excluded because it only removes an in-memory dependency and is
  not a persistent mutation;
- after graceful lock-safe close, a browser-test-only storage-control worker
  loads a separately named WASM built with the `browser-test-hooks` Cargo
  feature to alter either namespace metadata or one queue codec payload. The
  default production WASM inspector proves both destructive exports absent,
  and the one-external-production-cache-WASM deployment gate is unchanged.
  These cases are labeled real-browser **test-artifact** evidence, while reopen
  continues through the production host/worker, reports the exact coarse
  outcome, emits each reset phase once, wipes records and queue, and remains
  usable in Chromium and Firefox;
- payload-free queue telemetry is observed from the real worker path;
- no cross-origin isolation or SharedArrayBuffer dependency, and no supported
  total-worker-memory API in either tested worker context.

The treatment/kill selector test uses only the actual analytics singleton and
PostHog's supported `featureFlags.overrideFeatureFlags` API; no test-only
selector is substituted. Chromium additionally exercises local storage eviction through CDP. Firefox
eviction remains pending because no equivalent local control is available. The
complete lower-level and real-browser linkage is recorded in the Section 10
matrix; lower-level evidence is not relabeled as real-browser evidence.

## Decision

**Do not expose.** Production browser exposure remains **0%/off** until every
required matrix and external rollout gate is accepted. Follow
[`graphql-cache-turso-wp12-runbook.md`](graphql-cache-turso-wp12-runbook.md).
