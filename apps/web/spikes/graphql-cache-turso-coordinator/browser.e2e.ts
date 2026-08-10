import { expect, test } from '@playwright/test';

test('three tabs fence page and worker-only owner loss', async ({
  context,
  page,
}) => {
  const browserErrors: string[] = [];
  const watchPage = (browserPage: typeof page): void => {
    browserPage.on('console', (message) => {
      if (message.type() === 'error') browserErrors.push(message.text());
    });
    browserPage.on('pageerror', (error) => browserErrors.push(error.message));
  };
  watchPage(page);
  context.on('page', watchPage);

  await page.goto('/?autorun=1');
  const result = page.locator('#result');
  await expect(result).toHaveAttribute('data-status', 'passed', {
    timeout: 35_000,
  });
  const resultText = await result.textContent();
  expect(resultText).not.toBeNull();
  const report = JSON.parse(resultText ?? '') as {
    passed: boolean;
    openedTabs: number;
    ownerEpochs: number[];
    gracefulPreservedFakeDb: boolean;
    abruptLossWipedFakeDbBeforeActivation: boolean;
    workerOnlyLossPageStayedAlive: boolean;
    workerOnlyLossWipedFakeDbBeforeActivation: boolean;
    failedEngineTerminationEpochs: number[];
    staleResponseViaMessagePortDrops: number;
    routerMaxActiveOwners: number;
    maxObservedHeldOwnerLocks: number;
    activeEpochsBlockingIndependentOwnerLockProbe: number[];
    finalObservedOwnerLockCount: number;
    wipeLifecycleBeforeReadyEpochs: number[];
    directMessageChannelObserved: boolean;
  };

  expect(report).toMatchObject({
    passed: true,
    openedTabs: 3,
    ownerEpochs: [1, 2, 3, 4],
    gracefulPreservedFakeDb: true,
    abruptLossWipedFakeDbBeforeActivation: true,
    workerOnlyLossPageStayedAlive: true,
    workerOnlyLossWipedFakeDbBeforeActivation: true,
    failedEngineTerminationEpochs: [3],
    routerMaxActiveOwners: 1,
    maxObservedHeldOwnerLocks: 1,
    activeEpochsBlockingIndependentOwnerLockProbe: [1, 2, 3, 4],
    finalObservedOwnerLockCount: 1,
    wipeLifecycleBeforeReadyEpochs: [3, 4],
    directMessageChannelObserved: true,
  });
  expect(report.staleResponseViaMessagePortDrops).toBeGreaterThanOrEqual(1);
  expect(browserErrors).toEqual([]);
});
