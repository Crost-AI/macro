import { type FakeOperation, tabLivenessLockName } from './coordinator-core';
import type {
  ActivateEngine,
  CoordinatorToTab,
  EngineWorkerControl,
  HarnessEnvelope,
  TabToCoordinator,
} from './spike-wire';

const parameters = new URLSearchParams(location.search);
const tabId = parameters.get('tabId') ?? '';
const runId = parameters.get('runId') ?? '';
const scope = parameters.get('scope') ?? '';
if (!tabId || !runId || !scope)
  throw new Error('missing tab harness parameters');

document.title = `Coordinator spike: ${tabId}`;
const statusElement = document.querySelector<HTMLElement>('#status');
const harnessChannel = new BroadcastChannel(
  `graphql-cache-coordinator-spike:${runId}`
);

let coordinatorPort: MessagePort | undefined;
let engineWorker: Worker | undefined;
let engineEpoch: number | undefined;
let releaseLivenessLock: (() => void) | undefined;
let nextRequestId = 1;
let gracefulCommandId: string | undefined;
let crashCommandId: string | undefined;
const requestCommands = new Map<number, string>();

const setStatus = (value: string): void => {
  if (statusElement) statusElement.textContent = value;
};

const report = (
  event: Extract<HarnessEnvelope, { source: 'tab' }>['event']
): void => {
  harnessChannel.postMessage({
    source: 'tab',
    tabId,
    event,
  } satisfies HarnessEnvelope);
};

const postCoordinator = (
  message: TabToCoordinator,
  ports?: Transferable[]
): void => {
  if (!coordinatorPort) throw new Error('coordinator is not connected');
  coordinatorPort.postMessage(message, ports ?? []);
};

const holdLivenessLock = async (): Promise<string> => {
  const lockName = tabLivenessLockName(scope, tabId);
  let acquired: (() => void) | undefined;
  const acquiredPromise = new Promise<void>((resolve) => {
    acquired = resolve;
  });
  const heldUntilReleased = new Promise<void>((resolve) => {
    releaseLivenessLock = resolve;
  });
  void navigator.locks
    .request(lockName, { mode: 'exclusive' }, async (lock) => {
      if (!lock) throw new Error('tab liveness lock was not acquired');
      acquired?.();
      await heldUntilReleased;
    })
    .catch((error: unknown) => {
      setStatus(`liveness lock failed: ${String(error)}`);
    });
  await acquiredPromise;
  return lockName;
};

const terminateFailedEngine = (
  epoch: number,
  reason: string,
  reportLossToCoordinator: boolean
): boolean => {
  if (!engineWorker || engineEpoch !== epoch) return false;
  const worker = engineWorker;
  worker.onerror = null;
  worker.terminate();
  engineWorker = undefined;
  engineEpoch = undefined;
  setStatus(`standby after failed engine epoch ${epoch}`);
  report({ kind: 'worker-terminated', epoch, reason });
  if (reportLossToCoordinator) {
    postCoordinator({ kind: 'engine-lost', tabId, epoch, reason });
  }
  return true;
};

const spawnEngine = (
  epoch: number,
  databaseAction: Extract<
    CoordinatorToTab,
    { kind: 'become-owner' }
  >['databaseAction'],
  ownerLockName: string
): void => {
  if (engineWorker)
    throw new Error('an engine worker already exists in this tab');

  // Deliberately lazy: this is the spike's only `new Worker`, and it runs only
  // after a current-epoch election message. Standby tabs never construct one.
  const worker = new Worker(
    new URL('./fake-engine.worker.ts', import.meta.url),
    {
      type: 'module',
      name: `graphql-cache-fake-engine:${scope}:${epoch}`,
    }
  );
  engineWorker = worker;
  engineEpoch = epoch;
  report({ kind: 'worker-created', epoch });

  worker.onerror = (event) => {
    event.preventDefault();
    const error = event.message || 'dedicated engine worker error';
    report({ kind: 'worker-error', epoch, error });
    const terminated = terminateFailedEngine(epoch, error, true);
    if (terminated && crashCommandId) {
      report({
        kind: 'command-result',
        commandId: crashCommandId,
        ok: true,
      });
      crashCommandId = undefined;
    }
  };

  const directChannel = new MessageChannel();
  postCoordinator({ kind: 'attach-engine-port', tabId, epoch }, [
    directChannel.port1,
  ]);
  worker.postMessage(
    {
      kind: 'activate-engine',
      scope,
      tabId,
      epoch,
      databaseAction,
      ownerLockName,
    } satisfies ActivateEngine,
    [directChannel.port2]
  );
};

const request = (commandId: string, operation: FakeOperation): void => {
  const requestId = nextRequestId++;
  requestCommands.set(requestId, commandId);
  postCoordinator({ kind: 'request', tabId, requestId, operation });
};

const onCoordinatorMessage = (message: CoordinatorToTab): void => {
  switch (message.kind) {
    case 'registered':
      setStatus('registered standby');
      report({ kind: 'registered' });
      break;
    case 'become-owner':
      setStatus(`owner epoch ${message.epoch}`);
      spawnEngine(message.epoch, message.databaseAction, message.ownerLockName);
      break;
    case 'response': {
      const commandId = requestCommands.get(message.requestId);
      if (!commandId) return;
      requestCommands.delete(message.requestId);
      if (message.ok) {
        report({
          kind: 'command-result',
          commandId,
          ok: true,
          result: message.result,
        });
      } else {
        report({
          kind: 'command-result',
          commandId,
          ok: false,
          error: message.error,
        });
      }
      break;
    }
    case 'retire-complete':
      if (message.tabId !== tabId) return;
      engineWorker?.terminate();
      engineWorker = undefined;
      engineEpoch = undefined;
      releaseLivenessLock?.();
      report({ kind: 'retired', epoch: message.epoch });
      if (gracefulCommandId) {
        report({
          kind: 'command-result',
          commandId: gracefulCommandId,
          ok: true,
        });
      }
      coordinatorPort?.close();
      setTimeout(() => window.close(), 0);
      break;
    case 'terminate-failed-engine':
      if (message.tabId !== tabId) return;
      terminateFailedEngine(message.epoch, message.reason, false);
      break;
    case 'engine-replaced':
      report({ kind: 'engine-replaced', epoch: message.epoch });
      break;
    case 'snapshot':
      report({ kind: 'snapshot', snapshot: message.snapshot });
      break;
    case 'protocol-error':
      setStatus(`protocol error: ${message.error}`);
      report({
        kind: 'worker-error',
        epoch: engineEpoch ?? 0,
        error: message.error,
      });
      break;
    case 'operation-started':
      report({
        kind: 'operation-started',
        epoch: message.epoch,
        routeId: message.routeId,
        operation: message.operation,
      });
      break;
    case 'engine-lock-event':
      report(message);
      break;
    case 'stale-response-port-observed':
      report(message);
      break;
  }
};

const start = async (): Promise<void> => {
  report({ kind: 'tab-opened' });
  if (
    typeof SharedWorker !== 'function' ||
    typeof Worker !== 'function' ||
    typeof MessageChannel !== 'function' ||
    !navigator.locks
  ) {
    throw new Error('required worker/Web Locks capability is unavailable');
  }

  // Registration happens only after this page demonstrably owns its liveness
  // lock, avoiding a race where the coordinator could acquire it first.
  const livenessLockName = await holdLivenessLock();
  const coordinator = new SharedWorker(
    new URL('./coordinator.shared-worker.ts', import.meta.url),
    { type: 'module', name: `graphql-cache-coordinator-spike:${scope}` }
  );
  coordinatorPort = coordinator.port;
  coordinator.port.onmessage = (event: MessageEvent<CoordinatorToTab>) =>
    onCoordinatorMessage(event.data);
  coordinator.port.start();
  postCoordinator({
    kind: 'register-tab',
    scope,
    tabId,
    livenessLockName,
  });
};

harnessChannel.onmessage = (event: MessageEvent<HarnessEnvelope>) => {
  const envelope = event.data;
  if (envelope.source !== 'harness' || envelope.targetTabId !== tabId) return;
  const command = envelope.command;
  switch (command.kind) {
    case 'request':
      request(command.commandId, command.operation);
      break;
    case 'graceful-close':
      if (engineEpoch === undefined) {
        report({
          kind: 'command-result',
          commandId: command.commandId,
          ok: false,
          error: 'tab is not the active owner',
        });
        return;
      }
      gracefulCommandId = command.commandId;
      postCoordinator({
        kind: 'graceful-departure',
        tabId,
        epoch: engineEpoch,
      });
      break;
    case 'crash-engine':
      if (!engineWorker || engineEpoch === undefined) {
        report({
          kind: 'command-result',
          commandId: command.commandId,
          ok: false,
          error: 'tab has no active engine to crash',
        });
        return;
      }
      crashCommandId = command.commandId;
      engineWorker.postMessage({
        kind: 'crash-engine-for-harness',
      } satisfies EngineWorkerControl);
      break;
    case 'probe-stale-response-port':
      postCoordinator({
        kind: 'debug-probe-stale-response-port',
        epoch: command.epoch,
        routeId: command.routeId,
      });
      report({
        kind: 'command-result',
        commandId: command.commandId,
        ok: true,
      });
      break;
  }
};

void start().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  setStatus(`startup failed: ${message}`);
  report({ kind: 'worker-error', epoch: 0, error: message });
});
