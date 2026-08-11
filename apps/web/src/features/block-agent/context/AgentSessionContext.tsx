/**
 * Block-level state for the agent block, following the chat block's
 * `ChatInputProvider`/`useChatInputContext` convention: the provider owns the
 * session data and controllers, composer/container components consume them
 * from context, and the `ui/` leaves stay dumb — they only ever receive
 * derived props.
 *
 * The value is assembled from `create*` factories (CHANNEL_BLOCK_NOTES.md §3)
 * so each stateful concern stays a composable unit as wiring grows.
 */

import type { FoldedMessage } from '@service-agent-fold/generated/types';
import type { AgentSessionResponse } from '@service-agent-harness/generated/schemas';
import {
  type Accessor,
  createContext,
  type ParentProps,
  useContext,
} from 'solid-js';
import { createAgentSessionFeed } from './create-agent-session-feed';
import {
  type ComposerController,
  createComposerController,
} from './create-composer-controller';
import {
  createSessionStatusController,
  type SessionStatus,
} from './create-session-status-controller';

export type AgentSessionState = {
  sessionId: Accessor<string>;
  /** Session metadata, absent until the load resolves. */
  session: Accessor<AgentSessionResponse | undefined>;
  /** The folded transcript, ordered by turn, live-following the session. */
  messages: Accessor<FoldedMessage[]>;
  loadFailed: Accessor<boolean>;
  /** The agent is working on the newest turn. */
  working: Accessor<boolean>;
  /** The runtime's status: the GET snapshot, followed live over the log. */
  status: Accessor<SessionStatus>;
  composer: ComposerController;
};

const AgentSessionCtx = createContext<AgentSessionState>();

export function AgentSessionProvider(
  props: ParentProps & { sessionId: string }
) {
  const sessionId = () => props.sessionId;

  const feed = createAgentSessionFeed(sessionId);
  const composer = createComposerController({
    sessionId,
    working: feed.working,
  });
  const status = createSessionStatusController({
    sessionId,
    seed: () => feed.session()?.status,
  });

  return (
    <AgentSessionCtx.Provider
      value={{
        sessionId,
        session: feed.session,
        messages: feed.messages,
        loadFailed: feed.loadFailed,
        working: feed.working,
        status: status.status,
        composer,
      }}
    >
      {props.children}
    </AgentSessionCtx.Provider>
  );
}

export function useAgentSession(): AgentSessionState {
  const ctx = useContext(AgentSessionCtx);
  if (!ctx) {
    throw new Error(
      'useAgentSession must be used within <AgentSessionProvider />'
    );
  }
  return ctx;
}
