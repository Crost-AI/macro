#!/usr/bin/env node
'use strict';

Error.stackTraceLimit = 100;
const path = require('node:path');

const [variant, action] = process.argv.slice(2);
if (!['parent', 'head'].includes(variant) || !action) {
  console.error('usage: scripts/run-wasm-action.cjs <parent|head> <action>');
  process.exit(2);
}

const modulePath = path.resolve('target', 'wasm', variant, 'node', 'temp_fix.cjs');
const spike = require(modulePath);
const actions = {
  immediate: 'run_unused_immediate_probe',
  exclusive: 'run_unused_exclusive_probe',
  wp04: 'run_wp04_contract',
  explicit_temp_create: 'run_explicit_temp_create_probe',
  temp_after_immediate: 'run_temp_after_immediate_probe',
  builtin_memory_io: 'run_builtin_memory_io_probe',
};
const exportName = actions[action];
if (!exportName || typeof spike[exportName] !== 'function') {
  console.error(`unknown action or missing export: ${action}`);
  process.exit(2);
}

try {
  const raw = spike[exportName]();
  let result = raw ?? null;
  if (typeof raw === 'string') {
    result = JSON.parse(raw);
  }
  console.log(
    JSON.stringify({
      variant,
      action,
      outcome: 'success',
      result,
      linear_memory_bytes: spike.linear_memory_bytes(),
    }),
  );
} catch (error) {
  console.log(
    JSON.stringify({
      variant,
      action,
      outcome: 'trap',
      error_name: error?.name ?? typeof error,
      error_message: String(error?.message ?? error),
      stack: String(error?.stack ?? error),
      linear_memory_bytes: spike.linear_memory_bytes(),
    }),
  );
}
