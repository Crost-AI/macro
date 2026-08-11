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
  worker_constructor: /\bnew\s+(?:globalThis\.)?Worker\s*\(/.test(source),
  shared_worker: /\bSharedWorker\b/.test(source),
  import_scripts: /\bimportScripts\s*\(/.test(source),
  worker_threads: /\bworker_threads\b/.test(source),
  shared_memory_option: /\bshared\s*:\s*true\b/.test(source),
  dynamic_worker_constructor:
    /(?:globalThis|self|window)\s*\[\s*['"](?:Shared)?Worker['"]\s*\]/.test(source),
  wasm_memory_constructor: /new\s+WebAssembly\.Memory\s*\(/.test(source),
  dynamic_code: /\beval\s*\(|new\s+Function\s*\(/.test(source),
  node_runtime_import: /\bnode:|\brequire\s*\(/.test(source),
  pthread_marker: /\bpthread|__wbindgen_thread/i.test(source),
};
const requiredExports = [
  'begin_database_session',
  'begin_direct_probe_session',
  'begin_transaction_probe_session',
  'claim_owner',
  'close_session',
  'inject_next_recreation_conflict',
  'registry_lifecycle',
  'reset_closed_session_paths',
  'run_direct_file_probe',
  'run_explicit_temp_negative_probe',
  'run_full_cache_sql_probe',
  'run_transaction_mode_probe',
  'run_worker_kill_write_loop',
  'sql_count_kill_probe',
  'sql_write_marker',
  'verify_full_cache_sql_persistence',
];
const required = {
  lifecycle_exports: requiredExports.every((name) =>
    new RegExp(`export function ${name}\\(`).test(source),
  ),
  init_sync_export: /export \{ initSync,/.test(source),
  relative_wasm_url: /new URL\(['"]turso_opfs_spike_bg\.wasm['"], import\.meta\.url\)/.test(source),
  kill_progress_import: /globalThis\.__tursoOpfsKillProgress/.test(source),
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
