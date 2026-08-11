import { readFile } from "node:fs/promises";

const generated = new URL("../measurements/generated/", import.meta.url);
const matrix = JSON.parse(await readFile(new URL("browser-matrix.actual.json", generated)));
const sourceInspection = JSON.parse(
  await readFile(new URL("worker-source-inspection.actual.json", generated)),
);
if (!sourceInspection.source_contract_compliant) {
  throw new Error("worker source inspection did not prove the no-nested-worker contract");
}
if (matrix.transactionExpectation !== "head-success") {
  throw new Error("HEAD browser matrix used the wrong transaction expectation");
}

for (const name of ["chromium", "firefox"]) {
  const browserRuns = matrix.runs.filter((run) => run.name === name);
  const coldRuns = browserRuns.filter((run) => run.phase === "cold");
  const warmRuns = browserRuns.filter((run) => run.phase === "warm");
  if (coldRuns.length !== 2 || warmRuns.length !== 2) {
    throw new Error(`${name} did not complete two cold and two warm runs`);
  }
  for (const run of browserRuns) assertOperationalRun(run);
}

const webkitRuns = matrix.runs.filter((run) => run.name === "webkit-wpe");
if (webkitRuns.length !== 1) throw new Error("WebKit WPE was not honestly run exactly once");
const webkit = webkitRuns[0];
if (webkit.harnessError) throw new Error(`WebKit harness failed to report: ${webkit.harnessError.message}`);
if (webkit.report?.capabilities?.storageGetDirectory !== false) {
  throw new Error("WebKit WPE result changed; review the G0 recommendation");
}
if (
  webkit.report?.pass !== false ||
  webkit.report?.error?.wasmEnvironmentTrap !== false ||
  !/OPFS getDirectory is unavailable/.test(webkit.report?.error?.message ?? "")
) {
  throw new Error("WebKit WPE did not record the known non-trap worker OPFS capability failure");
}
if (
  webkit.report.runtimeSafety?.workerRuntimeObservationCount !== 1 ||
  webkit.report.runtimeSafety?.maxProductionReachableWasmTrapCount !== 0 ||
  webkit.report.runtimeSafety?.maxUnhandledWorkerRuntimeFailureCount !== 0
) {
  throw new Error("WebKit WPE worker path lacked clean runtime evidence");
}

for (const run of matrix.runs) {
  if (
    run.responseIsolationHeaders?.crossOriginOpenerPolicy !== null ||
    run.responseIsolationHeaders?.crossOriginEmbedderPolicy !== null ||
    run.report?.page?.crossOriginIsolated !== false
  ) {
    throw new Error(`${run.runId} unexpectedly used cross-origin isolation`);
  }
}

console.log(
  "browser assertions passed: two cold/two warm full runs in Chromium and Firefox; WebKit WPE honestly lacks worker OPFS",
);

function assertOperationalRun(run) {
  const { report } = run;
  const label = run.runId;
  if (run.harnessError) throw new Error(`${label} harness error: ${run.harnessError.message}`);
  if (report?.operationalPass !== true) {
    throw new Error(`${label} operational probe failed: ${report?.error?.message}`);
  }
  if (
    report.pass !== false ||
    report.fullCacheSqlContractPass !== false ||
    report.error?.name !== "ConformanceFailure"
  ) {
    throw new Error(`${label} WP-04 conformance result changed; review Gate G0`);
  }
  if (report.workersBeforeFirstUse !== 0) throw new Error(`${label} worker was not lazy`);
  if (report.webLockExclusion.contenderAcquired) throw new Error(`${label} Web Lock exclusion failed`);
  if (report.noNestedWorker.constructionCount !== 0) throw new Error(`${label} nested worker constructed`);
  if (run.phase === "warm" && report.warmStartPersistence?.value?.value !== "recovered-fresh") {
    throw new Error(`${label} did not prove warm-start persistence before reset`);
  }
  for (const mode of ["immediate", "exclusive"]) {
    const transaction = report.transactionModes?.[mode];
    if (
      transaction?.succeeded !== true ||
      transaction.result?.value?.committed_rows !== 1 ||
      transaction.result?.value?.rollback_preserved !== true
    ) {
      throw new Error(`${label} BEGIN ${mode.toUpperCase()} did not commit and roll back`);
    }
    if (
      transaction.shutdown?.evidence?.productionReachableWasmTrapCount !== 0 ||
      transaction.shutdown?.evidence?.unhandledRuntimeFailureCount !== 0
    ) {
      throw new Error(`${label} BEGIN ${mode.toUpperCase()} reached a WASM environment trap`);
    }
  }

  const cache = report.fullCacheSql?.value;
  for (const field of [
    "begin_immediate",
    "begin_exclusive",
    "ddl_rollback",
    "bound_text_blob_integer_null",
    "upsert_delete_affected_rows",
    "strict_head_fencing",
    "complete_discard_cascade",
    "foreign_key_violation_rejected",
    "autoincrement_nonreuse",
    "clear_atomic",
  ]) {
    if (cache?.[field] !== true) throw new Error(`${label} full cache SQL failed ${field}`);
  }
  const expectedForeignKeyViolation = {
    column_count: 4,
    rows: [
      {
        table: "optimistic_layers",
        rowid: 9_999_999,
        parent: "mutation_queue",
        fkid: 0,
      },
    ],
  };
  if (
    JSON.stringify(cache.foreign_key_check_expected_violation) !==
      JSON.stringify(expectedForeignKeyViolation) ||
    cache.foreign_key_check_violation_shape !== false ||
    cache.foreign_key_check_deliberate_violation_rows !== 0 ||
    cache.foreign_key_check_actual_violation?.column_count !== 0 ||
    !Array.isArray(cache.foreign_key_check_actual_violation?.rows) ||
    cache.foreign_key_check_actual_violation.rows.length !== 0
  ) {
    throw new Error(`${label} foreign_key_check exact four-column limitation changed`);
  }
  if (
    cache.quick_check !== "ok" ||
    cache.foreign_key_check_rows !== 0 ||
    cache.persisted_record_rows !== 3 ||
    JSON.stringify(cache.canonical_scan) !==
      JSON.stringify(["Type0:1", "Type:9", "Type:tenant:1"]) ||
    JSON.stringify(cache.exclusive_cursor) !== JSON.stringify(["Type:tenant:1"])
  ) {
    throw new Error(`${label} full cache SQL result shape/order/check failed`);
  }
  for (const persistence of [
    report.sameWorkerCachePersistence?.value,
    report.crossWorkerCachePersistence?.value,
  ]) {
    if (
      persistence?.metadata_rows !== 3 ||
      persistence?.record_rows !== 3 ||
      persistence?.queue_rows !== 0 ||
      persistence?.quick_check !== "ok" ||
      persistence?.foreign_key_check_rows !== 0
    ) {
      throw new Error(`${label} clean/cross-worker cache persistence failed`);
    }
  }

  if (!report.lifecycleFailure.closeFailed || !report.lifecycleFailure.deleteRejected) {
    throw new Error(`${label} did not poison uncertain close and reject delete`);
  }
  assertZeroProductionRuntimeEvidence(
    report.lifecycleFailure.runtimeEvidence,
    `${label} lifecycle failure worker`,
  );
  if (!report.lifecycleFailure.reopenRejected || !report.lifecycleFailure.lifecycle.startsWith("poisoned:")) {
    throw new Error(`${label} poisoned registry reopened`);
  }
  const removeFailure = report.removeEntryFailure;
  assertZeroProductionRuntimeEvidence(
    removeFailure.runtimeEvidence,
    `${label} removeEntry failure worker`,
  );
  if (
    !removeFailure.resetFailed ||
    !removeFailure.actualRemoveEntryFailure ||
    !removeFailure.secondResetRejected ||
    !removeFailure.reopenRejected ||
    !removeFailure.lifecycle.startsWith("poisoned:") ||
    !removeFailure.artifactCleaned
  ) {
    throw new Error(`${label} actual removeEntry failure was not deterministic and poisoned`);
  }
  const recreationFailure = report.recreationFailure;
  assertZeroProductionRuntimeEvidence(
    recreationFailure.runtimeEvidence,
    `${label} recreation failure worker`,
  );
  if (
    !recreationFailure.resetFailed ||
    !recreationFailure.actualRecreationFailure ||
    !recreationFailure.secondResetRejected ||
    !recreationFailure.reopenRejected ||
    !recreationFailure.lifecycle.startsWith("poisoned:") ||
    !recreationFailure.artifactCleaned
  ) {
    throw new Error(`${label} actual recreation failure was not deterministic and poisoned`);
  }
  if (!Array.isArray(report.serializedQueue) || report.serializedQueue.length !== 2) {
    throw new Error(`${label} serialized operation queue was not covered`);
  }

  const direct = report.directFile.operations;
  for (const field of [
    "empty_write_callbacks",
    "write_callbacks",
    "partial_write_callbacks",
    "read_callbacks",
    "short_read_callbacks",
    "eof_callbacks",
    "detected_short_read_callbacks",
    "zero_write_callbacks",
    "error_write_callbacks",
    "quota_write_callbacks",
  ]) {
    if (direct[field] !== 1) throw new Error(`${label} ${field} != 1`);
  }
  if (
    direct.empty_write_bytes !== 0 ||
    !direct.partial_write_retried ||
    direct.zero_write_error !== "ShortWrite" ||
    !direct.error_preserved ||
    !direct.quota_preserved ||
    direct.eof_bytes !== 0 ||
    direct.short_read_bytes !== 2
  ) {
    throw new Error(`${label} direct-file completion semantics failed`);
  }

  const kill = report.workerKill;
  if (!kill.terminatedWhileCallActive || !kill.pendingRpcRejected) {
    throw new Error(`${label} kill RPC was not pending and rejected at termination`);
  }
  if (kill.firstCommit.commitCount !== 1 || kill.firstCommit.finiteBound !== 10_000) {
    throw new Error(`${label} did not wait for the first successful commit`);
  }
  assertZeroProductionRuntimeEvidence(
    kill.firstCommit.runtimeEvidence,
    `${label} actively killed worker first-commit event`,
  );
  if (
    kill.firstCommit.preSizes["graphql-cache.db"] < 0 ||
    kill.firstCommit.postSizes["graphql-cache.db-wal"] <= 0
  ) {
    throw new Error(`${label} did not record valid pre/post kill sizes`);
  }

  const recovery = report.recovery;
  if (
    recovery.scope !== "web-lock+preopen+sql-count+close+removeEntry+recreate" ||
    recovery.deadlineMs !== 30_000 ||
    recovery.elapsedMs < 0 ||
    recovery.elapsedMs > recovery.deadlineMs ||
    recovery.candidateTerminations !== recovery.attempts ||
    recovery.unsuccessfulCandidateTerminations !== recovery.attempts - 1 ||
    recovery.successfulAttemptRemainingMs <= 0
  ) {
    throw new Error(`${label} complete OPFS recovery was not bounded`);
  }
  const committed = report.abruptReset.committed;
  if (
    committed.finite_bound !== kill.firstCommit.finiteBound ||
    committed.committed_rows < 1 ||
    committed.committed_rows >= committed.finite_bound
  ) {
    throw new Error(`${label} did not prove active termination before loop completion`);
  }
  const before = report.abruptReset.preopenSizes;
  const reset = report.abruptReset.reset;
  if (
    before["graphql-cache.db"] <= 0 ||
    before["graphql-cache.db-wal"] <= 0 ||
    !reset.deleted["graphql-cache.db"] ||
    !reset.deleted["graphql-cache.db-wal"] ||
    reset.recreated["graphql-cache.db"] !== 0 ||
    reset.recreated["graphql-cache.db-wal"] !== 0 ||
    report.freshRecovery.value.count_before !== 0
  ) {
    throw new Error(`${label} abrupt reset/fresh recovery failed`);
  }

  const explicitTemp = report.explicitTempNegative;
  if (
    explicitTemp?.retainedTempBackend !== "turso_core::MemoryIO" ||
    explicitTemp?.expectedTrap?.wasmEnvironmentTrap !== true ||
    explicitTemp.expectedTrap.routeClassification !== "explicit-temp-negative" ||
    explicitTemp.runtimeEvidence?.productionReachableWasmTrapCount !== 0 ||
    explicitTemp.runtimeEvidence?.expectedNegativeWasmTrapCount !== 1 ||
    explicitTemp.runtimeEvidence?.unhandledRuntimeFailureCount !== 0 ||
    explicitTemp.runtimeEvidence?.workerRouteClassification !==
      "explicit-temp-negative" ||
    !/std::time::Instant::now|not implemented on this platform/.test(
      explicitTemp.expectedTrap.stack ?? "",
    ) ||
    !/ensure_temp_database|create_temp_database|open_file_with_flags/.test(
      explicitTemp.expectedTrap.stack ?? "",
    )
  ) {
    throw new Error(`${label} explicit-temp negative trap evidence changed`);
  }

  const safety = report.runtimeSafety;
  if (
    safety?.pageUnhandledRuntimeFailureCount !== 0 ||
    safety?.pageWasmEnvironmentTrapCount !== 0 ||
    safety?.workerErrorEventCount !== 0 ||
    safety?.maxProductionReachableWasmTrapCount !== 0 ||
    safety?.maxExpectedNegativeWasmTrapCount !== 1 ||
    safety?.maxUnhandledWorkerRuntimeFailureCount !== 0 ||
    safety?.allRecordedProductionOrControlErrorsNonTrap !== true ||
    safety?.expectedNegativeErrorCount !== 1 ||
    !Array.isArray(safety?.workerRuntimeObservations) ||
    safety.workerRuntimeObservations.length === 0
  ) {
    throw new Error(`${label} observed a production-route WASM/runtime trap`);
  }
  for (const observation of safety.workerRuntimeObservations) {
    assertZeroProductionRuntimeEvidence(observation, `${label} ${observation.source}`);
    const expectedNegativeCount =
      observation.source === "rpc:explicitTempNegativeProbe" ? 1 : 0;
    const expectedRoute = observation.source === "rpc:explicitTempNegativeProbe"
      ? "explicit-temp-negative"
      : observation.workerRouteClassification;
    if (
      observation.expectedNegativeWasmTrapCount !== expectedNegativeCount ||
      !["production", "explicit-temp-negative"].includes(expectedRoute) ||
      (observation.workerRouteClassification === "explicit-temp-negative" &&
        ![
          "rpc:initOwner",
          "rpc:resetTransactionProbe",
          "rpc:explicitTempNegativeProbe",
        ].includes(observation.source)) ||
      (observation.source === "rpc:explicitTempNegativeProbe" &&
        observation.workerRouteClassification !== "explicit-temp-negative")
    ) {
      throw new Error(`${label} expected-negative evidence leaked across worker routes`);
    }
  }
  for (const source of [
    "rpc:lifecycleFailureProbe",
    "rpc:removeEntryFailureProbe",
    "rpc:recreationFailureProbe",
    "event:kill-first-commit",
    "rpc:recoverAfterKill",
    "rpc:explicitTempNegativeProbe",
  ]) {
    if (!safety.workerRuntimeObservations.some((observation) => observation.source === source)) {
      throw new Error(`${label} did not record runtime evidence for ${source}`);
    }
  }
  const errorRecords = collectErrorRecords(report);
  const expectedNegativeErrors = errorRecords.filter(
    (error) => error.routeClassification === "explicit-temp-negative",
  );
  const otherErrors = errorRecords.filter(
    (error) => error.routeClassification !== "explicit-temp-negative",
  );
  if (
    expectedNegativeErrors.length !== 1 ||
    otherErrors.some((error) => error.wasmEnvironmentTrap !== false)
  ) {
    throw new Error(`${label} recorded an unexpected WASM trap in an error record`);
  }
  if (
    run.consoleMessages.some((message) =>
      /panicked at|RuntimeError: unreachable|not implemented on this platform/i.test(message),
    )
  ) {
    throw new Error(`${label} console contained an unaccounted WASM environment trap`);
  }
}

function assertZeroProductionRuntimeEvidence(evidence, label) {
  if (
    evidence?.productionReachableWasmTrapCount !== 0 ||
    evidence?.unhandledRuntimeFailureCount !== 0 ||
    !["production", "explicit-temp-negative"].includes(evidence?.workerRouteClassification)
  ) {
    throw new Error(`${label} recorded a production trap or unhandled failure`);
  }
}

function collectErrorRecords(value, records = []) {
  if (value === null || typeof value !== "object") return records;
  if (typeof value.wasmEnvironmentTrap === "boolean") records.push(value);
  for (const entry of Array.isArray(value) ? value : Object.values(value)) {
    collectErrorRecords(entry, records);
  }
  return records;
}
