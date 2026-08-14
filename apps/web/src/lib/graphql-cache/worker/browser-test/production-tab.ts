import type { CacheRequest, CacheResponse } from '../../protocol';
import {
  createCacheCoordinatorPageAdapter,
  type DedicatedWorkerLike,
} from '../coordinator-page-adapter';
import type {
  ProductionHarnessCommand,
  ProductionHarnessEnvelope,
} from './production-browser-wire';

const parameters = new URLSearchParams(location.search);
const tabId = parameters.get('tabId') ?? '';
const scope = parameters.get('scope') ?? '';
if (!tabId || !scope) throw new Error('missing production harness parameters');

const QUERY = `query Soup($input: SoupInput!) {
  user {
    id
    soup(input: $input) {
      nextCursor
      items {
        __typename
        id
      }
    }
  }
}`;
const SLOW_QUERY = QUERY.replace('query Soup', 'query Slow');
const VARIABLES = { input: { limit: 1 } };

const channel = new BroadcastChannel(
  `graphql-cache-wp08-production-tabs:${scope}`
);
const report = (
  event: Extract<ProductionHarnessEnvelope, { source: 'tab' }>['event']
): void => {
  channel.postMessage({
    source: 'tab',
    tabId,
    event,
  } satisfies ProductionHarnessEnvelope);
};

let nextRequestId = 1;
let currentWorker: DedicatedWorkerLike | undefined;
const pending = new Map<number, (response: CacheResponse) => void>();

const adapter = createCacheCoordinatorPageAdapter({
  scope,
  tabId,
  createDedicatedWorker: (_workerScope, ownerEpoch) =>
    new Worker(
      new URL('./production-cache.engine-worker.ts', import.meta.url),
      {
        type: 'module',
        name: `graphql-cache-wp08-production:${scope}:${ownerEpoch}`,
      }
    ),
  onWorkerCreated: (worker, ownerEpoch) => {
    currentWorker = worker;
    report({ kind: 'worker-created', ownerEpoch });
  },
  onWorkerTerminated: (ownerEpoch, reason) => {
    currentWorker = undefined;
    report({ kind: 'worker-terminated', ownerEpoch, reason });
  },
  onProtocolError: (error) => {
    report({ kind: 'protocol-error', error: error.message });
  },
});

adapter.onmessage = (event) => {
  const message = event.data;
  if ('kind' in message) return;
  const resolve = pending.get(message.id);
  if (!resolve) return;
  pending.delete(message.id);
  resolve(message);
};

type CacheRequestWithoutId = CacheRequest extends infer Request
  ? Request extends CacheRequest
    ? Omit<Request, 'id'>
    : never
  : never;

const sendRequest = async (
  commandId: string,
  request: CacheRequestWithoutId
): Promise<void> => {
  const id = nextRequestId++;
  const response = new Promise<CacheResponse>((resolve) => {
    pending.set(id, resolve);
  });
  adapter.postMessage({ ...request, id } as CacheRequest);
  const result = await response;
  if (result.ok) {
    report({
      kind: 'command-result',
      commandId,
      ok: true,
      result: result.result,
    });
  } else {
    report({
      kind: 'command-result',
      commandId,
      ok: false,
      error: result.error,
    });
  }
};

const handleCommand = (command: ProductionHarnessCommand): void => {
  switch (command.kind) {
    case 'write':
      void sendRequest(command.commandId, {
        kind: 'write',
        query: QUERY,
        operationName: 'Soup',
        variables: VARIABLES,
        data: {
          user: {
            id: 'production-user',
            soup: {
              nextCursor: null,
              items: [
                {
                  __typename: 'GraphqlSoupDocument',
                  id: command.value,
                },
              ],
            },
          },
        },
        identity: 'production-test-user',
      });
      break;
    case 'read':
      void sendRequest(command.commandId, {
        kind: 'read',
        query: QUERY,
        operationName: 'Soup',
        variables: VARIABLES,
      });
      break;
    case 'slow-read':
      void sendRequest(command.commandId, {
        kind: 'read',
        query: SLOW_QUERY,
        operationName: 'Slow',
        variables: VARIABLES,
      });
      break;
    case 'graceful-close':
      void adapter.dispose({ graceful: true }).then(() => {
        report({
          kind: 'command-result',
          commandId: command.commandId,
          ok: true,
        });
        setTimeout(() => window.close());
      });
      break;
    case 'crash-worker':
      if (!currentWorker) {
        report({
          kind: 'command-result',
          commandId: command.commandId,
          ok: false,
          error: 'tab has no production worker',
        });
        return;
      }
      currentWorker.postMessage({ testKind: 'crash' }, []);
      report({
        kind: 'command-result',
        commandId: command.commandId,
        ok: true,
      });
      break;
  }
};

channel.onmessage = (event: MessageEvent<ProductionHarnessEnvelope>) => {
  const message = event.data;
  if (message.source === 'harness' && message.targetTabId === tabId) {
    handleCommand(message.command);
  }
};

adapter.postMessage({ id: nextRequestId++, kind: 'init', scope });
void adapter.start().then(() => report({ kind: 'registered' }));
