/**
 * Pure coordinator state machine for the WP-03 topology spike.
 *
 * Browser ports, workers, clocks, and Web Locks live in adapters. Keeping the
 * election and epoch rules here makes owner-loss behavior deterministic in
 * unit tests.
 */

export type TabId = string;
export type OwnerEpoch = number;

export type DatabaseAction = 'open-existing' | 'wipe-before-open';
export type DatabaseActionProof = 'opened-existing' | 'wiped-before-open';

export type FakeOperation =
  | { kind: 'put'; key: string; value: string }
  | { kind: 'get'; key: string }
  | { kind: 'delay-get'; key: string; delayMs: number };

export type CoordinatorState =
  | {
      kind: 'waiting-for-tab';
      nextDatabaseAction: DatabaseAction;
    }
  | {
      kind: 'activating';
      tabId: TabId;
      epoch: OwnerEpoch;
      databaseAction: DatabaseAction;
    }
  | {
      kind: 'active';
      tabId: TabId;
      epoch: OwnerEpoch;
    }
  | {
      kind: 'draining';
      tabId: TabId;
      epoch: OwnerEpoch;
    }
  | {
      kind: 'resetting-after-loss';
      previousTabId: TabId;
      previousEpoch: OwnerEpoch;
      nextEpoch: OwnerEpoch;
      reason: string;
    };

export type CoordinatorSnapshot = {
  scope: string;
  state: CoordinatorState;
  tabIds: TabId[];
  epoch: OwnerEpoch;
  queuedRequestCount: number;
  inFlightRequestCount: number;
  staleResponseDrops: number;
  protocolViolations: number;
  /** Routing-active owners. This is always zero or one by construction. */
  activeOwnerCount: 0 | 1;
};

type QueuedRequest = {
  tabId: TabId;
  requestId: number;
  operation: FakeOperation;
};

type InFlightRequest = QueuedRequest & {
  routeId: string;
  epoch: OwnerEpoch;
};

export type CoordinatorAction =
  | {
      kind: 'elect-owner';
      tabId: TabId;
      epoch: OwnerEpoch;
      databaseAction: DatabaseAction;
    }
  | {
      kind: 'route-request';
      tabId: TabId;
      requestId: number;
      routeId: string;
      epoch: OwnerEpoch;
      operation: FakeOperation;
    }
  | {
      kind: 'resolve-request';
      tabId: TabId;
      requestId: number;
      result: string | null;
    }
  | {
      kind: 'reject-request';
      tabId: TabId;
      requestId: number;
      error: string;
    }
  | { kind: 'drain-owner'; tabId: TabId; epoch: OwnerEpoch }
  | { kind: 'close-engine-route'; tabId: TabId; epoch: OwnerEpoch }
  | { kind: 'drop-tab'; tabId: TabId }
  | { kind: 'retire-tab'; tabId: TabId; epoch: OwnerEpoch }
  | { kind: 'schedule-reset-activation' }
  | { kind: 'broadcast-engine-replaced'; epoch: OwnerEpoch }
  | {
      kind: 'drop-stale-response';
      epoch: OwnerEpoch;
      routeId: string;
      reason: 'stale-epoch' | 'unknown-route';
    }
  | { kind: 'protocol-violation'; error: string };

export type EngineReady = {
  tabId: TabId;
  epoch: OwnerEpoch;
  ownerLockHeld: boolean;
  databaseActionProof: DatabaseActionProof;
};

export type EngineResponse =
  | {
      epoch: OwnerEpoch;
      routeId: string;
      ok: true;
      result: string | null;
    }
  | {
      epoch: OwnerEpoch;
      routeId: string;
      ok: false;
      error: string;
    };

export const tabLivenessLockName = (scope: string, tabId: string): string =>
  `graphql-cache-tab:${scope}:${tabId}`;

export const databaseOwnerLockName = (scope: string): string =>
  `graphql-cache-owner:${scope}`;

const proofFor = (action: DatabaseAction): DatabaseActionProof =>
  action === 'wipe-before-open' ? 'wiped-before-open' : 'opened-existing';

/** Deterministic election, routing, and owner-epoch state machine. */
export class CoordinatorCore {
  private stateValue: CoordinatorState = {
    kind: 'waiting-for-tab',
    nextDatabaseAction: 'open-existing',
  };
  private readonly tabs: TabId[] = [];
  private readonly retiringTabs = new Set<TabId>();
  private readonly queuedRequests: QueuedRequest[] = [];
  private readonly inFlight = new Map<string, InFlightRequest>();
  private currentEpoch = 0;
  private nextRouteNumber = 1;
  private staleResponseDrops = 0;
  private protocolViolations = 0;

  constructor(readonly scope: string) {
    if (scope.length === 0) throw new Error('scope must not be empty');
  }

  get state(): CoordinatorState {
    return { ...this.stateValue };
  }

  snapshot(): CoordinatorSnapshot {
    return {
      scope: this.scope,
      state: this.state,
      tabIds: [...this.tabs],
      epoch: this.currentEpoch,
      queuedRequestCount: this.queuedRequests.length,
      inFlightRequestCount: this.inFlight.size,
      staleResponseDrops: this.staleResponseDrops,
      protocolViolations: this.protocolViolations,
      activeOwnerCount: this.stateValue.kind === 'active' ? 1 : 0,
    };
  }

  /** Registers an already-liveness-locked tab. */
  registerTab(tabId: TabId): CoordinatorAction[] {
    if (this.tabs.includes(tabId)) return [];
    this.tabs.push(tabId);
    if (this.stateValue.kind !== 'waiting-for-tab') return [];
    return this.activateNext(this.stateValue.nextDatabaseAction);
  }

  /** Routes immediately only while an engine is current-epoch ready. */
  request(
    tabId: TabId,
    requestId: number,
    operation: FakeOperation
  ): CoordinatorAction[] {
    if (!this.tabs.includes(tabId)) {
      return [
        {
          kind: 'reject-request',
          tabId,
          requestId,
          error: 'requester tab is not registered',
        },
      ];
    }

    const request = { tabId, requestId, operation };
    if (this.stateValue.kind !== 'active') {
      this.queuedRequests.push(request);
      return [];
    }
    return [this.route(request, this.stateValue.epoch)];
  }

  /** Activates only after the engine proves both lock ownership and reset. */
  engineReady(ready: EngineReady): CoordinatorAction[] {
    const state = this.stateValue;
    if (
      state.kind !== 'activating' ||
      state.tabId !== ready.tabId ||
      state.epoch !== ready.epoch
    ) {
      return this.recordProtocolViolation(
        `unexpected engine-ready from ${ready.tabId} at epoch ${ready.epoch}`
      );
    }

    const expectedProof = proofFor(state.databaseAction);
    if (!ready.ownerLockHeld || ready.databaseActionProof !== expectedProof) {
      const error = !ready.ownerLockHeld
        ? 'engine became ready without the exclusive owner lock'
        : `engine did not prove ${state.databaseAction}`;
      return [
        ...this.recordProtocolViolation(error),
        ...this.transitionToAbruptLoss(state.tabId, state.epoch, error),
      ];
    }

    this.stateValue = {
      kind: 'active',
      tabId: state.tabId,
      epoch: state.epoch,
    };
    const actions: CoordinatorAction[] = [];
    if (state.epoch > 1) {
      actions.push({ kind: 'broadcast-engine-replaced', epoch: state.epoch });
    }
    while (this.queuedRequests.length > 0) {
      const request = this.queuedRequests.shift();
      if (request && this.tabs.includes(request.tabId)) {
        actions.push(this.route(request, state.epoch));
      }
    }
    this.assertInvariants();
    return actions;
  }

  engineResponse(response: EngineResponse): CoordinatorAction[] {
    const ownerEpoch =
      this.stateValue.kind === 'active' || this.stateValue.kind === 'draining'
        ? this.stateValue.epoch
        : undefined;
    if (ownerEpoch !== response.epoch) {
      this.staleResponseDrops += 1;
      return [
        {
          kind: 'drop-stale-response',
          epoch: response.epoch,
          routeId: response.routeId,
          reason: 'stale-epoch',
        },
      ];
    }

    const request = this.inFlight.get(response.routeId);
    if (!request || request.epoch !== response.epoch) {
      this.staleResponseDrops += 1;
      return [
        {
          kind: 'drop-stale-response',
          epoch: response.epoch,
          routeId: response.routeId,
          reason: 'unknown-route',
        },
      ];
    }

    this.inFlight.delete(response.routeId);
    if (response.ok) {
      return [
        {
          kind: 'resolve-request',
          tabId: request.tabId,
          requestId: request.requestId,
          result: response.result,
        },
      ];
    }
    return [
      {
        kind: 'reject-request',
        tabId: request.tabId,
        requestId: request.requestId,
        error: response.error,
      },
    ];
  }

  /** Stops new routing. The direct engine port drains all prior messages. */
  beginGracefulDeparture(tabId: TabId, epoch: OwnerEpoch): CoordinatorAction[] {
    const state = this.stateValue;
    if (
      state.kind !== 'active' ||
      state.tabId !== tabId ||
      state.epoch !== epoch
    ) {
      return this.recordProtocolViolation(
        `unexpected graceful departure from ${tabId} at epoch ${epoch}`
      );
    }
    this.retiringTabs.add(tabId);
    this.stateValue = { kind: 'draining', tabId, epoch };
    return [{ kind: 'drain-owner', tabId, epoch }];
  }

  /** Completes a clean handoff without requesting a fake-database wipe. */
  engineDrained(tabId: TabId, epoch: OwnerEpoch): CoordinatorAction[] {
    const state = this.stateValue;
    if (
      state.kind !== 'draining' ||
      state.tabId !== tabId ||
      state.epoch !== epoch
    ) {
      return this.recordProtocolViolation(
        `unexpected engine-drained from ${tabId} at epoch ${epoch}`
      );
    }

    const actions = this.rejectEpochRequests(
      epoch,
      'engine drained before delivering its response'
    );
    this.removeTabRecord(tabId);
    this.retiringTabs.delete(tabId);
    actions.push(
      { kind: 'close-engine-route', tabId, epoch },
      { kind: 'retire-tab', tabId, epoch }
    );
    this.stateValue = {
      kind: 'waiting-for-tab',
      nextDatabaseAction: 'open-existing',
    };
    actions.push(...this.activateNext('open-existing'));
    this.assertInvariants();
    return actions;
  }

  /**
   * Handles a reported engine crash. Page crashes normally arrive through
   * `tabLost`, after the coordinator acquires the released liveness lock.
   */
  ownerLost(
    tabId: TabId,
    epoch: OwnerEpoch,
    reason: string
  ): CoordinatorAction[] {
    return this.transitionToAbruptLoss(tabId, epoch, reason);
  }

  /** Handles acquisition of a released per-tab liveness Web Lock. */
  tabLost(tabId: TabId): CoordinatorAction[] {
    if (!this.tabs.includes(tabId)) return [];
    const state = this.stateValue;
    const wasOwner =
      (state.kind === 'active' ||
        state.kind === 'activating' ||
        state.kind === 'draining') &&
      state.tabId === tabId;

    this.removeTabRecord(tabId);
    this.retiringTabs.delete(tabId);
    const actions: CoordinatorAction[] = [{ kind: 'drop-tab', tabId }];
    if (wasOwner) {
      actions.push(
        ...this.transitionToAbruptLoss(
          tabId,
          state.epoch,
          'tab liveness lock was released'
        )
      );
    }
    this.assertInvariants();
    return actions;
  }

  /** Runs in a later task so observers can see `resetting-after-loss`. */
  resumeAfterLoss(): CoordinatorAction[] {
    const state = this.stateValue;
    if (state.kind !== 'resetting-after-loss') return [];
    const candidate = this.chooseCandidate(state.previousTabId);
    if (!candidate) {
      this.stateValue = {
        kind: 'waiting-for-tab',
        nextDatabaseAction: 'wipe-before-open',
      };
      return [];
    }
    this.currentEpoch = state.nextEpoch;
    this.stateValue = {
      kind: 'activating',
      tabId: candidate,
      epoch: state.nextEpoch,
      databaseAction: 'wipe-before-open',
    };
    return [
      {
        kind: 'elect-owner',
        tabId: candidate,
        epoch: state.nextEpoch,
        databaseAction: 'wipe-before-open',
      },
    ];
  }

  /** Runtime guard before accepting a transferred engine MessagePort. */
  expectsEngine(tabId: TabId, epoch: OwnerEpoch): boolean {
    return (
      this.stateValue.kind === 'activating' &&
      this.stateValue.tabId === tabId &&
      this.stateValue.epoch === epoch
    );
  }

  private transitionToAbruptLoss(
    tabId: TabId,
    epoch: OwnerEpoch,
    reason: string
  ): CoordinatorAction[] {
    const state = this.stateValue;
    if (
      (state.kind !== 'active' &&
        state.kind !== 'activating' &&
        state.kind !== 'draining') ||
      state.tabId !== tabId ||
      state.epoch !== epoch
    ) {
      return [];
    }

    const actions = this.rejectEpochRequests(
      epoch,
      `owner epoch ${epoch} was lost: ${reason}`
    );
    actions.push(
      { kind: 'close-engine-route', tabId, epoch },
      { kind: 'schedule-reset-activation' }
    );
    this.stateValue = {
      kind: 'resetting-after-loss',
      previousTabId: tabId,
      previousEpoch: epoch,
      nextEpoch: this.currentEpoch + 1,
      reason,
    };
    this.assertInvariants();
    return actions;
  }

  private rejectEpochRequests(
    epoch: OwnerEpoch,
    error: string
  ): CoordinatorAction[] {
    const actions: CoordinatorAction[] = [];
    for (const [routeId, request] of this.inFlight) {
      if (request.epoch !== epoch) continue;
      this.inFlight.delete(routeId);
      actions.push({
        kind: 'reject-request',
        tabId: request.tabId,
        requestId: request.requestId,
        error,
      });
    }
    return actions;
  }

  private activateNext(databaseAction: DatabaseAction): CoordinatorAction[] {
    const candidate = this.chooseCandidate();
    if (!candidate) {
      this.stateValue = {
        kind: 'waiting-for-tab',
        nextDatabaseAction: databaseAction,
      };
      return [];
    }
    this.currentEpoch += 1;
    this.stateValue = {
      kind: 'activating',
      tabId: candidate,
      epoch: this.currentEpoch,
      databaseAction,
    };
    return [
      {
        kind: 'elect-owner',
        tabId: candidate,
        epoch: this.currentEpoch,
        databaseAction,
      },
    ];
  }

  private chooseCandidate(avoid?: TabId): TabId | undefined {
    const eligible = this.tabs.filter((tabId) => !this.retiringTabs.has(tabId));
    return eligible.find((tabId) => tabId !== avoid) ?? eligible[0];
  }

  private route(
    request: QueuedRequest,
    epoch: OwnerEpoch
  ): Extract<CoordinatorAction, { kind: 'route-request' }> {
    const routeId = `${epoch}:${this.nextRouteNumber++}`;
    this.inFlight.set(routeId, { ...request, routeId, epoch });
    return { kind: 'route-request', ...request, routeId, epoch };
  }

  private removeTabRecord(tabId: TabId): void {
    const index = this.tabs.indexOf(tabId);
    if (index >= 0) this.tabs.splice(index, 1);
    for (let i = this.queuedRequests.length - 1; i >= 0; i -= 1) {
      if (this.queuedRequests[i]?.tabId === tabId) {
        this.queuedRequests.splice(i, 1);
      }
    }
    for (const [routeId, request] of this.inFlight) {
      if (request.tabId === tabId) this.inFlight.delete(routeId);
    }
  }

  private recordProtocolViolation(error: string): CoordinatorAction[] {
    this.protocolViolations += 1;
    return [{ kind: 'protocol-violation', error }];
  }

  private assertInvariants(): void {
    if (
      this.stateValue.kind !== 'active' &&
      this.stateValue.kind !== 'draining'
    ) {
      if (this.inFlight.size > 0) {
        throw new Error('invariant: requests are in flight without an owner');
      }
      return;
    }
    for (const request of this.inFlight.values()) {
      if (request.epoch !== this.stateValue.epoch) {
        throw new Error('invariant: in-flight request has a stale owner epoch');
      }
    }
  }
}
