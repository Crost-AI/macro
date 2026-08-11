import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";

const generated = new URL("../measurements/generated/", import.meta.url);
const readJson = async (name) => JSON.parse(await readFile(new URL(name, generated), "utf8"));
const matrix = await readJson("browser-matrix.actual.json");
const parent = await readJson("parent-differential.actual.json");
const provenance = await readJson("provenance.actual.json");
const wasm = await readJson("wasm-inspection.actual.json");
const glue = await readJson("web-glue-inspection.actual.json");
const source = await readJson("worker-source-inspection.actual.json");
const artifactPaths = await readJson("artifact-path-inspection.actual.json");
const standalone = await readJson("standalone-copy.actual.json");
const wasmBytes = await readFile(new URL("../pkg/turso_opfs_spike_bg.wasm", import.meta.url));

const summarizeHeadRun = (run) => {
  const report = run.report;
  const direct = report?.directFile?.operations;
  const before = report?.abruptReset?.preopenSizes;
  const reset = report?.abruptReset?.reset;
  return {
    name: run.name,
    runId: run.runId,
    phase: run.phase,
    browserVersion: run.browserVersion,
    actuallyRun: true,
    pass: report?.pass ?? false,
    operationalPass: report?.operationalPass ?? false,
    storageGetDirectory: report?.capabilities?.storageGetDirectory ?? null,
    warmStartPersistence:
      run.phase !== "warm" || report?.warmStartPersistence?.value?.value === "recovered-fresh",
    immediatePassed: report?.transactionModes?.immediate?.succeeded === true,
    exclusivePassed: report?.transactionModes?.exclusive?.succeeded === true,
    fullCacheSqlPassed: report?.fullCacheSqlContractPass === true,
    fullCacheSqlKnownFailure:
      report?.fullCacheSql?.value?.foreign_key_check_violation_shape === false &&
      report?.fullCacheSql?.value?.foreign_key_check_deliberate_violation_rows === 0 &&
      report?.fullCacheSql?.value?.foreign_key_check_actual_violation?.rows?.length === 0
        ? "PRAGMA foreign_key_check returned no decoded four-column row for a deliberate orphan"
        : null,
    cleanAndCrossWorkerPersistence:
      report?.sameWorkerCachePersistence?.value?.record_rows === 3 &&
      report?.crossWorkerCachePersistence?.value?.record_rows === 3,
    lazyWorker: report?.workersBeforeFirstUse === 0,
    noIsolationHeaders:
      run.responseIsolationHeaders?.crossOriginOpenerPolicy === null &&
      run.responseIsolationHeaders?.crossOriginEmbedderPolicy === null,
    noNestedWorker:
      report?.noNestedWorker?.runtimeMonitorInstalled === true &&
      report?.noNestedWorker?.constructionCount === 0,
    directCallbacksExact:
      direct &&
      [
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
      ].every((field) => direct[field] === 1),
    lifecycleAndActualResetFailures:
      report?.lifecycleFailure?.closeFailed === true &&
      report?.lifecycleFailure?.deleteRejected === true &&
      report?.removeEntryFailure?.actualRemoveEntryFailure === true &&
      report?.recreationFailure?.actualRecreationFailure === true,
    killAfterFirstCommit:
      report?.workerKill?.firstCommit?.commitCount === 1 &&
      report?.workerKill?.pendingRpcRejected === true,
    killLoopProvenIncomplete:
      report?.abruptReset?.committed?.committed_rows >= 1 &&
      report?.abruptReset?.committed?.committed_rows <
        report?.abruptReset?.committed?.finite_bound,
    boundedRecovery:
      report?.recovery?.scope ===
        "web-lock+preopen+sql-count+close+removeEntry+recreate" &&
      report?.recovery?.deadlineMs === 30000 &&
      report?.recovery?.elapsedMs <= report?.recovery?.deadlineMs,
    abruptResetCovered:
      before?.["graphql-cache.db"] > 0 &&
      before?.["graphql-cache.db-wal"] > 0 &&
      reset?.deleted?.["graphql-cache.db"] === true &&
      reset?.deleted?.["graphql-cache.db-wal"] === true &&
      reset?.recreated?.["graphql-cache.db"] === 0 &&
      reset?.recreated?.["graphql-cache.db-wal"] === 0,
    productionReachableWasmTraps:
      report?.runtimeSafety?.maxProductionReachableWasmTrapCount ?? null,
    unhandledWorkerRuntimeFailures:
      report?.runtimeSafety?.maxUnhandledWorkerRuntimeFailureCount ?? null,
    recordedProductionErrorsNonTrap:
      report?.runtimeSafety?.allRecordedProductionOrControlErrorsNonTrap ?? null,
    explicitTempNegativeTrap:
      report?.explicitTempNegative?.retainedTempBackend === "turso_core::MemoryIO" &&
      report?.explicitTempNegative?.expectedTrap?.wasmEnvironmentTrap === true &&
      report?.runtimeSafety?.maxExpectedNegativeWasmTrapCount === 1,
  };
};

const summarizeParentRun = (run) => ({
  name: run.name,
  browserVersion: run.browserVersion,
  actuallyRun: true,
  differentialExpectedFailure: run.report?.differentialExpectedFailure === true,
  immediateWasmTrap:
    run.report?.transactionModes?.immediate?.error?.wasmEnvironmentTrap === true,
  exclusiveWasmTrap:
    run.report?.transactionModes?.exclusive?.error?.wasmEnvironmentTrap === true,
  instantAndTempCause:
    ["immediate", "exclusive"].every((mode) => {
      const stack = run.report?.transactionModes?.[mode]?.error?.stack ?? "";
      return (
        /std::time::Instant::now|not implemented on this platform/.test(stack) &&
        /ensure_temp_database|create_temp_database|open_file_with_flags/.test(stack)
      );
    }),
});

const summary = {
  tursoRevision: provenance.tursoFork.revision,
  tursoParentRevision: provenance.tursoFork.parent,
  tursoTree: provenance.tursoFork.tree,
  forkWorktreeUnmodified: provenance.tursoFork.worktreeCleanBeforeAndAfter,
  dependency: provenance.dependency,
  playwrightVersion: matrix.playwrightVersion,
  repetitions: matrix.repetitionPlan,
  wasm: {
    bytes: wasmBytes.length,
    sha256: createHash("sha256").update(wasmBytes).digest("hex"),
    memoryCount: wasm.memories.length,
    shared: wasm.memories[0]?.shared,
    memory64: wasm.memories[0]?.memory64,
    atomicOperatorCount: wasm.atomic_operator_count,
    importsExact: wasm.imports_allowed,
    missingImports: wasm.missing_imports.length,
    unexpectedImports: wasm.unexpected_imports.length,
    duplicateImports: wasm.duplicate_imports.length,
    environmentTrapMarkersPresent: wasm.environment_trap_markers,
    browserContractCompliant: wasm.browser_contract_compliant,
  },
  glueCompliant: glue.browser_glue_compliant,
  workerSourceCompliant: source.source_contract_compliant,
  artifactAbsolutePathScanPerformed: artifactPaths.absolutePathScanPerformed,
  artifactHostSensitiveAbsolutePathFree: artifactPaths.hostSensitiveAbsolutePathFree,
  artifactReproducibleVirtualPathCounts: artifactPaths.reproducibleVirtualPathCounts,
  wasmBuild: matrix.exactToolchain.wasmBuild,
  standaloneCopy: standalone,
  headRuns: matrix.runs.map(summarizeHeadRun),
  parentDifferentialRuns: parent.runs.map(summarizeParentRun),
  safariWkWebViewActuallyRun: false,
  webkitWpeActuallyRun: matrix.runs.some((run) => run.name === "webkit-wpe"),
  webkitWpeWorkerOpfsAvailable:
    matrix.runs.find((run) => run.name === "webkit-wpe")?.report?.capabilities
      ?.storageGetDirectory ?? null,
  enumeratedHeadRoutesWithoutProductionWasmTrap: [
    "BEGIN IMMEDIATE commit/rollback",
    "BEGIN EXCLUSIVE commit/rollback",
    "selected WP-04 SQL/pragma/cache operations listed in README",
    "clean and cross-worker persistence",
    "direct-file completion/error callbacks",
    "lifecycle poison, removeEntry failure, and recreation failure",
    "active kill, bounded recovery, reset, and fresh reopen",
  ],
  retainedNegativeWasmTrap:
    "explicit CREATE TEMP TABLE reaches built-in temp MemoryIO/std::time::Instant outside enumerated production routes",
  g0ResolvedByFork: ["enumerated BEGIN IMMEDIATE route", "enumerated BEGIN EXCLUSIVE route"],
  g0StillOpen: [
    "approved Safari/WKWebView matrix",
    "WebKit WPE DedicatedWorker OPFS capability",
    "PRAGMA foreign_key_check deliberate-violation result shape",
    "numeric combined size/startup/active-memory budgets",
    "frozen production consuming OPFS lifecycle API",
  ],
  g0Recommendation: "NO-GO",
};
await writeFile(
  new URL("summary.actual.json", generated),
  `${JSON.stringify(summary, null, 2)}\n`,
);
console.log(JSON.stringify(summary, null, 2));
