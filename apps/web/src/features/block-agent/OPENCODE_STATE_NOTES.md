# opencode — Reference Notes for the Agent Block

How opencode (local checkout: `/Users/eric/Code/opencode`) structures its data layer, control
flow, and streaming state. Companion to `CHANNEL_BLOCK_NOTES.md` (Macro's channel block); the
closing section compares the two against our current one-shot fold. All paths below are relative
to `/Users/eric/Code/opencode` unless prefixed with `macro:`.

opencode is a SolidJS app talking to a local/remote server over HTTP + one SSE event stream per
server. The server does the equivalent of our fold **server-side**: the client receives
ready-made `Message` + `Part` objects and delta events, and only has to merge them.

---

## 1. Store architecture

### The `State` shape (`packages/app/src/context/global-sync/types.ts:33–86`)

One store type serves both the per-directory child stores and (a subset of fields) the
server-wide session store. Everything session-scoped is a **flat dictionary keyed by id**:

```ts
type State = {
  status: "loading" | "partial" | "complete"
  agent: Agent[]; command: CommandInfo[]; reference: ReferenceInfo[]
  project: string; provider: ...; config: Config; path: Path
  session: Session[]                    // sorted by id (ids are time-ordered)
  sessionTotal: number
  session_status:       { [sessionID]: SessionStatus }        // idle | busy | retry{attempt,message,next}
  session_working(id): boolean                                 // helper: status?.type !== "idle"
  session_diff:         { [sessionID]: FileDiffInfo[] }
  todo:                 { [sessionID]: Todo[] }
  permission:           { [sessionID]: PermissionRequest[] }   // ordered by id
  question:             { [sessionID]: QuestionRequest[] }
  message:              { [sessionID]: Message[] }             // sorted by id
  session_message:      { [sessionID]: SessionMessageInfo[] }  // v2 "source" projection
  part:                 { [messageID]: Part[] }                // sorted by id — NOT nested in message
  part_text_accum_delta:{ [partID]: string }                   // streaming text hot path
  limit: number                                                // session-list page size
  mcp / mcp_resource / lsp / vcs ...
}
```

Constants (`types.ts:132–135`): `MAX_DIR_STORES = 30`, `DIR_IDLE_TTL_MS = 20min`,
`SESSION_RECENT_WINDOW = 4h`, `SESSION_RECENT_LIMIT = 50`. Plus
`SESSION_CACHE_LIMIT = 40` (`global-sync/session-cache.ts:5`) and
`sessionInfoLimit = 2048` (`context/server-session.ts:30`).

### Why flat dictionaries keyed by id (not nested trees)

- **Parts live outside messages** (`part[messageID]`, not `message.parts`). A part event
  writes one row; the message row's identity is untouched, so message-level memos never
  re-run for part churn. Compare Share.tsx (§6), which nests and pays for it.
- **Delta text lives outside parts** (`part_text_accum_delta[partID]` is a single string
  signal). The 60Hz streaming write path touches exactly one store key.
- Arrays are kept **sorted by id** and mutated with `Binary.search` + `splice` inside
  `produce`, or per-row `reconcile(info)` — Solid granularity: an update to row *i* only
  notifies subscribers of row *i*.
- Ids are **time-ordered client-generatable** (`packages/app/src/utils/id.ts`:
  `Identifier.ascending("message")` = prefix + 6-byte hex of `timestamp*4096+counter` +
  base62 — ULID-ish). Sorting by id == sorting by creation time; binary insert is the only
  ordering logic anywhere; and the client can mint a message id the server will echo (§4).
- Eviction is `delete dict[key]` (`session-cache.ts:19–41 dropSessionCaches`) — no tree
  surgery.

### Store nesting: global → per-server → per-directory

```
GlobalProvider (context/global.tsx:13)
 └─ serverCtxs: Map<ServerConnection.Key, createRoot(createServerCtx)>   (global.tsx:38–71)
     createServerCtx (global.tsx:96): { queryClient (tanstack), sdk, sync }
      ├─ sdk  = createServerSdkContext (server-sdk.tsx:379)   — SSE stream + event bus (§2)
      └─ sync = createServerSyncContext (server-sync.tsx:720)
          ├─ globalStore: { path, project[], provider, config } (server-sync.tsx:63–72, 265)
          ├─ session = createServerSession(...)  (server-session.ts:185)   ← ONE per server:
          │     all session-scoped caches (message/part/permission/question/status/
          │     part_text_accum_delta) live here, shared across directories
          ├─ children = createChildStoreManager (global-sync/child-store.ts:23)
          │     children: Record<directoryKey, [Store<State>, SetStoreFunction]> (:38)
          │     per-directory: session list, agents, commands, mcp, lsp, vcs, providers
          └─ ensureDirSyncContext = createRefCountMap(createDirSyncContext) (server-sync.tsx:723)
```

Components consume via `useSync()` (`context/sync.tsx:112`), which is
`serverSync().ensureDirSyncContext(sdk().directory)` — a **refcounted per-directory facade**.

### The DirectorySync facade (`context/directory-sync.ts:24`)

`createDirSyncContext` exposes `data` as a **Proxy over `State`** (:32–38): the session-scoped
fields (`sessionFields` set, :11–22 — `message`, `part`, `part_text_accum_delta`, `permission`,
`question`, `todo`, `session_status`, `session_diff`, `session_message`, `session_working`)
read through to `serverSync.session.data`; everything else reads the per-directory child store.
`set` routes the same way (:39–46). So a component sees one store; physically it's two.

Rationale: session content must survive directory-store eviction and be visible from any
directory (a session can be viewed from Home, moved between worktrees, etc.), while
project-level data (agent list, MCP status) is genuinely per-directory.

### Child store lifecycle & eviction

- `ensureChild` (`child-store.ts:152–298`) lazily creates a store inside `createRoot`; the
  store's `provider/mcp/lsp/path` getters delegate to **tanstack queries** gated by
  activation signals (:191–202) — queries don't fire until the directory is really used.
- LRU bookkeeping: `mark` on every event (`server-sync.tsx:581`), `pin/unpin` around async
  work (boot, session loads), `pinForOwner` ties a pin to the consuming component's Solid
  owner (:83–101).
- `pickDirectoriesToEvict` (`global-sync/eviction.ts:3–19`): evict unpinned dirs idle past
  TTL, plus oldest-first overflow beyond `MAX_DIR_STORES`. `canDisposeDirectory` (:21) refuses
  while pinned/booting/loading.
- **Session-list trim** (`global-sync/session-trim.ts:33–57 trimSessions`): keep the `limit`
  most-recent root sessions + up to 50 roots updated in the last 4h + child sessions whose
  root survives **or which have pending permissions** (never trim a session awaiting a
  prompt). Dropped sessions get their content caches deleted
  (`event-reducer.ts:79–106 cleanupDroppedSessionCaches`).
- **Session content LRU** (`server-session.ts:513–535`): `touch(sessionID)` on every event/
  read; beyond `SESSION_CACHE_LIMIT=40`, evict — but `protectedSessions()` exempts sessions
  with pending permissions/questions, non-idle status, inflight loads, or optimistic sends.

---

## 2. Event stream & reducer

### SSE loop (`packages/app/src/context/server-sdk.tsx`)

One SSE connection per server, started once (`server-sync.tsx:627–642`, deferred a frame +
task so first paint wins).

- **Loop** (:260–317): `while (!aborted && started && generation === active)` → open the
  stream (`v1: eventSdk.global.event`, `v2: eventApi.event.subscribe`) with a per-attempt
  `AbortController` → `for await (const event of events)`. Any throw/close falls out of the
  iterator; if still started, `await wait(RECONNECT_DELAY_MS /* 250ms */)` and reconnect.
  A `generation` counter makes stale loops exit after stop/start races.
- **Cooperative yield** (:289–291): if the iterator has run > `STREAM_YIELD_MS = 8ms`
  without yielding to the event loop, `await wait(0)` — a firehose backlog can't starve
  rendering.
- **pagehide/pageshow** (:325–328): `stop()` on pagehide (abort attempt, bump generation);
  `resumeStreamAfterPageShow` (:162) restarts only when `event.persisted` (bfcache restore).

### Frame-budget batching

Events are not applied as they arrive; they're queued and flushed on a **16ms budget**:

- `enqueueServerEvent` (:68–77): last-writer-wins **at enqueue time** for events where only
  the latest matters — `coalescedKey` (:59) collapses consecutive `lsp.updated` per directory
  and consecutive `message.part.updated` per `(directory, messageID, partID)` by replacing
  the queue tail. Returns whether a flush needs scheduling.
- `schedule`/`flush` (:227–251): single `setTimeout(flush, max(0, 16 - elapsedSinceLastFlush))`;
  flush swaps `queue`/`buffer` arrays (no allocation), runs `coalesceServerEvents`, then
  emits everything inside one Solid `batch()` — one reactive propagation per frame no matter
  how many events arrived.
- `coalesceServerEvents` (:79–139): merges **consecutive delta events with the same key** by
  string-concatenating fragments — both v2 `session.text.delta` / `session.reasoning.delta` /
  `session.tool.input.delta` / `session.compaction.delta` (keyed session+message+ordinal/callID,
  :151–156) and v1 `message.part.delta` (keyed message+part+field, :110–137). Ten 5-char
  deltas in one frame become one 50-char delta hitting the store once.
- **Fan-out**: flush emits on a `createGlobalEmitter` keyed by **directory** (`"global"` for
  server-level events). `server-sync.tsx:526` has the single `listen` that runs the reducers;
  `createDirSdkContext` (:412–423) re-emits per-directory events onto a typed per-event-type
  emitter for ad-hoc subscribers.

### The reducer (`packages/app/src/context/global-sync/event-reducer.ts`)

Two entry points: `applyGlobalEvent` (:36 — `project.updated`, `server.connected`,
`global.disposed` → refresh) and `applyDirectoryEvent` (:108). The same event is *also* fed to
`session.apply` / `session.applyV2` in `server-session.ts`; the directory reducer is called
with `sessionContent: false` (`server-sync.tsx:602`) so the `SESSION_CONTENT_EVENTS` set
(:20–34) is skipped there — **session content is reduced exactly once, in the server-wide
session store**; the directory reducer only maintains the session *list* and project-level
state.

Event vocabulary and effects:

| Event | Effect (store op) |
|---|---|
| `session.created/updated` | binary insert or per-index `reconcile(info)` into `session`; re-`trimSessions`; archived ⇒ splice out + drop caches (:130–171) |
| `session.deleted/archived/moved` | splice by binary search, `cleanupSessionCaches`, adjust `sessionTotal` (:173–253) |
| `session.renamed/usage.updated` | patch row in place (:192–212) |
| `session.diff` | `reconcile(diffs, { key: "file" })` (:255) |
| `todo.updated` | `reconcile(todos, { key: "id" })` (:260) |
| `session.status` | `setStore("session_status", id, reconcile(status))` (:266) |
| `message.updated` | binary insert / `reconcile` per row (:271–290) |
| `message.removed` | splice message, delete `part[messageID]`, delete each part's accum delta (:292–310) |
| `message.part.updated` | **delete `part_text_accum_delta[part.id]`**, then insert/`reconcile` part (:312–337) |
| `message.part.removed` | delete accum delta, splice part (:339–361) |
| `message.part.delta` | see below (:363–387) |
| `permission.asked/replied` | binary insert / splice by requestID (:396–431) |
| `question.asked/replied/rejected` | same shape (:432–468) |
| `vcs.branch.updated` | patch + write persisted vcs cache (:388) |
| `lsp.updated` / `reference.updated` | trigger query refetch callbacks (:469–476) |

`SKIP_PARTS = {"patch","step-start","step-finish"}` (:19) are dropped at ingest — never stored.

### `message.part.delta` — the streaming-text hot path (:363–387)

```ts
const parts = store.part[props.messageID]           // must already have the part
const result = Binary.search(parts, props.partID)   // else drop the delta
const current = parts[result.index]?.[props.field]
// 1) accumulate into the side-table (seed from the part's current field value)
setStore("part_text_accum_delta", props.partID,
  (existing) => (existing ?? (typeof current === "string" ? current : "")) + props.delta)
// 2) ALSO append to the part's own field
setStore("part", props.messageID, produce((draft) => {
  draft[result.index][field] = (draft[result.index][field] ?? "") + props.delta
}))
```

Key contrasts:

- **delta ≠ replace.** `message.part.updated` *replaces* the part (reconcile) and clears the
  accum entry; `message.part.delta` *appends* to both the accum string and the part field.
  The final `part.updated` at stream end is the authoritative full text.
- **Why the duplicate accum table** if the part field is also appended? Two reasons visible
  in `server-session.ts`: (a) a page **refetch racing a live stream** would reconcile the
  part back to the server's older snapshot, losing appended text — `replaceParts`
  (:618–670) uses `deltaBases` (:217, first-delta base text) + the accum string to detect
  `part.text.startsWith(base) && accum.startsWith(part.text) && accum !== part.text` and
  mark such parts "touched" so the fetched copy doesn't clobber the live one; (b) reads of
  streaming text subscribe to **one string key** instead of the part object.
- The richer `server-session.ts:apply` version of every case additionally maintains
  `MessageLoadState` (:86–98) — touched/removed/optimistic/delta marker sets per inflight
  page load — so events observed *during* a fetch win over the fetched page
  (`reconcileFetched` :150–181: "events observed while the request is pending are the
  freshest client state for those identities").

### `readPartText` — joining accum with the part

`packages/session-ui/src/components/message-part-text.ts:1–3`:

```ts
export function readPartText(accum, part) {
  return (accum?.[part.id] ?? part.text ?? "").trim()
}
```

Accum wins while streaming; after the final `part.updated` clears the accum entry, the
part's own `text` is read. One function, called from every text/reasoning renderer.

---

## 3. Stream state in components

### SSE delta → painted character

```
SSE delta ──(enqueue, ≤16ms)──> coalesced delta ──batch──> reducer:
    part_text_accum_delta[partID] += delta            (one string signal)
        │
        ▼
TextPartDisplay (session-ui/src/components/message-part.tsx:1715–1748)
    text = () => readPartText(data.store.part_text_accum_delta, part())
    streaming = createMemo(() => role==="assistant" && time.completed === undefined)
        │
        ▼
<Show when={streaming()} fallback={<Markdown text={text()} streaming={false}/>}>
  <PacedMarkdown text={text()} cacheKey={part().id} streaming/>   (:336–347)
        │
        ▼
createPacedValue (:272–334) — typewriter pacing:
  TEXT_RENDER_PACE_MS = 24         // drip interval
  TEXT_RENDER_IMMEDIATE = 512      // ≤512 chars behind ⇒ show instantly
  step(): adaptive chunk 2/4/8/…/256 chars by backlog size (:256–261)
  next(): extend chunk ≤8 chars to snap at whitespace/punctuation (:263–270)
  non-live, shrink, or non-prefix change ⇒ sync immediately (interruptions, edits)
        │
        ▼
<Markdown text={paced()} cacheKey={partID}/>   — incremental markdown w/ cache key
```

The pacing layer means store updates (bursty, frame-coalesced) are decoupled from visual
updates (smooth 24ms drip) — and the whole thing rides on **one signal per streaming part**.

### What keeps the list cheap

`AssistantParts` (`message-part.tsx:743–…`):

- `grouped` memo (:764–778) flattens messages → renderable parts → `PartGroup[]` (single
  parts + collapsed "context tool" runs, `groupParts` :674–716) with
  **`equals: sameGroups`** (:667–672) — a structural comparator over `{key, refs}` only.
  Part *content* changes (i.e. every delta) do **not** re-emit the grouping memo; only
  add/remove/reorder does. The `<Index>` below it therefore never re-runs rows on deltas.
- Rows resolve their part by id through map memos (:756–762); leaves subscribe to exactly
  their own store paths.
- Everywhere else, `reconcile(..., { key })` on fetch and per-row `reconcile` on events keep
  object identity stable so `===`-based memos hold.

### `session_status` → UI

Status is a tiny state machine per session: `idle | busy | retry{attempt, message, next}`.

- Sources: `session.status` events (`event-reducer.ts:266`, `server-session.ts:1023`); v2
  events map `session.execution.started → busy`, `succeeded/failed/interrupted → idle`,
  `session.retry.scheduled → retry` (`server-session.ts:961–974`). Initial seeding from a
  `session.active()` snapshot at boot (`server-sync.tsx:168–177 seedActiveSessionStatuses`).
  **Optimistic**: the send path sets `busy` locally before the server confirms (§4).
- `session_working(id)` = `status?.type !== "idle"` (`child-store.ts:232`,
  `server-session.ts:204`).
- Consumers:
  - Composer: `live = session_working(id) || blocked()`
    (`pages/session/composer/session-composer-state.ts:62`) drives the todo dock open/close
    machine and gates behavior; `working()` also flips submit-with-empty-input into abort
    (`submit.ts:324–327`).
  - Timeline: `sessionStatus` memo (`pages/session/timeline/message-timeline.tsx:280–284`)
    feeds `createTimelineProjection` which appends a synthetic "Thinking" row (TextShimmer,
    :131, :1205–1213) while busy.
  - Retry card: `session-ui/src/components/session-retry.tsx` renders when
    `status.type === "retry"` — message + attempt + countdown driven by a per-second signal
    off `status.next`.

---

## 4. Optimistic writes & send path

### Ordered insert + reconcile (`context/sync.tsx`, `context/server-session.ts`)

`sync.tsx:62–110` is the distilled (and unit-tested) model; `server-session.ts` hosts the
production version:

- `applyOptimisticAdd` (:92) — binary-insert the synthetic user `Message` into
  `message[sessionID]`, set `part[messageID] = sortParts(parts)`. Works because the client
  mints the message id with `Identifier.ascending("message")` — time-ordered, so the binary
  insert lands it at the tail in correct order, and **the server reuses the same id**, so
  later server events target the same row instead of duplicating it.
- `mergeOptimisticPage` (:62 / server-session.ts:105) — when a fetched page arrives, merge
  optimistic items into it: message already in page **and** all parts present ⇒ `confirmed`
  (drop the optimistic item); otherwise merge missing parts into the page so the refetch
  can't erase the pending send.
- Event-driven confirmation (`server-session.ts`): `message.updated` for the id marks
  `confirmedMessage` (:1034–1039); each `message.part.updated` confirms that part
  (`confirmOptimisticPart` :373); when no unconfirmed parts remain the optimistic record is
  dropped. `optimistic.remove` (:1353) is failure rollback: if unconfirmed, splice the whole
  message; if partially confirmed, remove only the still-optimistic parts.

### The submit flow (`packages/app/src/components/prompt-input/submit.ts`)

`createPromptSubmit` (:225) / `handleSubmit` (:309):

1. **Capture & clear-with-restore** — `createPromptSubmissionState`
   (`submission-state.ts:5–33`) snapshots the prompt + context, clears the editor
   immediately, and can `restore()` on failure (only if the user hasn't typed since) or
   `retarget()` the draft onto a newly-created session's editor.
2. Empty input while `working()` ⇒ `abort()` (:250–269): clears todos locally, aborts a
   queued pending send (worktree wait) or calls `session.interrupt`.
3. **New session**: `api.session.create` → `seed` into stores (:294–307) → navigate inside
   `startTransition` (:411).
4. **Queue-while-busy**: `shouldQueue()` (wired in `pages/session.tsx:1753` —
   `settings.followup === "queue" && busy(id) && !composer.blocked() && !isChildSession()`)
   ⇒ `onQueue(draft)` pushes a `FollowupDraft` into a followup store
   (`session.tsx:1776–1783`); a mutation drains it via `sendFollowupDraft` when the session
   goes idle. The draft is a *full capture* (prompt parts, context, agent, model) — the
   composer is free immediately.
5. Otherwise `sendFollowupDraft` (:57–199):

```ts
const messageID = Identifier.ascending("message")
const { requestParts, optimisticParts } = buildRequestParts({...})   // §below
batch(() => { setBusy(); add() })          // session_status = busy + optimistic message+parts
try {
  await api.session.prompt({ sessionID, id: messageID, ...requestParts })
} catch { batch(() => { setIdle(); remove() }); throw }              // rollback + toast + restore input
```

`optimisticBusy` is set only when the session's directory == the composer's directory
(:614) — cross-worktree sends don't fake status for a store the user isn't watching.

### `buildRequestParts` (`prompt-input/build-request-parts.ts:91–213`)

Builds **two parallel lists from one pass**: `requestParts` (wire `TextPartInput` /
`FilePartInput` / `AgentPartInput`, each with its own client-minted `Identifier.ascending("part")`
id) and `optimisticParts = requestParts.map(toOptimisticPart)` (:55–89) — the same ids
expanded into full `Part` shapes with `sessionID`/`messageID` stamped on. Client-minted part
ids are what make per-part confirmation possible: the server's `message.part.updated` events
carry the ids the client already rendered.

---

## 5. Permission / question state

Brief — component layer covered elsewhere; this is the state shape + control flow.

### State shape

- Pending requests: `permission: Record<sessionID, PermissionRequest[]>` and
  `question: Record<sessionID, QuestionRequest[]>` in the server-session store, arrays
  ordered by id. `*.asked` binary-inserts (or reconciles a re-ask); `*.replied` /
  `question.rejected` splice by `requestID` (`server-session.ts:1231–1290`). **No local
  removal on user action** — the store only changes when the server's `replied` event lands;
  the UI meanwhile shows a `responding` guard.
- Auto-accept config: `createServerPermissionState` (`context/permission.tsx:190`) holds a
  **persisted** `autoAccept: Record<key, boolean>` where key is
  `base64(dir)/sessionID`, bare `sessionID` (legacy), or `base64(dir)/*` (whole directory)
  (`permission-auto-respond.ts:3–10`). One state per server, created in `createRoot` keyed by
  scope (`permission.tsx:66–100`), disposed when the server disappears.

### Control flow

- **Auto-respond**: raw event listener (`permission.tsx:334–349`) on `permission.asked` →
  `respondPending` → `shouldAutoRespondResolved` (:307) — resolves the session's lineage
  (fetching unknown ancestors) then walks child→parent
  (`permission-auto-respond.ts:22–38 sessionLineage`); the first session in the lineage with
  an explicit autoAccept value wins, else the directory wildcard. Replies go through a
  `responded` LRU (max 1000, 1h TTL, :228–244) so replays/races can't double-reply; a failed
  reply deletes the marker so it can retry. Enabling auto-accept also **replays** the current
  pending list from the server (:396–410, guarded by an `enableVersion` counter against
  toggle races).
- **Composer takeover** (`pages/session/composer/session-composer-state.ts`):
  - `permissionRequest()` / `questionRequest()` memos (:36–44) call
    `sessionPermissionRequest` (`session-request-tree.ts:3–34`) — **BFS from the current
    session down through its child-session tree** (parentID edges inverted into a
    children-map), returning the first pending request in the subtree. This is how a
    subagent's permission prompt surfaces in the parent's composer. Requests that
    `permission.autoResponds(...)` are filtered out (they're about to self-resolve).
  - `blocked() = !!permissionRequest() || !!questionRequest()` (:46–50). `blocked()` swaps
    the composer for the permission/question dock, and feeds `live()` (:62) alongside
    `session_working`.
  - `decide(response)` (:78–93): guard `responding === perm.id`, call
    `api.permission.reply`, toast on error, clear guard — removal itself arrives as the
    `permission.replied` event.

---

## 6. packages/web — the share page (`packages/web/src/components/Share.tsx`)

The legacy standalone consumer: a static Astro/Solid page fed by a raw WebSocket, no store
machinery. ~150 lines of state code.

- **Transport** (:95–174): `new WebSocket(api/share_poll?id=...)`; `onclose` →
  `setTimeout(setupWebSocket, 2000)` (fixed 2s, no backoff, no jitter); status is a
  `createSignal<[Status, string?]>` rendered as a dot + text.
- **State** (:60–78): `createStore({ info, messages: Record<messageID, MessageWithParts> })`
  — parts **nested inside** message rows. The server sends full key/value snapshots
  (`session/info`, `session/message/...`, `session/part/...`), so a dropped frame is healed
  by the next write to that key — reconnect needs no gap-replay protocol.
- **Merging** (:119–147): `info` → `reconcile`; `message` → `reconcile` per row (carrying
  forward existing `parts`); `part` → find-by-id then **copy the whole array**
  (`[...arr]`).
- **Rendering**: `messages` memo re-sorts `Object.values(...)` on every change (:78); a
  `data` memo re-walks all messages to sum cost/tokens (:253–296); pending/running tools are
  filtered out rather than streamed (:359–360); no deltas, no pacing, no batching, no
  eviction, no optimistic writes.

**As a minimal reference**: what it gets *right* — store + `reconcile` + memos is genuinely
enough for a read-only transcript; full-snapshot-per-key messages make reconnect trivial;
last-write-wins per key needs no reducer. What it gets *wrong* for anything interactive —
nested parts mean every part frame rebuilds a message row, re-sorts all messages, and
re-walks the stats memo (O(n) per frame); whole-array copies defeat fine-grained reactivity;
no frame batching, so a fast producer stutters the page; hiding running tools instead of
streaming them. It's the architecture our current one-shot block would naturally grow into
if we just "add a socket" — which is the argument for not doing that.

---

## 7. Synthesis for the Macro agent block

Three architectures now on the table:

| | opencode | Macro channel (CHANNEL_BLOCK_NOTES) | block-agent today |
|---|---|---|---|
| Source of truth | SSE events, server-shaped Message/Part | tanstack infinite query + WS cache surgery | one-shot query (`data/queries.ts:23`) |
| Streaming text | `message.part.delta` → accum side-table + append | fold machine (WASM worker) emits FoldedMessage rows | none — frozen after load |
| Ordering | client/server-shared time-ordered ids, binary insert | server rows + placeholder adoption (nonce) | fold output order |
| Batching | 16ms frame coalescing before one `batch()` | per-frame WS handling, query-cache writes | n/a |
| Status | `session_status` dict + optimistic busy | typing indicators etc. | none |
| Optimistic send | client-minted ids, add→confirm-by-event→rollback | nonce + `consumeNonce` + placeholder adoption | none |

The structural difference that changes the calculus: **opencode receives ready-made parts;
we fold client-side.** Their reducer's hardest code (`MessageLoadState`, `deltaBases`,
`mergeOptimisticPage` in `server-session.ts`) exists to merge *paged HTTP snapshots* against
*concurrent events* over mutable server objects. Our log is append-only with offsets as ids,
and the snapshot/live seam is already solved once, in `agent-session-stream.ts`
(`dropOverlap`) — frames after the fetched prefix simply continue the same fold machine. We
should **not** import their page-vs-event race machinery; we don't have the race.

### `createAgentSessionFeed` — concrete recommendations

1. **Ordered store + reconcile, not array-of-query-data.** Replace the one-shot
   `useAgentSessionBlockQuery` result array with a `createStore` of folded rows keyed by a
   stable row key (turn id / message id — the existing
   `bySessionId[sessionId][turn][authorKind]` shape in `folded-messages.ts` is already
   opencode-flat; reuse it). Apply fold outputs with per-row `reconcile` so unchanged rows
   keep identity. Add a `sameGroups`-style structural comparator on the *row-key list* memo
   (`message-part.tsx:667`) so the transcript `<Index>`/virtualizer only re-runs on
   add/remove, never on content churn.
2. **Frame-budget coalescing — yes, on both sides of the worker.** Before the machine:
   coalesce consecutive raw delta frames for the same target (opencode's
   `coalesceServerEvents` key trick, `server-sdk.tsx:79`) so we cross the worker boundary
   ≤ once per 16ms per stream, not per token. After the machine: apply a flush's worth of
   folded updates inside one Solid `batch()` on a `setTimeout(…, max(0, 16 - elapsed))`
   schedule (`server-sdk.tsx:227–251` is a 25-line pattern worth copying verbatim). Include
   the queue/buffer swap and the "flush on dispose" detail.
3. **Accum-delta buffer separate from folded rows — yes, and we get to design the emission.**
   opencode needs `part_text_accum_delta` partly for fetch races we don't have, but the other
   half of its value stands: streaming text as **one string signal per streaming segment**,
   read at the leaf via a `readPartText`-style join
   (`accum[id] ?? row.text`), with the accum entry deleted when the fold emits the segment's
   final form. Since our fold machine is ours, prefer having it emit
   `{ rowKey, segmentId, deltaText }` during streaming and a full row on completion — i.e.
   make the WASM machine speak opencode's `part.delta` / `part.updated` distinction — rather
   than re-emitting whole `FoldedMessage` rows per token (which would force whole-row
   reconciles down the hot path). Then pace at the leaf with `createPacedValue`
   (`message-part.tsx:272` — self-contained, 60 lines, copy it).
4. **Status controller.** A flat `session_status: Record<sessionId, Status>` with
   `idle | working | retry` derived from protocol/status frames, plus a `session_working()`
   helper, is the whole model. Copy the **optimistic busy** trick from `submit.ts:60–68`
   (set `working` on send, revert on send failure) — our status is derived from the log, so
   without it the shimmer lags the send by a round-trip. Consumers: TextShimmer row while
   working (we already have `ui/TextShimmer.tsx`), retry card off `status.retry`, composer
   `live = working || blocked`.
5. **Send path: optimistic user turn + reconcile against the fold.** Mint a client id, append
   an optimistic folded user row to the store, `batch` with the busy flip, POST; when the
   log echoes the user turn, the fold emits the authoritative row — reconcile it over the
   optimistic one by id (Macro already has the exact analog:
   `adoptAgentSessionPlaceholder` re-keys placeholder rows; extend that seam rather than
   building opencode's confirm-parts bookkeeping — we send text, not multi-part uploads, so
   whole-message confirmation suffices). On failure: remove the row + restore the composer
   draft — copy `createPromptSubmissionState` (`submission-state.ts`, 33 lines) for
   clear-now/restore-on-failure. If `working()`, queue a `FollowupDraft`-style capture
   instead of blocking the composer (`session.tsx:1753–1783`).
6. **Permission prompts.** `permission: Record<sessionId, Request[]>` — insert on asked,
   remove only on the replied event, `responding` id guard in the controller
   (`session-composer-state.ts:78–93`), `blocked()` memo swapping composer for the dock. Skip
   the lineage/BFS machinery (`session-request-tree.ts`) until turns can spawn sub-sessions;
   when they can, that file is the 30-line template. Auto-accept, if we add it, is a
   persisted `Record<key, boolean>` + replay-pending-on-enable + responded-LRU
   (`permission.tsx:228–283`) — nothing deeper.
7. **Eviction posture.** For a single-session block we don't need dir-store LRU, but the
   *protected-set* idea transfers: never evict/reset a session's feed state while it has
   pending permissions or non-idle status (`server-session.ts:513–530`) — relevant once the
   gallery/split views hold several feeds via the refcounted machine sharing in
   `agent-session-stream.ts`.

**One-line summary**: keep Macro's fold pipeline as the reducer (it's our server-side-parts
equivalent), and take from opencode the four cheap high-leverage pieces — 16ms coalesce+batch
flush, accum-delta string keys + leaf pacing, structural-equality row memos, and
client-minted-id optimistic turns with status flipped optimistically.
