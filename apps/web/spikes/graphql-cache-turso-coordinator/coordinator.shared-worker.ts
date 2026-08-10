/// <reference lib="webworker" />

import {
  type CoordinatorAction,
  CoordinatorCore,
  databaseOwnerLockName,
  tabLivenessLockName,
} from './coordinator-core';
import type {
  CoordinatorToEngine,
  CoordinatorToTab,
  EngineToCoordinator,
  TabToCoordinator,
} from './spike-wire';

declare const self: SharedWorkerGlobalScope;

type TabConnection = {
  port: MessagePort;
  livenessLockName: string;
  watchToken: object;
};

let core: CoordinatorCore | undefined;
const tabs = new Map<string, TabConnection>();
const enginePorts = new Map<string, MessagePort>();

const engineKey = (tabId: string, epoch: number): string => `${tabId}:${epoch}`;

const postToTab = (tabId: string, message: CoordinatorToTab): void => {
  tabs.get(tabId)?.port.postMessage(message);
};

const broadcast = (message: CoordinatorToTab): void => {
  for (const connection of tabs.values()) {
    connection.port.postMessage(message);
  }
};

const broadcastSnapshot = (): void => {
  if (!core) return;
  broadcast({ kind: 'snapshot', snapshot: core.snapshot() });
};

const applyActions = (actions: CoordinatorAction[]): void => {
  if (!core) return;
  const nestedActions: CoordinatorAction[] = [];
  for (const action of actions) {
    switch (action.kind) {
      case 'elect-owner':
        postToTab(action.tabId, {
          kind: 'become-owner',
          tabId: action.tabId,
          epoch: action.epoch,
          databaseAction: action.databaseAction,
          ownerLockName: databaseOwnerLockName(core.scope),
        });
        break;
      case 'route-request': {
        const port = enginePorts.get(
          engineKey(
            core.state.kind === 'active' ? core.state.tabId : '',
            action.epoch
          )
        );
        if (!port) {
          const state = core.state;
          if (state.kind === 'active') {
            const reason = 'direct engine MessagePort is missing';
            postToTab(state.tabId, {
              kind: 'terminate-failed-engine',
              tabId: state.tabId,
              epoch: state.epoch,
              reason,
            });
            nestedActions.push(
              ...core.ownerLost(state.tabId, state.epoch, reason)
            );
          }
          break;
        }
        const message: CoordinatorToEngine = {
          kind: 'engine-request',
          epoch: action.epoch,
          routeId: action.routeId,
          operation: action.operation,
        };
        port.postMessage(message);
        break;
      }
      case 'resolve-request':
        postToTab(action.tabId, {
          kind: 'response',
          requestId: action.requestId,
          ok: true,
          result: action.result,
        });
        break;
      case 'reject-request':
        postToTab(action.tabId, {
          kind: 'response',
          requestId: action.requestId,
          ok: false,
          error: action.error,
        });
        break;
      case 'drain-owner': {
        const port = enginePorts.get(engineKey(action.tabId, action.epoch));
        if (!port) {
          const reason = 'direct engine MessagePort disappeared before drain';
          postToTab(action.tabId, {
            kind: 'terminate-failed-engine',
            tabId: action.tabId,
            epoch: action.epoch,
            reason,
          });
          nestedActions.push(
            ...core.ownerLost(action.tabId, action.epoch, reason)
          );
          break;
        }
        const message: CoordinatorToEngine = {
          kind: 'drain-engine',
          epoch: action.epoch,
        };
        port.postMessage(message);
        break;
      }
      case 'close-engine-route': {
        const key = engineKey(action.tabId, action.epoch);
        enginePorts.get(key)?.close();
        enginePorts.delete(key);
        break;
      }
      case 'drop-tab': {
        const connection = tabs.get(action.tabId);
        tabs.delete(action.tabId);
        connection?.port.close();
        break;
      }
      case 'retire-tab': {
        postToTab(action.tabId, {
          kind: 'retire-complete',
          tabId: action.tabId,
          epoch: action.epoch,
        });
        const connection = tabs.get(action.tabId);
        tabs.delete(action.tabId);
        connection?.port.close();
        break;
      }
      case 'schedule-reset-activation':
        queueMicrotask(() => {
          if (!core) return;
          applyActions(core.resumeAfterLoss());
        });
        break;
      case 'broadcast-engine-replaced':
        broadcast({ kind: 'engine-replaced', epoch: action.epoch });
        break;
      case 'drop-stale-response':
        break;
      case 'protocol-violation':
        broadcast({ kind: 'protocol-error', error: action.error });
        break;
    }
  }
  if (nestedActions.length > 0) applyActions(nestedActions);
  broadcastSnapshot();
};

const watchTabLiveness = (
  tabId: string,
  lockName: string,
  watchToken: object
): void => {
  void navigator.locks
    .request(lockName, { mode: 'exclusive' }, async (lock) => {
      if (!lock) return;
      const connection = tabs.get(tabId);
      if (!connection || connection.watchToken !== watchToken || !core) return;
      applyActions(core.tabLost(tabId));
    })
    .catch((error: unknown) => {
      postToTab(tabId, {
        kind: 'protocol-error',
        error: `liveness lock watch failed: ${String(error)}`,
      });
    });
};

const handleEngineMessage = (message: EngineToCoordinator): void => {
  if (!core) return;
  switch (message.kind) {
    case 'engine-ready': {
      const actions = core.engineReady(message);
      const violation = actions.find(
        (action) => action.kind === 'protocol-violation'
      );
      if (violation?.kind === 'protocol-violation') {
        postToTab(message.tabId, {
          kind: 'terminate-failed-engine',
          tabId: message.tabId,
          epoch: message.epoch,
          reason: violation.error,
        });
      }
      applyActions(actions);
      break;
    }
    case 'engine-response':
      applyActions(core.engineResponse(message));
      break;
    case 'engine-drained':
      applyActions(core.engineDrained(message.tabId, message.epoch));
      break;
    case 'engine-activation-failed':
      postToTab(message.tabId, {
        kind: 'terminate-failed-engine',
        tabId: message.tabId,
        epoch: message.epoch,
        reason: message.error,
      });
      applyActions(core.ownerLost(message.tabId, message.epoch, message.error));
      break;
    case 'operation-started':
      broadcast({
        kind: 'operation-started',
        epoch: message.epoch,
        routeId: message.routeId,
        operation: message.operation,
      });
      break;
    case 'engine-lock-event':
      broadcast(message);
      break;
  }
};

const probeStaleResponseThroughMessagePort = (
  epoch: number,
  routeId: string
): void => {
  const probe = new MessageChannel();
  probe.port1.onmessage = (event: MessageEvent<EngineToCoordinator>) => {
    handleEngineMessage(event.data);
    broadcast({ kind: 'stale-response-port-observed', epoch, routeId });
    probe.port1.close();
    probe.port2.close();
  };
  probe.port1.start();
  probe.port2.postMessage({
    kind: 'engine-response',
    epoch,
    routeId,
    ok: true,
    result: 'synthetic stale result sent through a MessagePort',
  } satisfies EngineToCoordinator);
};

const attachEnginePort = (
  tabId: string,
  epoch: number,
  port: MessagePort | undefined
): void => {
  if (!core?.expectsEngine(tabId, epoch) || !port) {
    port?.close();
    broadcast({
      kind: 'protocol-error',
      error: `unexpected engine port from ${tabId} at epoch ${epoch}`,
    });
    return;
  }

  const key = engineKey(tabId, epoch);
  enginePorts.get(key)?.close();
  enginePorts.set(key, port);
  port.onmessage = (event: MessageEvent<EngineToCoordinator>) => {
    handleEngineMessage(event.data);
  };
  port.onmessageerror = () => {
    if (!core) return;
    const reason = 'engine MessagePort messageerror';
    postToTab(tabId, {
      kind: 'terminate-failed-engine',
      tabId,
      epoch,
      reason,
    });
    applyActions(core.ownerLost(tabId, epoch, reason));
  };
  port.start();
};

const register = (
  port: MessagePort,
  message: Extract<TabToCoordinator, { kind: 'register-tab' }>
): void => {
  if (!core) core = new CoordinatorCore(message.scope);
  if (core.scope !== message.scope) {
    port.postMessage({
      kind: 'protocol-error',
      error: `coordinator scope mismatch: ${message.scope}`,
    } satisfies CoordinatorToTab);
    port.close();
    return;
  }
  const expectedLockName = tabLivenessLockName(message.scope, message.tabId);
  if (message.livenessLockName !== expectedLockName) {
    port.postMessage({
      kind: 'protocol-error',
      error: 'tab registered without the expected liveness-lock contract',
    } satisfies CoordinatorToTab);
    port.close();
    return;
  }

  const watchToken = {};
  tabs.set(message.tabId, {
    port,
    livenessLockName: message.livenessLockName,
    watchToken,
  });
  port.postMessage({
    kind: 'registered',
    tabId: message.tabId,
  } satisfies CoordinatorToTab);
  watchTabLiveness(message.tabId, message.livenessLockName, watchToken);
  applyActions(core.registerTab(message.tabId));
};

self.onconnect = (event: MessageEvent) => {
  const port = event.ports[0];
  if (!port) return;
  let tabId: string | undefined;
  port.onmessage = (messageEvent: MessageEvent<TabToCoordinator>) => {
    const message = messageEvent.data;
    if (message.kind === 'register-tab') {
      tabId = message.tabId;
      register(port, message);
      return;
    }
    if (!core || !tabId) {
      port.postMessage({
        kind: 'protocol-error',
        error: 'message arrived before tab registration',
      } satisfies CoordinatorToTab);
      return;
    }
    if ('tabId' in message && message.tabId !== tabId) {
      port.postMessage({
        kind: 'protocol-error',
        error: 'message tab id does not match the registered port',
      } satisfies CoordinatorToTab);
      return;
    }

    switch (message.kind) {
      case 'request':
        applyActions(
          core.request(message.tabId, message.requestId, message.operation)
        );
        break;
      case 'attach-engine-port':
        attachEnginePort(message.tabId, message.epoch, messageEvent.ports[0]);
        break;
      case 'graceful-departure':
        applyActions(core.beginGracefulDeparture(message.tabId, message.epoch));
        break;
      case 'engine-lost':
        applyActions(
          core.ownerLost(message.tabId, message.epoch, message.reason)
        );
        break;
      case 'debug-probe-stale-response-port':
        probeStaleResponseThroughMessagePort(message.epoch, message.routeId);
        break;
    }
  };
  port.start();
};
