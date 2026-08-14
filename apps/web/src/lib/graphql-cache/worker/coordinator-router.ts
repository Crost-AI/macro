import type { CacheResponse } from '../protocol';
import {
  type CoordinatorAction,
  CoordinatorCore,
  type CoordinatorSnapshot,
} from './coordinator-core';
import {
  CACHE_COORDINATOR_PROTOCOL_VERSION,
  type CoordinatorToEngineEnvelope,
  type CoordinatorToTabEnvelope,
  databaseOwnerLockName,
  type TabToCoordinatorEnvelope,
  tabLivenessLockName,
  validateEngineToCoordinatorEnvelope,
  validateTabToCoordinatorEnvelope,
} from './coordinator-protocol';

export type CoordinatorMessagePort = Pick<
  MessagePort,
  'postMessage' | 'close' | 'start' | 'onmessage' | 'onmessageerror'
>;

export type CancelLivenessWatch = () => void;

export interface CoordinatorRouterOptions {
  activationTimeoutMs?: number;
  heartbeatIntervalMs?: number;
  heartbeatTimeoutMs?: number;
  verifyTabLockHeld?: (lockName: string) => Promise<boolean>;
  watchTabLock?: (
    lockName: string,
    onReleased: () => void
  ) => CancelLivenessWatch;
  setTimeout?: typeof globalThis.setTimeout;
  clearTimeout?: typeof globalThis.clearTimeout;
  queueMicrotask?: typeof globalThis.queueMicrotask;
}

type TabConnection = {
  port: CoordinatorMessagePort;
  cancelLivenessWatch: CancelLivenessWatch;
};

type PendingRegistration = { cancelled: boolean };

type EngineRoute = {
  tabId: string;
  ownerEpoch: number;
  port: CoordinatorMessagePort;
};

const DEFAULT_ACTIVATION_TIMEOUT_MS = 20_000;
const DEFAULT_HEARTBEAT_INTERVAL_MS = 2_000;
const DEFAULT_HEARTBEAT_TIMEOUT_MS = 5_000;

type WithoutVersion<T> = T extends unknown
  ? Omit<T, 'coordinatorVersion'>
  : never;

const envelope = <T extends { coordinatorVersion: 1 }>(
  value: WithoutVersion<T>
): T =>
  ({
    coordinatorVersion: CACHE_COORDINATOR_PROTOCOL_VERSION,
    ...value,
  }) as unknown as T;

/** Independently checks that registration cannot acquire the page-held lock. */
export async function verifyTabLivenessLockHeld(
  lockName: string
): Promise<boolean> {
  return await navigator.locks.request(
    lockName,
    { mode: 'exclusive', ifAvailable: true },
    (lock) => lock === null
  );
}

/** Waits for the page's liveness lock to be released or abandoned. */
export function watchTabLivenessLock(
  lockName: string,
  onReleased: () => void
): CancelLivenessWatch {
  const abortController = new AbortController();
  void navigator.locks
    .request(
      lockName,
      { mode: 'exclusive', signal: abortController.signal },
      (lock) => {
        if (lock) onReleased();
      }
    )
    .catch((error: unknown) => {
      if (
        !abortController.signal.aborted &&
        (!(error instanceof DOMException) || error.name !== 'AbortError')
      ) {
        console.error('[graphql-cache] tab liveness watch failed');
      }
    });
  return () => abortController.abort();
}

/** SharedWorker adapter around the deterministic coordinator state machine. */
export class CoordinatorRouter {
  private coreValue: CoordinatorCore | undefined;
  private hotCapacity: number | undefined;
  private readonly tabs = new Map<string, TabConnection>();
  private readonly portTabs = new Map<CoordinatorMessagePort, string>();
  private readonly pendingRegistrations = new Map<
    CoordinatorMessagePort,
    PendingRegistration
  >();
  private engineRoute: EngineRoute | undefined;
  private activationTimer: ReturnType<typeof setTimeout> | undefined;
  private heartbeatIntervalTimer: ReturnType<typeof setTimeout> | undefined;
  private heartbeatTimeoutTimer: ReturnType<typeof setTimeout> | undefined;
  private nextHeartbeatId = 1;
  private pendingHeartbeat:
    | { ownerEpoch: number; heartbeatId: number }
    | undefined;

  private readonly activationTimeoutMs: number;
  private readonly heartbeatIntervalMs: number;
  private readonly heartbeatTimeoutMs: number;
  private readonly verifyTabLockHeld: (lockName: string) => Promise<boolean>;
  private readonly watchTabLock: (
    lockName: string,
    onReleased: () => void
  ) => CancelLivenessWatch;
  private readonly setTimeoutFn: typeof globalThis.setTimeout;
  private readonly clearTimeoutFn: typeof globalThis.clearTimeout;
  private readonly queueMicrotaskFn: typeof globalThis.queueMicrotask;

  constructor(options: CoordinatorRouterOptions = {}) {
    this.activationTimeoutMs =
      options.activationTimeoutMs ?? DEFAULT_ACTIVATION_TIMEOUT_MS;
    this.heartbeatIntervalMs =
      options.heartbeatIntervalMs ?? DEFAULT_HEARTBEAT_INTERVAL_MS;
    this.heartbeatTimeoutMs =
      options.heartbeatTimeoutMs ?? DEFAULT_HEARTBEAT_TIMEOUT_MS;
    this.verifyTabLockHeld =
      options.verifyTabLockHeld ?? verifyTabLivenessLockHeld;
    this.watchTabLock = options.watchTabLock ?? watchTabLivenessLock;
    this.setTimeoutFn =
      options.setTimeout ?? globalThis.setTimeout.bind(globalThis);
    this.clearTimeoutFn =
      options.clearTimeout ?? globalThis.clearTimeout.bind(globalThis);
    this.queueMicrotaskFn =
      options.queueMicrotask ?? globalThis.queueMicrotask.bind(globalThis);
  }

  get core(): CoordinatorCore | undefined {
    return this.coreValue;
  }

  snapshot(): CoordinatorSnapshot | undefined {
    return this.coreValue?.snapshot();
  }

  connect(port: CoordinatorMessagePort): void {
    port.onmessage = (event: MessageEvent<unknown>) => {
      void this.handleTabMessage(port, event.data, event.ports);
    };
    port.onmessageerror = () => {
      const tabId = this.portTabs.get(port);
      if (tabId) {
        this.loseTab(tabId, 'tab MessagePort messageerror');
        return;
      }
      const registration = this.pendingRegistrations.get(port);
      if (registration) {
        registration.cancelled = true;
        if (this.pendingRegistrations.get(port) === registration) {
          this.pendingRegistrations.delete(port);
        }
      }
      port.close();
    };
    port.start();
  }

  async handleTabMessage(
    port: CoordinatorMessagePort,
    rawMessage: unknown,
    transferredPorts: readonly MessagePort[] = []
  ): Promise<void> {
    const parsed = validateTabToCoordinatorEnvelope(rawMessage);
    if (!parsed.ok) {
      this.postProtocolError(port, parsed.error);
      return;
    }
    const message = parsed.value;
    const expectedPortCount = message.kind === 'attach-engine-port' ? 1 : 0;
    if (transferredPorts.length !== expectedPortCount) {
      for (const transferredPort of transferredPorts) transferredPort.close();
      const tabId = this.portTabs.get(port);
      if (message.kind === 'attach-engine-port' && tabId) {
        this.failOwner(
          tabId,
          message.ownerEpoch,
          'engine attachment transferred the wrong number of ports'
        );
      } else {
        this.postProtocolError(
          port,
          'coordinator envelope transferred unexpected ports'
        );
      }
      return;
    }
    if (message.kind === 'register-tab') {
      await this.register(port, message);
      return;
    }

    const tabId = this.portTabs.get(port);
    if (!tabId) {
      this.postProtocolError(port, 'message arrived before tab registration');
      return;
    }
    if (message.tabId !== tabId) {
      this.postProtocolError(
        port,
        'message tab id does not match the registered port'
      );
      return;
    }
    const core = this.coreValue;
    if (!core) {
      this.postProtocolError(port, 'coordinator is not initialized');
      return;
    }

    switch (message.kind) {
      case 'cache-request':
        this.applyActions(core.request(tabId, message.request));
        break;
      case 'attach-engine-port':
        this.attachEnginePort(tabId, message.ownerEpoch, transferredPorts[0]);
        break;
      case 'graceful-departure':
        this.applyActions(
          core.beginGracefulDeparture(tabId, message.ownerEpoch)
        );
        break;
      case 'engine-lost':
        this.failOwner(tabId, message.ownerEpoch, message.reason);
        break;
      case 'disconnect-tab':
        this.loseTab(tabId, message.reason);
        break;
    }
  }

  private async register(
    port: CoordinatorMessagePort,
    message: Extract<TabToCoordinatorEnvelope, { kind: 'register-tab' }>
  ): Promise<void> {
    if (this.portTabs.has(port) || this.pendingRegistrations.has(port)) {
      this.postProtocolError(port, 'MessagePort already registered');
      return;
    }
    if (this.tabs.has(message.tabId)) {
      this.postProtocolError(port, 'tab id is already registered');
      port.close();
      return;
    }
    const expectedLockName = tabLivenessLockName(message.scope, message.tabId);
    if (message.livenessLockName !== expectedLockName) {
      this.postProtocolError(
        port,
        'tab registered without the expected liveness-lock name'
      );
      port.close();
      return;
    }
    if (this.coreValue && this.coreValue.scope !== message.scope) {
      this.postProtocolError(port, 'coordinator scope mismatch');
      port.close();
      return;
    }
    if (this.coreValue && this.hotCapacity !== message.hotCapacity) {
      this.postProtocolError(port, 'coordinator hot-capacity mismatch');
      port.close();
      return;
    }

    const registration: PendingRegistration = { cancelled: false };
    this.pendingRegistrations.set(port, registration);
    let lockHeld = false;
    try {
      lockHeld = await this.verifyTabLockHeld(message.livenessLockName);
    } catch {
      if (
        registration.cancelled ||
        this.pendingRegistrations.get(port) !== registration
      ) {
        return;
      }
      this.pendingRegistrations.delete(port);
      this.postProtocolError(port, 'tab liveness-lock verification failed');
      port.close();
      return;
    }
    if (
      registration.cancelled ||
      this.pendingRegistrations.get(port) !== registration
    ) {
      return;
    }
    this.pendingRegistrations.delete(port);
    if (!lockHeld) {
      this.postProtocolError(
        port,
        'tab registration requires an already-held liveness lock'
      );
      port.close();
      return;
    }
    // Recheck after the asynchronous lock probe so concurrent registrations
    // cannot race a duplicate tab or mismatched first scope into the router.
    if (this.portTabs.has(port) || this.tabs.has(message.tabId)) {
      this.postProtocolError(port, 'tab registration raced another port');
      port.close();
      return;
    }
    if (
      this.coreValue &&
      (this.coreValue.scope !== message.scope ||
        this.hotCapacity !== message.hotCapacity)
    ) {
      this.postProtocolError(port, 'coordinator registration raced a mismatch');
      port.close();
      return;
    }

    if (!this.coreValue) {
      this.coreValue = new CoordinatorCore(message.scope);
      this.hotCapacity = message.hotCapacity;
    }
    let connection: TabConnection;
    const cancelLivenessWatch = this.watchTabLock(
      message.livenessLockName,
      () => {
        const current = this.tabs.get(message.tabId);
        if (current && current === connection) {
          this.loseTab(message.tabId, 'tab liveness lock was released');
        }
      }
    );
    connection = { port, cancelLivenessWatch };
    this.tabs.set(message.tabId, connection);
    this.portTabs.set(port, message.tabId);
    this.postToTab(
      message.tabId,
      envelope<CoordinatorToTabEnvelope>({
        kind: 'registered',
        tabId: message.tabId,
      })
    );
    this.applyActions(this.coreValue.registerTab(message.tabId));
  }

  private attachEnginePort(
    tabId: string,
    ownerEpoch: number,
    transferredPort: MessagePort | undefined
  ): void {
    const core = this.coreValue;
    if (
      !core?.expectsEngine(tabId, ownerEpoch) ||
      !transferredPort ||
      this.engineRoute
    ) {
      transferredPort?.close();
      if (core?.expectsEngine(tabId, ownerEpoch)) {
        this.failOwner(tabId, ownerEpoch, 'invalid direct engine attachment');
      } else {
        this.postProtocolError(
          this.tabs.get(tabId)?.port,
          `stale engine port for epoch ${ownerEpoch}`
        );
      }
      return;
    }

    const route: EngineRoute = {
      tabId,
      ownerEpoch,
      port: transferredPort,
    };
    this.engineRoute = route;
    transferredPort.onmessage = (event: MessageEvent<unknown>) => {
      if (this.engineRoute !== route) return;
      if (event.ports.length > 0) {
        for (const port of event.ports) port.close();
        this.failOwner(
          tabId,
          ownerEpoch,
          'engine envelope transferred an unexpected port'
        );
        return;
      }
      this.handleEngineMessage(route, event.data);
    };
    transferredPort.onmessageerror = () => {
      if (this.engineRoute === route) {
        this.failOwner(tabId, ownerEpoch, 'engine MessagePort messageerror');
      }
    };
    transferredPort.start();
  }

  private handleEngineMessage(route: EngineRoute, rawMessage: unknown): void {
    const parsed = validateEngineToCoordinatorEnvelope(rawMessage);
    if (!parsed.ok) {
      this.failOwner(
        route.tabId,
        route.ownerEpoch,
        `invalid engine envelope: ${parsed.error}`
      );
      return;
    }
    const message = parsed.value;
    const core = this.coreValue;
    if (!core) return;
    if (
      message.ownerEpoch !== route.ownerEpoch ||
      ('tabId' in message && message.tabId !== route.tabId)
    ) {
      this.failOwner(
        route.tabId,
        route.ownerEpoch,
        'engine envelope owner tuple does not match its direct route'
      );
      return;
    }

    switch (message.kind) {
      case 'engine-ready': {
        const actions = core.engineReady({
          ...message,
          expectedOwnerLockName: databaseOwnerLockName(core.scope),
        });
        const violated = actions.some(
          (action) => action.kind === 'protocol-violation'
        );
        this.applyActions(actions);
        if (violated) {
          if (this.isCurrentOwner(route.tabId, route.ownerEpoch)) {
            this.failOwner(
              route.tabId,
              route.ownerEpoch,
              'engine readiness proof was rejected'
            );
          } else {
            this.postTerminateEngine(
              route.tabId,
              route.ownerEpoch,
              'engine readiness proof was rejected'
            );
          }
          break;
        }
        if (
          core.state.kind === 'active' &&
          core.state.ownerEpoch === message.ownerEpoch
        ) {
          this.clearActivationTimer();
          this.scheduleHeartbeat(message.ownerEpoch);
        }
        break;
      }
      case 'engine-response':
        this.applyActions(
          core.engineResponse(
            message.ownerEpoch,
            message.routeId,
            message.response
          )
        );
        break;
      case 'engine-push':
        this.applyActions(core.enginePush(message.ownerEpoch, message.push));
        break;
      case 'engine-drained': {
        const actions = core.engineDrained(message.tabId, message.ownerEpoch);
        if (actions.some((action) => action.kind === 'protocol-violation')) {
          this.applyActions(actions);
          this.failOwner(
            route.tabId,
            route.ownerEpoch,
            'unexpected engine-drained from current direct route'
          );
          break;
        }
        this.applyActions(actions);
        break;
      }
      case 'engine-fatal':
      case 'activation-failed':
        this.failOwner(message.tabId, message.ownerEpoch, message.reason);
        break;
      case 'heartbeat-ack':
        this.acceptHeartbeat(message.ownerEpoch, message.heartbeatId);
        break;
    }
  }

  private applyActions(actions: CoordinatorAction[]): void {
    const core = this.coreValue;
    if (!core) return;
    for (const action of actions) {
      switch (action.kind) {
        case 'elect-owner':
          this.clearEngineWatchdogs();
          this.postToTab(
            action.tabId,
            envelope<CoordinatorToTabEnvelope>({
              kind: 'become-owner',
              scope: core.scope,
              tabId: action.tabId,
              ownerEpoch: action.ownerEpoch,
              databaseAction: action.databaseAction,
              ownerLockName: databaseOwnerLockName(core.scope),
              hotCapacity: this.hotCapacity,
            })
          );
          this.activationTimer = this.setTimeoutFn(() => {
            this.failOwner(
              action.tabId,
              action.ownerEpoch,
              'engine activation watchdog timed out'
            );
          }, this.activationTimeoutMs);
          break;
        case 'route-request': {
          const route = this.engineRoute;
          if (
            !route ||
            route.tabId !== action.ownerTabId ||
            route.ownerEpoch !== action.ownerEpoch
          ) {
            this.failOwner(
              action.ownerTabId,
              action.ownerEpoch,
              'direct engine MessagePort is missing'
            );
            break;
          }
          route.port.postMessage(
            envelope<CoordinatorToEngineEnvelope>({
              kind: 'engine-request',
              ownerEpoch: action.ownerEpoch,
              routeId: action.routeId,
              request: action.request,
            })
          );
          break;
        }
        case 'deliver-response':
          this.postCacheResponse(action.tabId, action.response);
          break;
        case 'broadcast-push':
          this.broadcast(
            envelope<CoordinatorToTabEnvelope>({
              kind: 'cache-message',
              message: action.push,
            })
          );
          break;
        case 'reject-request':
          this.postCacheResponse(action.tabId, {
            id: action.requestId,
            ok: false,
            error: action.error,
            ...(action.errorCode === undefined
              ? {}
              : { errorCode: action.errorCode }),
          });
          break;
        case 'drain-owner': {
          this.clearHeartbeatTimers();
          const route = this.engineRoute;
          if (
            !route ||
            route.tabId !== action.tabId ||
            route.ownerEpoch !== action.ownerEpoch
          ) {
            this.failOwner(
              action.tabId,
              action.ownerEpoch,
              'direct engine MessagePort disappeared before drain'
            );
            break;
          }
          route.port.postMessage(
            envelope<CoordinatorToEngineEnvelope>({
              kind: 'drain-engine',
              ownerEpoch: action.ownerEpoch,
            })
          );
          break;
        }
        case 'close-engine-route':
          this.clearEngineWatchdogs();
          if (
            this.engineRoute?.tabId === action.tabId &&
            this.engineRoute.ownerEpoch === action.ownerEpoch
          ) {
            this.engineRoute.port.close();
            this.engineRoute = undefined;
          }
          break;
        case 'drop-tab':
          this.removeTabConnection(action.tabId);
          break;
        case 'retire-tab':
          this.postToTab(
            action.tabId,
            envelope<CoordinatorToTabEnvelope>({
              kind: 'retire-complete',
              tabId: action.tabId,
              ownerEpoch: action.ownerEpoch,
            })
          );
          this.removeTabConnection(action.tabId);
          break;
        case 'schedule-reset-activation':
          this.queueMicrotaskFn(() => {
            if (this.coreValue) {
              this.applyActions(this.coreValue.resumeAfterLoss());
            }
          });
          break;
        case 'broadcast-engine-replaced':
          this.broadcast(
            envelope<CoordinatorToTabEnvelope>({
              kind: 'engine-replaced',
              ownerEpoch: action.ownerEpoch,
            })
          );
          break;
        case 'drop-stale-engine-message':
          break;
        case 'protocol-violation':
          this.broadcast(
            envelope<CoordinatorToTabEnvelope>({
              kind: 'protocol-error',
              error: action.error,
            })
          );
          break;
      }
    }
  }

  private failOwner(tabId: string, ownerEpoch: number, reason: string): void {
    const core = this.coreValue;
    if (!core) return;
    const actions = core.ownerLost(tabId, ownerEpoch, reason);
    if (actions.length === 0) return;
    this.postTerminateEngine(tabId, ownerEpoch, reason);
    this.clearEngineWatchdogs();
    this.applyActions(actions);
  }

  private loseTab(tabId: string, reason: string): void {
    const core = this.coreValue;
    if (!core) return;
    const state = core.state;
    if (
      state.kind !== 'waiting-for-tab' &&
      state.kind !== 'resetting-after-loss' &&
      state.tabId === tabId
    ) {
      // The page may still be alive after a liveness or MessagePort failure.
      // Tell it to kill the orphanable DedicatedWorker before dropping its port.
      this.postTerminateEngine(tabId, state.ownerEpoch, reason);
      this.clearEngineWatchdogs();
    }
    this.applyActions(core.tabLost(tabId, reason));
  }

  private isCurrentOwner(tabId: string, ownerEpoch: number): boolean {
    const state = this.coreValue?.state;
    return Boolean(
      state &&
        state.kind !== 'waiting-for-tab' &&
        state.kind !== 'resetting-after-loss' &&
        state.tabId === tabId &&
        state.ownerEpoch === ownerEpoch
    );
  }

  private postTerminateEngine(
    tabId: string,
    ownerEpoch: number,
    reason: string
  ): void {
    this.postToTab(
      tabId,
      envelope<CoordinatorToTabEnvelope>({
        kind: 'terminate-engine',
        tabId,
        ownerEpoch,
        reason,
      })
    );
  }

  private postCacheResponse(tabId: string, response: CacheResponse): void {
    this.postToTab(
      tabId,
      envelope<CoordinatorToTabEnvelope>({
        kind: 'cache-message',
        message: response,
      })
    );
  }

  private postToTab(tabId: string, message: CoordinatorToTabEnvelope): void {
    this.tabs.get(tabId)?.port.postMessage(message);
  }

  private broadcast(message: CoordinatorToTabEnvelope): void {
    for (const connection of this.tabs.values()) {
      connection.port.postMessage(message);
    }
  }

  private postProtocolError(
    port: CoordinatorMessagePort | undefined,
    error: string
  ): void {
    port?.postMessage(
      envelope<CoordinatorToTabEnvelope>({ kind: 'protocol-error', error })
    );
  }

  private removeTabConnection(tabId: string): void {
    const connection = this.tabs.get(tabId);
    if (!connection) return;
    this.tabs.delete(tabId);
    this.portTabs.delete(connection.port);
    connection.cancelLivenessWatch();
    connection.port.close();
  }

  private scheduleHeartbeat(ownerEpoch: number): void {
    this.clearHeartbeatTimers();
    this.heartbeatIntervalTimer = this.setTimeoutFn(() => {
      const route = this.engineRoute;
      const state = this.coreValue?.state;
      if (
        !route ||
        state?.kind !== 'active' ||
        state.ownerEpoch !== ownerEpoch
      ) {
        return;
      }
      const heartbeatId = this.nextHeartbeatId++;
      this.pendingHeartbeat = { ownerEpoch, heartbeatId };
      route.port.postMessage(
        envelope<CoordinatorToEngineEnvelope>({
          kind: 'heartbeat',
          ownerEpoch,
          heartbeatId,
        })
      );
      this.heartbeatTimeoutTimer = this.setTimeoutFn(() => {
        if (
          this.pendingHeartbeat?.ownerEpoch === ownerEpoch &&
          this.pendingHeartbeat.heartbeatId === heartbeatId
        ) {
          this.failOwner(
            route.tabId,
            ownerEpoch,
            'engine heartbeat watchdog timed out'
          );
        }
      }, this.heartbeatTimeoutMs);
    }, this.heartbeatIntervalMs);
  }

  private acceptHeartbeat(ownerEpoch: number, heartbeatId: number): void {
    if (
      this.pendingHeartbeat?.ownerEpoch !== ownerEpoch ||
      this.pendingHeartbeat.heartbeatId !== heartbeatId
    ) {
      return;
    }
    if (this.heartbeatTimeoutTimer !== undefined) {
      this.clearTimeoutFn(this.heartbeatTimeoutTimer);
      this.heartbeatTimeoutTimer = undefined;
    }
    this.pendingHeartbeat = undefined;
    this.scheduleHeartbeat(ownerEpoch);
  }

  private clearActivationTimer(): void {
    if (this.activationTimer !== undefined) {
      this.clearTimeoutFn(this.activationTimer);
      this.activationTimer = undefined;
    }
  }

  private clearHeartbeatTimers(): void {
    if (this.heartbeatIntervalTimer !== undefined) {
      this.clearTimeoutFn(this.heartbeatIntervalTimer);
      this.heartbeatIntervalTimer = undefined;
    }
    if (this.heartbeatTimeoutTimer !== undefined) {
      this.clearTimeoutFn(this.heartbeatTimeoutTimer);
      this.heartbeatTimeoutTimer = undefined;
    }
    this.pendingHeartbeat = undefined;
  }

  private clearEngineWatchdogs(): void {
    this.clearActivationTimer();
    this.clearHeartbeatTimers();
  }
}
