#!/usr/bin/env node
'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');

const path = process.argv[2];
if (!path) {
  console.error('usage: node scripts/inspect-web-glue.cjs <generated-web.js>');
  process.exit(2);
}

const source = fs.readFileSync(path, 'utf8');
const forbidden = {
  shared_array_buffer: /\bSharedArrayBuffer\b/.test(source),
  atomics: /\bAtomics\b/.test(source),
  worker_constructor: /\bnew\s+Worker\s*\(/.test(source),
  worker_threads: /\bworker_threads\b/.test(source),
  shared_memory_option: /\bshared\s*:\s*true\b/.test(source),
};
const required = {
  esm_sql_export: /export function run_sql_spike\(\)/.test(source),
  init_sync_export: /export \{ initSync,/.test(source),
  relative_wasm_url: /new URL\('turso_core_wasm_spike_bg\.wasm', import\.meta\.url\)/.test(source),
};
const compliant = !Object.values(forbidden).some(Boolean) && Object.values(required).every(Boolean);
console.log(
  JSON.stringify(
    {
      path,
      bytes: Buffer.byteLength(source),
      sha256: crypto.createHash('sha256').update(source).digest('hex'),
      forbidden_constructs_present: forbidden,
      required_web_glue_present: required,
      browser_glue_compliant: compliant,
    },
    null,
    2,
  ),
);
if (!compliant) process.exit(1);
