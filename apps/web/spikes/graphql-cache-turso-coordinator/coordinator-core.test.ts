import { describe, expect, it } from 'vitest';
import {
  type CoordinatorAction,
  CoordinatorCore,
  type DatabaseActionProof,
  type EngineResponse,
} from './coordinator-core';

const findAction = <K extends CoordinatorAction['kind']>(
  actions: CoordinatorAction[],
  kind: K
): Extract<CoordinatorAction, { kind: K }> => {
  const action = actions.find((candidate) => candidate.kind === kind);
  if (!action) throw new Error(`missing ${kind} action`);
  return action as Extract<CoordinatorAction, { kind: K }>;
};

const ready = (
  core: CoordinatorCore,
  tabId: string,
  epoch: number,
  databaseActionProof: DatabaseActionProof
): CoordinatorAction[] =>
  core.engineReady({
    tabId,
    epoch,
    ownerLockHeld: true,
    databaseActionProof,
  });

describe('CoordinatorCore', () => {
  it('elects one of three tabs lazily and queues traffic until lock-backed readiness', () => {
    const core = new CoordinatorCore('scope-three-tabs');

    const election = findAction(core.registerTab('tab-a'), 'elect-owner');
    expect(election).toEqual({
      kind: 'elect-owner',
      tabId: 'tab-a',
      epoch: 1,
      databaseAction: 'open-existing',
    });
    expect(core.registerTab('tab-b')).toEqual([]);
    expect(core.registerTab('tab-c')).toEqual([]);
    expect(core.snapshot().activeOwnerCount).toBe(0);

    expect(
      core.request('tab-c', 1, { kind: 'get', key: 'before-ready' })
    ).toEqual([]);
    expect(core.snapshot().queuedRequestCount).toBe(1);

    const activated = ready(core, 'tab-a', 1, 'opened-existing');
    const route = findAction(activated, 'route-request');
    expect(route.tabId).toBe('tab-c');
    expect(route.epoch).toBe(1);
    expect(core.snapshot()).toMatchObject({
      state: { kind: 'active', tabId: 'tab-a', epoch: 1 },
      activeOwnerCount: 1,
      queuedRequestCount: 0,
      inFlightRequestCount: 1,
    });

    expect(core.registerTab('tab-a')).toEqual([]);
    expect(core.snapshot().activeOwnerCount).toBe(1);
  });

  it('preserves fake DB state after a graceful drain and increments the epoch', () => {
    const core = new CoordinatorCore('scope-graceful');
    const fakeDb = new Map<string, string>();
    core.registerTab('tab-a');
    core.registerTab('tab-b');
    core.registerTab('tab-c');
    ready(core, 'tab-a', 1, 'opened-existing');

    const put = findAction(
      core.request('tab-b', 10, {
        kind: 'put',
        key: 'proof',
        value: 'survives-graceful-drain',
      }),
      'route-request'
    );
    fakeDb.set('proof', 'survives-graceful-drain');
    expect(
      core.engineResponse({
        epoch: put.epoch,
        routeId: put.routeId,
        ok: true,
        result: null,
      })
    ).toContainEqual({
      kind: 'resolve-request',
      tabId: 'tab-b',
      requestId: 10,
      result: null,
    });

    expect(core.beginGracefulDeparture('tab-a', 1)).toEqual([
      { kind: 'drain-owner', tabId: 'tab-a', epoch: 1 },
    ]);
    expect(core.snapshot().activeOwnerCount).toBe(0);

    const handoff = core.engineDrained('tab-a', 1);
    expect(findAction(handoff, 'retire-tab')).toEqual({
      kind: 'retire-tab',
      tabId: 'tab-a',
      epoch: 1,
    });
    expect(findAction(handoff, 'elect-owner')).toEqual({
      kind: 'elect-owner',
      tabId: 'tab-b',
      epoch: 2,
      databaseAction: 'open-existing',
    });

    const activation = ready(core, 'tab-b', 2, 'opened-existing');
    expect(activation).toContainEqual({
      kind: 'broadcast-engine-replaced',
      epoch: 2,
    });
    expect(core.snapshot()).toMatchObject({
      state: { kind: 'active', tabId: 'tab-b', epoch: 2 },
      tabIds: ['tab-b', 'tab-c'],
      activeOwnerCount: 1,
    });

    const read = findAction(
      core.request('tab-c', 11, { kind: 'get', key: 'proof' }),
      'route-request'
    );
    const response: EngineResponse = {
      epoch: read.epoch,
      routeId: read.routeId,
      ok: true,
      result: fakeDb.get('proof') ?? null,
    };
    expect(core.engineResponse(response)).toContainEqual({
      kind: 'resolve-request',
      tabId: 'tab-c',
      requestId: 11,
      result: 'survives-graceful-drain',
    });
  });

  it('rejects abrupt in-flight work, requires wipe proof, and drops stale responses', () => {
    const core = new CoordinatorCore('scope-abrupt');
    const fakeDb = new Map([['proof', 'must-be-wiped']]);
    core.registerTab('tab-a');
    core.registerTab('tab-b');
    core.registerTab('tab-c');
    ready(core, 'tab-a', 1, 'opened-existing');

    const delayed = findAction(
      core.request('tab-c', 20, {
        kind: 'delay-get',
        key: 'proof',
        delayMs: 10_000,
      }),
      'route-request'
    );
    const loss = core.ownerLost('tab-a', 1, 'dedicated worker terminated');
    expect(core.state).toEqual({
      kind: 'resetting-after-loss',
      previousTabId: 'tab-a',
      previousEpoch: 1,
      nextEpoch: 2,
      reason: 'dedicated worker terminated',
    });
    expect(loss).toContainEqual({
      kind: 'reject-request',
      tabId: 'tab-c',
      requestId: 20,
      error: 'owner epoch 1 was lost: dedicated worker terminated',
    });
    expect(findAction(loss, 'schedule-reset-activation')).toBeDefined();
    expect(core.snapshot().activeOwnerCount).toBe(0);

    expect(findAction(core.resumeAfterLoss(), 'elect-owner')).toEqual({
      kind: 'elect-owner',
      tabId: 'tab-b',
      epoch: 2,
      databaseAction: 'wipe-before-open',
    });
    fakeDb.clear();
    ready(core, 'tab-b', 2, 'wiped-before-open');
    expect(core.snapshot().activeOwnerCount).toBe(1);

    const stale = core.engineResponse({
      epoch: delayed.epoch,
      routeId: delayed.routeId,
      ok: true,
      result: 'must-not-escape',
    });
    expect(stale).toEqual([
      {
        kind: 'drop-stale-response',
        epoch: 1,
        routeId: delayed.routeId,
        reason: 'stale-epoch',
      },
    ]);
    expect(core.snapshot().staleResponseDrops).toBe(1);

    const read = findAction(
      core.request('tab-c', 21, { kind: 'get', key: 'proof' }),
      'route-request'
    );
    expect(
      core.engineResponse({
        epoch: read.epoch,
        routeId: read.routeId,
        ok: true,
        result: fakeDb.get('proof') ?? null,
      })
    ).toContainEqual({
      kind: 'resolve-request',
      tabId: 'tab-c',
      requestId: 21,
      result: null,
    });
  });

  it('uses liveness loss for owner failover without disturbing a standby loss', () => {
    const core = new CoordinatorCore('scope-liveness');
    core.registerTab('tab-a');
    core.registerTab('tab-b');
    core.registerTab('tab-c');
    ready(core, 'tab-a', 1, 'opened-existing');

    expect(core.tabLost('tab-c')).toEqual([
      { kind: 'drop-tab', tabId: 'tab-c' },
    ]);
    expect(core.state).toEqual({ kind: 'active', tabId: 'tab-a', epoch: 1 });

    const actions = core.tabLost('tab-a');
    expect(actions[0]).toEqual({ kind: 'drop-tab', tabId: 'tab-a' });
    expect(findAction(actions, 'schedule-reset-activation')).toBeDefined();
    expect(core.state.kind).toBe('resetting-after-loss');

    expect(findAction(core.resumeAfterLoss(), 'elect-owner')).toEqual({
      kind: 'elect-owner',
      tabId: 'tab-b',
      epoch: 2,
      databaseAction: 'wipe-before-open',
    });
  });

  it('never activates an engine that lacks owner-lock or wipe proof', () => {
    const core = new CoordinatorCore('scope-lock-contract');
    core.registerTab('tab-a');
    core.registerTab('tab-b');

    const noLock = core.engineReady({
      tabId: 'tab-a',
      epoch: 1,
      ownerLockHeld: false,
      databaseActionProof: 'opened-existing',
    });
    expect(findAction(noLock, 'protocol-violation').error).toContain(
      'exclusive owner lock'
    );
    expect(core.snapshot()).toMatchObject({
      state: { kind: 'resetting-after-loss' },
      activeOwnerCount: 0,
    });

    const nextElection = findAction(core.resumeAfterLoss(), 'elect-owner');
    expect(nextElection).toMatchObject({
      tabId: 'tab-b',
      epoch: 2,
      databaseAction: 'wipe-before-open',
    });
    const wrongWipeProof = core.engineReady({
      tabId: 'tab-b',
      epoch: 2,
      ownerLockHeld: true,
      databaseActionProof: 'opened-existing',
    });
    expect(findAction(wrongWipeProof, 'protocol-violation').error).toContain(
      'wipe-before-open'
    );
    expect(core.snapshot().activeOwnerCount).toBe(0);
  });
});
