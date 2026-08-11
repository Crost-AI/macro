#!/usr/bin/env node
import { readFile, stat } from "node:fs/promises";
import { relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const artifactNames = [
  "target/wasm32-unknown-unknown/release/turso_opfs_spike.wasm",
  "pkg/turso_opfs_spike_bg.wasm",
  "pkg/turso_opfs_spike.js",
  "pkg/turso_opfs_spike.d.ts",
  "pkg/turso_opfs_spike_bg.wasm.d.ts",
];
const hostSensitivePatterns = [
  ["unix_home", /\/(?:home|Users)\/[A-Za-z0-9_.+@/-]+/g],
  ["nix_store", /\/nix\/store\/[A-Za-z0-9_.+@/-]+/g],
  ["temporary_root", /\/(?:tmp|private\/tmp|var\/folders)\/[A-Za-z0-9_.+@/-]+/g],
  ["windows_user_or_build_root", /[A-Za-z]:[\\/](?:Users|home|src|work|workspace|tmp)[\\/][A-Za-z0-9_.+@\\/-]+/g],
];
const absoluteSourcePattern =
  /(?:\/[A-Za-z0-9_.+@=-]+){2,}\.(?:rs|toml|lock|js|ts|c|cc|cpp|h)(?=[^A-Za-z0-9_.-]|$)/g;
const artifacts = [];
const hostSensitiveMatches = [];
const absolutePathInventory = [];
for (const name of artifactNames) {
  const path = resolve(root, name);
  const bytes = await readFile(path);
  artifacts.push({ path: relative(root, path), bytes: (await stat(path)).size });
  const text = bytes.toString("latin1");
  for (const [kind, pattern] of hostSensitivePatterns) {
    pattern.lastIndex = 0;
    for (const match of text.matchAll(pattern)) {
      hostSensitiveMatches.push({ artifact: name, kind, value: match[0] });
    }
  }
  absoluteSourcePattern.lastIndex = 0;
  for (const match of text.matchAll(absoluteSourcePattern)) {
    absolutePathInventory.push({ artifact: name, value: match[0] });
  }
}
const inventoryKinds = {
  cargo_registry_virtual: absolutePathInventory.filter(({ value }) =>
    value.startsWith("/.cargo/registry/"),
  ).length,
  rustc_virtual: absolutePathInventory.filter(({ value }) => value.startsWith("/rustc/"))
    .length,
  cargo_dependency_virtual: absolutePathInventory.filter(
    ({ value }) =>
      !value.startsWith("/.cargo/registry/") &&
      !value.startsWith("/rustc/") &&
      !hostSensitivePatterns.some(([, pattern]) => {
        pattern.lastIndex = 0;
        return pattern.test(value);
      }),
  ).length,
};
const result = {
  artifacts,
  remapPathPrefixDestinations: [
    "spike-src",
    "cargo-home",
    "host-home",
    "nix-store",
    "rustc",
  ],
  absolutePathScanPerformed: true,
  absoluteSourcePathCandidateCount: absolutePathInventory.length,
  reproducibleVirtualPathCounts: inventoryKinds,
  absoluteSourcePathSamples: [...new Set(absolutePathInventory.map(({ value }) => value))].slice(
    0,
    12,
  ),
  forbiddenHostSensitivePathKinds: hostSensitivePatterns.map(([kind]) => kind),
  hostSensitiveMatches,
  hostSensitiveAbsolutePathFree: hostSensitiveMatches.length === 0,
};
console.log(JSON.stringify(result, null, 2));
if (!result.hostSensitiveAbsolutePathFree) process.exitCode = 1;
