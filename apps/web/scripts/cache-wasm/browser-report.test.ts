import { describe, expect, it } from 'vitest';
import {
  type CacheWasmBrowserReport,
  browserArtifactViolations,
  browserReportViolations,
  percentile95,
} from './browser-report';

function validReport(): CacheWasmBrowserReport {
  const origin = 'http://127.0.0.1:4189';
  const samples = Array.from({ length: 5 }, (_, index) => ({
    browser: 'chromium' as const,
    mode: 'production' as const,
    activationMs: 10 + index,
    browserReadyMs: 100 + index,
    hostFirstReadyMs: 110 + index,
    workerActivationMs: 90 + index,
    linearMemoryBytes: 9_961_472,
    sharedWorkerConstructions: 1,
    dedicatedWorkerConstructions: 1,
    nestedWorkerConstructions: 0,
    wasmFetchCount: 1,
    wasmSha256: 'a'.repeat(64),
    ownerEpochs: [1],
    crossOriginIsolated: false,
    sharedArrayBufferAvailable: false,
    sharedWorkerUrl: `${origin}/app/assets/cache.coordinator.shared-worker-hash.js`,
    productionEngineUrl: `${origin}/app/assets/cache.engine-worker-hash.js`,
    instrumentedEngineUrl: `${origin}/app/assets/measurement-cache.engine-worker-hash.js`,
    wasmUrl: `${origin}/app/assets/cache_wasm_bg-fixture1.wasm`,
  }));
  return {
    schemaVersion: 1,
    project: 'chromium-production',
    browser: 'chromium',
    mode: 'production',
    origin,
    browserVersion: '1.2.3',
    executablePath: '/nix/store/browser',
    userAgent: 'fixture browser',
    freshScopeCount: 5,
    samples,
    p95: {
      activationMs: percentile95(samples.map((sample) => sample.activationMs)),
      browserReadyMs: percentile95(
        samples.map((sample) => sample.browserReadyMs)
      ),
      hostFirstReadyMs: percentile95(
        samples.map((sample) => sample.hostFirstReadyMs)
      ),
      workerActivationMs: percentile95(
        samples.map((sample) => sample.workerActivationMs)
      ),
      linearMemoryBytes: percentile95(
        samples.map((sample) => sample.linearMemoryBytes)
      ),
    },
    assetUrls: {
      sharedWorkers: [samples[0].sharedWorkerUrl],
      productionEngines: [samples[0].productionEngineUrl],
      instrumentedEngines: [samples[0].instrumentedEngineUrl],
      wasm: [samples[0].wasmUrl],
    },
    sourceMapUrls: [
      `${samples[0].sharedWorkerUrl}.map`,
      `${samples[0].productionEngineUrl}.map`,
      `${samples[0].instrumentedEngineUrl}.map`,
    ],
    delivery: {
      basePath: '/app/',
      wasmUrl: samples[0].wasmUrl,
      wasmContentType: 'application/wasm',
      wasmContentEncoding: 'br',
      wasmCompilationSucceeded: true,
      wasmSha256: samples[0].wasmSha256,
      localPrecompressedOrigin: true,
      liveS3CloudFrontVerified: false,
    },
  };
}

const violations = (report: unknown): string[] =>
  browserReportViolations(report, {
    browser: 'chromium',
    mode: 'production',
    project: 'chromium-production',
  });

describe('cache WASM browser report validation', () => {
  it('accepts five complete recomputable raw samples', () => {
    expect(violations(validReport())).toEqual([]);
  });

  it.each([
    ['empty report', {}],
    [
      'empty raw samples with fabricated aggregate',
      { ...validReport(), samples: [], freshScopeCount: 5 },
    ],
    [
      'non-finite raw metric',
      (() => {
        const report = validReport();
        report.samples[0].browserReadyMs = Number.NaN;
        return report;
      })(),
    ],
    [
      'fabricated p95',
      (() => {
        const report = validReport();
        report.p95.browserReadyMs = 1;
        return report;
      })(),
    ],
    [
      'multiple engines',
      (() => {
        const report = validReport();
        report.samples[0].dedicatedWorkerConstructions = 2;
        return report;
      })(),
    ],
    [
      'multiple owner epochs',
      (() => {
        const report = validReport();
        report.samples[0].ownerEpochs = [1, 2];
        return report;
      })(),
    ],
    [
      'fabricated aggregate URL',
      (() => {
        const report = validReport();
        report.assetUrls.wasm = [
          'http://127.0.0.1:4189/app/assets/fabricated.wasm',
        ];
        return report;
      })(),
    ],
    [
      'inconsistent sample artifact hash',
      (() => {
        const report = validReport();
        report.samples[0].wasmSha256 = 'b'.repeat(64);
        return report;
      })(),
    ],
    [
      'cross-origin isolated sample',
      (() => {
        const report = validReport();
        report.samples[0].crossOriginIsolated = true;
        return report;
      })(),
    ],
    [
      'SharedArrayBuffer exposure',
      (() => {
        const report = validReport();
        report.samples[0].sharedArrayBufferAvailable = true;
        return report;
      })(),
    ],
  ])('rejects %s', (_label, report) => {
    expect(violations(report).length).toBeGreaterThan(0);
  });

  it('rejects a structurally coherent stale production URL', () => {
    const report = validReport();
    const staleUrl = `${report.origin}/app/assets/cache_wasm_bg-stale000.wasm`;
    for (const sample of report.samples) sample.wasmUrl = staleUrl;
    report.assetUrls.wasm = [staleUrl];
    report.delivery.wasmUrl = staleUrl;
    expect(violations(report)).toEqual([]);
    expect(
      browserArtifactViolations(report, {
        wasmSha256: 'a'.repeat(64),
        wasmBasename: 'cache_wasm_bg-fixture1.wasm',
      }).join('\n')
    ).toContain('basename');
  });

  it('rejects a structurally coherent report for stale WASM bytes', () => {
    const report = validReport();
    for (const sample of report.samples) sample.wasmSha256 = 'b'.repeat(64);
    report.delivery.wasmSha256 = 'b'.repeat(64);
    expect(violations(report)).toEqual([]);
    expect(
      browserArtifactViolations(report, {
        wasmSha256: 'a'.repeat(64),
        wasmBasename: 'cache_wasm_bg-fixture1.wasm',
      }).join('\n')
    ).toContain('hash differs');
  });
});
