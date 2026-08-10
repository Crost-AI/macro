// Browser execution is intentional: these are wasm32 runtime failures.
import { readFile } from "node:fs/promises";

const source = await readFile(new URL("../src/lib.rs", import.meta.url), "utf8");
const requiredSource = [
  "pub fn run_builtin_memory_io_probe()",
  "MemoryIO::new()",
  "pub fn run_begin_immediate_probe(owner: u32, session: u32)",
  'connection.execute("BEGIN IMMEDIATE")',
];
for (const fragment of requiredSource) {
  if (!source.includes(fragment)) throw new Error(`missing preserved blocker fragment: ${fragment}`);
}
console.log("preserved minimal built-in MemoryIO and BEGIN IMMEDIATE wasm32 probes");
