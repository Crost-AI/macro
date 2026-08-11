#!/usr/bin/env node
import crypto from 'node:crypto';
import fs from 'node:fs';

const [kind, sourcePath] = process.argv.slice(2);
if (!['web', 'node'].includes(kind) || !sourcePath) {
  console.error('usage: scripts/inspect-glue.mjs <web|node> <generated-js>');
  process.exit(2);
}
const source = fs.readFileSync(sourcePath, 'utf8');
const count = (pattern) => [...source.matchAll(pattern)].length;
const modules = [
  ...source.matchAll(/(?:require\(|from\s+)["']([^"']+)["']/g),
].map((match) => match[1]);
const evidence = {
  kind,
  path: sourcePath,
  bytes: Buffer.byteLength(source),
  sha256: crypto.createHash('sha256').update(source).digest('hex'),
  modules,
  clock_time: {
    date_now_calls: count(/\bDate\.now\s*\(/g),
    new_date_calls: count(/\bnew\s+Date\s*\(/g),
    performance_now_calls: count(/\bperformance\.now\s*\(/g),
  },
  random_crypto: {
    get_random_values_sites: count(/\bgetRandomValues\s*\(/g),
    random_fill_sync_sites: count(/\brandomFillSync\s*\(/g),
    crypto_references: count(/\bcrypto\b/g),
  },
  filesystem: {
    node_fs_imports: modules.filter((module) => module === 'fs' || module === 'node:fs'),
    browser_filesystem_api_sites: count(/FileSystem(?:SyncAccessHandle|FileHandle|DirectoryHandle)/g),
  },
  forbidden_runtime_constructs: {
    shared_array_buffer: /\bSharedArrayBuffer\b/.test(source),
    atomics: /\bAtomics\b/.test(source),
    worker_constructor: /\bnew\s+(?:Shared)?Worker\s*\(/.test(source),
    worker_threads: /\bworker_threads\b/.test(source),
    import_scripts: /\bimportScripts\s*\(/.test(source),
    wasi: /\bwasi(?:_|:|\b)/i.test(source),
    shared_memory_option: /\bshared\s*:\s*true\b/.test(source),
    memory64: /\bmemory64\b/i.test(source),
  },
  required_web_glue:
    kind === 'web'
      ? {
          relative_wasm_url: /new URL\('temp_fix_bg\.wasm', import\.meta\.url\)/.test(source),
          init_sync_export: /export \{ initSync,/.test(source),
          wp04_export: /export function run_wp04_contract\(\)/.test(source),
        }
      : null,
};
const forbidden = Object.values(evidence.forbidden_runtime_constructs).some(Boolean);
const webFsFree = kind !== 'web' || evidence.filesystem.node_fs_imports.length === 0;
const required =
  kind !== 'web' || Object.values(evidence.required_web_glue).every(Boolean);
evidence.compliant = !forbidden && webFsFree && required;
console.log(JSON.stringify(evidence, null, 2));
if (!evidence.compliant) process.exit(1);
