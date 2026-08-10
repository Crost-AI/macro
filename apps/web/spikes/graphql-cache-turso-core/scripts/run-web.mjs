#!/usr/bin/env node

import fs from 'node:fs';
import { pathToFileURL } from 'node:url';
import { performance } from 'node:perf_hooks';

const [generatedModulePath, wasmPath] = process.argv.slice(2);
if (!generatedModulePath || !wasmPath) {
  console.error('usage: node scripts/run-web.mjs <generated-web.js> <module.wasm>');
  process.exit(2);
}

const generated = await import(pathToFileURL(generatedModulePath).href);
const bytes = fs.readFileSync(wasmPath);
const instantiateStart = performance.now();
generated.initSync({ module: bytes });
const instantiateMs = performance.now() - instantiateStart;
const linearBefore = generated.linear_memory_bytes();
const sqlStart = performance.now();
const result = generated.run_sql_spike();
const sqlMs = performance.now() - sqlStart;

console.log(
  JSON.stringify(
    {
      node: process.version,
      bindgen_target: 'web',
      optimized: true,
      instantiate_ms: instantiateMs,
      sql_ms: sqlMs,
      linear_memory_bytes: {
        before: linearBefore,
        after_sql: generated.linear_memory_bytes(),
      },
      result,
    },
    null,
    2,
  ),
);
