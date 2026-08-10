#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const path = require('node:path');

const directory = process.argv[2];
if (!directory) {
  console.error('usage: node scripts/summarize-measurements.cjs <measurement-directory>');
  process.exit(2);
}

const sampleNames = fs
  .readdirSync(directory)
  .filter((name) => /^node-optimized-[0-9]+\.json$/.test(name))
  .sort();
if (sampleNames.length !== 5) throw new Error(`expected 5 optimized samples, found ${sampleNames.length}`);
const samples = sampleNames.map((name) => JSON.parse(fs.readFileSync(path.join(directory, name), 'utf8')));

function median(values) {
  const sorted = [...values].sort((left, right) => left - right);
  return Number(sorted[Math.floor(sorted.length / 2)].toFixed(6));
}

function values(key) {
  return samples.map((sample) => sample[key]);
}

function nested(group, key) {
  return samples.map((sample) => sample[group][key]);
}

function parseTsv(name) {
  const [header, ...lines] = fs.readFileSync(path.join(directory, name), 'utf8').trim().split('\n');
  const keys = header.split('\t');
  return lines.map((line) => Object.fromEntries(line.split('\t').map((value, index) => [keys[index], /^-?[0-9]+$/.test(value) ? Number(value) : value])));
}

const summary = {
  schema_version: 1,
  measured_artifact: 'wasm-bindgen-node-wasm-opt-117-Oz',
  sample_count: samples.length,
  raw_sample_files: sampleNames,
  build_seconds: {
    clean_target: Number(fs.readFileSync(path.join(directory, 'release-clean-seconds.txt'), 'utf8')),
    no_op: Number(fs.readFileSync(path.join(directory, 'release-noop-seconds.txt'), 'utf8')),
  },
  sizes: parseTsv('sizes.tsv'),
  hashes: parseTsv('hashes.tsv'),
  runtime_medians: {
    instantiate_ms: median(values('instantiate_ms')),
    first_open_close_ms: median(values('first_open_close_ms')),
    warm_open_close_ms: median(values('warm_open_close_ms')),
    first_sql_ms: median(values('first_sql_ms')),
    warm_sql_ms: median(values('warm_sql_ms')),
    linear_memory_bytes: {
      before: median(nested('linear_memory_bytes', 'before')),
      after_first_open: median(nested('linear_memory_bytes', 'after_first_open')),
      after_first_sql: median(nested('linear_memory_bytes', 'after_first_sql')),
      after_warm_sql: median(nested('linear_memory_bytes', 'after_warm_sql')),
    },
    process_rss_bytes: {
      delta: median(nested('process_rss_bytes', 'delta')),
    },
  },
};
process.stdout.write(`${JSON.stringify(summary, null, 2)}\n`);
