/**
 * The agent session's chat state, as a pure ts-pattern state machine in the
 * chat block's idiom (`@core/component/AI/state/chatState.ts`): a phase
 * union, an event union, and a `transition` returning the next state plus
 * side effects as data. The Solid shell that dispatches events and executes
 * effects is `context/create-composer-controller.ts`.
 *
 * Queueing model (collaborative — the queue must eventually live on the
 * backend; see `post_prompts` in `Effect`): any number of prompts may be
 * queued while a turn runs, and the whole queue flushes to the agent as one
 * batch when the turn settles. "Running with a queue" is therefore not a
 * phase — the queue is orthogonal data, and composites derive from
 * `phase × queue` (opencode's lesson, OPENCODE_STATE_NOTES.md §3).
 *
 * The machine deliberately has no `messages` — the fold owns the transcript.
 * `working` transitions arrive as events derived from the feed, which also
 * covers turns started by other collaborators: an idle session can enter
 * `working` without us ever sending.
 */

import { match, P } from 'ts-pattern';

// --- Phases ---

export type SessionPhase =
  /** No turn in flight, nothing queued. */
  | { type: 'idle' }
  /** Prompts are posted (or flushing) but the turn hasn't been observed yet. */
  | { type: 'sending' }
  /** The agent is mid-turn (observed from the fold). */
  | { type: 'working' };

/** A prompt waiting for the current turn to settle. */
export type QueuedPrompt = {
  /** Client-minted, so a queued row can be adopted/removed stably. */
  id: string;
  markdown: string;
};

export type SessionState = {
  phase: SessionPhase;
  queue: QueuedPrompt[];
};

export const initialSessionState: SessionState = {
  phase: { type: 'idle' },
  queue: [],
};

// --- Events ---

export type SessionEvent =
  /** The user submitted the composer. */
  | { type: 'send_requested'; prompt: QueuedPrompt }
  /** The prompt POST (or queue flush) was accepted by the backend. */
  | { type: 'post_succeeded' }
  /** The prompt POST (or queue flush) failed. */
  | { type: 'post_failed'; prompts: QueuedPrompt[] }
  /** The fold reports a turn opening (ours or another collaborator's). */
  | { type: 'turn_started' }
  /** The fold reports the open turn settled (stop reason arrived). */
  | { type: 'turn_settled' }
  /** The user asked to stop the running turn. */
  | { type: 'stop_requested' };

// --- Effects (data, executed by the controller) ---

export type SessionEffect =
  /** Send prompts to the agent — one send or a whole queue flush. */
  | { type: 'post_prompts'; prompts: QueuedPrompt[] }
  /** Ask the backend to interrupt the running turn. */
  | { type: 'post_stop' }
  | { type: 'toast'; message: string };

export type TransitionResult = {
  state: SessionState;
  effects: SessionEffect[];
};

const none = (state: SessionState): TransitionResult => ({
  state,
  effects: [],
});

const rejected = (state: SessionState, event: string): TransitionResult => {
  console.warn(`session transition: ${event} from ${state.phase.type}`);
  return none(state);
};

export function transition(
  state: SessionState,
  event: SessionEvent
): TransitionResult {
  return (
    match([state.phase, event] as const)
      .with([{ type: 'idle' }, { type: 'send_requested' }], ([, e]) => ({
        state: { phase: { type: 'sending' as const }, queue: state.queue },
        effects: [
          { type: 'post_prompts' as const, prompts: [e.prompt] },
        ] as SessionEffect[],
      }))

      // A send while a turn is in flight (or one is already being posted)
      // queues; the flush happens when the turn settles.
      .with(
        [{ type: P.union('sending', 'working') }, { type: 'send_requested' }],
        ([, e]) =>
          none({ phase: state.phase, queue: [...state.queue, e.prompt] })
      )

      .with(
        [{ type: 'sending' }, { type: 'post_succeeded' }],
        // Stay in `sending` — `working` is the fold's call, not the POST's.
        () => none(state)
      )

      .with([{ type: 'sending' }, { type: 'post_failed' }], ([, e]) => ({
        // The failed prompts return to the front of the queue rather than
        // vanishing; the user decides whether to retry (send again) or edit.
        state: {
          phase: { type: 'idle' as const },
          queue: [...e.prompts, ...state.queue],
        },
        effects: [
          { type: 'toast' as const, message: 'Message could not be sent' },
        ] as SessionEffect[],
      }))

      // A flush posts prompts one at a time, and the turn can open while the
      // rest are still posting — a late failure lands in `working`. The
      // failed prompts re-queue for the next settle; the turn keeps running.
      .with([{ type: 'working' }, { type: 'post_failed' }], ([, e]) => ({
        state: {
          phase: state.phase,
          queue: [...e.prompts, ...state.queue],
        },
        effects: [
          { type: 'toast' as const, message: 'Message could not be sent' },
        ] as SessionEffect[],
      }))
      .with([{ type: 'working' }, { type: 'post_succeeded' }], () =>
        none(state)
      )

      // The fold saw the turn open. From `idle` this is a collaborator's (or a
      // pre-existing) turn — equally valid.
      .with(
        [{ type: P.union('idle', 'sending') }, { type: 'turn_started' }],
        () => none({ phase: { type: 'working' as const }, queue: state.queue })
      )

      .with([{ type: 'working' }, { type: 'turn_settled' }], () => {
        if (state.queue.length === 0) {
          return none({ phase: { type: 'idle' as const }, queue: [] });
        }
        // Flush-all: the entire queue goes to the agent as one batch.
        return {
          state: { phase: { type: 'sending' as const }, queue: [] },
          effects: [
            { type: 'post_prompts' as const, prompts: state.queue },
          ] as SessionEffect[],
        };
      })

      .with(
        [{ type: P.union('sending', 'working') }, { type: 'stop_requested' }],
        () => ({
          // Stay put until the fold confirms via `turn_settled` — stop is a
          // request, not a state change. From `sending` the harness decides
          // what stopping a not-yet-open turn means.
          state,
          effects: [{ type: 'post_stop' as const }] as SessionEffect[],
        })
      )

      // Harmless races the fold can produce while catching up.
      .with([{ type: 'working' }, { type: 'turn_started' }], () => none(state))
      .with(
        [{ type: P.union('idle', 'sending') }, { type: 'turn_settled' }],
        () => none(state)
      )

      .otherwise(([, e]) => rejected(state, e.type))
  );
}
