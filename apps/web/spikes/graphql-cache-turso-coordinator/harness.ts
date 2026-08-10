import type { CoordinatorSnapshot, FakeOperation } from './coordinator-core';
import type {
  EngineLockEvent,
  EngineLockPhase,
  HarnessCommand,
  HarnessEnvelope,
} from './spike-wire';

const runButton = document.querySelector<HTMLButtonElement>('#run');
const resultElement = document.querySelector<HTMLElement>('#result');
const logElement = document.querySelector<HTMLElement>('#log');
if (!runButton || !resultElement || !logElement) {
  throw new Error('harness elements are missing');
}

const log = (message: string): void => {
  logElement.textContent += `${message}\n`;
};

const waitUntil = async (
  description: string,
  predicate: () => boolean,
  timeoutMs = 20_000
): Promise<void> => {
  const startedAt = performance.now();
  while (!predicate()) {
    if (performance.now() - startedAt > timeoutMs) {
      throw new Error(`timed out waiting for ${description}`);
    }
    await new Promise<void>((resolve) => setTimeout(resolve, 20));
  }
};

const assert: (condition: unknown, message: string) => asserts condition = (
  condition,
  message
) => {
  if (!condition) throw new Error(message);
};

type DistributiveOmit<T, K extends PropertyKey> = T extends unknown
  ? Omit<T, K>
  : never;

type HarnessCommandWithoutId = DistributiveOmit<HarnessCommand, 'commandId'>;

const capabilityReport = () => ({
  sharedWorker: typeof SharedWorker === 'function',
  dedicatedWorker: typeof Worker === 'function',
  messageChannel: typeof MessageChannel === 'function',
  webLocks: Boolean(navigator.locks),
  indexedDbFake: Boolean(indexedDB),
  opfs: typeof navigator.storage?.getDirectory === 'function',
  syncAccessHandleOnHarnessPage:
    typeof FileSystemFileHandle !== 'undefined' &&
    'createSyncAccessHandle' in FileSystemFileHandle.prototype,
  crossOriginIsolated,
  sharedArrayBufferPresent: typeof SharedArrayBuffer === 'function',
});

const runScenario = async (): Promise<Record<string, unknown>> => {
  const runId = crypto.randomUUID();
  const scope = `wp03-${runId}`;
  const channel = new BroadcastChannel(
    `graphql-cache-coordinator-spike:${runId}`
  );
  const tabIds = ['tab-a', 'tab-b', 'tab-c'] as const;
  const popups = new Map<string, Window>();
  const registered = new Set<string>();
  const snapshots: CoordinatorSnapshot[] = [];
  const workerEpochs: number[] = [];
  const terminatedWorkerEpochs: number[] = [];
  const engineReplacedEpochs: number[] = [];
  const engineLockEvents = new Map<string, EngineLockEvent>();
  const stalePortObservations = new Set<string>();
  const ownerLockSamples: number[] = [];
  const contentionProbeEpochs: number[] = [];
  const operationStarts: Array<{
    epoch: number;
    routeId: string;
    operation: FakeOperation;
  }> = [];
  const pendingCommands = new Map<
    string,
    {
      resolve: (result: string | null | undefined) => void;
      reject: (error: Error) => void;
      timer: ReturnType<typeof setTimeout>;
    }
  >();

  channel.onmessage = (event: MessageEvent<HarnessEnvelope>) => {
    const envelope = event.data;
    if (envelope.source !== 'tab') return;
    const tabEvent = envelope.event;
    switch (tabEvent.kind) {
      case 'registered':
        registered.add(envelope.tabId);
        log(`${envelope.tabId}: registered with liveness lock held`);
        break;
      case 'worker-created':
        workerEpochs.push(tabEvent.epoch);
        log(`${envelope.tabId}: lazily created engine epoch ${tabEvent.epoch}`);
        break;
      case 'snapshot':
        snapshots.push(tabEvent.snapshot);
        break;
      case 'engine-replaced':
        engineReplacedEpochs.push(tabEvent.epoch);
        break;
      case 'operation-started':
        operationStarts.push(tabEvent);
        break;
      case 'command-result': {
        const pending = pendingCommands.get(tabEvent.commandId);
        if (!pending) return;
        pendingCommands.delete(tabEvent.commandId);
        clearTimeout(pending.timer);
        if (tabEvent.ok) pending.resolve(tabEvent.result);
        else pending.reject(new Error(tabEvent.error));
        break;
      }
      case 'worker-error':
        log(`${envelope.tabId}: worker error: ${tabEvent.error}`);
        break;
      case 'worker-terminated':
        terminatedWorkerEpochs.push(tabEvent.epoch);
        log(
          `${envelope.tabId}: terminated failed engine epoch ${tabEvent.epoch}: ${tabEvent.reason}`
        );
        break;
      case 'engine-lock-event':
        engineLockEvents.set(`${tabEvent.epoch}:${tabEvent.phase}`, tabEvent);
        break;
      case 'stale-response-port-observed':
        stalePortObservations.add(`${tabEvent.epoch}:${tabEvent.routeId}`);
        break;
      case 'retired':
        log(`${envelope.tabId}: retired epoch ${tabEvent.epoch}`);
        break;
      case 'tab-opened':
        log(`${envelope.tabId}: opened`);
        break;
    }
  };

  const command = async (
    targetTabId: string,
    commandValue: HarnessCommandWithoutId
  ): Promise<string | null | undefined> => {
    const commandId = crypto.randomUUID();
    const result = new Promise<string | null | undefined>((resolve, reject) => {
      const timer = setTimeout(() => {
        pendingCommands.delete(commandId);
        reject(new Error(`command timed out: ${commandValue.kind}`));
      }, 20_000);
      pendingCommands.set(commandId, { resolve, reject, timer });
    });
    channel.postMessage({
      source: 'harness',
      targetTabId,
      command: { ...commandValue, commandId } as HarnessCommand,
    } satisfies HarnessEnvelope);
    return await result;
  };

  const latestActive = (minimumEpoch = 0): CoordinatorSnapshot | undefined =>
    snapshots
      .toReversed()
      .find(
        (snapshot) =>
          snapshot.state.kind === 'active' && snapshot.epoch >= minimumEpoch
      );

  const ownerLockName = `graphql-cache-owner:${scope}`;
  const ownerLockCount = async (): Promise<number> => {
    const lockState = await navigator.locks.query();
    const count =
      lockState.held?.filter((lock) => lock.name === ownerLockName).length ?? 0;
    ownerLockSamples.push(count);
    return count;
  };

  const verifyOwnerLockContention = async (epoch: number): Promise<void> => {
    assert(
      (await ownerLockCount()) === 1,
      `epoch ${epoch} owner Web Lock count was not one`
    );
    const unavailable = await navigator.locks.request(
      ownerLockName,
      { mode: 'exclusive', ifAvailable: true },
      (lock) => lock === null
    );
    assert(
      unavailable,
      `epoch ${epoch} owner lock was independently available`
    );
    contentionProbeEpochs.push(epoch);
  };

  const lockEvent = (
    epoch: number,
    phase: EngineLockPhase
  ): EngineLockEvent | undefined => engineLockEvents.get(`${epoch}:${phase}`);

  const waitForLockPhases = async (
    epoch: number,
    phases: EngineLockPhase[]
  ): Promise<void> => {
    await waitUntil(`epoch ${epoch} lock lifecycle ${phases.join(',')}`, () =>
      phases.every((phase) => lockEvent(epoch, phase) !== undefined)
    );
    const timestamps = phases.map(
      (phase) => lockEvent(epoch, phase)?.timestampMs ?? Number.NaN
    );
    assert(
      timestamps.every(
        (timestamp, index) => index === 0 || timestamp >= timestamps[index - 1]!
      ),
      `epoch ${epoch} lock lifecycle timestamps were out of order`
    );
  };

  const capabilities = capabilityReport();
  assert(capabilities.sharedWorker, 'SharedWorker is required');
  assert(capabilities.dedicatedWorker, 'DedicatedWorker is required');
  assert(capabilities.messageChannel, 'MessageChannel is required');
  assert(capabilities.webLocks, 'Web Locks are required');

  for (const tabId of tabIds) {
    const url = new URL('./tab.html', location.href);
    url.searchParams.set('runId', runId);
    url.searchParams.set('scope', scope);
    url.searchParams.set('tabId', tabId);
    const popup = window.open(url, `${runId}:${tabId}`);
    if (!popup) throw new Error(`popup was blocked for ${tabId}`);
    popups.set(tabId, popup);
  }

  await waitUntil('all three registered tabs', () => registered.size === 3);
  await waitUntil('initial active owner', () => latestActive(1) !== undefined);
  const first = latestActive(1);
  assert(first?.state.kind === 'active', 'initial owner did not activate');
  const firstOwner = first.state.tabId;
  assert(first.tabIds.length === 3, 'coordinator did not retain three tabs');
  assert(
    first.activeOwnerCount === 1,
    'initial active owner count was not one'
  );
  assert(
    workerEpochs.filter((epoch) => epoch === 1).length === 1,
    'standby tab eagerly created an engine'
  );
  await waitForLockPhases(1, [
    'requesting',
    'acquired',
    'database-opened',
    'ready-sent',
  ]);
  await verifyOwnerLockContention(1);
  log(`epoch 1 owner: ${firstOwner}; exclusive owner lock observed`);

  const firstRequester = tabIds.find((tabId) => tabId !== firstOwner);
  assert(firstRequester, 'missing first requester');
  await command(firstRequester, {
    kind: 'request',
    operation: {
      kind: 'put',
      key: 'handoff-proof',
      value: 'preserved-by-graceful-drain',
    },
  });
  const firstReader = tabIds.find(
    (tabId) => tabId !== firstOwner && tabId !== firstRequester
  );
  assert(firstReader, 'missing cross-tab reader');
  assert(
    (await command(firstReader, {
      kind: 'request',
      operation: { kind: 'get', key: 'handoff-proof' },
    })) === 'preserved-by-graceful-drain',
    'cross-tab request did not use the active direct engine route'
  );

  await command(firstOwner, { kind: 'graceful-close' });
  await waitUntil('epoch 2 active owner', () => latestActive(2) !== undefined);
  const second = latestActive(2);
  assert(
    second?.state.kind === 'active',
    'graceful replacement did not activate'
  );
  const secondOwner = second.state.tabId;
  assert(
    secondOwner !== firstOwner,
    'graceful handoff re-elected retiring owner'
  );
  assert(second.epoch === 2, 'graceful handoff did not increment epoch');
  assert(
    second.activeOwnerCount === 1,
    'graceful replacement owner count was not one'
  );
  await waitForLockPhases(1, ['database-closed', 'releasing']);
  await waitForLockPhases(2, [
    'requesting',
    'acquired',
    'database-opened',
    'ready-sent',
  ]);
  assert(
    lockEvent(2, 'acquired')!.timestampMs >=
      lockEvent(1, 'releasing')!.timestampMs,
    'replacement acquired the owner lock before graceful release began'
  );
  await verifyOwnerLockContention(2);
  const liveAfterGraceful = tabIds.filter((tabId) => tabId !== firstOwner);
  const gracefulReader =
    liveAfterGraceful.find((tabId) => tabId !== secondOwner) ?? secondOwner;
  assert(
    (await command(gracefulReader, {
      kind: 'request',
      operation: { kind: 'get', key: 'handoff-proof' },
    })) === 'preserved-by-graceful-drain',
    'graceful handoff did not preserve fake DB state'
  );
  log(`epoch 2 owner: ${secondOwner}; fake DB value preserved`);

  const abruptRequester = liveAfterGraceful.find(
    (tabId) => tabId !== secondOwner
  );
  assert(abruptRequester, 'missing requester for abrupt-loss test');
  const slowCommand = command(abruptRequester, {
    kind: 'request',
    operation: {
      kind: 'delay-get',
      key: 'handoff-proof',
      delayMs: 10_000,
    },
  });
  await waitUntil('old owner to start delayed request', () =>
    operationStarts.some(
      (started) =>
        started.epoch === second.epoch && started.operation.kind === 'delay-get'
    )
  );
  const oldOperation = operationStarts.find(
    (started) =>
      started.epoch === second.epoch && started.operation.kind === 'delay-get'
  );
  assert(oldOperation, 'missing delayed operation route');

  // No dispose message is sent. Closing the real popup releases its tab lock,
  // terminates its dedicated worker, and exercises the correctness path.
  popups.get(secondOwner)?.close();
  let abruptError = '';
  try {
    await slowCommand;
  } catch (error) {
    abruptError = error instanceof Error ? error.message : String(error);
  }
  assert(
    abruptError.includes(`owner epoch ${second.epoch} was lost`),
    'abrupt old-epoch request was not rejected'
  );
  await waitUntil('observable resetting-after-loss state', () =>
    snapshots.some((snapshot) => snapshot.state.kind === 'resetting-after-loss')
  );
  await waitUntil('epoch 3 active owner', () => latestActive(3) !== undefined);
  const third = latestActive(3);
  assert(third?.state.kind === 'active', 'abrupt replacement did not activate');
  const thirdOwner = third.state.tabId;
  assert(third.epoch === 3, 'abrupt handoff did not increment epoch');
  assert(
    third.activeOwnerCount === 1,
    'abrupt replacement owner count was not one'
  );
  await waitForLockPhases(3, [
    'requesting',
    'acquired',
    'wipe-started',
    'wipe-completed',
    'database-opened',
    'ready-sent',
  ]);
  await verifyOwnerLockContention(3);
  assert(
    (await command(thirdOwner, {
      kind: 'request',
      operation: { kind: 'get', key: 'handoff-proof' },
    })) === null,
    'abrupt replacement activated before wiping fake DB state'
  );

  await command(thirdOwner, {
    kind: 'probe-stale-response-port',
    epoch: oldOperation.epoch,
    routeId: oldOperation.routeId,
  });
  await waitUntil('old-epoch response to traverse the probe MessagePort', () =>
    stalePortObservations.has(`${oldOperation.epoch}:${oldOperation.routeId}`)
  );
  await waitUntil('stale response drop counter', () =>
    snapshots.some((snapshot) => snapshot.staleResponseDrops >= 1)
  );
  log(
    `epoch 3 owner: ${thirdOwner}; fake DB wiped; MessagePort stale response dropped`
  );

  await command(thirdOwner, {
    kind: 'request',
    operation: {
      kind: 'put',
      key: 'worker-only-loss-proof',
      value: 'must-be-wiped-after-worker-only-loss',
    },
  });
  const workerOnlyPage = popups.get(thirdOwner);
  assert(
    workerOnlyPage && !workerOnlyPage.closed,
    'worker-only owner page closed early'
  );
  await command(thirdOwner, { kind: 'crash-engine' });
  await waitUntil('failed epoch 3 worker termination', () =>
    terminatedWorkerEpochs.includes(3)
  );
  assert(
    !workerOnlyPage.closed,
    'worker-only failure unexpectedly closed the owning page'
  );
  await waitUntil(
    'epoch 4 active owner after worker-only loss',
    () => latestActive(4) !== undefined
  );
  const fourth = latestActive(4);
  assert(
    fourth?.state.kind === 'active',
    'worker-only replacement did not activate'
  );
  const fourthOwner = fourth.state.tabId;
  assert(
    fourth.tabIds.includes(thirdOwner),
    'failed worker page lost its tab registration/liveness contract'
  );
  await waitForLockPhases(4, [
    'requesting',
    'acquired',
    'wipe-started',
    'wipe-completed',
    'database-opened',
    'ready-sent',
  ]);
  await verifyOwnerLockContention(4);
  assert(
    (await command(fourthOwner, {
      kind: 'request',
      operation: { kind: 'get', key: 'worker-only-loss-proof' },
    })) === null,
    'worker-only replacement activated before wiping fake DB state'
  );
  log(
    `epoch 4 owner: ${fourthOwner}; failed worker retired while page stayed alive; fake DB wiped`
  );

  const routerMaxActiveOwners = Math.max(
    ...snapshots.map((snapshot) => snapshot.activeOwnerCount)
  );
  assert(
    routerMaxActiveOwners === 1,
    'coordinator observed more than one routing-active owner'
  );
  assert(
    workerEpochs.join(',') === '1,2,3,4',
    `unexpected lazy worker creation epochs: ${workerEpochs.join(',')}`
  );
  const maxObservedHeldOwnerLocks = Math.max(...ownerLockSamples);
  assert(
    maxObservedHeldOwnerLocks === 1,
    'Web Locks queries observed more than one held owner lock'
  );
  assert(
    contentionProbeEpochs.join(',') === '1,2,3,4',
    'not every active epoch blocked an independent exclusive lock probe'
  );

  const finalSnapshot = snapshots
    .toReversed()
    .find((snapshot) => snapshot.staleResponseDrops >= 1);
  assert(finalSnapshot, 'missing final coordinator snapshot');
  const result = {
    passed: true,
    capabilities,
    openedTabs: 3,
    ownerEpochs: workerEpochs,
    gracefulPreservedFakeDb: true,
    abruptLossRejectedInFlight: true,
    abruptLossWipedFakeDbBeforeActivation: true,
    workerOnlyLossPageStayedAlive: !workerOnlyPage.closed,
    workerOnlyLossWipedFakeDbBeforeActivation: true,
    failedEngineTerminationEpochs: [...new Set(terminatedWorkerEpochs)],
    staleResponseViaMessagePortDrops: finalSnapshot.staleResponseDrops,
    routerMaxActiveOwners,
    maxObservedHeldOwnerLocks,
    activeEpochsBlockingIndependentOwnerLockProbe: contentionProbeEpochs,
    finalObservedOwnerLockCount: await ownerLockCount(),
    lockLifecycleOrderingObserved: true,
    lockLifecycleTimestampsMs: Object.fromEntries(
      [...engineLockEvents.entries()].map(([key, event]) => [
        key,
        event.timestampMs,
      ])
    ),
    wipeLifecycleBeforeReadyEpochs: [3, 4],
    directMessageChannelObserved: operationStarts.length > 0,
    statesObserved: [
      ...new Set(snapshots.map((snapshot) => snapshot.state.kind)),
    ],
    engineReplacedEpochs: [...new Set(engineReplacedEpochs)],
  };

  channel.close();
  for (const popup of popups.values()) popup.close();
  return result;
};

runButton.addEventListener('click', () => {
  runButton.disabled = true;
  resultElement.dataset.status = 'running';
  resultElement.textContent = 'running';
  void runScenario().then(
    (result) => {
      resultElement.dataset.status = 'passed';
      resultElement.textContent = JSON.stringify(result, null, 2);
    },
    (error: unknown) => {
      resultElement.dataset.status = 'failed';
      resultElement.textContent =
        error instanceof Error ? (error.stack ?? error.message) : String(error);
    }
  );
});

if (new URLSearchParams(location.search).get('autorun') === '1') {
  window.addEventListener('load', () => runButton.click(), { once: true });
}
