export interface CacheWasmBuildObservation {
  schemaVersion: 1;
  kind: 'first-target-fill-observation';
  elapsedMs: number;
  command: string;
  workingDirectory: 'apps/web';
  cacheState: string;
  measuredRevisionChangeId: string;
  toolIdentity: {
    rustc: string;
    cargo: string;
    wasmPack: string;
    wasmOpt: string;
  };
  wasmSha256: string;
}

const EXPECTED_COMMAND =
  'wasm-pack build ../../crates/client/cache-wasm --target web --release --out-dir src/lib/graphql-cache/wasm';

export interface CacheWasmBuildObservationExpected {
  measuredRevisionChangeId: string;
  wasmSha256: string;
  toolIdentity: CacheWasmBuildObservation['toolIdentity'];
}

export function assertBuildObservation(
  value: unknown,
  expected: CacheWasmBuildObservationExpected
): asserts value is CacheWasmBuildObservation {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error('first-target-fill observation must be an object');
  }
  const record = value as Record<string, unknown>;
  if (
    record.schemaVersion !== 1 ||
    record.kind !== 'first-target-fill-observation'
  ) {
    throw new Error('invalid first-target-fill observation schema');
  }
  if (
    typeof record.elapsedMs !== 'number' ||
    !Number.isFinite(record.elapsedMs) ||
    record.elapsedMs <= 0
  ) {
    throw new Error('first-target-fill elapsedMs must be finite and positive');
  }
  if (record.command !== EXPECTED_COMMAND)
    throw new Error('first-target-fill command does not match the build recipe');
  if (record.workingDirectory !== 'apps/web')
    throw new Error('first-target-fill working directory must be apps/web');
  if (typeof record.cacheState !== 'string' || record.cacheState.length === 0)
    throw new Error('first-target-fill cache state is missing');
  if (
    typeof record.measuredRevisionChangeId !== 'string' ||
    !/^[k-z]{32}$/.test(record.measuredRevisionChangeId)
  ) {
    throw new Error('first-target-fill measured revision change ID is invalid');
  }
  if (
    typeof record.wasmSha256 !== 'string' ||
    !/^[a-f0-9]{64}$/.test(record.wasmSha256)
  ) {
    throw new Error('first-target-fill WASM SHA-256 is invalid');
  }
  const tools = record.toolIdentity;
  if (typeof tools !== 'object' || tools === null || Array.isArray(tools)) {
    throw new Error('first-target-fill tool identity is missing');
  }
  for (const tool of ['rustc', 'cargo', 'wasmPack', 'wasmOpt']) {
    if (
      typeof (tools as Record<string, unknown>)[tool] !== 'string' ||
      ((tools as Record<string, unknown>)[tool] as string).length === 0
    ) {
      throw new Error(`first-target-fill ${tool} identity is missing`);
    }
  }
  if (record.measuredRevisionChangeId !== expected.measuredRevisionChangeId) {
    throw new Error(
      'first-target-fill change ID differs from the explicitly measured revision'
    );
  }
  if (record.wasmSha256 !== expected.wasmSha256) {
    throw new Error(
      'first-target-fill WASM SHA-256 differs from the inspected package'
    );
  }
  for (const tool of ['rustc', 'cargo', 'wasmPack', 'wasmOpt'] as const) {
    if (
      (tools as Record<string, unknown>)[tool] !== expected.toolIdentity[tool]
    ) {
      throw new Error(
        `first-target-fill ${tool} identity differs from the report environment`
      );
    }
  }
}
