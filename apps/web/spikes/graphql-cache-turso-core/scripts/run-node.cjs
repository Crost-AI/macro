#!/usr/bin/env node
'use strict';

Error.stackTraceLimit = 100;

const path = require('node:path');
const { performance } = require('node:perf_hooks');

const generatedDir = process.argv[2];
const artifact = process.argv[3] ?? 'wasm-bindgen-node';
if (!generatedDir) {
  console.error('usage: node scripts/run-node.cjs <wasm-bindgen-output-dir> [artifact-label]');
  process.exit(2);
}

const modulePath = path.resolve(generatedDir, 'turso_core_wasm_spike.cjs');
const rssBefore = process.memoryUsage().rss;
const instantiateStart = performance.now();
const spike = require(modulePath);
const instantiateMs = performance.now() - instantiateStart;
const linearBefore = spike.linear_memory_bytes();
const firstOpenStart = performance.now();
spike.run_open_close_spike();
const firstOpenCloseMs = performance.now() - firstOpenStart;
const linearAfterFirstOpen = spike.linear_memory_bytes();
const warmOpenStart = performance.now();
spike.run_open_close_spike();
const warmOpenCloseMs = performance.now() - warmOpenStart;
const firstStart = performance.now();
const firstResult = spike.run_sql_spike();
const firstSqlMs = performance.now() - firstStart;
const linearAfterFirst = spike.linear_memory_bytes();
const warmStart = performance.now();
const warmResult = spike.run_sql_spike();
const warmSqlMs = performance.now() - warmStart;
const linearAfterWarm = spike.linear_memory_bytes();
const rssAfter = process.memoryUsage().rss;

console.log(
  JSON.stringify(
    {
      node: process.version,
      artifact,
      optimized: artifact.includes('optimized'),
      instantiate_ms: instantiateMs,
      first_open_close_ms: firstOpenCloseMs,
      warm_open_close_ms: warmOpenCloseMs,
      first_sql_ms: firstSqlMs,
      warm_sql_ms: warmSqlMs,
      linear_memory_bytes: {
        before: linearBefore,
        after_first_open: linearAfterFirstOpen,
        after_first_sql: linearAfterFirst,
        after_warm_sql: linearAfterWarm,
      },
      process_rss_bytes: {
        before: rssBefore,
        after: rssAfter,
        delta: rssAfter - rssBefore,
      },
      first_result: firstResult,
      warm_result: warmResult,
    },
    null,
    2,
  ),
);
