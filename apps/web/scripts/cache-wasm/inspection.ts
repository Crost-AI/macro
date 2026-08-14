import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  existsSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { basename, extname, join, relative, resolve, sep } from 'node:path';
import {
  brotliCompressSync,
  brotliDecompressSync,
  constants as zlibConstants,
} from 'node:zlib';
import { CACHE_WASM_BUDGETS } from './budgets';
import { nestedWorkerConstructionViolations } from './static-worker-analysis';
import { wasmBindgenGlueImportNames } from './wasm-bindgen-glue';
import {
  inspectWasmBinary,
  type WasmBinaryInspection,
  wasmBindgenImportViolations,
  wasmContractViolations,
} from './wasm-binary';

export interface CacheWasmPackageInspection {
  wasmPath: string;
  wasmBytes: number;
  wasmSha256: string;
  gluePath: string;
  glueBytes: number;
  glueImportFunctionCount: number;
  enabledWasmFeatures: string[];
  binary: WasmBinaryInspection;
  violations: string[];
}

export interface CacheWasmDistInspection {
  distPath: string;
  cacheWasmPath: string;
  cacheWasmBrotliPath: string;
  rawBytes: number;
  gzipBytes: number;
  brotliBytes: number;
  cacheWasmSha256: string;
  loroWasmPath?: string;
  loroWasmSha256?: string;
  loroBinary?: WasmBinaryInspection;
  unrelatedWasmPaths: string[];
  entryPaths: string[];
  entryStaticImportPaths: string[];
  modulePreloadPaths: string[];
  sourceMapEvidence: {
    entry: boolean;
    coordinatorWorker: boolean;
    engineWorker: boolean;
  };
  engineWorkerChunkPath?: string;
  violations: string[];
}

const requireCondition: (
  condition: unknown,
  message: string
) => asserts condition = (condition, message) => {
  if (!condition) throw new Error(message);
};

function walkFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true })
    .flatMap((entry) => {
      const path = join(directory, entry.name);
      return entry.isDirectory() ? walkFiles(path) : [path];
    })
    .sort();
}

function wasmFeatures(path: string): string[] {
  const output = execFileSync(
    'wasm-opt',
    [path, '--print-features', '-o', '/dev/null'],
    { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }
  );
  return output
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean)
    .sort();
}

function sourceContractViolations(
  source: string,
  sourceName: string
): string[] {
  const checks: Array<[RegExp, string]> = [
    [/\bSharedArrayBuffer\b/, 'SharedArrayBuffer'],
    [/\bAtomics\s*\./, 'Atomics'],
    [/\bimportScripts\s*\(/, 'importScripts'],
    [/\bworker_threads\b/, 'Node worker_threads'],
    [/\b(?:wasi|pthread)\b/i, 'WASI/pthread marker'],
  ];
  return [
    ...checks.flatMap(([pattern, description]) =>
      pattern.test(source) ? [`${sourceName} contains ${description}`] : []
    ),
    ...nestedWorkerConstructionViolations(source, sourceName),
  ];
}

export function inspectCacheWasmPackage(
  repoRoot: string
): CacheWasmPackageInspection {
  const packageDirectory = join(
    repoRoot,
    'apps/web/src/lib/graphql-cache/wasm'
  );
  requireCondition(
    existsSync(packageDirectory),
    `cache WASM package is missing: ${packageDirectory}`
  );
  const files = walkFiles(packageDirectory);
  const wasmFiles = files.filter((path) => extname(path) === '.wasm');
  const expectedWasmPath = join(packageDirectory, 'cache_wasm_bg.wasm');
  const violations: string[] = [];
  if (wasmFiles.length !== 1 || wasmFiles[0] !== expectedWasmPath) {
    violations.push(
      `expected only cache_wasm_bg.wasm, found ${wasmFiles
        .map((path) => basename(path))
        .join(', ')}`
    );
  }
  requireCondition(
    existsSync(expectedWasmPath),
    `combined cache WASM is missing: ${expectedWasmPath}`
  );
  const gluePath = join(packageDirectory, 'cache_wasm.js');
  requireCondition(existsSync(gluePath), `WASM glue is missing: ${gluePath}`);
  const bytes = readFileSync(expectedWasmPath);
  const features = wasmFeatures(expectedWasmPath);
  const binary = inspectWasmBinary(bytes);
  const glue = readFileSync(gluePath, 'utf8');
  const glueImportNames = wasmBindgenGlueImportNames(glue);
  violations.push(
    ...wasmContractViolations(binary, features, glueImportNames)
  );
  violations.push(
    ...sourceContractViolations(glue, 'generated cache WASM glue')
  );

  const engineWorkerPath = join(
    repoRoot,
    'apps/web/src/lib/graphql-cache/worker/cache.engine-worker.ts'
  );
  violations.push(
    ...sourceContractViolations(
      readFileSync(engineWorkerPath, 'utf8'),
      'production cache engine worker'
    )
  );

  for (const packageMetadataPath of [
    join(repoRoot, 'package.json'),
    join(repoRoot, 'bun.lock'),
    join(repoRoot, 'apps/web/package.json'),
  ]) {
    const contents = readFileSync(packageMetadataPath, 'utf8');
    if (/['"/@](?:turso|tursodatabase|libsql)(?:['"/@-]|$)/i.test(contents)) {
      violations.push(
        `${relative(repoRoot, packageMetadataPath)} contains a Turso/libSQL npm artifact`
      );
    }
  }

  const glueBytes = statSync(gluePath).size;
  if (glueBytes > CACHE_WASM_BUDGETS.glueBytes) {
    violations.push(
      `generated glue ${glueBytes} B exceeds ${CACHE_WASM_BUDGETS.glueBytes} B`
    );
  }
  return {
    wasmPath: relative(repoRoot, expectedWasmPath),
    wasmBytes: bytes.byteLength,
    wasmSha256: createHash('sha256').update(bytes).digest('hex'),
    gluePath: relative(repoRoot, gluePath),
    glueBytes,
    glueImportFunctionCount: glueImportNames.size,
    enabledWasmFeatures: features,
    binary,
    violations,
  };
}

function cacheWasmCandidates(distPath: string): string[] {
  return walkFiles(distPath).filter(
    (path) =>
      path.endsWith('.wasm') &&
      /^cache_wasm_bg(?:-[\w-]+)?\.wasm$/.test(basename(path))
  );
}

const LORO_WASM_FILENAME = /^loro_wasm_bg-[A-Za-z0-9_-]{8}\.wasm$/;

export function loroWasmContractViolations(
  loroBytes: Uint8Array,
  cacheBytes: Uint8Array
): string[] {
  const violations: string[] = [];
  const loroSha256 = createHash('sha256').update(loroBytes).digest('hex');
  const cacheSha256 = createHash('sha256').update(cacheBytes).digest('hex');
  if (loroSha256 === cacheSha256) {
    violations.push(
      'known Loro WASM is byte-identical to the combined cache WASM'
    );
  }
  try {
    const binary = inspectWasmBinary(loroBytes);
    violations.push(
      ...wasmBindgenImportViolations(binary, './loro_wasm_bg.js').map(
        (violation) => `known Loro WASM ${violation}`
      )
    );
  } catch (error) {
    violations.push(
      `known Loro WASM is invalid: ${
        error instanceof Error ? error.message : String(error)
      }`
    );
  }
  return violations;
}

export function unexpectedDistWasmPaths(
  wasmPaths: readonly string[],
  cacheWasmPath: string
): string[] {
  return wasmPaths.filter(
    (path) =>
      path !== cacheWasmPath && !LORO_WASM_FILENAME.test(basename(path))
  );
}

export function removeCacheWasmBrotliSidecar(distPath: string): string {
  const sidecars = walkFiles(distPath).filter((path) =>
    /^cache_wasm_bg(?:-[\w-]+)?\.wasm\.br$/.test(basename(path))
  );
  requireCondition(
    sidecars.length === 1,
    `expected one cache WASM Brotli sidecar in ${distPath}, found ${sidecars.length}`
  );
  rmSync(sidecars[0]);
  return sidecars[0];
}

export function writeCacheWasmBrotliSidecar(distPath: string): string {
  const candidates = cacheWasmCandidates(distPath);
  requireCondition(
    candidates.length === 1,
    `expected one cache WASM in ${distPath}, found ${candidates.length}`
  );
  const raw = readFileSync(candidates[0]);
  const sidecarPath = `${candidates[0]}.br`;
  const compressed = brotliCompressSync(raw, {
    params: {
      [zlibConstants.BROTLI_PARAM_MODE]: zlibConstants.BROTLI_MODE_GENERIC,
      [zlibConstants.BROTLI_PARAM_QUALITY]: 11,
      [zlibConstants.BROTLI_PARAM_SIZE_HINT]: raw.byteLength,
    },
  });
  writeFileSync(sidecarPath, compressed);
  requireCondition(
    brotliDecompressSync(compressed).equals(raw),
    'Brotli sidecar does not decompress to the raw cache WASM'
  );
  return sidecarPath;
}

function distPathForUrl(
  distPath: string,
  urlValue: string
): string | undefined {
  if (/^(?:data:|https?:|#)/.test(urlValue)) return undefined;
  const withoutQuery = urlValue.split(/[?#]/, 1)[0];
  const normalized = withoutQuery.startsWith('/app/')
    ? withoutQuery.slice('/app/'.length)
    : withoutQuery.startsWith('/')
      ? withoutQuery.slice(1)
      : withoutQuery;
  const path = resolve(distPath, normalized);
  const root = resolve(distPath);
  return (path === root || path.startsWith(`${root}${sep}`)) && existsSync(path)
    ? path
    : undefined;
}

function htmlAssetPaths(
  distPath: string,
  html: string,
  pattern: RegExp
): string[] {
  return [...html.matchAll(pattern)]
    .flatMap((match) => {
      const path = distPathForUrl(distPath, match[1]);
      return path ? [path] : [];
    })
    .sort();
}

export function staticImportSpecifiers(source: string): string[] {
  const fromImports = [
    ...source.matchAll(
      /\b(?:import|export)\s*(?:[^'";]*?\bfrom\s*)?['"]([^'"]+)['"]/g
    ),
  ].map((match) => match[1]);
  return [...new Set(fromImports)].sort();
}

function staticEntryGraph(distPath: string, entries: string[]): string[] {
  const visited = new Set<string>();
  const visit = (path: string): void => {
    if (visited.has(path) || extname(path) !== '.js') return;
    visited.add(path);
    const source = readFileSync(path, 'utf8');
    for (const specifier of staticImportSpecifiers(source)) {
      const imported = distPathForUrl(
        distPath,
        specifier.startsWith('.')
          ? join(relative(distPath, resolve(path, '..')), specifier)
          : specifier
      );
      if (imported) visit(imported);
    }
  };
  for (const entry of entries) visit(entry);
  return [...visited].sort();
}

function mapContainsExactSource(mapPath: string, sourceSuffix: string): boolean {
  const map = JSON.parse(readFileSync(mapPath, 'utf8')) as {
    sources?: unknown;
  };
  return (
    Array.isArray(map.sources) &&
    map.sources.some(
      (source) =>
        typeof source === 'string' &&
        source.replaceAll('\\', '/').endsWith(sourceSuffix)
    )
  );
}

export function inspectCacheWasmDist(
  repoRoot: string,
  distPathValue: string,
  expectedBase = '/app/'
): CacheWasmDistInspection {
  const distPath = resolve(distPathValue);
  const packageInspection = inspectCacheWasmPackage(repoRoot);
  const violations = [...packageInspection.violations];
  const cacheWasmFiles = cacheWasmCandidates(distPath);
  if (cacheWasmFiles.length !== 1) {
    violations.push(
      `expected exactly one external cache WASM in dist, found ${cacheWasmFiles.length}`
    );
  }
  requireCondition(cacheWasmFiles.length > 0, 'dist cache WASM is missing');
  const cacheWasmPath = cacheWasmFiles[0];
  const sidecars = walkFiles(distPath).filter((path) =>
    /^cache_wasm_bg(?:-[\w-]+)?\.wasm\.br$/.test(basename(path))
  );
  if (sidecars.length !== 1 || sidecars[0] !== `${cacheWasmPath}.br`) {
    violations.push(
      `expected exactly one adjacent cache WASM Brotli sidecar, found ${sidecars.length}`
    );
  }
  requireCondition(sidecars.length > 0, 'cache WASM Brotli sidecar is missing');
  const cacheWasmBrotliPath = sidecars[0];
  const raw = readFileSync(cacheWasmPath);
  const sourceRaw = readFileSync(resolve(repoRoot, packageInspection.wasmPath));
  if (!raw.equals(sourceRaw)) {
    violations.push(
      'dist cache WASM differs from the inspected combined package WASM'
    );
  }
  const brotli = readFileSync(cacheWasmBrotliPath);
  if (!brotliDecompressSync(brotli).equals(raw)) {
    violations.push(
      'dist Brotli sidecar does not decompress to the raw cache WASM'
    );
  }
  const gzipBytes = execFileSync('gzip', ['-9', '-c'], {
    input: raw,
    maxBuffer: 20 * 1024 * 1024,
  }).byteLength;
  const sizes: Array<[string, number, number]> = [
    ['raw', raw.byteLength, CACHE_WASM_BUDGETS.rawBytes],
    ['gzip -9', gzipBytes, CACHE_WASM_BUDGETS.gzipBytes],
    ['Brotli -11', brotli.byteLength, CACHE_WASM_BUDGETS.brotliBytes],
  ];
  for (const [name, actual, budget] of sizes) {
    if (actual > budget)
      violations.push(`${name} ${actual} B exceeds ${budget} B`);
  }

  const allFiles = walkFiles(distPath);
  const wasmPaths = allFiles.filter((path) => path.endsWith('.wasm'));
  const unrelatedWasmPaths = wasmPaths
    .filter((path) => path !== cacheWasmPath)
    .map((path) => relative(distPath, path));
  const unexpectedWasmPaths = unexpectedDistWasmPaths(
    wasmPaths,
    cacheWasmPath
  );
  if (unexpectedWasmPaths.length > 0) {
    violations.push(
      `unknown extra WASM artifacts: ${unexpectedWasmPaths
        .map((path) => relative(distPath, path))
        .join(', ')}`
    );
  }
  const loroWasmPaths = wasmPaths.filter((path) =>
    LORO_WASM_FILENAME.test(basename(path))
  );
  if (loroWasmPaths.length !== 1) {
    violations.push(
      `expected exactly one known hashed Loro WASM, found ${loroWasmPaths.length}`
    );
  }
  let loroBinary: WasmBinaryInspection | undefined;
  let loroWasmSha256: string | undefined;
  if (loroWasmPaths.length === 1) {
    const loroBytes = readFileSync(loroWasmPaths[0]);
    loroWasmSha256 = createHash('sha256').update(loroBytes).digest('hex');
    violations.push(...loroWasmContractViolations(loroBytes, raw));
    try {
      loroBinary = inspectWasmBinary(loroBytes);
    } catch {
      // loroWasmContractViolations already records the parse failure.
    }
  }
  const unexpectedCacheScripts = allFiles.filter(
    (path) => /(?:turso|libsql).*(?:\.wasm|\.js)$/i.test(basename(path))
  );
  if (unexpectedCacheScripts.length > 0) {
    violations.push(
      `unexpected cache/Turso artifacts: ${unexpectedCacheScripts
        .map((path) => relative(distPath, path))
        .join(', ')}`
    );
  }

  const indexPath = join(distPath, 'index.html');
  requireCondition(
    existsSync(indexPath),
    `dist index is missing: ${indexPath}`
  );
  const indexHtml = readFileSync(indexPath, 'utf8');
  const rawUrls = [...indexHtml.matchAll(/(?:src|href)=['"]([^'"]+)['"]/g)].map(
    (match) => match[1]
  );
  const localAssetUrls = rawUrls.filter(
    (url) => !/^(?:data:|https?:|#)/.test(url)
  );
  const wrongBase = localAssetUrls.filter(
    (url) => !url.startsWith(expectedBase) && url !== '/app'
  );
  if (wrongBase.length > 0) {
    violations.push(
      `unexpected production asset URLs: ${wrongBase.join(', ')}`
    );
  }
  const entryPaths = htmlAssetPaths(
    distPath,
    indexHtml,
    /<script\b[^>]*type=['"]module['"][^>]*src=['"]([^'"]+)['"][^>]*>/g
  );
  const modulePreloadPaths = htmlAssetPaths(
    distPath,
    indexHtml,
    /<link\b[^>]*rel=['"]modulepreload['"][^>]*href=['"]([^'"]+)['"][^>]*>/g
  );
  requireCondition(
    entryPaths.length > 0,
    'index.html has no module page entry'
  );
  const entryStaticImportPaths = staticEntryGraph(distPath, [
    ...entryPaths,
    ...modulePreloadPaths,
  ]);
  const lazyArtifactPattern =
    /(?:cache[_-](?:engine|wasm)|cache_wasm_bg|\.wasm$)/i;
  const eagerlyLoaded = [
    ...entryPaths,
    ...modulePreloadPaths,
    ...entryStaticImportPaths,
  ].filter((path) => lazyArtifactPattern.test(relative(distPath, path)));
  if (eagerlyLoaded.length > 0) {
    violations.push(
      `page entry/import/preload graph eagerly reaches cache engine/WASM: ${eagerlyLoaded
        .map((path) => relative(distPath, path))
        .join(', ')}`
    );
  }
  if (
    /cache_wasm_bg|cache[._-]engine-worker|\.wasm(?:['"?#<]|$)/i.test(indexHtml)
  ) {
    violations.push('index.html directly references the cache engine or WASM');
  }

  const mapPaths = allFiles.filter((path) => path.endsWith('.js.map'));
  const engineWorkerMap = mapPaths.find((path) =>
    mapContainsExactSource(
      path,
      '/graphql-cache/worker/cache.engine-worker.ts'
    )
  );
  const sourceMapEvidence = {
    entry: entryPaths.every((path) => existsSync(`${path}.map`)),
    coordinatorWorker: mapPaths.some((path) =>
      mapContainsExactSource(
        path,
        '/graphql-cache/worker/cache.coordinator.shared-worker.ts'
      )
    ),
    engineWorker: engineWorkerMap !== undefined,
  };
  if (engineWorkerMap) {
    const engineWorkerBundle = engineWorkerMap.slice(0, -'.map'.length);
    violations.push(
      ...sourceContractViolations(
        readFileSync(engineWorkerBundle, 'utf8'),
        'production cache engine worker bundle'
      )
    );
  }
  for (const [kind, present] of Object.entries(sourceMapEvidence)) {
    if (!present)
      violations.push(`missing ${kind} production source-map evidence`);
  }

  return {
    distPath: relative(repoRoot, distPath),
    cacheWasmPath: relative(distPath, cacheWasmPath),
    cacheWasmBrotliPath: relative(distPath, cacheWasmBrotliPath),
    rawBytes: raw.byteLength,
    gzipBytes,
    brotliBytes: brotli.byteLength,
    cacheWasmSha256: createHash('sha256').update(raw).digest('hex'),
    loroWasmPath:
      loroWasmPaths.length === 1
        ? relative(distPath, loroWasmPaths[0])
        : undefined,
    loroWasmSha256,
    loroBinary,
    unrelatedWasmPaths,
    entryPaths: entryPaths.map((path) => relative(distPath, path)),
    entryStaticImportPaths: entryStaticImportPaths.map((path) =>
      relative(distPath, path)
    ),
    modulePreloadPaths: modulePreloadPaths.map((path) =>
      relative(distPath, path)
    ),
    sourceMapEvidence,
    engineWorkerChunkPath: engineWorkerMap
      ? relative(distPath, engineWorkerMap.slice(0, -'.map'.length))
      : undefined,
    violations,
  };
}

export function assertInspection(
  name: string,
  inspection: { violations: string[] }
): void {
  if (inspection.violations.length > 0) {
    throw new Error(`${name} failed:\n- ${inspection.violations.join('\n- ')}`);
  }
}
