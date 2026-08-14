#!/usr/bin/env bun

import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { cpus, platform, release, type } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  assertBrowserArtifact,
  assertBrowserReport,
  type CacheWasmBrowserReport,
  percentile95,
} from './browser-report';
import { assertBuildObservation } from './build-observation';
import { CACHE_WASM_BUDGETS } from './budgets';
import {
  assertInspection,
  inspectCacheWasmDist,
  inspectCacheWasmPackage,
  removeCacheWasmBrotliSidecar,
  writeCacheWasmBrotliSidecar,
} from './inspection';

const webRoot = resolve(fileURLToPath(new URL('../..', import.meta.url)));
const repoRoot = resolve(webRoot, '../..');

function argument(name: string, fallback?: string): string {
  const index = process.argv.indexOf(name);
  const value = index >= 0 ? process.argv[index + 1] : fallback;
  if (!value) throw new Error(`missing ${name}`);
  return value;
}

const stringify = (value: unknown): string =>
  JSON.stringify(
    value,
    (_key, candidate) =>
      typeof candidate === 'bigint' ? candidate.toString() : candidate,
    2
  );

function print(value: unknown): void {
  process.stdout.write(`${stringify(value)}\n`);
}

function commandOutput(command: string, args: string[] = []): string {
  try {
    return execFileSync(command, args, {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
    }).trim();
  } catch (error) {
    return `unavailable: ${error instanceof Error ? error.message : String(error)}`;
  }
}

interface NodeSample {
  compileMs: number;
  instantiateMs: number;
  compileInstantiateMs: number;
  linearMemoryBytes: number;
}

function nodeSamples(count: number): NodeSample[] {
  const gluePath = join(webRoot, 'src/lib/graphql-cache/wasm/cache_wasm.js');
  const wasmPath = join(
    webRoot,
    'src/lib/graphql-cache/wasm/cache_wasm_bg.wasm'
  );
  const runner = join(webRoot, 'scripts/cache-wasm/node-benchmark.mjs');
  return Array.from({ length: count }, (_, index) => {
    const result = spawnSync(
      process.execPath,
      [runner, gluePath, wasmPath, String(index)],
      { encoding: 'utf8' }
    );
    if (result.status !== 0) {
      throw new Error(
        `Node cold benchmark ${index + 1} failed:\n${result.stderr || result.stdout}`
      );
    }
    const sample = JSON.parse(result.stdout) as Partial<NodeSample>;
    return {
      compileMs: requireFiniteMetric(sample.compileMs, 'Node compile time'),
      instantiateMs: requireFiniteMetric(
        sample.instantiateMs,
        'Node instantiate time'
      ),
      compileInstantiateMs: requireFiniteMetric(
        sample.compileInstantiateMs,
        'Node compile + instantiate time'
      ),
      linearMemoryBytes: requireFiniteMetric(
        sample.linearMemoryBytes,
        'Node linear memory'
      ),
    };
  });
}

function readJson(path: string): unknown {
  if (!existsSync(path)) throw new Error(`required evidence is missing: ${path}`);
  return JSON.parse(readFileSync(path, 'utf8'));
}

function requireFiniteMetric(value: unknown, name: string): number {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) {
    throw new Error(`${name} must be a finite non-negative number`);
  }
  return value;
}

function report(): void {
  const measuredRevisionChangeId = argument('--revision-change-id');
  if (!/^[k-z]{32}$/.test(measuredRevisionChangeId)) {
    throw new Error('--revision-change-id must be one stable jj change ID');
  }
  const distPath = resolve(argument('--dist', join(webRoot, 'dist')));
  const browserDirectory = resolve(
    argument('--browser-dir', join(webRoot, 'measurements/generated'))
  );
  const outputPath = resolve(
    argument('--output', join(webRoot, 'measurements/cache-wasm-wp11.json'))
  );
  const packageInspection = inspectCacheWasmPackage(repoRoot);
  const distInspection = inspectCacheWasmDist(repoRoot, distPath);
  assertInspection('cache WASM package inspection', packageInspection);
  assertInspection('cache WASM dist inspection', distInspection);
  const samples = nodeSamples(5);
  const nodeP95 = percentile95(
    samples.map((sample) => sample.compileInstantiateMs)
  );
  if (nodeP95 > CACHE_WASM_BUDGETS.nodeCompileInstantiateP95Ms) {
    throw new Error(
      `Node compile+instantiate p95 ${nodeP95.toFixed(3)} ms exceeds ${CACHE_WASM_BUDGETS.nodeCompileInstantiateP95Ms} ms`
    );
  }

  const expectedBrowserReports = [
    ['chromium', 'development', 'chromium'],
    ['chromium', 'production', 'chromium-production'],
    ['firefox', 'development', 'firefox'],
    ['firefox', 'production', 'firefox-production'],
  ] as const;
  const browserReports = expectedBrowserReports.map(
    ([browser, mode, project]) => {
      const path = join(
        browserDirectory,
        `cache-wasm-${browser}-${mode}.json`
      );
      const value = readJson(path);
      assertBrowserReport(value, { browser, mode, project });
      assertBrowserArtifact(value, {
        wasmSha256:
          mode === 'production'
            ? distInspection.cacheWasmSha256
            : packageInspection.wasmSha256,
        wasmBasename:
          mode === 'production'
            ? basename(distInspection.cacheWasmPath)
            : basename(packageInspection.wasmPath),
      });
      return value;
    }
  );
  for (const browserReport of (
    browserReports as CacheWasmBrowserReport[]
  ).filter(
    ({ mode }) => mode === 'production'
  )) {
    if (
      browserReport.p95.browserReadyMs > CACHE_WASM_BUDGETS.browserReadyP95Ms
    ) {
      throw new Error(
        `${browserReport.project} DB-ready p95 ${browserReport.p95.browserReadyMs} ms exceeds ${CACHE_WASM_BUDGETS.browserReadyP95Ms} ms`
      );
    }
    if (
      browserReport.p95.hostFirstReadyMs >
      CACHE_WASM_BUDGETS.hostFirstReadyP95Ms
    ) {
      throw new Error(
        `${browserReport.project} host first-ready p95 ${browserReport.p95.hostFirstReadyMs} ms exceeds ${CACHE_WASM_BUDGETS.hostFirstReadyP95Ms} ms`
      );
    }
    if (
      browserReport.p95.linearMemoryBytes > CACHE_WASM_BUDGETS.linearMemoryBytes
    ) {
      throw new Error(
        `${browserReport.project} linear memory ${browserReport.p95.linearMemoryBytes} B exceeds ${CACHE_WASM_BUDGETS.linearMemoryBytes} B`
      );
    }
  }
  const build = readJson(
    join(webRoot, 'src/lib/graphql-cache/wasm/.wp11-build.json')
  ) as { elapsedMs?: number; wasmPack?: string };
  const firstTargetFill = readJson(
    join(
      webRoot,
      'measurements/generated/cache-wasm-first-target-fill.json'
    )
  );
  const currentToolIdentity = {
    rustc: commandOutput('rustc', ['--version']),
    cargo: commandOutput('cargo', ['--version']),
    wasmPack: commandOutput('wasm-pack', ['--version']),
    wasmOpt: commandOutput('wasm-opt', ['--version']),
  };
  assertBuildObservation(firstTargetFill, {
    measuredRevisionChangeId,
    wasmSha256: packageInspection.wasmSha256,
    toolIdentity: currentToolIdentity,
  });
  if (firstTargetFill.elapsedMs > CACHE_WASM_BUDGETS.buildMs) {
    throw new Error(
      `first target fill ${firstTargetFill.elapsedMs} ms exceeds ${CACHE_WASM_BUDGETS.buildMs} ms`
    );
  }
  if (build.wasmPack !== currentToolIdentity.wasmPack) {
    throw new Error(
      'latest cache WASM build wasm-pack identity differs from report environment'
    );
  }
  const buildElapsedMs = requireFiniteMetric(
    build.elapsedMs,
    'cache WASM build elapsed time'
  );
  if (buildElapsedMs > CACHE_WASM_BUDGETS.buildMs) {
    throw new Error(
      `WASM build ${buildElapsedMs} ms exceeds ${CACHE_WASM_BUDGETS.buildMs} ms`
    );
  }

  const result = {
    schemaVersion: 1,
    measuredArtifact:
      'production combined cache-core + cache-turso + turso-opfs + cache-wasm',
    environment: {
      generatedAtUtc: new Date().toISOString(),
      measuredRevisionChangeId,
      operatingSystem: `${type()} ${release()}`,
      platform: platform(),
      architecture: process.arch,
      cpu: cpus()[0]?.model ?? 'unknown',
      cpuCount: cpus().length,
      rustc: currentToolIdentity.rustc,
      rustcVerbose: commandOutput('rustc', ['-vV']),
      cargo: currentToolIdentity.cargo,
      wasmPack: currentToolIdentity.wasmPack,
      wasmBindgenOnPath: commandOutput('wasm-bindgen', ['--version']),
      wasmBindgenCrateGraph: commandOutput('cargo', [
        'tree',
        '-p',
        'cache-wasm',
        '--target',
        'wasm32-unknown-unknown',
      ])
        .split('\n')
        .filter((line) => /wasm-bindgen v/.test(line)),
      wasmOpt: currentToolIdentity.wasmOpt,
      node: commandOutput('node', ['--version']),
      bun: commandOutput('bun', ['--version']),
      vite: commandOutput('bunx', ['--bun', 'vite', '--version']),
      playwright: commandOutput('bunx', ['--bun', 'playwright', '--version']),
      brotli: commandOutput('brotli', ['--version']).split('\n')[0],
      gzip: commandOutput('gzip', ['--version']).split('\n')[0],
    },
    budgets: CACHE_WASM_BUDGETS,
    build: {
      latest: build,
      firstTargetFill,
    },
    package: packageInspection,
    dist: distInspection,
    sizes: {
      rawBytes: distInspection.rawBytes,
      gzip9Bytes: distInspection.gzipBytes,
      brotli11Bytes: distInspection.brotliBytes,
      glueBytes: packageInspection.glueBytes,
    },
    nodeCold: {
      freshProcessSamples: samples,
      compileInstantiateP95Ms: nodeP95,
      linearMemoryBytesAfterInstantiate: Math.max(
        ...samples.map((sample) => sample.linearMemoryBytes)
      ),
    },
    browserReports,
    exposure: {
      status: 'blocked',
      pending: [
        'product owner acceptance of WP-11 numeric budgets',
        'WP-12 exact Safari, navigation, telemetry, and rollout evidence',
      ],
    },
  };
  mkdirSync(dirname(outputPath), { recursive: true });
  writeFileSync(outputPath, `${stringify(result)}\n`);
  print(result);
}

const command = process.argv[2];
switch (command) {
  case 'inspect-package': {
    const inspection = inspectCacheWasmPackage(repoRoot);
    assertInspection('cache WASM package inspection', inspection);
    print(inspection);
    break;
  }
  case 'package-dist': {
    const distPath = resolve(argument('--dist', join(webRoot, 'dist')));
    const sidecarPath = writeCacheWasmBrotliSidecar(distPath);
    print({ sidecarPath });
    break;
  }
  case 'remove-sidecar': {
    const distPath = resolve(argument('--dist', join(webRoot, 'dist')));
    const removedPath = removeCacheWasmBrotliSidecar(distPath);
    print({ removedPath });
    break;
  }
  case 'inspect-dist': {
    const distPath = resolve(argument('--dist', join(webRoot, 'dist')));
    const expectedBase = argument('--base', '/app/');
    const inspection = inspectCacheWasmDist(repoRoot, distPath, expectedBase);
    assertInspection('cache WASM dist inspection', inspection);
    print(inspection);
    break;
  }
  case 'report':
    report();
    break;
  default:
    throw new Error(
      'usage: cli.ts <inspect-package|package-dist|remove-sidecar|inspect-dist|report> [options]'
    );
}
