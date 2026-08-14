import { expect, test } from '@playwright/test';

test('three pages fence graceful, abrupt, stale, and worker-only ownership', async ({
  context,
  page,
}) => {
  const browserErrors: string[] = [];
  const watch = (candidate: typeof page): void => {
    candidate.on('console', (message) => {
      if (message.type() === 'error') browserErrors.push(message.text());
    });
    candidate.on('pageerror', (error) => browserErrors.push(error.message));
  };
  watch(page);
  context.on('page', watch);

  await page.goto('/');
  const result = page.locator('#result');
  await expect(result).toHaveAttribute('data-status', 'passed', {
    timeout: 40_000,
  });
  const report = JSON.parse((await result.textContent()) ?? '') as {
    passed: boolean;
    openedTabs: number;
    ownerEpochs: number[];
    maxWorkersPerEpoch: number;
    noEagerWorker: boolean;
    collidingRequestIdsRewritten: boolean;
    gracefulPreserved: boolean;
    abruptRejectedInflight: boolean;
    abruptWiped: boolean;
    workerOnlyPageStayedAlive: boolean;
    workerOnlyWiped: boolean;
    livenessPageStayedAlive: boolean;
    livenessTerminationReason: string;
    livenessWiped: boolean;
    staleMessageDrops: number;
    staleMessagePortResponseDropped: boolean;
    pushReachedAllTabs: boolean;
    ownerLockContentionEpochs: number[];
    engineReplacedEpochs: number[];
    protocolErrors: string[];
  };

  expect(report).toMatchObject({
    passed: true,
    openedTabs: 3,
    ownerEpochs: [1, 2, 3, 4],
    maxWorkersPerEpoch: 1,
    noEagerWorker: true,
    collidingRequestIdsRewritten: true,
    gracefulPreserved: true,
    abruptRejectedInflight: true,
    abruptWiped: true,
    workerOnlyPageStayedAlive: true,
    workerOnlyWiped: true,
    livenessPageStayedAlive: true,
    livenessTerminationReason: 'tab liveness lock was released',
    livenessWiped: true,
    staleMessageDrops: 1,
    staleMessagePortResponseDropped: true,
    pushReachedAllTabs: true,
    ownerLockContentionEpochs: [1, 2, 3, 4],
    engineReplacedEpochs: [2, 3, 4],
    protocolErrors: [],
  });
  expect(browserErrors).toEqual([]);
});

test('production cache-wasm Turso engine preserves graceful data and atomically recovers abrupt loss', async ({
  context,
  page,
}) => {
  const browserErrors: string[] = [];
  const watch = (candidate: typeof page): void => {
    candidate.on('console', (message) => {
      if (message.type() === 'error') browserErrors.push(message.text());
    });
    candidate.on('pageerror', (error) => browserErrors.push(error.message));
  };
  watch(page);
  context.on('page', watch);

  await page.goto('/production.html');
  const result = page.locator('#result');
  await expect(result).toHaveAttribute('data-status', 'passed', {
    timeout: 60_000,
  });
  const report = JSON.parse((await result.textContent()) ?? '') as {
    passed: boolean;
    realTursoDataPreservedGracefully: boolean;
    gracefulCloseReleasedOwnerLock: boolean;
    gracefulReplacementWaitedForPhysicalLock: boolean;
    gracefulPendingOwnerLockRequests: number;
    abruptInflightRejected: boolean;
    abruptRequestReplayCount: number;
    abruptOwnerPageStayedAlive: boolean;
    recoveryReplacementWaitedForPhysicalLock: boolean;
    recoveryPendingOwnerLockRequests: number;
    atomicRecoveryOpenWipedToMiss: boolean;
    recoveryDatabaseAction: string;
    ownerEpochs: number[];
    protocolErrors: string[];
  };

  expect(report).toEqual({
    passed: true,
    realTursoDataPreservedGracefully: true,
    gracefulCloseReleasedOwnerLock: true,
    gracefulReplacementWaitedForPhysicalLock: true,
    gracefulPendingOwnerLockRequests: 1,
    abruptInflightRejected: true,
    abruptRequestReplayCount: 1,
    abruptOwnerPageStayedAlive: true,
    recoveryReplacementWaitedForPhysicalLock: true,
    recoveryPendingOwnerLockRequests: 1,
    atomicRecoveryOpenWipedToMiss: true,
    recoveryDatabaseAction: 'wipe-before-open',
    ownerEpochs: [1, 2, 3],
    protocolErrors: [],
  });
  expect(browserErrors).toEqual([]);
});
