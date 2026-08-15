/// <reference lib="webworker" />

import { loadBrowserTestCacheWasm } from './browser-test-wasm-module';

type StorageControlRequest = {
  id: number;
  scope: string;
  kind: 'incompatible-namespace' | 'corrupt-queue-payload';
};

type StorageControlResponse =
  | { id: number; ok: true; wasmUrl: string }
  | { id: number; ok: false; error: string };

const worker = self as unknown as DedicatedWorkerGlobalScope;

worker.onmessage = (event: MessageEvent<StorageControlRequest>) => {
  const request = event.data;
  void (async () => {
    const { module, wasmUrl } = await loadBrowserTestCacheWasm();
    if (request.kind === 'incompatible-namespace') {
      await module.browserTestMakeNamespaceIncompatible(request.scope);
    } else if (request.kind === 'corrupt-queue-payload') {
      await module.browserTestCorruptQueuePayload(request.scope);
    } else {
      throw new Error('unsupported browser-test storage mutation');
    }
    worker.postMessage({
      id: request.id,
      ok: true,
      wasmUrl,
    } satisfies StorageControlResponse);
  })().catch((error: unknown) => {
    worker.postMessage({
      id: request.id,
      ok: false,
      error: error instanceof Error ? error.message : String(error),
    } satisfies StorageControlResponse);
  });
};
