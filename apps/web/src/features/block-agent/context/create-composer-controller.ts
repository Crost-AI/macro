/**
 * The Solid shell around the session state machine
 * (`state/session-state.ts`), in the chat block's controller idiom
 * (`@core/component/AI/state/createChatController.ts`): signals hold the
 * machine's state, `dispatch` runs the pure transition and executes the
 * returned effects, and the impure edges — the fold's `working` flips —
 * are watched here and fed in as events.
 */

import { toast } from '@core/component/Toast/Toast';
import { agentHarnessServiceClient } from '@service-agent-harness/client';
import { type Accessor, createEffect, on, untrack } from 'solid-js';
import { createStore, reconcile } from 'solid-js/store';
import { match } from 'ts-pattern';
import {
  initialSessionState,
  type QueuedPrompt,
  type SessionEffect,
  type SessionEvent,
  type SessionPhase,
  type SessionState,
  transition,
} from '../state/session-state';

export type ComposerController = {
  phase: Accessor<SessionPhase>;
  /** Prompts waiting for the running turn to settle. */
  queue: Accessor<QueuedPrompt[]>;
  /** A turn is running or a post is in flight — the stop affordance shows. */
  busy: Accessor<boolean>;
  send: (markdown: string) => void;
  stop: () => void;
};

export function createComposerController(options: {
  sessionId: Accessor<string>;
  /** The feed's turn-in-flight signal; edges become machine events. */
  working: Accessor<boolean>;
}): ComposerController {
  const [state, setState] = createStore<SessionState>(initialSessionState);

  /**
   * Post prompts to the harness in order, one control call each — a queue
   * flush is sequential so the agent reads them in the order they were
   * queued. The first failure re-queues the prompts that never made it (the
   * machine decides what that means for the phase); a full run reports one
   * success. Frames stream back through the connection gateway into the
   * session fold — the response body carries nothing.
   */
  const postPrompts = async (prompts: QueuedPrompt[]) => {
    for (let index = 0; index < prompts.length; index++) {
      const prompt = prompts[index]!;
      const result = await agentHarnessServiceClient
        .control(options.sessionId(), {
          type: 'prompt',
          prompt: prompt.markdown,
        })
        .catch(() => undefined);
      if (result === undefined || result.isErr()) {
        dispatch({ type: 'post_failed', prompts: prompts.slice(index) });
        return;
      }
    }
    dispatch({ type: 'post_succeeded' });
  };

  const postStop = async () => {
    const result = await agentHarnessServiceClient
      .control(options.sessionId(), { type: 'stop' })
      .catch(() => undefined);
    if (result === undefined || result.isErr()) {
      toast.failure('The agent could not be stopped');
    }
    // Success is observed through the fold: the turn settles and the
    // `working` edge dispatches `turn_settled`.
  };

  const executeEffects = (effects: SessionEffect[]) => {
    for (const effect of effects) {
      match(effect)
        .with({ type: 'post_prompts' }, (e) => {
          void postPrompts(e.prompts);
        })
        .with({ type: 'post_stop' }, () => {
          void postStop();
        })
        .with({ type: 'toast' }, (e) => {
          toast.failure(e.message);
        })
        .exhaustive();
    }
  };

  const dispatch = (event: SessionEvent) => {
    const result = transition(
      untrack(() => ({ ...state })),
      event
    );
    setState(reconcile(result.state));
    executeEffects(result.effects);
  };

  // The fold is the authority on turns: its `working` edges are events, which
  // also captures turns started by other collaborators.
  createEffect(
    on(
      options.working,
      (working, wasWorking) => {
        if (working === wasWorking) return;
        dispatch({ type: working ? 'turn_started' : 'turn_settled' });
      },
      { defer: true }
    )
  );

  return {
    phase: () => state.phase,
    queue: () => state.queue,
    busy: () => state.phase.type !== 'idle',
    send: (markdown) => {
      dispatch({
        type: 'send_requested',
        prompt: { id: crypto.randomUUID(), markdown },
      });
    },
    stop: () => {
      dispatch({ type: 'stop_requested' });
    },
  };
}
