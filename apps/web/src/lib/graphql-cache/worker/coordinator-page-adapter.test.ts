import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  createCacheCoordinatorPageAdapter,
  type DedicatedWorkerLike,
} from './coordinator-page-adapter';
import { CACHE_COORDINATOR_PROTOCOL_VERSION } from './coordinator-protocol';

class FakeCoordinatorPort {
  readonly messages: Array<{ message: unknown; transfer: Transferable[] }> = [];
  onmessage: ((event: MessageEvent<unknown>) => void) | null = null;
  onmessageerror: (() => void) | null = null;
  closed = false;

  postMessage(message: unknown, transfer: Transferable[] = []): void {
    this.messages.push({ message, transfer });
  }

  start(): void {}

  close(): void {
    this.closed = true;
  }

  receive(message: unknown): void {
    this.onmessage?.({ data: message } as MessageEvent);
  }
}

class FakeTransferPort {
  onmessage = null;
  onmessageerror = null;
  postMessage(): void {}
  start(): void {}
  close(): void {}
}

class FakeMessageChannel {
  port1 = new FakeTransferPort();
  port2 = new FakeTransferPort();
}

class FakeWorker implements DedicatedWorkerLike {
  onerror: DedicatedWorkerLike['onerror'] = null;
  onmessageerror: DedicatedWorkerLike['onmessageerror'] = null;
  readonly messages: Array<{ message: unknown; transfer: Transferable[] }> = [];
  terminated = false;

  postMessage(message: unknown, transfer: Transferable[]): void {
    this.messages.push({ message, transfer });
  }

  terminate(): void {
    this.terminated = true;
  }
}

const version = {
  coordinatorVersion: CACHE_COORDINATOR_PROTOCOL_VERSION,
} as const;

const heldLockManager = (events: string[] = []) =>
  ({
    request: vi.fn(
      async (
        name: string,
        _options: LockOptions,
        callback: (lock: Lock | null) => unknown
      ) => {
        events.push(`lock:${name}`);
        return await callback({ name, mode: 'exclusive' } as Lock);
      }
    ),
  }) as unknown as Pick<LockManager, 'request'>;

const election = (ownerEpoch: number) => ({
  ...version,
  kind: 'become-owner',
  scope: 'scope',
  tabId: 'tab-a',
  ownerEpoch,
  databaseAction:
    ownerEpoch === 1
      ? ('open-existing' as const)
      : ('wipe-before-open' as const),
  ownerLockName: 'physical-lock',
});

describe('CacheCoordinatorPageAdapter', () => {
  beforeEach(() => {
    vi.stubGlobal('MessageChannel', FakeMessageChannel);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('is construction-safe, creates SharedWorker on first use, and DedicatedWorker only on election', async () => {
    const events: string[] = [];
    const coordinatorPort = new FakeCoordinatorPort();
    const sharedFactory = vi.fn(() => {
      events.push('shared-worker');
      return { port: coordinatorPort as unknown as MessagePort };
    });
    const dedicatedFactory = vi.fn(() => new FakeWorker());
    const adapter = createCacheCoordinatorPageAdapter({
      scope: 'scope',
      tabId: 'tab-a',
      lockManager: heldLockManager(events),
      createSharedWorker: sharedFactory,
      createDedicatedWorker: dedicatedFactory,
    });

    expect(events).toEqual([]);
    expect(sharedFactory).not.toHaveBeenCalled();
    expect(dedicatedFactory).not.toHaveBeenCalled();

    adapter.postMessage({ id: 1, kind: 'clear' });
    await vi.waitFor(() => expect(sharedFactory).toHaveBeenCalledOnce());
    expect(events).toEqual([
      'lock:graphql-cache-tab:scope:tab-a',
      'shared-worker',
    ]);
    expect(dedicatedFactory).not.toHaveBeenCalled();
    expect(
      (coordinatorPort.messages[0]!.message as { kind?: string }).kind
    ).toBe('register-tab');

    coordinatorPort.receive({ ...version, kind: 'registered', tabId: 'tab-a' });
    expect(
      coordinatorPort.messages.some(
        ({ message }) => (message as { kind?: string }).kind === 'cache-request'
      )
    ).toBe(true);
    expect(dedicatedFactory).not.toHaveBeenCalled();

    coordinatorPort.receive(election(1));
    expect(dedicatedFactory).toHaveBeenCalledOnce();
    const attach = coordinatorPort.messages.find(
      ({ message }) =>
        (message as { kind?: string }).kind === 'attach-engine-port'
    );
    expect(attach?.transfer).toHaveLength(1);
  });

  it('terminates and clears a failed worker before reporting owner loss', async () => {
    const order: string[] = [];
    const coordinatorPort = new FakeCoordinatorPort();
    const worker = new FakeWorker();
    worker.terminate = () => {
      worker.terminated = true;
      order.push('terminated');
    };
    coordinatorPort.postMessage = (message, transfer = []) => {
      coordinatorPort.messages.push({ message, transfer });
      if ((message as { kind?: string }).kind === 'engine-lost') {
        order.push('loss-reported');
      }
    };
    const adapter = createCacheCoordinatorPageAdapter({
      scope: 'scope',
      tabId: 'tab-a',
      lockManager: heldLockManager(),
      createSharedWorker: () => ({
        port: coordinatorPort as unknown as MessagePort,
      }),
      createDedicatedWorker: () => worker,
    });
    const started = adapter.start();
    await vi.waitFor(() => expect(coordinatorPort.messages).toHaveLength(1));
    coordinatorPort.receive({ ...version, kind: 'registered', tabId: 'tab-a' });
    await started;
    coordinatorPort.receive(election(1));

    worker.onerror?.call(
      {} as AbstractWorker,
      {
        message: 'worker crashed',
        preventDefault: vi.fn(),
      } as unknown as ErrorEvent
    );

    expect(order).toEqual(['terminated', 'loss-reported']);
    expect(worker.terminated).toBe(true);
    expect(coordinatorPort.messages).toContainEqual(
      expect.objectContaining({
        message: expect.objectContaining({
          kind: 'engine-lost',
          ownerEpoch: 1,
          reason: 'worker crashed',
        }),
      })
    );
  });

  it('accepts same-page replacement only after the failed worker was cleared', async () => {
    const coordinatorPort = new FakeCoordinatorPort();
    const workers = [new FakeWorker(), new FakeWorker()];
    const dedicatedFactory = vi.fn(() => workers.shift() as FakeWorker);
    const adapter = createCacheCoordinatorPageAdapter({
      scope: 'scope',
      tabId: 'tab-a',
      lockManager: heldLockManager(),
      createSharedWorker: () => ({
        port: coordinatorPort as unknown as MessagePort,
      }),
      createDedicatedWorker: dedicatedFactory,
    });
    const started = adapter.start();
    await vi.waitFor(() => expect(coordinatorPort.messages).toHaveLength(1));
    coordinatorPort.receive({ ...version, kind: 'registered', tabId: 'tab-a' });
    await started;
    coordinatorPort.receive(election(1));
    coordinatorPort.receive({
      ...version,
      kind: 'terminate-engine',
      tabId: 'tab-a',
      ownerEpoch: 1,
      reason: 'heartbeat timeout',
    });
    coordinatorPort.receive(election(2));

    expect(dedicatedFactory).toHaveBeenCalledTimes(2);
  });

  it('forwards only validated unchanged cache messages to its consumer', async () => {
    const coordinatorPort = new FakeCoordinatorPort();
    const protocolErrors: string[] = [];
    const adapter = createCacheCoordinatorPageAdapter({
      scope: 'scope',
      tabId: 'tab-a',
      lockManager: heldLockManager(),
      createSharedWorker: () => ({
        port: coordinatorPort as unknown as MessagePort,
      }),
      createDedicatedWorker: () => new FakeWorker(),
      onProtocolError: (error) => protocolErrors.push(error.message),
    });
    const messages: unknown[] = [];
    adapter.onmessage = (event) => messages.push(event.data);
    const started = adapter.start();
    await vi.waitFor(() => expect(coordinatorPort.messages).toHaveLength(1));
    coordinatorPort.receive({ ...version, kind: 'registered', tabId: 'tab-a' });
    await started;

    coordinatorPort.receive({
      ...version,
      kind: 'cache-message',
      message: { id: 8, ok: true, result: 'ok' },
    });
    coordinatorPort.receive({
      ...version,
      kind: 'cache-message',
      message: { id: 'bad', ok: true, result: 'ignored' },
    });

    expect(messages).toEqual([{ id: 8, ok: true, result: 'ok' }]);
    expect(protocolErrors).toContain(
      'invalid cache-message coordinator envelope'
    );
  });

  it('keeps the worker alive through graceful drain and terminates after retire-complete', async () => {
    vi.useFakeTimers();
    const coordinatorPort = new FakeCoordinatorPort();
    const worker = new FakeWorker();
    const adapter = createCacheCoordinatorPageAdapter({
      scope: 'scope',
      tabId: 'tab-a',
      lockManager: heldLockManager(),
      createSharedWorker: () => ({
        port: coordinatorPort as unknown as MessagePort,
      }),
      createDedicatedWorker: () => worker,
    });
    const started = adapter.start();
    await vi.waitFor(() => expect(coordinatorPort.messages).toHaveLength(1));
    coordinatorPort.receive({ ...version, kind: 'registered', tabId: 'tab-a' });
    await started;
    coordinatorPort.receive(election(1));

    const disposed = adapter.dispose({ graceful: true });
    expect(worker.terminated).toBe(false);
    expect(coordinatorPort.messages).toContainEqual(
      expect.objectContaining({
        message: expect.objectContaining({ kind: 'graceful-departure' }),
      })
    );
    coordinatorPort.receive({
      ...version,
      kind: 'retire-complete',
      tabId: 'tab-a',
      ownerEpoch: 1,
    });
    await disposed;

    expect(worker.terminated).toBe(true);
    expect(coordinatorPort.closed).toBe(true);
    vi.useRealTimers();
  });
});
