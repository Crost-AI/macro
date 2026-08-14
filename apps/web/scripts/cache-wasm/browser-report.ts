export type CacheWasmBrowser = 'chromium' | 'firefox';
export type CacheWasmBuildMode = 'development' | 'production';

export interface CacheWasmBrowserSample {
  browser: CacheWasmBrowser;
  mode: CacheWasmBuildMode;
  activationMs: number;
  browserReadyMs: number;
  hostFirstReadyMs: number;
  workerActivationMs: number;
  linearMemoryBytes: number;
  sharedWorkerConstructions: number;
  dedicatedWorkerConstructions: number;
  nestedWorkerConstructions: number;
  wasmFetchCount: number;
  wasmSha256: string;
  ownerEpochs: number[];
  crossOriginIsolated: boolean;
  sharedArrayBufferAvailable: boolean;
  sharedWorkerUrl: string;
  productionEngineUrl: string;
  instrumentedEngineUrl: string;
  wasmUrl: string;
}

export interface CacheWasmBrowserReport {
  schemaVersion: 1;
  project: string;
  browser: CacheWasmBrowser;
  mode: CacheWasmBuildMode;
  origin: string;
  browserVersion: string;
  executablePath: string;
  userAgent: string;
  freshScopeCount: 5;
  samples: CacheWasmBrowserSample[];
  p95: {
    activationMs: number;
    browserReadyMs: number;
    hostFirstReadyMs: number;
    workerActivationMs: number;
    linearMemoryBytes: number;
  };
  assetUrls: {
    sharedWorkers: string[];
    productionEngines: string[];
    instrumentedEngines: string[];
    wasm: string[];
  };
  sourceMapUrls: string[];
  delivery: {
    basePath: '/' | '/app/';
    wasmUrl: string;
    wasmContentType: 'application/wasm';
    wasmContentEncoding: null | 'br';
    wasmCompilationSucceeded: true;
    wasmSha256: string;
    localPrecompressedOrigin: boolean;
    liveS3CloudFrontVerified: false;
  };
}

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value);

const finiteNonNegative = (value: unknown): value is number =>
  typeof value === 'number' && Number.isFinite(value) && value >= 0;

const validSha256 = (value: unknown): value is string =>
  typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);

export function percentile95(values: readonly number[]): number {
  if (values.length === 0) throw new Error('cannot compute p95 of no samples');
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.ceil(sorted.length * 0.95) - 1];
}

const uniqueSorted = (values: readonly string[]): string[] =>
  [...new Set(values)].sort();

function equalStrings(left: unknown, right: readonly string[]): boolean {
  return (
    Array.isArray(left) &&
    left.every((value) => typeof value === 'string') &&
    JSON.stringify([...left].sort()) === JSON.stringify([...right].sort())
  );
}

function validSameOriginUrl(value: unknown, origin: string): value is string {
  if (typeof value !== 'string' || value.length === 0) return false;
  try {
    return new URL(value).origin === origin;
  } catch {
    return false;
  }
}

export function browserReportViolations(
  value: unknown,
  expected: {
    browser: CacheWasmBrowser;
    mode: CacheWasmBuildMode;
    project: string;
  }
): string[] {
  const violations: string[] = [];
  if (!isRecord(value)) return ['browser report must be an object'];
  if (value.schemaVersion !== 1) violations.push('schemaVersion must be 1');
  if (value.project !== expected.project)
    violations.push(`project must be ${expected.project}`);
  if (value.browser !== expected.browser)
    violations.push(`browser must be ${expected.browser}`);
  if (value.mode !== expected.mode)
    violations.push(`mode must be ${expected.mode}`);
  if (
    typeof value.origin !== 'string' ||
    !/^http:\/\/127\.0\.0\.1:\d+$/.test(value.origin)
  ) {
    violations.push('origin must be an explicit local 127.0.0.1 origin');
  }
  const origin = typeof value.origin === 'string' ? value.origin : '';
  for (const field of ['browserVersion', 'executablePath', 'userAgent'] as const) {
    if (typeof value[field] !== 'string' || value[field].length === 0) {
      violations.push(`${field} must be a non-empty string`);
    }
  }
  if (value.freshScopeCount !== 5)
    violations.push('freshScopeCount must be exactly 5');
  if (!Array.isArray(value.samples) || value.samples.length !== 5) {
    violations.push('samples must contain exactly 5 raw samples');
    return violations;
  }

  const samples = value.samples;
  const metricNames = [
    'activationMs',
    'browserReadyMs',
    'hostFirstReadyMs',
    'workerActivationMs',
    'linearMemoryBytes',
  ] as const;
  const urlNames = [
    'sharedWorkerUrl',
    'productionEngineUrl',
    'instrumentedEngineUrl',
    'wasmUrl',
  ] as const;
  for (const [index, sampleValue] of samples.entries()) {
    if (!isRecord(sampleValue)) {
      violations.push(`sample ${index} must be an object`);
      continue;
    }
    if (sampleValue.browser !== expected.browser)
      violations.push(`sample ${index} browser must be ${expected.browser}`);
    if (sampleValue.mode !== expected.mode)
      violations.push(`sample ${index} mode must be ${expected.mode}`);
    for (const metric of metricNames) {
      if (!finiteNonNegative(sampleValue[metric]))
        violations.push(`sample ${index} ${metric} must be finite and non-negative`);
    }
    if (sampleValue.sharedWorkerConstructions !== 1)
      violations.push(`sample ${index} must construct one coordinator owner`);
    if (sampleValue.dedicatedWorkerConstructions !== 1)
      violations.push(`sample ${index} must construct one engine`);
    if (sampleValue.nestedWorkerConstructions !== 0)
      violations.push(`sample ${index} constructed a nested worker`);
    if (sampleValue.wasmFetchCount !== 1)
      violations.push(`sample ${index} must fetch exactly one cache WASM`);
    if (!validSha256(sampleValue.wasmSha256))
      violations.push(`sample ${index} WASM SHA-256 is invalid`);
    if (
      !Array.isArray(sampleValue.ownerEpochs) ||
      sampleValue.ownerEpochs.length !== 1 ||
      sampleValue.ownerEpochs[0] !== 1
    ) {
      violations.push(`sample ${index} must observe only owner epoch 1`);
    }
    if (sampleValue.crossOriginIsolated !== false)
      violations.push(`sample ${index} must not be cross-origin isolated`);
    if (sampleValue.sharedArrayBufferAvailable !== false)
      violations.push(`sample ${index} must not expose SharedArrayBuffer`);
    for (const urlName of urlNames) {
      if (!validSameOriginUrl(sampleValue[urlName], origin))
        violations.push(`sample ${index} ${urlName} must be a same-origin URL`);
    }
    if (
      sampleValue.productionEngineUrl === sampleValue.instrumentedEngineUrl
    ) {
      violations.push(
        `sample ${index} must distinguish production and instrumented engine URLs`
      );
    }
    const paths = Object.fromEntries(
      urlNames.map((name) => {
        try {
          return [name, new URL(String(sampleValue[name])).pathname];
        } catch {
          return [name, ''];
        }
      })
    );
    const expectedPaths =
      expected.mode === 'production'
        ? {
            sharedWorkerUrl:
              /^\/app\/assets\/cache\.coordinator\.shared-worker-[\w-]+\.js$/,
            productionEngineUrl:
              /^\/app\/assets\/cache\.engine-worker-[\w-]+\.js$/,
            instrumentedEngineUrl:
              /^\/app\/assets\/measurement-cache\.engine-worker-[\w-]+\.js$/,
            wasmUrl:
              /^\/app\/assets\/cache_wasm_bg-[A-Za-z0-9_-]{8}\.wasm$/,
          }
        : {
            sharedWorkerUrl:
              /\/graphql-cache\/worker\/cache\.coordinator\.shared-worker\.ts$/,
            productionEngineUrl:
              /\/graphql-cache\/worker\/cache\.engine-worker\.ts$/,
            instrumentedEngineUrl: /\/measurement-cache\.engine-worker\.ts$/,
            wasmUrl: /\/graphql-cache\/wasm\/cache_wasm_bg\.wasm$/,
          };
    for (const name of urlNames) {
      if (!expectedPaths[name].test(String(paths[name]))) {
        violations.push(`sample ${index} ${name} has an unexpected asset path`);
      }
    }
  }

  const typedSamples = samples.filter(isRecord) as Array<
    Record<
      (typeof metricNames)[number] | (typeof urlNames)[number] | 'wasmSha256',
      unknown
    >
  >;
  if (typedSamples.length !== 5) return violations;
  if (!isRecord(value.p95)) {
    violations.push('p95 must be an object');
  } else {
    for (const metric of metricNames) {
      const raw = typedSamples.map((sample) => sample[metric]);
      if (!raw.every(finiteNonNegative)) continue;
      const recomputed = percentile95(raw);
      if (value.p95[metric] !== recomputed) {
        violations.push(`${metric} p95 does not match the five raw samples`);
      }
    }
  }

  const wasmHashes = uniqueSorted(
    typedSamples.map((sample) => String(sample.wasmSha256))
  );
  if (wasmHashes.length !== 1 || !validSha256(wasmHashes[0])) {
    violations.push('all five samples must report one identical WASM SHA-256');
  }

  const expectedAssets = {
    sharedWorkers: uniqueSorted(
      typedSamples.map((sample) => String(sample.sharedWorkerUrl))
    ),
    productionEngines: uniqueSorted(
      typedSamples.map((sample) => String(sample.productionEngineUrl))
    ),
    instrumentedEngines: uniqueSorted(
      typedSamples.map((sample) => String(sample.instrumentedEngineUrl))
    ),
    wasm: uniqueSorted(typedSamples.map((sample) => String(sample.wasmUrl))),
  };
  if (!isRecord(value.assetUrls)) {
    violations.push('assetUrls must be an object');
  } else {
    for (const [kind, urls] of Object.entries(expectedAssets)) {
      if (urls.length !== 1)
        violations.push(`${kind} must resolve to exactly one build URL`);
      if (!equalStrings(value.assetUrls[kind], urls))
        violations.push(`${kind} does not match raw sample URLs`);
    }
  }

  const production = expected.mode === 'production';
  const expectedPrefix = production ? '/app/assets/' : '/';
  for (const urls of Object.values(expectedAssets)) {
    for (const url of urls) {
      try {
        if (!new URL(url).pathname.startsWith(expectedPrefix)) {
          violations.push(`${url} does not use expected ${expectedPrefix} base`);
        }
      } catch {
        // The sample-level URL violation is already precise.
      }
    }
  }
  if (!Array.isArray(value.sourceMapUrls)) {
    violations.push('sourceMapUrls must be an array');
  } else if (production) {
    const expectedSourceMaps = [
      ...expectedAssets.sharedWorkers,
      ...expectedAssets.productionEngines,
      ...expectedAssets.instrumentedEngines,
    ].map((url) => `${url}.map`);
    if (
      !equalStrings(value.sourceMapUrls, expectedSourceMaps) ||
      !value.sourceMapUrls.every((url) => validSameOriginUrl(url, origin))
    ) {
      violations.push(
        'production source maps must match all three fetched worker URLs'
      );
    }
  } else if (value.sourceMapUrls.length !== 0) {
    violations.push('development must not report production source maps');
  }

  if (!isRecord(value.delivery)) {
    violations.push('delivery must be an object');
  } else {
    if (value.delivery.basePath !== (production ? '/app/' : '/'))
      violations.push('delivery basePath does not match mode');
    if (
      value.delivery.wasmUrl !== expectedAssets.wasm[0] ||
      !validSameOriginUrl(value.delivery.wasmUrl, origin)
    ) {
      violations.push('delivery WASM URL does not match raw samples');
    }
    if (value.delivery.wasmContentType !== 'application/wasm')
      violations.push('delivery WASM content type is invalid');
    if (value.delivery.wasmContentEncoding !== (production ? 'br' : null))
      violations.push('delivery WASM content encoding does not match mode');
    if (value.delivery.wasmCompilationSucceeded !== true)
      violations.push('delivery must prove browser WASM compilation');
    if (
      value.delivery.wasmSha256 !== wasmHashes[0] ||
      !validSha256(value.delivery.wasmSha256)
    ) {
      violations.push('delivery WASM SHA-256 does not match raw samples');
    }
    if (value.delivery.localPrecompressedOrigin !== production)
      violations.push('local precompressed-origin flag does not match mode');
    if (value.delivery.liveS3CloudFrontVerified !== false)
      violations.push('WP-11 must not claim live S3/CloudFront verification');
  }
  return violations;
}

export function browserArtifactViolations(
  report: CacheWasmBrowserReport,
  expected: { wasmSha256: string; wasmBasename: string }
): string[] {
  const violations: string[] = [];
  const sampleHashes = uniqueSorted(
    report.samples.map((sample) => sample.wasmSha256)
  );
  if (
    sampleHashes.length !== 1 ||
    sampleHashes[0] !== expected.wasmSha256 ||
    report.delivery.wasmSha256 !== expected.wasmSha256
  ) {
    violations.push('browser report WASM hash differs from inspected artifact');
  }
  const urls = [
    ...report.samples.map((sample) => sample.wasmUrl),
    ...report.assetUrls.wasm,
    report.delivery.wasmUrl,
  ];
  for (const url of urls) {
    let actualBasename = '';
    try {
      actualBasename = new URL(url).pathname.split('/').at(-1) ?? '';
    } catch {
      // Structural validation reports malformed URLs separately.
    }
    if (actualBasename !== expected.wasmBasename) {
      violations.push(
        `browser report WASM URL basename ${actualBasename || '<invalid>'} differs from inspected ${expected.wasmBasename}`
      );
      break;
    }
  }
  return violations;
}

export function assertBrowserArtifact(
  report: CacheWasmBrowserReport,
  expected: { wasmSha256: string; wasmBasename: string }
): void {
  const violations = browserArtifactViolations(report, expected);
  if (violations.length > 0) {
    throw new Error(`browser artifact binding failed:\n- ${violations.join('\n- ')}`);
  }
}

export function assertBrowserReport(
  value: unknown,
  expected: {
    browser: CacheWasmBrowser;
    mode: CacheWasmBuildMode;
    project: string;
  }
): asserts value is CacheWasmBrowserReport {
  const violations = browserReportViolations(value, expected);
  if (violations.length > 0) {
    throw new Error(`invalid ${expected.project} browser report:\n- ${violations.join('\n- ')}`);
  }
}
