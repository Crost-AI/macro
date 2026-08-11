#!/usr/bin/env node
'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');

const workerPath = process.argv[2];
const mainPath = process.argv[3];
if (!workerPath || !mainPath) {
  console.error('usage: node scripts/inspect-worker-source.cjs <worker.js> <main.js>');
  process.exit(2);
}
const worker = fs.readFileSync(workerPath, 'utf8');
const main = fs.readFileSync(mainPath, 'utf8');
const workerForbidden = {
  nested_worker_new: /\bnew\s+(?:globalThis\.)?Worker\s*\(/.test(worker),
  shared_worker: /\bSharedWorker\b/.test(worker),
  import_scripts: /\bimportScripts\s*\(/.test(worker),
  worker_threads: /\bworker_threads\b/.test(worker),
  shared_array_buffer_allocation: /\bnew\s+SharedArrayBuffer\s*\(/.test(worker),
  atomics_use: /\bAtomics\s*\./.test(worker),
  dynamic_worker_constructor:
    /(?:globalThis|self)\s*\[\s*['"](?:Shared)?Worker['"]\s*\]/.test(worker),
  wasm_memory_constructor: /new\s+WebAssembly\.Memory\s*\(/.test(worker),
  dynamic_code: /\beval\s*\(|new\s+Function\s*\(/.test(worker),
  pthread_marker: /\bpthread|__wbindgen_thread/i.test(worker),
};
const topLevelConstructors = main.match(/\bnew\s+Worker\s*\(/g) ?? [];
const required = {
  exactly_one_top_level_worker_site: topLevelConstructors.length === 1,
  lazy_constructor_class:
    /class ProbeWorker[\s\S]*constructor\([^)]*\)[\s\S]*new Worker\(/.test(main),
  operation_queue: /operationQueue = operationQueue\.then\(run, run\)/.test(worker),
  runtime_nested_worker_monitor: /nestedWorkerConstructionCount \+= 1/.test(worker),
  kill_rpc_kept_pending: /Deliberately keep this RPC pending/.test(worker),
  first_commit_event: /kill-first-commit/.test(worker),
  finite_kill_count_before_reset:
    /sql_count_kill_probe/.test(worker) && /committed_rows < .*finite_bound/s.test(main),
  bounded_full_recovery_rpc:
    /recoverAfterKill/.test(worker) &&
    /remainingMs/.test(worker) &&
    /scope: "web-lock\+preopen\+sql-count\+close\+removeEntry\+recreate"/.test(main),
  recovery_candidates_terminated:
    /candidate\.terminate\(\);[\s\S]*candidateTerminations \+= 1/.test(main),
  actual_reset_failure_probes:
    /removeEntryFailureProbe/.test(worker) && /recreationFailureProbe/.test(worker),
  both_immediate_transaction_modes:
    /\["immediate", "exclusive"\]/.test(main) && /run_transaction_mode_probe/.test(worker),
  full_cache_sql_contract:
    /run_full_cache_sql_probe/.test(worker) && /verify_full_cache_sql_persistence/.test(worker),
  warm_start_persistence: /warmStartPersistence/.test(main) && /runPhase === "warm"/.test(main),
  production_trap_accounting:
    /productionReachableWasmTrapCount/.test(worker) &&
    /expectedNegativeWasmTrapCount/.test(worker) &&
    /unhandledRuntimeFailureCount/.test(worker) &&
    /pageUnhandledRuntimeFailureCount/.test(main) &&
    /workerRuntimeObservations/.test(main),
  all_rpc_runtime_evidence: /resultWithEvidence/.test(worker) && /runtimeEvidence/.test(worker),
  active_kill_runtime_evidence:
    /event: "kill-first-commit"[\s\S]*runtimeEvidence: runtimeEvidence\(\)/.test(worker),
  poisoned_worker_runtime_evidence:
    /lifecycleFailureProbe/.test(main) &&
    /removeEntryFailureProbe/.test(main) &&
    /recreationFailureProbe/.test(main) &&
    /assertHeadRuntimeSafety/.test(main),
  recorded_errors_non_trap: /allRecordedProductionOrControlErrorsNonTrap/.test(main),
  isolated_explicit_temp_negative:
    /explicitTempNegativeProbe/.test(worker) &&
    /explicit-temp-negative/.test(worker) &&
    /expectedNegativeWasmTrapCount === 1/.test(main),
  parent_differential_mode: /parent-failure/.test(main) && /differentialExpectedFailure/.test(main),
};
const compliant =
  !Object.values(workerForbidden).some(Boolean) && Object.values(required).every(Boolean);
console.log(
  JSON.stringify(
    {
      workerPath,
      mainPath,
      workerSha256: crypto.createHash('sha256').update(worker).digest('hex'),
      mainSha256: crypto.createHash('sha256').update(main).digest('hex'),
      workerForbidden,
      required,
      source_contract_compliant: compliant,
    },
    null,
    2,
  ),
);
if (!compliant) process.exit(1);
