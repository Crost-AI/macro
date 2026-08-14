#!/usr/bin/env node

import { readFileSync } from 'node:fs';
import { performance } from 'node:perf_hooks';
import { pathToFileURL } from 'node:url';

const [gluePath, wasmPath, sampleId] = process.argv.slice(2);
if (!gluePath || !wasmPath || !sampleId) {
  throw new Error(
    'usage: node-benchmark.mjs <glue.js> <module.wasm> <sample-id>'
  );
}

const bytes = readFileSync(wasmPath);
const compileStarted = performance.now();
const module = new WebAssembly.Module(bytes);
const compileMs = performance.now() - compileStarted;
const glue = await import(`${pathToFileURL(gluePath).href}?sample=${sampleId}`);
const instantiateStarted = performance.now();
const exports = glue.initSync({ module });
const instantiateMs = performance.now() - instantiateStarted;
const memory = exports.memory;
if (!(memory instanceof WebAssembly.Memory)) {
  throw new Error('combined cache WASM did not expose its memory');
}
process.stdout.write(
  `${JSON.stringify({
    compileMs,
    instantiateMs,
    compileInstantiateMs: compileMs + instantiateMs,
    linearMemoryBytes: memory.buffer.byteLength,
  })}\n`
);
