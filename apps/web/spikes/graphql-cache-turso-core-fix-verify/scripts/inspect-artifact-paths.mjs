#!/usr/bin/env node
import { readFile, stat } from "node:fs/promises";
import { relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const artifacts = [];
for (const variant of ["parent", "head"]) {
  const crate = variant === "parent" ? "turso_temp_fix_parent" : "turso_temp_fix_head";
  artifacts.push(`target/cargo-${variant}/wasm32-unknown-unknown/release/${crate}.wasm`);
  for (const target of ["node", "web"]) {
    artifacts.push(
      `target/wasm/${variant}/${target}/temp_fix_bg.wasm`,
      `target/wasm/${variant}/${target}/temp_fix.js`,
      `target/wasm/${variant}/${target}/temp_fix.d.ts`,
      `target/wasm/${variant}/${target}/temp_fix_bg.wasm.d.ts`,
    );
  }
  artifacts.push(`target/wasm/${variant}/node/temp_fix.cjs`);
}

const hostSensitivePatterns = [
  ["unix_home", /\/(?:home|Users)\/[A-Za-z0-9_.+@/=-]+/g],
  ["nix_store", /\/nix\/store\/[A-Za-z0-9_.+@/=-]+/g],
  ["temporary_root", /\/(?:tmp|private\/tmp|var\/folders)\/[A-Za-z0-9_.+@/=-]+/g],
  ["unix_build_root", /\/build\/[A-Za-z0-9_.+@/=-]+/g],
  [
    "incompletely_remapped_host_root",
    /(?:host-home|repository-root)\/[A-Za-z0-9_.+@/=-]+/g,
  ],
  [
    "windows_user_or_build_root",
    /[A-Za-z]:[\\/](?:Users|home|src|work|workspace|tmp)[\\/][A-Za-z0-9_.+@\\/=-]+/g,
  ],
];
const absoluteSourcePattern =
  /(?:\/[A-Za-z0-9_.+@=-]+){2,}\.(?:rs|toml|lock|js|ts|c|cc|cpp|h)(?=[^A-Za-z0-9_.-]|$)/g;
const artifactEvidence = [];
const hostSensitiveMatches = [];
const absolutePathInventory = [];
const virtualPathPatterns = [
  ["spikeSource", /spike-src\//g],
  ["cargoHome", /cargo-home\//g],
  ["nixStore", /nix-store\//g],
  ["rustc", /rustc\//g],
  ["repositoryRoot", /repository-root\//g],
  ["hostHome", /host-home\//g],
];
const virtualPathOccurrences = Object.fromEntries(
  virtualPathPatterns.map(([kind]) => [kind, 0]),
);
for (const name of artifacts) {
  const path = resolve(root, name);
  const bytes = await readFile(path);
  artifactEvidence.push({ path: relative(root, path), bytes: (await stat(path)).size });
  const text = bytes.toString("latin1");
  for (const [kind, pattern] of hostSensitivePatterns) {
    pattern.lastIndex = 0;
    for (const match of text.matchAll(pattern)) {
      hostSensitiveMatches.push({ artifact: name, kind, value: match[0] });
    }
  }
  for (const [kind, pattern] of virtualPathPatterns) {
    pattern.lastIndex = 0;
    virtualPathOccurrences[kind] += [...text.matchAll(pattern)].length;
  }
  absoluteSourcePattern.lastIndex = 0;
  for (const match of text.matchAll(absoluteSourcePattern)) {
    absolutePathInventory.push({ artifact: name, value: match[0] });
  }
}

const result = {
  artifacts: artifactEvidence,
  remapPathPrefixDestinations: [
    "nix-store",
    "rustc",
    "host-home",
    "cargo-home",
    "repository-root",
    "spike-src",
  ],
  absolutePathScanPerformed: true,
  absoluteSourcePathCandidateCount: absolutePathInventory.length,
  reproducibleVirtualPathOccurrences: virtualPathOccurrences,
  absoluteSourcePathSamples: [
    ...new Set(absolutePathInventory.map(({ value }) => value)),
  ].slice(0, 12),
  forbiddenHostSensitivePathKinds: hostSensitivePatterns.map(([kind]) => kind),
  hostSensitiveMatches,
  hostSensitiveAbsolutePathFree: hostSensitiveMatches.length === 0,
};
console.log(JSON.stringify(result, null, 2));
if (!result.hostSensitiveAbsolutePathFree) process.exitCode = 1;
