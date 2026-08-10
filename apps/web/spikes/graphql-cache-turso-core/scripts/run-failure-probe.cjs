#!/usr/bin/env node
'use strict';

Error.stackTraceLimit = 100;

const path = require('node:path');
const [generatedDir, probeName] = process.argv.slice(2);
if (!generatedDir || !probeName) {
  console.error(
    'usage: node scripts/run-failure-probe.cjs <wasm-bindgen-output-dir> <export>',
  );
  process.exit(2);
}

const spike = require(path.resolve(generatedDir, 'turso_core_wasm_spike.cjs'));
if (typeof spike[probeName] !== 'function') {
  console.error(`missing probe export: ${probeName}`);
  process.exit(2);
}
spike[probeName]();
console.error(`probe unexpectedly succeeded: ${probeName}`);
process.exit(1);
