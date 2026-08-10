/// <reference lib="webworker" />

import type {
  ActivateEngine,
  CoordinatorToEngine,
  EngineToCoordinator,
  EngineWorkerControl,
} from './spike-wire';

declare const self: DedicatedWorkerGlobalScope;

const fakeDatabaseName = (scope: string): string =>
  `graphql-cache-turso-coordinator-spike:${scope}`;

const deleteFakeDatabase = async (scope: string): Promise<void> => {
  await new Promise<void>((resolve, reject) => {
    const request = indexedDB.deleteDatabase(fakeDatabaseName(scope));
    request.onsuccess = () => resolve();
    request.onerror = () => reject(request.error ?? new Error('delete failed'));
    request.onblocked = () =>
      reject(new Error('fake database deletion was blocked'));
  });
};

const openFakeDatabase = async (scope: string): Promise<IDBDatabase> =>
  await new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(fakeDatabaseName(scope), 1);
    request.onupgradeneeded = () => {
      if (!request.result.objectStoreNames.contains('records')) {
        request.result.createObjectStore('records');
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error('open failed'));
    request.onblocked = () =>
      reject(new Error('fake database open was blocked'));
  });

const putRecord = async (
  database: IDBDatabase,
  key: string,
  value: string
): Promise<void> => {
  await new Promise<void>((resolve, reject) => {
    const transaction = database.transaction('records', 'readwrite');
    transaction.objectStore('records').put(value, key);
    transaction.oncomplete = () => resolve();
    transaction.onerror = () =>
      reject(transaction.error ?? new Error('fake put failed'));
    transaction.onabort = () =>
      reject(transaction.error ?? new Error('fake put aborted'));
  });
};

const getRecord = async (
  database: IDBDatabase,
  key: string
): Promise<string | null> =>
  await new Promise<string | null>((resolve, reject) => {
    const transaction = database.transaction('records', 'readonly');
    const request = transaction.objectStore('records').get(key);
    request.onsuccess = () =>
      resolve(typeof request.result === 'string' ? request.result : null);
    request.onerror = () =>
      reject(request.error ?? new Error('fake get failed'));
  });

const delay = async (delayMs: number): Promise<void> => {
  await new Promise<void>((resolve) => setTimeout(resolve, delayMs));
};

const eventTimestampMs = (): number =>
  performance.timeOrigin + performance.now();

const activate = async (
  activation: ActivateEngine,
  port: MessagePort
): Promise<void> => {
  const postLockEvent = (
    phase: Extract<EngineToCoordinator, { kind: 'engine-lock-event' }>['phase']
  ): void => {
    port.postMessage({
      kind: 'engine-lock-event',
      tabId: activation.tabId,
      epoch: activation.epoch,
      phase,
      timestampMs: eventTimestampMs(),
    } satisfies EngineToCoordinator);
  };

  postLockEvent('requesting');
  await navigator.locks.request(
    activation.ownerLockName,
    { mode: 'exclusive' },
    async (lock) => {
      if (!lock) throw new Error('exclusive owner lock was not acquired');
      postLockEvent('acquired');

      let database: IDBDatabase | undefined;
      let databaseClosed = false;
      try {
        const databaseActionProof =
          activation.databaseAction === 'wipe-before-open'
            ? 'wiped-before-open'
            : 'opened-existing';
        if (activation.databaseAction === 'wipe-before-open') {
          postLockEvent('wipe-started');
          await deleteFakeDatabase(activation.scope);
          postLockEvent('wipe-completed');
        }
        database = await openFakeDatabase(activation.scope);
        const activeDatabase = database;
        postLockEvent('database-opened');
        let draining = false;
        let queue = Promise.resolve();
        let requestShutdown: (() => void) | undefined;
        const shutdownRequested = new Promise<void>((resolve) => {
          requestShutdown = resolve;
        });

        port.onmessage = (event: MessageEvent<CoordinatorToEngine>) => {
          const message = event.data;
          if (message.epoch !== activation.epoch) return;
          if (message.kind === 'drain-engine') {
            if (draining) return;
            draining = true;
            void queue.finally(() => requestShutdown?.());
            return;
          }

          if (draining) {
            port.postMessage({
              kind: 'engine-response',
              epoch: activation.epoch,
              routeId: message.routeId,
              ok: false,
              error: 'engine is draining',
            } satisfies EngineToCoordinator);
            return;
          }

          queue = queue.then(async () => {
            port.postMessage({
              kind: 'operation-started',
              epoch: activation.epoch,
              routeId: message.routeId,
              operation: message.operation,
            } satisfies EngineToCoordinator);
            try {
              let result: string | null = null;
              switch (message.operation.kind) {
                case 'put':
                  await putRecord(
                    activeDatabase,
                    message.operation.key,
                    message.operation.value
                  );
                  break;
                case 'get':
                  result = await getRecord(
                    activeDatabase,
                    message.operation.key
                  );
                  break;
                case 'delay-get':
                  await delay(message.operation.delayMs);
                  result = await getRecord(
                    activeDatabase,
                    message.operation.key
                  );
                  break;
              }
              port.postMessage({
                kind: 'engine-response',
                epoch: activation.epoch,
                routeId: message.routeId,
                ok: true,
                result,
              } satisfies EngineToCoordinator);
            } catch (error) {
              port.postMessage({
                kind: 'engine-response',
                epoch: activation.epoch,
                routeId: message.routeId,
                ok: false,
                error: error instanceof Error ? error.message : String(error),
              } satisfies EngineToCoordinator);
            }
          });
        };
        port.start();
        postLockEvent('ready-sent');
        port.postMessage({
          kind: 'engine-ready',
          tabId: activation.tabId,
          epoch: activation.epoch,
          ownerLockHeld: true,
          databaseActionProof,
        } satisfies EngineToCoordinator);

        await shutdownRequested;
        await queue;
        database.close();
        databaseClosed = true;
        postLockEvent('database-closed');
        postLockEvent('releasing');
        port.postMessage({
          kind: 'engine-drained',
          tabId: activation.tabId,
          epoch: activation.epoch,
        } satisfies EngineToCoordinator);
      } finally {
        if (database && !databaseClosed) {
          database.close();
          postLockEvent('database-closed');
        }
        if (!databaseClosed) postLockEvent('releasing');
      }
    }
  );
  port.close();
  self.close();
};

self.onmessage = (event: MessageEvent<EngineWorkerControl>) => {
  const message = event.data;
  if (message.kind === 'crash-engine-for-harness') {
    setTimeout(() => {
      throw new Error('harness-induced worker-only failure');
    }, 0);
    return;
  }
  const activation = message;
  const port = event.ports[0];
  if (!port) return;
  void activate(activation, port).catch((error: unknown) => {
    port.postMessage({
      kind: 'engine-activation-failed',
      tabId: activation.tabId,
      epoch: activation.epoch,
      error: error instanceof Error ? error.message : String(error),
    } satisfies EngineToCoordinator);
  });
};
