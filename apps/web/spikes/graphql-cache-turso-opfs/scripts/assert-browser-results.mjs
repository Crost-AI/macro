import { readFile } from "node:fs/promises";

const generated = new URL("../measurements/generated/", import.meta.url);
const matrix = JSON.parse(await readFile(new URL("browser-matrix.actual.json", generated)));
const sourceInspection = JSON.parse(
  await readFile(new URL("worker-source-inspection.actual.json", generated)),
);
if (!sourceInspection.source_contract_compliant) {
  throw new Error("worker source inspection did not prove the no-nested-worker contract");
}
const runs = new Map(matrix.runs.map((run) => [run.name, run]));
for (const name of ["chromium", "firefox", "webkit-wpe"]) {
  if (!runs.has(name)) throw new Error(`${name} was not actually run`);
}
for (const name of ["chromium", "firefox"]) {
  const run = runs.get(name);
  const report = run.report;
  if (!report?.pass) throw new Error(`${name} operational probe failed: ${report?.error?.message}`);
  if (report.workersBeforeFirstUse !== 0) throw new Error(`${name} worker was not lazy`);
  if (report.webLockExclusion.contenderAcquired) throw new Error(`${name} Web Lock exclusion failed`);
  if (report.noNestedWorker.constructionCount !== 0) throw new Error(`${name} nested worker constructed`);
  if (!report.lifecycleFailure.closeFailed || !report.lifecycleFailure.deleteRejected) {
    throw new Error(`${name} did not poison uncertain close and reject delete`);
  }
  if (!report.lifecycleFailure.reopenRejected || !report.lifecycleFailure.lifecycle.startsWith("poisoned:")) {
    throw new Error(`${name} poisoned registry reopened`);
  }
  const removeFailure = report.removeEntryFailure;
  if (
    !removeFailure.resetFailed ||
    !removeFailure.actualRemoveEntryFailure ||
    !removeFailure.secondResetRejected ||
    !removeFailure.reopenRejected ||
    !removeFailure.lifecycle.startsWith("poisoned:") ||
    !removeFailure.artifactCleaned
  ) {
    throw new Error(`${name} actual removeEntry failure was not deterministic and poisoned`);
  }
  const recreationFailure = report.recreationFailure;
  if (
    !recreationFailure.resetFailed ||
    !recreationFailure.actualRecreationFailure ||
    !recreationFailure.secondResetRejected ||
    !recreationFailure.reopenRejected ||
    !recreationFailure.lifecycle.startsWith("poisoned:") ||
    !recreationFailure.artifactCleaned
  ) {
    throw new Error(`${name} actual recreation failure was not deterministic and poisoned`);
  }
  if (!Array.isArray(report.serializedQueue) || report.serializedQueue.length !== 2) {
    throw new Error(`${name} serialized operation queue was not covered`);
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
    if (direct[field] !== 1) throw new Error(`${name} ${field} != 1`);
  }
  if (
    direct.empty_write_bytes !== 0 ||
    !direct.partial_write_retried ||
    direct.zero_write_error !== "ShortWrite"
  ) {
    throw new Error(`${name} partial/zero write behavior failed`);
  }
  if (!direct.error_preserved || !direct.quota_preserved) {
    throw new Error(`${name} lost a specific completion error`);
  }
  if (direct.eof_bytes !== 0 || direct.short_read_bytes !== 2) {
    throw new Error(`${name} EOF/short-read coverage failed`);
  }

  const kill = report.workerKill;
  if (!kill.terminatedWhileCallActive || !kill.pendingRpcRejected) {
    throw new Error(`${name} kill RPC was not pending and rejected at termination`);
  }
  if (kill.firstCommit.commitCount !== 1 || kill.firstCommit.finiteBound !== 10_000) {
    throw new Error(`${name} did not wait for the first successful commit of the finite loop`);
  }
  if (
    kill.firstCommit.preSizes["graphql-cache.db"] < 0 ||
    kill.firstCommit.postSizes["graphql-cache.db-wal"] <= 0
  ) {
    throw new Error(`${name} did not record valid pre/post kill sizes`);
  }
  if (!kill.firstCommit.writeStartedAt || !kill.firstCommit.firstCommitObservedAt || !kill.terminatedAt) {
    throw new Error(`${name} kill timestamps are missing`);
  }

  const recovery = report.recovery;
  if (
    recovery.scope !== "web-lock+preopen+sql-count+close+removeEntry+recreate" ||
    recovery.deadlineMs !== 30_000 ||
    recovery.elapsedMs < 0 ||
    recovery.elapsedMs > recovery.deadlineMs ||
    recovery.candidateTerminations !== recovery.attempts ||
    recovery.unsuccessfulCandidateTerminations !== recovery.attempts - 1 ||
    recovery.successfulAttemptRemainingMs <= 0 ||
    !recovery.startedAt ||
    !recovery.completedAt
  ) {
    throw new Error(`${name} complete OPFS recovery was not bounded by the real-time deadline`);
  }
  const committed = report.abruptReset.committed;
  if (
    committed.finite_bound !== kill.firstCommit.finiteBound ||
    committed.committed_rows < 1 ||
    committed.committed_rows >= committed.finite_bound
  ) {
    throw new Error(`${name} did not prove termination before the finite write loop completed`);
  }
  const before = report.abruptReset.preopenSizes;
  const reset = report.abruptReset.reset;
  if (before["graphql-cache.db"] <= 0 || before["graphql-cache.db-wal"] <= 0) {
    throw new Error(`${name} reset did not observe both files after abrupt loss`);
  }
  if (!reset.deleted["graphql-cache.db"] || !reset.deleted["graphql-cache.db-wal"]) {
    throw new Error(`${name} did not delete both files`);
  }
  if (reset.recreated["graphql-cache.db"] !== 0 || reset.recreated["graphql-cache.db-wal"] !== 0) {
    throw new Error(`${name} files were not freshly recreated`);
  }
  if (report.freshRecovery.value.count_before !== 0) throw new Error(`${name} SQL DB was not fresh`);

  const memoryStack = report.memoryIoProbe.error?.stack ?? "";
  const immediateStack = report.beginImmediateProbe.error?.stack ?? "";
  if (!report.memoryIoProbe.trapped || !memoryStack.includes("std::time::Instant::now")) {
    throw new Error(`${name} MemoryIO blocker lost the Instant cause`);
  }
  if (
    !report.beginImmediateProbe.trapped ||
    !immediateStack.includes("std::time::Instant::now") ||
    !/ensure_temp_database|create_temp_database|open_file_with_flags/.test(immediateStack)
  ) {
    throw new Error(`${name} BEGIN IMMEDIATE blocker lost the temp-DB/Instant cause`);
  }
}

const webkit = runs.get("webkit-wpe");
if (webkit.report?.capabilities?.storageGetDirectory !== false) {
  throw new Error("WebKit WPE result changed; review the G0 recommendation");
}
for (const run of runs.values()) {
  if (
    run.responseIsolationHeaders.crossOriginOpenerPolicy !== null ||
    run.responseIsolationHeaders.crossOriginEmbedderPolicy !== null ||
    run.report.page.crossOriginIsolated !== false
  ) {
    throw new Error(`${run.name} unexpectedly used cross-origin isolation`);
  }
}
console.log(
  "browser assertions passed: aggregate writes, actual reset failures, finite active-kill, and bounded full recovery covered; WebKit WPE OPFS unavailable",
);
