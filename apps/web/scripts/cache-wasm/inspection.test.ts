import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
  loroWasmContractViolations,
  productionGlueHookViolations,
  removeCacheWasmBrotliSidecar,
  staticImportSpecifiers,
  unexpectedDistWasmPaths,
  writeCacheWasmBrotliSidecar,
} from './inspection';
import { nestedWorkerConstructionViolations } from './static-worker-analysis';
import { wasmBindgenGlueImportNames } from './wasm-bindgen-glue';
import { inspectWasmBinary, wasmContractViolations } from './wasm-binary';

const temporaryDirectories: string[] = [];
afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

const unsigned = (value: number): number[] => {
  const bytes: number[] = [];
  do {
    let byte = value & 0x7f;
    value >>>= 7;
    if (value !== 0) byte |= 0x80;
    bytes.push(byte);
  } while (value !== 0);
  return bytes;
};
const string = (value: string): number[] => {
  const bytes = [...new TextEncoder().encode(value)];
  return [...unsigned(bytes.length), ...bytes];
};
const section = (id: number, contents: number[]): number[] => [
  id,
  ...unsigned(contents.length),
  ...contents,
];
const module = (...sections: number[][]): Uint8Array =>
  Uint8Array.from([
    0x00,
    0x61,
    0x73,
    0x6d,
    0x01,
    0x00,
    0x00,
    0x00,
    ...sections.flat(),
  ]);
const memorySection = (flags: number, maximum?: number): number[] =>
  section(5, [1, flags, 1, ...(maximum === undefined ? [] : [maximum])]);
const memoryExport = (): number[] => section(7, [1, ...string('memory'), 2, 0]);
const functionImport = (importModule: string, name: string): number[] =>
  section(2, [1, ...string(importModule), ...string(name), 0, 0]);
const cacheImport = (): number[] =>
  functionImport('./cache_wasm_bg.js', '__wbg_fixture_deadbeef');

function violations(bytes: Uint8Array, features: string[] = []): string[] {
  return wasmContractViolations(inspectWasmBinary(bytes), features);
}

describe('cache WASM binary contract', () => {
  it('accepts one external, unshared exported memory', () => {
    expect(
      violations(module(cacheImport(), memorySection(0), memoryExport()))
    ).toEqual([]);
  });

  it.each([
    ['shared memory', module(memorySection(3, 2), memoryExport()), 'shared'],
    ['memory64', module(memorySection(4), memoryExport()), 'memory64'],
    [
      'imported memory',
      module(
        section(2, [1, ...string('env'), ...string('memory'), 2, 0, 1]),
        memoryExport()
      ),
      'imported',
    ],
    [
      'multiple memories',
      module(section(5, [2, 0, 1, 0, 1]), memoryExport()),
      'exactly one',
    ],
    ['missing export', module(memorySection(0)), 'externally exported'],
  ])('rejects %s', (_name, bytes, message) => {
    expect(violations(bytes as Uint8Array).join('\n')).toContain(message);
  });

  it('accepts only wasm-bindgen function imports from exact generated glue', () => {
    const bytes = module(
      section(2, [
        1,
        ...string('./cache_wasm_bg.js'),
        ...string('__wbg_call_deadbeef'),
        0,
        0,
      ]),
      memorySection(0),
      memoryExport()
    );
    const binary = inspectWasmBinary(bytes);
    expect(
      wasmContractViolations(
        binary,
        [],
        new Set(['__wbg_call_deadbeef'])
      )
    ).toEqual([]);
    expect(
      wasmContractViolations(binary, [], new Set(['__wbg_other'])).join('\n')
    ).toContain('unexpected imports for ./cache_wasm_bg.js');
  });

  it('extracts exact generated glue import-object function names', () => {
    expect(
      wasmBindgenGlueImportNames(`
        const import0 = {
          __wbg_expected_deadbeef: function() {},
          __wbindgen_expected: value,
        };
        const imports = { "./cache_wasm_bg.js": import0 };
      `)
    ).toEqual(
      new Set(['__wbg_expected_deadbeef', '__wbindgen_expected'])
    );
  });

  it.each([
    ['WASI fd_write', 'wasi_snapshot_preview1', 'fd_write', 0, [0]],
    ['env function', 'env', 'fd_write', 0, [0]],
    ['unknown function module', './other.js', '__wbg_call_deadbeef', 0, [0]],
    [
      'non-function wasm-bindgen import',
      './cache_wasm_bg.js',
      '__wbg_memory_deadbeef',
      2,
      [0, 1],
    ],
  ])('rejects %s', (_label, importModule, name, kind, descriptor) => {
    const bytes = module(
      section(2, [
        1,
        ...string(importModule as string),
        ...string(name as string),
        kind as number,
        ...(descriptor as number[]),
      ]),
      memorySection(0),
      memoryExport()
    );
    expect(violations(bytes).join('\n')).toContain(
      'unexpected imports for ./cache_wasm_bg.js'
    );
  });

  it('rejects atomics/threads features', () => {
    expect(
      violations(module(memorySection(0), memoryExport()), [
        '--enable-threads',
      ]).join('\n')
    ).toContain('atomics/threads');
  });
});

describe('production cache WASM export inspection', () => {
  it('rejects destructive browser-test hook exports', () => {
    expect(productionGlueHookViolations('export function openCache() {}')).toEqual(
      []
    );
    expect(
      productionGlueHookViolations(
        'export function browserTestCorruptQueuePayload() {}'
      )
    ).toEqual([
      'production cache WASM glue exports forbidden test hook browserTestCorruptQueuePayload',
    ]);
  });
});

describe('fail-closed dist WASM inspection', () => {
  it('allows only the combined cache WASM and one known hashed Loro artifact', () => {
    const cache = '/dist/assets/cache_wasm_bg-hash.wasm';
    expect(
      unexpectedDistWasmPaths(
        [cache, '/dist/assets/loro_wasm_bg-BK86pe5_.wasm'],
        cache
      )
    ).toEqual([]);
  });

  it('rejects an opaque duplicate WASM fixture', () => {
    const cache = '/dist/assets/cache_wasm_bg-hash.wasm';
    expect(
      unexpectedDistWasmPaths(
        [cache, '/dist/assets/opaque-database-copy.wasm'],
        cache
      )
    ).toEqual(['/dist/assets/opaque-database-copy.wasm']);
  });

  it('inspects the exact Loro wasm-bindgen function import module', () => {
    const cache = module(
      cacheImport(),
      memorySection(0),
      memoryExport()
    );
    const loro = module(
      functionImport('./loro_wasm_bg.js', '__wbg_loro_deadbeef'),
      memorySection(0),
      memoryExport()
    );
    expect(loroWasmContractViolations(loro, cache)).toEqual([]);
    expect(
      loroWasmContractViolations(
        module(
          functionImport('./renamed_cache_wasm_bg.js', '__wbg_loro_deadbeef'),
          memorySection(0),
          memoryExport()
        ),
        cache
      ).join('\n')
    ).toContain('./loro_wasm_bg.js');
    expect(
      loroWasmContractViolations(
        module(
          section(2, [
            1,
            ...string('./loro_wasm_bg.js'),
            ...string('__wbg_loro_deadbeef'),
            2,
            0,
            1,
          ]),
          memorySection(0),
          memoryExport()
        ),
        cache
      ).join('\n')
    ).toContain('(memory)');
  });

  it('rejects a cache binary renamed to the allowed Loro filename', () => {
    const cache = module(
      cacheImport(),
      memorySection(0),
      memoryExport()
    );
    expect(loroWasmContractViolations(cache, cache).join('\n')).toContain(
      'byte-identical'
    );
  });
});

describe('nested Worker static analysis', () => {
  it.each([
    ['direct Worker', 'new Worker(url)'],
    ['globalThis Worker', 'new globalThis.Worker(url)'],
    ['self Worker', 'new self["Worker"](url)'],
    ['constructor alias', 'const Engine = self.Worker; new Engine(url)'],
    [
      'global and constructor aliases',
      'const root = globalThis; const Engine = root.Worker; new Engine(url)',
    ],
    ['destructured alias', 'const { Worker: Engine } = self; new Engine(url)'],
    ['Reflect.construct', 'Reflect.construct(self.Worker, [url])'],
    [
      'Reflect.construct aliases',
      'const make = Reflect.construct; const Engine = Worker; make(Engine, [url])',
    ],
    ['SharedWorker alias', 'const Shared = globalThis.SharedWorker; new Shared(url)'],
    ['Proxy Worker', 'const Wrapped = new Proxy(Worker, {}); void Wrapped'],
    ['Worker bind', 'const Wrapped = Worker.bind(globalThis); void Wrapped'],
    ['sequence Worker', 'const Wrapped = (0, Worker); void Wrapped'],
    [
      'computed concatenated Worker',
      `const Wrapped = self['Wor' + 'ker']; void Wrapped`,
    ],
  ])('rejects %s', (_label, source) => {
    expect(
      nestedWorkerConstructionViolations(source, 'fixture.js').length
    ).toBeGreaterThan(0);
  });

  it('does not treat unrelated constructors as workers', () => {
    expect(
      nestedWorkerConstructionViolations(
        'class WorkerPool {}\nnew WorkerPool();\nReflect.construct(URL, [value]);',
        'fixture.js'
      )
    ).toEqual([]);
  });
});

describe('production static import inspection', () => {
  it('finds formatted, minified, side-effect, and re-export imports', () => {
    expect(
      staticImportSpecifiers(`
        import { formatted } from './formatted.js';
        import{minified}from"./minified.js";
        import './side-effect.js';
        export*from'./re-export.js';
        void import('./dynamic.js');
      `)
    ).toEqual([
      './formatted.js',
      './minified.js',
      './re-export.js',
      './side-effect.js',
    ]);
  });
});

describe('cache WASM Brotli packaging', () => {
  it('retains raw and unrelated WASM byte-for-byte', () => {
    const directory = mkdtempSync(join(tmpdir(), 'cache-wasm-packaging-'));
    temporaryDirectories.push(directory);
    const rawPath = join(directory, 'cache_wasm_bg-hash.wasm');
    const unrelatedPath = join(directory, 'unrelated.wasm');
    const raw = Buffer.from('combined cache wasm bytes'.repeat(100));
    const unrelated = Buffer.from('unrelated bytes');
    writeFileSync(rawPath, raw);
    writeFileSync(unrelatedPath, unrelated);

    const sidecar = writeCacheWasmBrotliSidecar(directory);

    expect(sidecar).toBe(`${rawPath}.br`);
    expect(readFileSync(rawPath)).toEqual(raw);
    expect(readFileSync(unrelatedPath)).toEqual(unrelated);
    expect(readFileSync(sidecar).byteLength).toBeLessThan(raw.byteLength);

    expect(removeCacheWasmBrotliSidecar(directory)).toBe(sidecar);
    expect(() => readFileSync(sidecar)).toThrow();
    expect(readFileSync(rawPath)).toEqual(raw);
    expect(readFileSync(unrelatedPath)).toEqual(unrelated);
  });

  it('rejects unexpected multiple cache WASM modules', () => {
    const directory = mkdtempSync(join(tmpdir(), 'cache-wasm-multiple-'));
    temporaryDirectories.push(directory);
    writeFileSync(join(directory, 'cache_wasm_bg-a.wasm'), 'a');
    writeFileSync(join(directory, 'cache_wasm_bg-b.wasm'), 'b');
    expect(() => writeCacheWasmBrotliSidecar(directory)).toThrow(
      'expected one cache WASM'
    );
  });
});
