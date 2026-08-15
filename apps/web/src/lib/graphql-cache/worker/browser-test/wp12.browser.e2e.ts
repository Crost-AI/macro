import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { rm, writeFile } from 'node:fs/promises';
import { basename, resolve } from 'node:path';
import { expect, test } from '@playwright/test';
import {
  computeMeasuredSourceDigest,
  sha256File,
  type Wp12BrowserEvidence,
} from '../../../../../scripts/cache-wp12/report';

const harnessPath = (projectName: string): string =>
  projectName.includes('production') ? '/app/wp12.html' : '/wp12.html';

// Capture once before any evidence rewrite. The change ID remains stable while
// this revision is amended, and excludes an empty child working copy.
const measuredSourceDigestPromise = computeMeasuredSourceDigest();
const measuredRevisionChangeId = execFileSync(
  'jj',
  [
    'log',
    '-r',
    'latest(ancestors(@) & ~empty())',
    '--no-graph',
    '-T',
    'change_id',
  ],
  { encoding: 'utf8' }
).trim();

const isHit = (value: unknown): boolean =>
  typeof value === 'object' &&
  value !== null &&
  (value as { kind?: unknown }).kind === 'hit';

const isMiss = (value: unknown): boolean =>
  typeof value === 'object' &&
  value !== null &&
  (value as { kind?: unknown }).kind === 'miss';

test.describe.configure({ mode: 'serial' });

const cacheResources = (urls: string[]) => ({
  wasm: [
    ...new Set(
      urls.filter((url) => /cache_wasm_bg(?:-[\w-]+)?\.wasm(?:\?|$)/.test(url))
    ),
  ],
  engine: [
    ...new Set(urls.filter((url) => /cache\.engine-worker[^/]*\.js/.test(url))),
  ],
  coordinator: [
    ...new Set(
      urls.filter((url) =>
        /cache\.coordinator\.shared-worker[^/]*\.js/.test(url)
      )
    ),
  ],
});

const REQUIRED_EVIDENCE_TESTS = [
  'lifecycle',
  'real-standby',
  'logout',
  'production-default-off',
  'posthog-treatment',
  'mutating-termination',
  'recovery-test-artifact',
] as const;
type RequiredEvidenceTest = (typeof REQUIRED_EVIDENCE_TESTS)[number];
type Scenario = keyof Wp12BrowserEvidence['scenarios'];
type EvidenceState = {
  completedTests: Set<RequiredEvidenceTest>;
  scenarios: Partial<Record<Scenario, true>>;
  productionResourceUrls: Set<string>;
  navigation?: { controlDurationMs: number; treatmentDurationMs: number };
  artifact?: {
    origin: Wp12BrowserEvidence['origin'];
    userAgent: string;
    wasmUrl: string;
    wasmSha256: string;
  };
  recoveryHookUrls: Set<string>;
  recoveryHookSha256?: string;
};
const evidenceStates = new Map<string, EvidenceState>();
const evidenceState = (project: string): EvidenceState => {
  let state = evidenceStates.get(project);
  if (!state) {
    state = {
      completedTests: new Set(),
      scenarios: {},
      productionResourceUrls: new Set(),
      recoveryHookUrls: new Set(),
    };
    evidenceStates.set(project, state);
  }
  return state;
};
const completeEvidenceTest = (
  project: string,
  testId: RequiredEvidenceTest,
  scenarios: readonly Scenario[]
): void => {
  const state = evidenceState(project);
  state.completedTests.add(testId);
  for (const scenario of scenarios) state.scenarios[scenario] = true;
};
const evidenceOutput = (project: string): string =>
  resolve(
    import.meta.dirname,
    `../../../../../measurements/generated/cache-wasm-wp12-${project}.json`
  );

test('WP-12 exact host stays navigation-lazy, preserves offline handoff, resets identity, and wipes abrupt loss', async ({
  context,
  page,
}, testInfo) => {
  const requestedUrls: string[] = [];
  const browserErrors: string[] = [];
  const failedResponses: string[] = [];
  const failedRequests: string[] = [];
  context.on('request', (request) => requestedUrls.push(request.url()));
  context.on('requestfailed', (request) => {
    failedRequests.push(
      `${request.failure()?.errorText ?? 'failed'} ${request.url()}`
    );
  });
  context.on('response', (response) => {
    if (response.status() >= 400) {
      failedResponses.push(`${response.status()} ${response.url()}`);
    }
  });
  page.on('console', (message) => {
    if (message.type() === 'error') {
      browserErrors.push(
        `${message.text()} @ ${message.location().url || 'unknown'}`
      );
    }
  });
  page.on('pageerror', (error) => browserErrors.push(error.message));

  const path = harnessPath(testInfo.project.name);
  await page.goto(path);
  await expect(page.locator('#result')).toHaveAttribute('data-status', 'ready');
  await expect(page.locator('#result')).toHaveAttribute(
    'data-rollout',
    'control'
  );
  expect(
    await page.evaluate(() => ({
      mode: window.wp12CacheHarness.rolloutMode(),
      hostCount: window.wp12CacheHarness.hostConstructionCount(),
      workerCount: window.wp12CacheHarness.constructedWorkerUrls().length,
    }))
  ).toEqual({ mode: 'control', hostCount: 0, workerCount: 0 });
  const controlDurationMs = await page.evaluate(() =>
    window.wp12CacheHarness.navigationDurationMs()
  );
  expect(cacheResources(requestedUrls)).toEqual({
    wasm: [],
    engine: [],
    coordinator: [],
  });

  const scope = `wp12-playwright-${crypto.randomUUID()}`;
  await page.goto(`${path}?treatment=true&scope=${encodeURIComponent(scope)}`);
  await expect(page.locator('#result')).toHaveAttribute('data-status', 'ready');
  await expect(page.locator('#result')).toHaveAttribute(
    'data-rollout',
    'treatment'
  );
  expect(
    await page.evaluate(() => window.wp12CacheHarness.hostConstructionCount())
  ).toBe(0);
  const treatmentDurationMs = await page.evaluate(() =>
    window.wp12CacheHarness.navigationDurationMs()
  );
  const beforeFirstUse = cacheResources([
    ...requestedUrls,
    ...(await page.evaluate(() =>
      window.wp12CacheHarness.constructedWorkerUrls()
    )),
  ]);
  expect(beforeFirstUse.wasm).toEqual([]);
  expect(beforeFirstUse.engine).toEqual([]);
  expect(beforeFirstUse.coordinator).toEqual([]);

  await page.evaluate(() => window.wp12CacheHarness.start());
  await expect
    .poll(() => cacheResources(requestedUrls).wasm.length, { timeout: 30_000 })
    .toBe(1);
  const afterFirstUse = cacheResources([
    ...requestedUrls,
    ...(await page.evaluate(() =>
      window.wp12CacheHarness.constructedWorkerUrls()
    )),
  ]);
  expect(afterFirstUse.engine).toHaveLength(1);
  expect(afterFirstUse.coordinator).toHaveLength(1);

  await page.evaluate(() =>
    window.wp12CacheHarness.write('offline-preserved', 'wp12-offline-user')
  );
  const standbyClose = await page.evaluate(() =>
    window.wp12CacheHarness.closeSamePageStandbyHost()
  );
  expect(isHit(standbyClose.ownerRead)).toBe(true);
  expect(standbyClose.engineWorkerCount).toBe(1);
  await page.evaluate(() => window.wp12CacheHarness.startStandby());
  await context.setOffline(true);
  const offlineHandoff = await page.evaluate(() =>
    window.wp12CacheHarness.cleanOwnerHandoff()
  );
  expect(isHit(offlineHandoff)).toBe(true);
  await context.setOffline(false);

  const identity = await page.evaluate(() =>
    window.wp12CacheHarness.identityReset()
  );
  expect(isMiss(identity.old)).toBe(true);
  expect(isHit(identity.current)).toBe(true);

  const abrupt = await page.evaluate(() =>
    window.wp12CacheHarness.abruptOwnerLoss()
  );
  expect(abrupt.oldRequestRejected).toBe(true);
  expect(isMiss(abrupt.replacement)).toBe(true);

  const isolation = await page.evaluate(() => ({
    crossOriginIsolated: globalThis.crossOriginIsolated,
    sharedArrayBufferCreated:
      typeof globalThis.SharedArrayBuffer === 'function' &&
      globalThis.crossOriginIsolated,
  }));
  expect(isolation).toEqual({
    crossOriginIsolated: false,
    sharedArrayBufferCreated: false,
  });
  expect({ browserErrors, failedResponses, failedRequests }).toEqual({
    browserErrors: [],
    failedResponses: [],
    failedRequests: [],
  });

  const resources = cacheResources([
    ...requestedUrls,
    ...(await page.evaluate(() =>
      window.wp12CacheHarness.constructedWorkerUrls()
    )),
  ]);
  const mode = testInfo.project.name.includes('production')
    ? 'production'
    : 'development';
  if (mode === 'production') {
    expect(resources.wasm).toHaveLength(1);
    expect(basename(new URL(resources.wasm[0]!).pathname)).toMatch(
      /^cache_wasm_bg-[\w-]+\.wasm$/
    );
    expect(resources.engine.every((url) => url.includes('/app/'))).toBe(true);
    expect(resources.coordinator.every((url) => url.includes('/app/'))).toBe(
      true
    );
  }

  if (mode === 'production') {
    const state = evidenceState(testInfo.project.name);
    for (const url of [
      ...resources.wasm,
      ...resources.engine,
      ...resources.coordinator,
    ]) {
      state.productionResourceUrls.add(url);
    }
    state.navigation = { controlDurationMs, treatmentDurationMs };
    if (process.env.CACHE_WP12_WRITE_MEASUREMENTS === 'true') {
      const wasmUrl = resources.wasm[0];
      if (!wasmUrl) throw new Error('missing WP-12 production WASM URL');
      const wasmResponse = await context.request.get(wasmUrl);
      expect(wasmResponse.ok()).toBe(true);
      state.artifact = {
        origin: new URL(page.url()).origin as Wp12BrowserEvidence['origin'],
        userAgent: await page.evaluate(() => navigator.userAgent),
        wasmUrl,
        wasmSha256: createHash('sha256')
          .update(await wasmResponse.body())
          .digest('hex'),
      };
    }
  }

  await page.evaluate(() => window.wp12CacheHarness.dispose());
  completeEvidenceTest(testInfo.project.name, 'lifecycle', [
    'harnessControlLazyResourceEvidence',
    'treatmentLazyResources',
    'navigationResourceGate',
    'samePageStandbyHostCloseOwnerStable',
    'cleanOwnerRestartOfflineHit',
    'identityChangeReset',
    'gracefulPreserve',
    'abruptLossWipe',
    'crossOriginIsolationNotRequired',
    'sharedArrayBufferNotCreated',
  ]);
});

test('WP-12 real standby tab close preserves the same active engine', async ({
  context,
  page,
}, testInfo) => {
  test.skip(
    !testInfo.project.name.includes('production'),
    'WP-12 rollout evidence requires the production bundle'
  );
  const scope = `wp12-standby-tab-${crypto.randomUUID()}`;
  const path = `${harnessPath(testInfo.project.name)}?treatment=true&scope=${encodeURIComponent(scope)}`;
  await page.goto(path);
  await expect(page.locator('#result')).toHaveAttribute('data-status', 'ready');
  await page.evaluate(() => window.wp12CacheHarness.startSingle());
  expect(
    await page.evaluate(() => window.wp12CacheHarness.engineWorkerCount())
  ).toBe(1);
  const ownerWorkerUrls = await page.evaluate(() =>
    window.wp12CacheHarness.constructedWorkerUrls()
  );
  await page.evaluate(() =>
    window.wp12CacheHarness.write('real-standby-preserved')
  );

  const standbyPage = await context.newPage();
  await standbyPage.goto(path);
  await expect(standbyPage.locator('#result')).toHaveAttribute(
    'data-status',
    'ready'
  );
  await standbyPage.evaluate(() => window.wp12CacheHarness.startSingle());
  expect(
    await standbyPage.evaluate(() =>
      window.wp12CacheHarness.engineWorkerCount()
    )
  ).toBe(0);
  expect(
    await standbyPage.evaluate(() => window.wp12CacheHarness.read())
  ).toMatchObject({
    kind: 'hit',
    data: {
      user: {
        soup: { items: [{ id: 'real-standby-preserved' }] },
      },
    },
  });

  await standbyPage.close();
  await page.evaluate(() =>
    window.wp12CacheHarness.write('owner-after-real-standby-close')
  );
  expect(
    await page.evaluate(() => window.wp12CacheHarness.read())
  ).toMatchObject({
    kind: 'hit',
    data: {
      user: {
        soup: { items: [{ id: 'owner-after-real-standby-close' }] },
      },
    },
  });
  expect(
    await page.evaluate(() => window.wp12CacheHarness.engineWorkerCount())
  ).toBe(1);
  expect(
    await page.evaluate(() => window.wp12CacheHarness.constructedWorkerUrls())
  ).toEqual(ownerWorkerUrls);
  await page.evaluate(() => window.wp12CacheHarness.dispose());
  completeEvidenceTest(testInfo.project.name, 'real-standby', [
    'realStandbyTabCloseOwnerStable',
  ]);
});

test('WP-12 actual logout cache lifecycle wipes the registered production host', async ({
  page,
}, testInfo) => {
  test.skip(
    !testInfo.project.name.includes('production'),
    'WP-12 rollout evidence requires the production bundle'
  );
  const scope = `wp12-logout-${crypto.randomUUID()}`;
  const path = `${harnessPath(testInfo.project.name)}?treatment=true&scope=${encodeURIComponent(scope)}`;
  await page.goto(path);
  await expect(page.locator('#result')).toHaveAttribute('data-status', 'ready');
  await page.evaluate(() => window.wp12CacheHarness.startLogoutHost());
  await page.evaluate(() => window.wp12CacheHarness.write('logout-must-wipe'));
  expect(isHit(await page.evaluate(() => window.wp12CacheHarness.read()))).toBe(
    true
  );
  expect(
    isMiss(await page.evaluate(() => window.wp12CacheHarness.logoutReset()))
  ).toBe(true);
  await page.evaluate(() => window.wp12CacheHarness.write('post-logout'));
  expect(isHit(await page.evaluate(() => window.wp12CacheHarness.read()))).toBe(
    true
  );
  await page.evaluate(() => window.wp12CacheHarness.dispose());
  completeEvidenceTest(testInfo.project.name, 'logout', ['realLogoutReset']);
});

test('WP-12 actual GraphQL Soup selector stays default-off without browser cache resources', async ({
  page,
}, testInfo) => {
  test.skip(
    !testInfo.project.name.includes('production'),
    'WP-12 rollout evidence requires the production bundle'
  );
  const requestedUrls: string[] = [];
  page.on('request', (request) => requestedUrls.push(request.url()));
  await page.goto(
    testInfo.project.name.includes('production')
      ? '/app/wp12-graphql-soup.html'
      : '/wp12-graphql-soup.html'
  );
  await expect(page.locator('#result')).toHaveAttribute('data-status', 'ready');
  const result = await page.evaluate(() =>
    window.wp12GraphqlSoupHarness.resolveDefaultOff()
  );
  expect(result).toEqual({
    cacheEnabled: false,
    cacheHostPresent: false,
    resources: { workerUrls: [], sharedWorkerUrls: [] },
  });
  expect(cacheResources(requestedUrls)).toEqual({
    wasm: [],
    engine: [],
    coordinator: [],
  });
  completeEvidenceTest(testInfo.project.name, 'production-default-off', [
    'productionGraphqlSoupDefaultOffNoResources',
  ]);
});

test('WP-12 actual PostHog treatment override is lazy and kill applies on navigation', async ({
  page,
}, testInfo) => {
  test.skip(
    !testInfo.project.name.includes('production'),
    'WP-12 rollout evidence requires the production bundle'
  );
  const requestedUrls: string[] = [];
  page.on('request', (request) => requestedUrls.push(request.url()));
  const posthogRequests = () =>
    requestedUrls.filter((url) => {
      const parsed = new URL(url);
      return (
        parsed.hostname.includes('posthog') ||
        parsed.pathname.includes('/__wp12-posthog-disabled') ||
        parsed.pathname.includes('/i/ph/')
      );
    });
  const path = '/app/wp12-graphql-soup.html';
  await page.goto(path);
  await expect(page.locator('#result')).toHaveAttribute('data-status', 'ready');
  const treatment = await page.evaluate(() =>
    window.wp12GraphqlSoupHarness.tryTreatment()
  );
  expect(
    treatment.overrideApplied,
    'the required supported PostHog override must be available'
  ).toBe(true);
  if (!treatment.overrideApplied) {
    throw new Error('required PostHog treatment override was unavailable');
  }

  expect(treatment.lazyBeforeRead).toBe(true);
  expect(treatment.readKind).toBe('miss');
  expect(treatment.samePageKillLatched).toBe(true);
  expect(treatment.posthogBeforeSendCalls).toBe(0);
  expect(treatment.resources.workerUrls).toHaveLength(1);
  expect(treatment.resources.workerUrls[0]).toMatch(
    /cache\.engine-worker-[A-Za-z0-9_-]+\.js$/
  );
  expect(treatment.resources.sharedWorkerUrls).toHaveLength(1);
  expect(treatment.resources.sharedWorkerUrls[0]).toMatch(
    /cache\.coordinator\.shared-worker-[A-Za-z0-9_-]+\.js$/
  );
  expect(cacheResources(requestedUrls).wasm).toHaveLength(1);
  expect(posthogRequests()).toEqual([]);

  const requestCountBeforeNavigation = requestedUrls.length;
  await page.goto(path);
  await expect(page.locator('#result')).toHaveAttribute('data-status', 'ready');
  const afterNavigation = await page.evaluate(() =>
    window.wp12GraphqlSoupHarness.resolveAfterNavigationKill()
  );
  expect(afterNavigation).toEqual({
    cacheEnabled: false,
    cacheHostPresent: false,
    posthogBeforeSendCalls: 0,
    resources: { workerUrls: [], sharedWorkerUrls: [] },
  });
  expect(
    cacheResources(requestedUrls.slice(requestCountBeforeNavigation))
  ).toEqual({ wasm: [], engine: [], coordinator: [] });
  expect(posthogRequests()).toEqual([]);
  completeEvidenceTest(testInfo.project.name, 'posthog-treatment', [
    'posthogTreatmentOverrideExactResources',
    'nextNavigationEmergencyDisableNoResources',
    'posthogNetworkSuppressed',
  ]);
});

test('WP-12 terminates each admitted mutating RPC before core without replay', async ({
  context,
  page,
}, testInfo) => {
  test.skip(
    !testInfo.project.name.includes('production'),
    'WP-12 fault evidence requires the production bundle'
  );
  testInfo.setTimeout(360_000);
  const requestedUrls: string[] = [];
  context.on('request', (request) => requestedUrls.push(request.url()));
  await page.goto('/app/wp12-internal.html');
  await expect(page.locator('#result')).toHaveAttribute('data-status', 'ready');
  const kinds = await page.evaluate(
    () => window.wp12InternalHarness.mutatingRequestKinds
  );
  expect(kinds).toEqual([
    'write',
    'enqueue-optimistic-mutation',
    'claim-next-mutation',
    'defer-optimistic-write',
    'commit-optimistic-write',
    'rollback-optimistic-write',
    'invalidate',
    'delete-records',
    'clear',
  ]);
  const results = [];
  for (const kind of kinds) {
    results.push(
      await page.evaluate(
        async (requestKind) =>
          await window.wp12InternalHarness.runFaultKind(requestKind),
        kind
      )
    );
  }
  for (const result of results) {
    expect(result).toMatchObject({
      actualDedicatedWorkerTerminated: true,
      requestAdmittedBeforeCore: true,
      midSqlExecutionClaimed: false,
      oldRequestRejected: true,
      replacementRecordsEmpty: true,
      replacementQueueEmpty: true,
      mutationAdmissionCount: 1,
      unexpectedReplacementMutatingAdmissionCount: 0,
      replacementAdmissionBarrierObserved: true,
      resetPhaseSequence: [
        'graphql_cache.storage_reset_required',
        'graphql_cache.logical_reset',
        'graphql_cache.reset_wipe',
      ],
      exactProductionTursoWasm: true,
      queueTelemetryObserved: true,
      performanceMemoryAvailable: false,
      userAgentSpecificMemoryAvailable: false,
    });
  }
  expect(cacheResources(requestedUrls).wasm).toHaveLength(1);
  expect(cacheResources(requestedUrls).engine).toHaveLength(1);
  completeEvidenceTest(testInfo.project.name, 'mutating-termination', [
    'admittedMutatingRpcTerminationWipesNoReplay',
    'queueDiagnosticsTelemetry',
    'workerMemoryApiUnavailableWithoutIsolation',
  ]);
});

test('WP-12 lock-safe incompatible namespace and corrupt queue payload recover exactly once', async ({
  context,
  page,
}, testInfo) => {
  test.skip(
    !testInfo.project.name.includes('production'),
    'WP-12 recovery evidence requires the production bundle'
  );
  testInfo.setTimeout(180_000);
  const requestedUrls: string[] = [];
  context.on('request', (request) => requestedUrls.push(request.url()));
  await page.goto('/app/wp12-internal.html');
  await expect(page.locator('#result')).toHaveAttribute('data-status', 'ready');
  const recoveryHookUrls = new Set<string>();
  for (const [kind, openOutcome] of [
    ['incompatible-namespace', 'reset-incompatible'],
    ['corrupt-queue-payload', 'reset-corrupt'],
  ] as const) {
    const result = await page.evaluate(
      async (recoveryKind) =>
        await window.wp12InternalHarness.runRecoveryKind(recoveryKind),
      kind
    );
    expect(result).toMatchObject({
      kind,
      gracefulCloseBeforeMutation: true,
      separateFeatureGatedBrowserTestArtifact: true,
      productionArtifactDebugExportsAbsent: true,
      browserTestWasmUrl: expect.stringMatching(
        /cache_wasm_browser_test_hooks_bg-[A-Za-z0-9_-]+\.wasm$/
      ),
      browserTestWorkerOnlyControl: true,
      productionCoordinatorProtocolUnchanged: true,
      openOutcome,
      recordsWiped: true,
      durableQueueWiped: true,
      usableAfterReset: true,
      storageResetRequiredCount: 1,
      logicalResetCount: 1,
      resetWipeCount: 1,
      queueTelemetryObserved: true,
    });
    recoveryHookUrls.add(result.browserTestWasmUrl);
  }
  expect(recoveryHookUrls.size).toBe(1);
  const [recoveryHookUrl] = [...recoveryHookUrls];
  if (!recoveryHookUrl) throw new Error('missing recovery-hook WASM URL');
  const hookResponse = await context.request.get(recoveryHookUrl);
  expect(hookResponse.ok()).toBe(true);
  const state = evidenceState(testInfo.project.name);
  state.recoveryHookUrls.add(recoveryHookUrl);
  state.recoveryHookSha256 = createHash('sha256')
    .update(await hookResponse.body())
    .digest('hex');
  expect(cacheResources(requestedUrls).wasm).toHaveLength(1);
  expect(cacheResources(requestedUrls).engine).toHaveLength(1);
  completeEvidenceTest(testInfo.project.name, 'recovery-test-artifact', [
    'incompatibleNamespaceRecovery',
    'corruptQueuePayloadRecovery',
    'initializationRecoveryOutcomeTelemetry',
  ]);
});

test.afterAll(async ({ browser }, testInfo) => {
  if (
    !testInfo.project.name.includes('production') ||
    process.env.CACHE_WP12_WRITE_MEASUREMENTS !== 'true'
  ) {
    return;
  }
  const output = evidenceOutput(testInfo.project.name);
  const state = evidenceState(testInfo.project.name);
  const missing = REQUIRED_EVIDENCE_TESTS.filter(
    (testId) => !state.completedTests.has(testId)
  );
  if (missing.length > 0) {
    await rm(output, { force: true });
    throw new Error(
      `refusing partial WP-12 evidence; required tests did not pass: ${missing.join(', ')}`
    );
  }
  if (
    !state.artifact ||
    !state.navigation ||
    !state.recoveryHookSha256 ||
    state.recoveryHookUrls.size !== 1
  ) {
    await rm(output, { force: true });
    throw new Error('refusing incomplete finalizer artifact evidence');
  }

  const family = testInfo.project.name.startsWith('firefox')
    ? 'firefox'
    : 'chromium';
  const executablePath =
    family === 'chromium'
      ? process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH
      : process.env.PLAYWRIGHT_FIREFOX_EXECUTABLE_PATH;
  if (!executablePath) throw new Error('browser executable path is required');
  const productionResources = cacheResources([...state.productionResourceUrls]);
  if (
    productionResources.wasm.length !== 1 ||
    productionResources.engine.length !== 1 ||
    productionResources.coordinator.length !== 1
  ) {
    throw new Error(
      'finalizer requires one exact production resource of each kind'
    );
  }
  const wasmBasename = basename(new URL(state.artifact.wasmUrl).pathname);
  const recoveryHookUrl = [...state.recoveryHookUrls][0]!;
  const recoveryHookBasename = basename(new URL(recoveryHookUrl).pathname);
  const evidence: Wp12BrowserEvidence = {
    schemaVersion: 5,
    measuredAt: new Date().toISOString(),
    measuredRevisionChangeId,
    measuredSourceDigest: await measuredSourceDigestPromise,
    source: 'local-playwright-finalizer',
    project: `${family}-production`,
    mode: 'production',
    finalizer: {
      kind: 'playwright-after-all',
      requiredTests: REQUIRED_EVIDENCE_TESTS,
      completedTests: REQUIRED_EVIDENCE_TESTS,
    },
    runner: {
      playwrightVersion: testInfo.config.version,
      executablePath,
      executableSha256: await sha256File(executablePath),
    },
    browser: {
      family,
      executableVersion: browser.version(),
      userAgent: state.artifact.userAgent,
    },
    origin: state.artifact.origin,
    exactProductionHost: true,
    exactProductionWorker: true,
    liveS3CloudFrontVerified: false,
    scenarios: state.scenarios as Wp12BrowserEvidence['scenarios'],
    resources: {
      wasm: {
        basename: wasmBasename,
        url: state.artifact.wasmUrl,
        expectedProductionUrl: `${state.artifact.origin}/app/assets/${wasmBasename}`,
        sha256: state.artifact.wasmSha256,
      },
      engineWorkerUrls: productionResources.engine,
      coordinatorWorkerUrls: productionResources.coordinator,
    },
    testArtifacts: {
      recoveryHooks: {
        classification: 'real-browser-test-artifact',
        productionArtifact: false,
        cargoFeature: 'browser-test-hooks',
        wasm: {
          basename: recoveryHookBasename,
          url: recoveryHookUrl,
          sha256: state.recoveryHookSha256,
        },
      },
    },
    navigation: state.navigation,
  };
  await writeFile(output, `${JSON.stringify(evidence, null, 2)}\n`);
});

test('WP-12 Chromium storage eviction reopens empty without fabricated Firefox coverage', async ({
  context,
  page,
}, testInfo) => {
  test.skip(
    !testInfo.project.name.startsWith('chromium'),
    'Chromium CDP is the only local storage-eviction control available'
  );
  const path = harnessPath(testInfo.project.name);
  const scope = `wp12-eviction-${crypto.randomUUID()}`;
  await page.goto(`${path}?treatment=true&scope=${encodeURIComponent(scope)}`);
  await expect(page.locator('#result')).toHaveAttribute('data-status', 'ready');
  await page.evaluate(async () => {
    await window.wp12CacheHarness.start();
    await window.wp12CacheHarness.write('evict-me', 'wp12-eviction-user');
    window.wp12CacheHarness.dispose();
  });
  await page.waitForTimeout(1_000);

  const cdp = await context.newCDPSession(page);
  await cdp.send('Storage.clearDataForOrigin', {
    origin: new URL(page.url()).origin,
    storageTypes: 'file_systems',
  });
  const reopened = await page.evaluate(async () => {
    await window.wp12CacheHarness.start();
    return await window.wp12CacheHarness.read();
  });
  expect(isMiss(reopened)).toBe(true);
  await page.evaluate(() => window.wp12CacheHarness.dispose());
});
