#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const spikeRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const runs = [];

function normalize(text) {
  return text
    .replaceAll(spikeRoot, '<SPIKE_ROOT>')
    .replace(/\u001b\[[0-9;]*m/g, '')
    .trim();
}

function run(variant, action, repetition) {
  const child = spawnSync(
    process.execPath,
    ['scripts/run-wasm-action.cjs', variant, action],
    { cwd: spikeRoot, encoding: 'utf8' },
  );
  if (child.status !== 0) {
    throw new Error(
      `${variant}/${action} runner exited ${child.status}: ${child.stderr}`,
    );
  }
  const result = JSON.parse(child.stdout);
  result.repetition = repetition;
  result.stderr = normalize(child.stderr);
  result.stack = normalize(result.stack ?? '');
  runs.push(result);
  return result;
}

function expectTrap(result, label, requiresTempPath = true) {
  if (result.outcome !== 'trap') throw new Error(`${label} unexpectedly succeeded`);
  if (result.error_name !== 'RuntimeError' || result.error_message !== 'unreachable') {
    throw new Error(`${label} returned an unexpected trap: ${JSON.stringify(result)}`);
  }
  if (!result.stack.includes('std::time::Instant::now')) {
    throw new Error(`${label} did not preserve the unsupported std::time stack frame`);
  }
  if (requiresTempPath && !result.stack.includes('ensure_temp_database')) {
    throw new Error(`${label} did not preserve the internal temp-database stack frame`);
  }
}

function expectUnusedSuccess(result, mode) {
  if (result.outcome !== 'success') throw new Error(`head ${mode} trapped`);
  if (result.result.temp_database_listed !== false) {
    throw new Error(`head ${mode} still listed temp`);
  }
  if (JSON.stringify(result.result.database_names) !== JSON.stringify(['main'])) {
    throw new Error(`head ${mode} database_list changed`);
  }
  if (result.result.io.monotonic_clock_calls < 1) {
    throw new Error(`head ${mode} did not use the supplied custom clock`);
  }
}

function expectWp04Coverage(report) {
  if ('full_wp04_gate_passed' in report) {
    throw new Error('obsolete full WP-04 gate claim remains in runtime evidence');
  }
  if (report.runnable_wp04_sql_passed !== true) {
    throw new Error('head runnable WP-04 SQL contract did not pass');
  }
  const statuses = new Map(
    report.coverage_matrix.map((item) => [item.requirement, item.status]),
  );
  if (
    statuses.get('foreign_key_check_valid_and_invalid_result_shape') !==
    'tested_failed'
  ) {
    throw new Error('foreign_key_check failure is absent from the coverage matrix');
  }
  for (const requirement of [
    'rollback_io_failure_classification',
    'application_reset_after_uncertain_commit_or_rollback',
    'physical_reset_for_metadata_schema_integrity_and_scope_mismatch',
    'cache_core_codec_corruption_and_storage_trait_conformance',
    'real_opfs_quota_private_mode_eviction_and_crash_durability',
  ]) {
    if (statuses.get(requirement) !== 'not_tested') {
      throw new Error(`WP-04 untested requirement was not explicit: ${requirement}`);
    }
  }
}

for (let repetition = 1; repetition <= 5; repetition += 1) {
  expectTrap(run('parent', 'immediate', repetition), 'parent immediate');
  expectTrap(run('parent', 'exclusive', repetition), 'parent exclusive');
  expectTrap(run('parent', 'wp04', repetition), 'parent WP-04');
  expectUnusedSuccess(run('head', 'immediate', repetition), 'immediate');
  expectUnusedSuccess(run('head', 'exclusive', repetition), 'exclusive');
  const wp04 = run('head', 'wp04', repetition);
  if (wp04.outcome !== 'success') throw new Error('head WP-04 trapped');
  for (const field of [
    'ddl_dml_passed',
    'canonical_scan_passed',
    'queue_contract_passed',
    'transaction_contract_passed',
    'clean_reopen_passed',
    'foreign_keys_connection_local',
    'conversion_contract_passed',
  ]) {
    if (wp04.result[field] !== true) throw new Error(`head WP-04 ${field} failed`);
  }
  if (wp04.result.foreign_key_check_supported !== false) {
    throw new Error('head WP-04 unsupported pragma was not reported honestly');
  }
  expectWp04Coverage(wp04.result);
}

for (const variant of ['parent', 'head']) {
  expectTrap(
    run(variant, 'explicit_temp_create', 1),
    `${variant} explicit temp create`,
  );
  expectTrap(
    run(variant, 'temp_after_immediate', 1),
    `${variant} temp after immediate`,
  );
  expectTrap(
    run(variant, 'builtin_memory_io', 1),
    `${variant} built-in MemoryIO`,
    false,
  );
}

const summary = {
  node: process.version,
  repetitions: 5,
  fresh_process_per_action: true,
  expectations: {
    parent_unused_immediate_and_exclusive: 'trap',
    parent_wp04_immediate_contract: 'trap',
    head_unused_immediate_and_exclusive: 'success_without_temp',
    head_wp04_runnable_contract:
      'tested_sql_success_with_foreign_key_check_failure_and_explicit_untested_matrix',
    explicit_temp_both_revisions: 'trap_at_builtin_temp_MemoryIO_clock',
    builtin_memory_io_both_revisions: 'trap_at_std_time',
  },
  runs,
};
fs.mkdirSync(path.join(spikeRoot, 'target', 'evidence'), { recursive: true });
fs.writeFileSync(
  path.join(spikeRoot, 'target', 'evidence', 'wasm-runtime-matrix.json'),
  `${JSON.stringify(summary, null, 2)}\n`,
);
console.log(`recorded ${runs.length} fresh-process WASM actions`);
