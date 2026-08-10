# WP-03 browser report

Executed: **2026-08-10** on the repository's Nix development environment.

## Results

The same three-real-tab harness was executed in all three Playwright browser
engines. Each run opened three popup tabs on one origin and completed epochs
1 through 4: graceful page departure, abrupt page loss, and worker-only failure
while the page remained alive.

| Engine | Browser build | Topology scenario | Reported page capabilities |
|---|---|---|---|
| Chromium | Chrome for Testing 149.0.7827.55 | PASS | SharedWorker, Worker, MessageChannel, Web Locks, OPFS |
| Firefox | Firefox 151.0 | PASS | SharedWorker, Worker, MessageChannel, Web Locks, OPFS |
| WebKit | Playwright WebKit 26.5 | PASS | SharedWorker, Worker, MessageChannel, Web Locks; no page OPFS in this Linux build |

Every passing run reported:

```json
{
  "openedTabs": 3,
  "ownerEpochs": [1, 2, 3, 4],
  "gracefulPreservedFakeDb": true,
  "abruptLossRejectedInFlight": true,
  "abruptLossWipedFakeDbBeforeActivation": true,
  "workerOnlyLossPageStayedAlive": true,
  "workerOnlyLossWipedFakeDbBeforeActivation": true,
  "failedEngineTerminationEpochs": [3],
  "staleResponseViaMessagePortDrops": 1,
  "routerMaxActiveOwners": 1,
  "maxObservedHeldOwnerLocks": 1,
  "activeEpochsBlockingIndependentOwnerLockProbe": [1, 2, 3, 4],
  "finalObservedOwnerLockCount": 1,
  "lockLifecycleOrderingObserved": true,
  "wipeLifecycleBeforeReadyEpochs": [3, 4],
  "directMessageChannelObserved": true,
  "statesObserved": [
    "activating",
    "active",
    "draining",
    "resetting-after-loss"
  ],
  "engineReplacedEpochs": [2, 3, 4]
}
```

The live report also includes `lockLifecycleTimestampsMs`, keyed by
`<epoch>:<phase>`, for requesting/acquired/wipe/open/ready/close/release events.

The epoch-3 worker throws an uncaught harness error. `Worker.onerror` on the
still-open page explicitly terminates and clears that worker before reporting
loss; epoch 4 then acquires the exclusive owner lock, wipes, and activates on
the same still-registered page. Reading the epoch-3 marker returns `null`.

The old epoch-2 response used for stale fencing traverses a new probe
`MessageChannel` and the normal coordinator engine-message adapter. The actual
failed route is deliberately closed and its worker is gone, so the report does
not claim the stale envelope came from that closed physical port.

`routerMaxActiveOwners` is state-machine evidence. The Web Locks figures are
independent page observations at each active epoch: `navigator.locks.query()`
reported one held owner lock and an `ifAvailable` exclusive probe could not
acquire it. Worker lifecycle timestamps reported lock acquisition, wipe,
database open, ready, close, and release ordering. Those timestamps remain
trusted worker instrumentation, and periodic observations cannot prove the
absence of an overlap between samples; physical mutual exclusion relies on the
browser's exclusive Web Lock contract.

`crossOriginIsolated` was false and `SharedArrayBuffer` was unavailable in all
three harness pages. `createSyncAccessHandle` was not exposed on the page-side
`FileSystemFileHandle` prototype; that API must be checked inside a dedicated
worker by WP-02 and is not exercised by this fake-DB topology spike.

Chromium and Firefox ran with the checked-in Playwright test and the matching
Nix browser bundle available on this machine. The repository currently resolves
Playwright 1.62 while the available Nix WebKit bundle is paired with Playwright
1.61; running that mismatched pair failed during Playwright setup on an unknown
`PushAPIEnabled` setting. The WebKit scenario above was therefore executed
with the installed matching Playwright 1.61 runner and WebKit 26.5 bundle. The
application harness itself passed without console or page errors.

Playwright WebKit is a real browser-engine run but does not replace validation
on WP-00's eventual approved macOS Safari/WKWebView version. The lack of OPFS in
this Linux WebKit build is a Gate-G0 input for WP-02, not a coordinator failure.

## Automated reproduction

From the repository root, with matching Playwright browsers installed from the
existing `@playwright/test` tooling:

```sh
direnv exec . bash -lc '\cd apps/web && bunx --bun playwright test \
  --config spikes/graphql-cache-turso-coordinator/playwright.config.ts'
```

The test starts the local Vite server itself. If the browser binaries are not
present, install only the existing Playwright browser assets (no manifest or
lockfile change):

```sh
direnv exec . bash -lc '\cd apps/web && bunx --bun playwright install \
  chromium firefox webkit'
```

## Manual reproduction

1. Start the spike server:

   ```sh
   direnv exec . bash -lc '\cd apps/web && bunx --bun vite \
     --config spikes/graphql-cache-turso-coordinator/vite.config.ts'
   ```

2. Open <http://127.0.0.1:4179/> in the target browser.
3. Allow same-origin popups, then click **Run three-tab scenario**.
4. Confirm the result block becomes green and reports `"passed": true`.
5. Inspect the event log: epoch 1 is followed by a graceful epoch-2 handoff,
   abrupt page-loss epoch 3, then worker-only-loss epoch 4 while the final page
   remains open. Both loss replacements wipe before activation.
6. Confirm the stale probe reports a MessagePort delivery/drop and the epoch-3
   worker reports explicit termination.
7. In browser developer tools, inspect Web Locks while epoch 4 is active. There
   must be exactly one held `graphql-cache-owner:<scope>` lock, and a competing
   `ifAvailable` exclusive request must fail to acquire it.

## Remaining browser blockers

- WP-00 still needs to name the approved Safari/WKWebView target; run this
  harness there rather than treating Playwright WebKit as the final target.
- WP-02 must test OPFS and sync access handles inside the dedicated worker.
- Production should add a bounded engine heartbeat/watchdog for silent
  worker-only death while the owning page remains alive. This spike validates
  explicit retirement on `Worker.onerror`; the error event remains a fast
  path, not the sole failure detector.
