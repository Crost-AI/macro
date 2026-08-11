import { readFile } from "node:fs/promises";

const source = await readFile(new URL("../src/lib.rs", import.meta.url), "utf8");
const tursoConnection = await readFile(
  new URL("../.turso-source/core/connection.rs", import.meta.url),
  "utf8",
);
for (const fragment of [
  "pub fn run_transaction_mode_probe(",
  '"immediate" => ("BEGIN IMMEDIATE"',
  '"exclusive" => ("BEGIN EXCLUSIVE"',
  "pub fn run_explicit_temp_negative_probe(",
  "CREATE TEMP TABLE explicit_temp_negative_probe",
  "pub fn run_full_cache_sql_probe(",
  "pub fn verify_full_cache_sql_persistence(",
  "PRAGMA quick_check",
  "PRAGMA foreign_key_check",
  "LEFT JOIN optimistic_layers",
  "(__typename || ':' || id) COLLATE BINARY",
]) {
  if (!source.includes(fragment)) throw new Error(`missing runtime contract fragment: ${fragment}`);
}
if (
  !/fn create_temp_database[\s\S]*?Arc::new\(MemoryIO::new\(\)\)/.test(tursoConnection)
) {
  throw new Error("exact Turso source no longer shows built-in temp MemoryIO; review negative probe");
}
for (const forbidden of [
  "run_builtin_memory_io_probe",
  "run_begin_immediate_probe",
  '#[cfg(feature = "failing-runtime-probes")]',
]) {
  if (source.includes(forbidden)) {
    throw new Error(`obsolete expected-trap route remains production-reachable: ${forbidden}`);
  }
}
console.log(
  "enumerated HEAD production routes require IMMEDIATE/EXCLUSIVE and selected cache SQL without traps; explicit temp remains a negative-only trap route",
);
