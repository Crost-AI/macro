import type {
  CoordinatorSnapshot,
  DatabaseAction,
  DatabaseActionProof,
  FakeOperation,
} from './coordinator-core';

export type EngineLockPhase =
  | 'requesting'
  | 'acquired'
  | 'wipe-started'
  | 'wipe-completed'
  | 'database-opened'
  | 'ready-sent'
  | 'database-closed'
  | 'releasing';

export type EngineLockEvent = {
  kind: 'engine-lock-event';
  tabId: string;
  epoch: number;
  phase: EngineLockPhase;
  timestampMs: number;
};

export type TabToCoordinator =
  | {
      kind: 'register-tab';
      scope: string;
      tabId: string;
      livenessLockName: string;
    }
  | {
      kind: 'request';
      tabId: string;
      requestId: number;
      operation: FakeOperation;
    }
  | {
      kind: 'attach-engine-port';
      tabId: string;
      epoch: number;
    }
  | {
      kind: 'graceful-departure';
      tabId: string;
      epoch: number;
    }
  | {
      kind: 'engine-lost';
      tabId: string;
      epoch: number;
      reason: string;
    }
  | {
      kind: 'debug-probe-stale-response-port';
      epoch: number;
      routeId: string;
    };

export type CoordinatorToTab =
  | { kind: 'registered'; tabId: string }
  | {
      kind: 'become-owner';
      tabId: string;
      epoch: number;
      databaseAction: DatabaseAction;
      ownerLockName: string;
    }
  | {
      kind: 'response';
      requestId: number;
      ok: true;
      result: string | null;
    }
  | { kind: 'response'; requestId: number; ok: false; error: string }
  | { kind: 'retire-complete'; tabId: string; epoch: number }
  | {
      kind: 'terminate-failed-engine';
      tabId: string;
      epoch: number;
      reason: string;
    }
  | { kind: 'engine-replaced'; epoch: number }
  | { kind: 'snapshot'; snapshot: CoordinatorSnapshot }
  | { kind: 'protocol-error'; error: string }
  | {
      kind: 'operation-started';
      epoch: number;
      routeId: string;
      operation: FakeOperation;
    }
  | EngineLockEvent
  | {
      kind: 'stale-response-port-observed';
      epoch: number;
      routeId: string;
    };

export type ActivateEngine = {
  kind: 'activate-engine';
  scope: string;
  tabId: string;
  epoch: number;
  databaseAction: DatabaseAction;
  ownerLockName: string;
};

export type EngineWorkerControl =
  | ActivateEngine
  | { kind: 'crash-engine-for-harness' };

export type CoordinatorToEngine =
  | {
      kind: 'engine-request';
      epoch: number;
      routeId: string;
      operation: FakeOperation;
    }
  | { kind: 'drain-engine'; epoch: number };

export type EngineToCoordinator =
  | {
      kind: 'engine-ready';
      tabId: string;
      epoch: number;
      ownerLockHeld: boolean;
      databaseActionProof: DatabaseActionProof;
    }
  | {
      kind: 'engine-response';
      epoch: number;
      routeId: string;
      ok: true;
      result: string | null;
    }
  | {
      kind: 'engine-response';
      epoch: number;
      routeId: string;
      ok: false;
      error: string;
    }
  | { kind: 'engine-drained'; tabId: string; epoch: number }
  | {
      kind: 'operation-started';
      epoch: number;
      routeId: string;
      operation: FakeOperation;
    }
  | {
      kind: 'engine-activation-failed';
      tabId: string;
      epoch: number;
      error: string;
    }
  | EngineLockEvent;

export type HarnessCommand =
  | { kind: 'request'; commandId: string; operation: FakeOperation }
  | { kind: 'graceful-close'; commandId: string }
  | { kind: 'crash-engine'; commandId: string }
  | {
      kind: 'probe-stale-response-port';
      commandId: string;
      epoch: number;
      routeId: string;
    };

export type HarnessEnvelope =
  | { source: 'harness'; targetTabId: string; command: HarnessCommand }
  | {
      source: 'tab';
      tabId: string;
      event:
        | { kind: 'tab-opened' }
        | { kind: 'registered' }
        | { kind: 'worker-created'; epoch: number }
        | { kind: 'worker-error'; epoch: number; error: string }
        | { kind: 'worker-terminated'; epoch: number; reason: string }
        | { kind: 'retired'; epoch: number }
        | {
            kind: 'command-result';
            commandId: string;
            ok: true;
            result?: string | null;
          }
        | {
            kind: 'command-result';
            commandId: string;
            ok: false;
            error: string;
          }
        | { kind: 'snapshot'; snapshot: CoordinatorSnapshot }
        | { kind: 'engine-replaced'; epoch: number }
        | {
            kind: 'operation-started';
            epoch: number;
            routeId: string;
            operation: FakeOperation;
          }
        | EngineLockEvent
        | {
            kind: 'stale-response-port-observed';
            epoch: number;
            routeId: string;
          };
    };
