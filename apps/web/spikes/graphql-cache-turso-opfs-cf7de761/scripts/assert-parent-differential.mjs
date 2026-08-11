import { readFile } from "node:fs/promises";

const matrix = JSON.parse(
  await readFile(
    new URL("../measurements/generated/parent-differential.actual.json", import.meta.url),
    "utf8",
  ),
);
if (matrix.transactionExpectation !== "parent-failure") {
  throw new Error("parent differential used the wrong transaction expectation");
}
for (const name of ["chromium", "firefox"]) {
  const matching = matrix.runs.filter((run) => run.name === name);
  if (matching.length !== 1) throw new Error(`${name} parent differential was not run exactly once`);
  const report = matching[0].report;
  if (!report?.pass || !report.differentialExpectedFailure) {
    throw new Error(`${name} did not reproduce the expected parent failure`);
  }
  for (const mode of ["immediate", "exclusive"]) {
    const result = report.transactionModes?.[mode];
    if (
      result?.succeeded !== false ||
      result.error?.wasmEnvironmentTrap !== true ||
      result.error?.runtimeEvidence?.productionReachableWasmTrapCount !== 1 ||
      result.error?.runtimeEvidence?.unhandledRuntimeFailureCount !== 0
    ) {
      throw new Error(`${name} ${mode} did not produce one accounted WASM environment trap`);
    }
    if (!/std::time::Instant::now|not implemented on this platform/.test(result.error.stack ?? "")) {
      throw new Error(`${name} ${mode} lost the Instant/platform stack cause`);
    }
    if (!/ensure_temp_database|create_temp_database|open_file_with_flags/.test(result.error.stack ?? "")) {
      throw new Error(`${name} ${mode} lost the temp-database stack cause`);
    }
  }
}
console.log("parent differential assertions passed in Chromium and Firefox");
