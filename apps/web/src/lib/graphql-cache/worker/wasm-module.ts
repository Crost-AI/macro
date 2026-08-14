/**
 * Typed surface of the generated wasm package (`cache-wasm`), loaded
 * dynamically so the repo type-checks without the generated artifacts.
 *
 * Build the package with:
 *   just build-cache-wasm
 * which runs wasm-pack over crates/client/cache-wasm into
 * src/lib/graphql-cache/wasm/ (gitignored).
 */

import type { EntityResolverWire } from '../exchange/entity-resolvers';
import type {
  CachedQueryInstanceWire,
  CachedQueryVariantWire,
  ClaimedMutation,
  EnqueueOptimisticMutationResult,
  OptimisticLinkPatchWire,
  QueryRevalidationWire,
  ReadResult,
  RecordCursor,
  SelectedRecordPageWire,
  WriteResult,
} from '../protocol';

/** Stable, payload-free marker latched on reset-required WASM errors. */
export interface CacheStorageResetRequiredError extends Error {
  readonly cacheStorageResetRequired: true;
}

export interface CacheEngine {
  boundIdentity(): Promise<string | null>;
  readQuery(
    opId: string | undefined,
    query: string,
    operationName: string | undefined,
    variables: Record<string, unknown> | undefined,
    entityResolvers: readonly EntityResolverWire[] | undefined
  ): Promise<ReadResult>;
  readRecords(
    document: string,
    fragmentName: string,
    cursor: RecordCursor | undefined,
    limit: number
  ): Promise<SelectedRecordPageWire>;
  writeQuery(
    originOpId: string | undefined,
    query: string,
    operationName: string | undefined,
    variables: Record<string, unknown> | undefined,
    data: unknown,
    identity: string | undefined
  ): Promise<WriteResult>;
  enqueueOptimisticMutation(
    originOpId: string | undefined,
    query: string,
    operationName: string | undefined,
    variables: Record<string, unknown> | undefined,
    data: unknown,
    linkPatches: OptimisticLinkPatchWire[] | undefined,
    revalidations: QueryRevalidationWire[] | undefined,
    createdAtMs: number,
    leaseOwner: string,
    nowMs: number,
    leaseExpiresAtMs: number
  ): Promise<EnqueueOptimisticMutationResult>;
  inspectQueryVariants(
    query: string,
    operationName: string | undefined,
    path: Array<{ field: string }>
  ): Promise<CachedQueryVariantWire[]>;
  inspectQuery(
    query: string,
    operationName: string | undefined,
    path: Array<{ field: string }>,
    variableFilters: Array<Record<string, unknown>>
  ): Promise<CachedQueryInstanceWire[]>;
  claimNextMutation(
    owner: string,
    nowMs: number,
    leaseExpiresAtMs: number
  ): Promise<ClaimedMutation | undefined>;
  deferOptimisticWrite(
    transactionId: string,
    leaseOwner: string,
    leaseGeneration: string,
    nextAttemptAtMs: number,
    error: string
  ): Promise<void>;
  commitOptimisticWrite(
    transactionId: string,
    leaseOwner: string,
    leaseGeneration: string,
    query: string,
    operationName: string | undefined,
    variables: Record<string, unknown> | undefined,
    data: unknown
  ): Promise<WriteResult>;
  rollbackOptimisticWrite(
    transactionId: string,
    leaseOwner: string,
    leaseGeneration: string
  ): Promise<WriteResult>;
  invalidateKeys(keys: string[]): Promise<string[]>;
  deleteKeys(keys: string[]): Promise<string[]>;
  teardownOperation(opId: string): Promise<void>;
  clear(): Promise<void>;
  /** Reset/recreate OPFS; concurrent calls wait for the fresh engine. */
  physicalReset(): Promise<void>;
  /** Gracefully close Turso/OPFS and release the owner lock. */
  close(): Promise<void>;
}

export interface CacheWasmModule {
  default: (input?: { module_or_path?: unknown }) => Promise<unknown>;
  openCache(scope: string, hotCapacity?: number): Promise<CacheEngine>;
  /** Atomically wipes before Turso open while retaining one OPFS owner lock. */
  openCacheForRecovery(
    scope: string,
    hotCapacity?: number
  ): Promise<CacheEngine>;
  destroyCache(scope: string): Promise<void>;
  schemaHash(): string;
}

let modulePromise: Promise<CacheWasmModule> | undefined;
let wasmMemory: WebAssembly.Memory | undefined;

/** Returns the combined module's current unshared linear-memory allocation. */
export function cacheWasmLinearMemoryBytes(): number {
  if (!wasmMemory) throw new Error('cache WASM memory is not initialized');
  return wasmMemory.buffer.byteLength;
}

/** Loads and initializes the wasm module exactly once per worker context. */
export function loadCacheWasm(): Promise<CacheWasmModule> {
  if (!modulePromise) {
    modulePromise = (async () => {
      const url = new URL('../wasm/cache_wasm.js', import.meta.url).href;
      const mod = (await import(/* @vite-ignore */ url)) as CacheWasmModule;
      // Resolve the wasm binary explicitly: vite copies the generated JS as
      // an opaque asset, so its internal relative `cache_wasm_bg.wasm` URL
      // would 404 in production. This `new URL` pattern is statically
      // analyzable, so vite emits the binary as an asset and rewrites it.
      const wasmUrl = new URL('../wasm/cache_wasm_bg.wasm', import.meta.url);
      const exports = (await mod.default({ module_or_path: wasmUrl })) as {
        memory?: WebAssembly.Memory;
      };
      if (!(exports.memory instanceof WebAssembly.Memory)) {
        throw new Error('cache WASM did not export its linear memory');
      }
      wasmMemory = exports.memory;
      return mod;
    })();
  }
  return modulePromise;
}
