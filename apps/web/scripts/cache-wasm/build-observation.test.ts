import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { assertBuildObservation } from './build-observation';

const observation = JSON.parse(
  readFileSync(
    resolve(
      import.meta.dirname,
      '../../measurements/generated/cache-wasm-first-target-fill.json'
    ),
    'utf8'
  )
) as unknown;

const record = observation as {
  measuredRevisionChangeId: string;
  wasmSha256: string;
  toolIdentity: {
    rustc: string;
    cargo: string;
    wasmPack: string;
    wasmOpt: string;
  };
};
const expected = {
  measuredRevisionChangeId: record.measuredRevisionChangeId,
  wasmSha256: record.wasmSha256,
  toolIdentity: record.toolIdentity,
};

describe('first target-fill build observation', () => {
  it('retains command, cache state, revision, tool, and artifact identity', () => {
    expect(() => assertBuildObservation(observation, expected)).not.toThrow();
  });

  it('rejects a fabricated or non-finite sample', () => {
    expect(() =>
      assertBuildObservation(
        {
          ...(observation as Record<string, unknown>),
          elapsedMs: Number.NaN,
        },
        expected
      )
    ).toThrow('elapsedMs');
    expect(() =>
      assertBuildObservation(
        {
          ...(observation as Record<string, unknown>),
          command: 'echo fabricated',
        },
        expected
      )
    ).toThrow('command');
  });

  it('captures one explicit stable revision before generated browser evidence', () => {
    const justfile = readFileSync(
      resolve(import.meta.dirname, '../../justfile'),
      'utf8'
    );
    const capture = justfile.indexOf('measured_revision=$(jj log');
    const playwright = justfile.indexOf(
      'CACHE_WASM_WRITE_MEASUREMENTS=true bunx --bun playwright'
    );
    expect(capture).toBeGreaterThan(0);
    expect(capture).toBeLessThan(playwright);
    expect(justfile).toContain(
      'report --dist dist --revision-change-id "$measured_revision"'
    );
  });

  it.each([
    ['elapsedMs', { elapsedMs: 0 }, 'positive'],
    [
      'change ID',
      { measuredRevisionChangeId: 'z'.repeat(32) },
      'change ID differs',
    ],
    ['WASM hash', { wasmSha256: 'b'.repeat(64) }, 'SHA-256 differs'],
    [
      'cargo identity',
      {
        toolIdentity: {
          ...record.toolIdentity,
          cargo: 'cargo fabricated',
        },
      },
      'cargo identity differs',
    ],
  ])('rejects stale %s evidence', (_label, replacement, message) => {
    expect(() =>
      assertBuildObservation(
        { ...(observation as Record<string, unknown>), ...replacement },
        expected
      )
    ).toThrow(message as string);
  });
});
