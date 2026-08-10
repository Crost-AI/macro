import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";

const generated = new URL("../measurements/generated/", import.meta.url);
const readJson = async (name) => JSON.parse(await readFile(new URL(name, generated), "utf8"));
const matrix = await readJson("browser-matrix.actual.json");
const wasm = await readJson("wasm-inspection.actual.json");
const glue = await readJson("web-glue-inspection.actual.json");
const source = await readJson("worker-source-inspection.actual.json");
const wasmBytes = await readFile(new URL("../pkg/turso_opfs_spike_bg.wasm", import.meta.url));

const summarizeRun = (run) => {
  const report = run.report;
  const direct = report?.directFile?.operations;
  const before = report?.abruptReset?.preopenSizes;
  const reset = report?.abruptReset?.reset;
  return {
    name: run.name,
    label: run.label,
    browserVersion: run.browserVersion,
    actuallyRun: true,
    pass: report?.pass ?? false,
    storageGetDirectory: report?.capabilities?.storageGetDirectory ?? null,
    lazyWorker: report?.workersBeforeFirstUse === 0,
    noIsolationHeaders:
      run.responseIsolationHeaders?.crossOriginOpenerPolicy === null &&
      run.responseIsolationHeaders?.crossOriginEmbedderPolicy === null,
    noNestedWorker:
      (report?.noNestedWorker?.runtimeMonitorInstalled === true &&
        report?.noNestedWorker?.constructionCount === 0) ||
      (report?.capabilities?.nestedWorkerMonitorInstalled === true &&
        report?.capabilities?.nestedWorkerConstructionCount === 0),
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
    emptyWriteIsZero: direct?.empty_write_bytes === 0,
    partialWriteRetried: direct?.partial_write_retried ?? null,
    eofAndShortReadCovered: direct?.eof_bytes === 0 && direct?.short_read_bytes === 2,
    specificWriteErrorsPreserved:
      direct?.zero_write_error === "ShortWrite" &&
      direct?.error_preserved === true &&
      direct?.quota_preserved === true,
    lifecyclePoisonCovered:
      report?.lifecycleFailure?.closeFailed === true &&
      report?.lifecycleFailure?.deleteRejected === true &&
      report?.lifecycleFailure?.reopenRejected === true,
    actualResetFailuresCovered:
      report?.removeEntryFailure?.actualRemoveEntryFailure === true &&
      report?.removeEntryFailure?.secondResetRejected === true &&
      report?.removeEntryFailure?.reopenRejected === true &&
      report?.recreationFailure?.actualRecreationFailure === true &&
      report?.recreationFailure?.secondResetRejected === true &&
      report?.recreationFailure?.reopenRejected === true,
    serializedQueueCovered: Array.isArray(report?.serializedQueue),
    killAfterFirstCommit:
      report?.workerKill?.firstCommit?.commitCount === 1 &&
      report?.workerKill?.pendingRpcRejected === true,
    killLoopProvenIncomplete:
      report?.abruptReset?.committed?.committed_rows >= 1 &&
      report?.abruptReset?.committed?.committed_rows <
        report?.abruptReset?.committed?.finite_bound &&
      report?.abruptReset?.committed?.finite_bound ===
        report?.workerKill?.firstCommit?.finiteBound,
    killSizesRecorded:
      report?.workerKill?.firstCommit?.preSizes?.["graphql-cache.db"] >= 0 &&
      report?.workerKill?.firstCommit?.postSizes?.["graphql-cache.db-wal"] > 0,
    recoveryTimingRecorded:
      report?.recovery?.scope ===
        "web-lock+preopen+sql-count+close+removeEntry+recreate" &&
      report?.recovery?.deadlineMs === 30000 &&
      report?.recovery?.elapsedMs >= 0 &&
      report?.recovery?.elapsedMs <= report?.recovery?.deadlineMs &&
      report?.recovery?.candidateTerminations === report?.recovery?.attempts &&
      typeof report?.recovery?.startedAt === "string" &&
      typeof report?.recovery?.completedAt === "string",
    abruptResetCovered:
      before?.["graphql-cache.db"] > 0 &&
      before?.["graphql-cache.db-wal"] > 0 &&
      reset?.deleted?.["graphql-cache.db"] === true &&
      reset?.deleted?.["graphql-cache.db-wal"] === true &&
      reset?.recreated?.["graphql-cache.db"] === 0 &&
      reset?.recreated?.["graphql-cache.db-wal"] === 0,
    memoryInstantCause: report?.memoryIoProbe?.error?.stack?.includes("std::time::Instant::now") ?? false,
    immediateInstantCause:
      report?.beginImmediateProbe?.error?.stack?.includes("std::time::Instant::now") ?? false,
    immediateTempDatabaseCause:
      /ensure_temp_database|create_temp_database|open_file_with_flags/.test(
        report?.beginImmediateProbe?.error?.stack ?? "",
      ),
  };
};

const summary = {
  tursoRevision: "ed15b13f8e5f77d7ae24af321a63d7cd0fa53365",
  playwrightVersion: matrix.playwrightVersion,
  wasm: {
    bytes: wasmBytes.length,
    sha256: createHash("sha256").update(wasmBytes).digest("hex"),
    memoryCount: wasm.memories.length,
    shared: wasm.memories[0]?.shared,
    memory64: wasm.memories[0]?.memory64,
    atomicOperatorCount: wasm.atomic_operator_count,
    importsAllowed: wasm.imports_allowed,
    browserContractCompliant: wasm.browser_contract_compliant,
  },
  glueCompliant: glue.browser_glue_compliant,
  workerSourceCompliant: source.source_contract_compliant,
  runs: matrix.runs.map(summarizeRun),
  safariWkWebViewActuallyRun: false,
  g0Recommendation: "NO-GO",
};
await writeFile(
  new URL("summary.actual.json", generated),
  `${JSON.stringify(summary, null, 2)}\n`,
);
console.log(JSON.stringify(summary, null, 2));
