/// <reference lib="webworker" />

import type { CacheRequest, CacheResponse } from '../protocol';
import {
  CACHE_COORDINATOR_PROTOCOL_VERSION,
  type CoordinatorToEngineEnvelope,
  type EngineToCoordinatorEnvelope,
  isCachePush,
  isCacheResponse,
  type PageToEngineEnvelope,
  validateCoordinatorToEngineEnvelope,
  validatePageToEngineEnvelope,
} from './coordinator-protocol';
import { CacheWorkerCore, type CacheWorkerCoreOptions } from './worker-core';

export type CacheEngineRuntimeEvent =
  | { kind: 'activation-started'; activation: PageToEngineEnvelope }
  | {
      kind: 'request-admitted';
      activation: PageToEngineEnvelope;
      request: CacheRequest;
    }
  | { kind: 'ready'; activation: PageToEngineEnvelope }
  | { kind: 'drained'; activation: PageToEngineEnvelope }
  | { kind: 'fatal'; activation: PageToEngineEnvelope; reason: string };

export interface CacheEngineRuntimeHooks {
  beforeRequest?: (
    request: CacheRequest,
    activation: PageToEngineEnvelope
  ) => void | Promise<void>;
  onEvent?: (event: CacheEngineRuntimeEvent) => void;
}

interface CacheWorkerCoreLike {
  addPort(port: { postMessage(message: unknown): void }): void;
  handleRequest(
    port: { postMessage(message: unknown): void },
    request: CacheRequest
  ): Promise<void>;
  drain(): Promise<void>;
}

interface DedicatedWorkerScopeLike {
  onmessage: ((event: MessageEvent<unknown>) => void) | null;
  close(): void;
}

export interface CacheEngineRuntimeOptions {
  scope?: DedicatedWorkerScopeLike;
  hooks?: CacheEngineRuntimeHooks;
  createCore?: (options: CacheWorkerCoreOptions) => CacheWorkerCoreLike;
  ownerLockIsHeld?: (ownerLockName: string) => Promise<boolean>;
}

const withVersion = <T extends { coordinatorVersion: 1 }>(
  value: T extends unknown ? Omit<T, 'coordinatorVersion'> : never
): T =>
  ({
    coordinatorVersion: CACHE_COORDINATOR_PROTOCOL_VERSION,
    ...value,
  }) as unknown as T;

const errorMessage = (error: unknown): string =>
  error instanceof Error ? error.message : String(error);

async function initializeCore(
  core: CacheWorkerCoreLike,
  activation: PageToEngineEnvelope
): Promise<void> {
  let response: CacheResponse | undefined;
  await core.handleRequest(
    {
      postMessage(message: unknown) {
        if (isCacheResponse(message)) response = message;
      },
    },
    {
      id: 0,
      kind: 'init',
      scope: activation.scope,
      hotCapacity: activation.hotCapacity,
    }
  );
  if (!response || !response.ok) {
    throw new Error(
      response && !response.ok
        ? response.error
        : 'cache engine initialization returned no response'
    );
  }
}

async function defaultOwnerLockIsHeld(ownerLockName: string): Promise<boolean> {
  const snapshot = await navigator.locks.query();
  return Boolean(
    snapshot.held?.some(
      (lock) => lock.name === ownerLockName && lock.mode === 'exclusive'
    )
  );
}

async function activate(
  activation: PageToEngineEnvelope,
  directPort: MessagePort,
  workerScope: DedicatedWorkerScopeLike,
  options: CacheEngineRuntimeOptions
): Promise<void> {
  const hooks = options.hooks;
  hooks?.onEvent?.({ kind: 'activation-started', activation });
  let failed = false;
  let draining = false;
  const pendingAdmissions = new Set<Promise<void>>();
  const post = (message: EngineToCoordinatorEnvelope): void => {
    directPort.postMessage(message);
  };
  const fatal = (reason: string): void => {
    if (failed) return;
    failed = true;
    hooks?.onEvent?.({ kind: 'fatal', activation, reason });
    post(
      withVersion<EngineToCoordinatorEnvelope>({
        kind: 'engine-fatal',
        tabId: activation.tabId,
        ownerEpoch: activation.ownerEpoch,
        reason,
      })
    );
  };

  const createCore =
    options.createCore ??
    ((coreOptions: CacheWorkerCoreOptions) => new CacheWorkerCore(coreOptions));
  const core = createCore({
    recoveryOpen: activation.databaseAction === 'wipe-before-open',
    onStorageResetRequired: () => {
      fatal('cache storage requested physical reset');
    },
  });
  const enginePort = {
    postMessage(message: unknown): void {
      if (isCacheResponse(message)) {
        if (!message.ok && message.errorCode !== undefined) {
          fatal('CacheWorkerCore emitted a coordinator-only cache error code');
          return;
        }
        post(
          withVersion<EngineToCoordinatorEnvelope>({
            kind: 'engine-response',
            ownerEpoch: activation.ownerEpoch,
            routeId: message.id,
            response: message,
          })
        );
        return;
      }
      if (isCachePush(message)) {
        post(
          withVersion<EngineToCoordinatorEnvelope>({
            kind: 'engine-push',
            ownerEpoch: activation.ownerEpoch,
            push: message,
          })
        );
        return;
      }
      fatal('CacheWorkerCore emitted an invalid cache message');
    },
  };

  const admitRequest = (request: CacheRequest): void => {
    hooks?.onEvent?.({ kind: 'request-admitted', activation, request });
    if (!hooks?.beforeRequest) {
      void core.handleRequest(enginePort, request);
      return;
    }
    const admission = Promise.resolve(hooks.beforeRequest(request, activation))
      .then(async () => {
        if (!failed) await core.handleRequest(enginePort, request);
      })
      .catch((error: unknown) => {
        fatal(`engine request hook failed: ${errorMessage(error)}`);
      });
    pendingAdmissions.add(admission);
    void admission.finally(() => pendingAdmissions.delete(admission));
  };

  directPort.onmessage = (event: MessageEvent<unknown>) => {
    const parsed = validateCoordinatorToEngineEnvelope(event.data);
    if (!parsed.ok) {
      fatal(`invalid coordinator envelope: ${parsed.error}`);
      return;
    }
    const message: CoordinatorToEngineEnvelope = parsed.value;
    if (failed) return;
    if (message.ownerEpoch !== activation.ownerEpoch) {
      fatal('coordinator envelope owner epoch does not match activation');
      return;
    }
    switch (message.kind) {
      case 'engine-request':
        if (draining) {
          fatal('coordinator routed a request after drain began');
          return;
        }
        admitRequest(message.request);
        break;
      case 'drain-engine':
        if (draining) return;
        draining = true;
        void (async () => {
          await Promise.all(pendingAdmissions);
          await core.drain();
          hooks?.onEvent?.({ kind: 'drained', activation });
          post(
            withVersion<EngineToCoordinatorEnvelope>({
              kind: 'engine-drained',
              tabId: activation.tabId,
              ownerEpoch: activation.ownerEpoch,
            })
          );
          directPort.close();
          workerScope.close();
        })().catch((error: unknown) =>
          fatal(`engine drain failed: ${errorMessage(error)}`)
        );
        break;
      case 'heartbeat':
        post(
          withVersion<EngineToCoordinatorEnvelope>({
            kind: 'heartbeat-ack',
            ownerEpoch: activation.ownerEpoch,
            heartbeatId: message.heartbeatId,
          })
        );
        break;
    }
  };
  directPort.onmessageerror = () => {
    fatal('coordinator direct MessagePort messageerror');
  };
  directPort.start();

  try {
    await initializeCore(core, activation);
    if (failed) return;
    const ownerLockIsHeld = options.ownerLockIsHeld ?? defaultOwnerLockIsHeld;
    if (!(await ownerLockIsHeld(activation.ownerLockName))) {
      throw new Error('cache engine does not hold its physical owner Web Lock');
    }
    core.addPort(enginePort);
    post(
      withVersion<EngineToCoordinatorEnvelope>({
        kind: 'engine-ready',
        tabId: activation.tabId,
        ownerEpoch: activation.ownerEpoch,
        ownerLockName: activation.ownerLockName,
        ownerLockHeld: true,
        databaseActionProof:
          activation.databaseAction === 'wipe-before-open'
            ? 'wiped-before-open'
            : 'opened-existing',
      })
    );
    hooks?.onEvent?.({ kind: 'ready', activation });
  } catch (error) {
    post(
      withVersion<EngineToCoordinatorEnvelope>({
        kind: 'activation-failed',
        tabId: activation.tabId,
        ownerEpoch: activation.ownerEpoch,
        reason: errorMessage(error),
      })
    );
  }
}

/** Installs the production dedicated-worker transport around CacheWorkerCore. */
export function installCacheEngineWorker(
  options: CacheEngineRuntimeOptions = {}
): void {
  const workerScope =
    options.scope ?? (self as unknown as DedicatedWorkerScopeLike);
  let activated = false;
  workerScope.onmessage = (event: MessageEvent<unknown>) => {
    const parsed = validatePageToEngineEnvelope(event.data);
    const directPort = event.ports[0];
    if (!parsed.ok || event.ports.length !== 1 || !directPort || activated) {
      for (const port of event.ports) port.close();
      return;
    }
    activated = true;
    void activate(parsed.value, directPort, workerScope, options);
  };
}
