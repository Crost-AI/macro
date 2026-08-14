/// <reference lib="webworker" />

import type { CacheRequest } from '../../protocol';
import {
  type CacheEngineRuntimeEvent,
  installCacheEngineWorker,
} from '../cache-engine-runtime';

let telemetry: BroadcastChannel | undefined;
const report = (event: CacheEngineRuntimeEvent): void => {
  telemetry ??= new BroadcastChannel(
    `graphql-cache-wp08-production:${event.activation.scope}`
  );
  telemetry.postMessage({
    kind: event.kind,
    tabId: event.activation.tabId,
    ownerEpoch: event.activation.ownerEpoch,
    databaseAction: event.activation.databaseAction,
    requestId: event.kind === 'request-admitted' ? event.request.id : undefined,
    requestKind:
      event.kind === 'request-admitted' ? event.request.kind : undefined,
    slow:
      event.kind === 'request-admitted' &&
      event.request.kind === 'read' &&
      event.request.query.includes('Slow'),
    reason: event.kind === 'fatal' ? event.reason : undefined,
  });
};

const blockInjectedSlowRead = async (request: CacheRequest): Promise<void> => {
  if (request.kind === 'read' && request.query.includes('Slow')) {
    await new Promise<void>(() => {});
  }
};

installCacheEngineWorker({
  hooks: {
    beforeRequest: blockInjectedSlowRead,
    onEvent: report,
  },
});

const runtimeOnMessage = self.onmessage;
self.onmessage = (event: MessageEvent<unknown>) => {
  if (
    typeof event.data === 'object' &&
    event.data !== null &&
    'testKind' in event.data &&
    event.data.testKind === 'crash'
  ) {
    setTimeout(() => {
      throw new Error('production harness induced worker crash');
    });
    return;
  }
  runtimeOnMessage?.call(self, event);
};
