export const CACHE_WASM_BUDGETS = {
  rawBytes: 12 * 1024 * 1024,
  brotliBytes: 3 * 1024 * 1024,
  gzipBytes: Math.floor(4.5 * 1024 * 1024),
  glueBytes: 64 * 1024,
  nodeCompileInstantiateP95Ms: 1_000,
  browserReadyP95Ms: 3_000,
  hostFirstReadyP95Ms: 5_000,
  linearMemoryBytes: 32 * 1024 * 1024,
  buildMs: 180_000,
} as const;
