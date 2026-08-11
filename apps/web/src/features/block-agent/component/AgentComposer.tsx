/**
 * The block's composer container: reads the composer controller from the
 * session context and drives the dumb `AgentInput` with derived props. All
 * block-level state stays on this side of the boundary.
 */

import { useAgentSession } from '../context/AgentSessionContext';
import { AgentInput } from '../ui';

export function AgentComposer() {
  const { composer, loadFailed } = useAgentSession();

  return (
    <AgentInput
      placeholder="Message the agent"
      busy={composer.busy()}
      disabled={loadFailed()}
      onSend={composer.send}
      onStop={composer.stop}
    />
  );
}
