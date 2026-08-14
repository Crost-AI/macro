import type { CacheRequest, WorkerMessage } from '../protocol';
import {
  CACHE_COORDINATOR_PROTOCOL_VERSION,
  type CoordinatorToTabEnvelope,
  isCacheRequest,
  type PageToEngineEnvelope,
  type TabToCoordinatorEnvelope,
  tabLivenessLockName,
  validateCoordinatorToTabEnvelope,
} from './coordinator-protocol';

export interface SharedWorkerLike {
  readonly port: MessagePort;
}

export interface DedicatedWorkerLike {
  onerror: ((this: AbstractWorker, event: ErrorEvent) => unknown) | null;
  onmessageerror: ((this: Worker, event: MessageEvent) => unknown) | null;
  postMessage(message: unknown, transfer: Transferable[]): void;
  terminate(): void;
}

export interface CacheCoordinatorPageAdapterOptions {
  scope: string;
  hotCapacity?: number;
  tabId?: string;
  createSharedWorker?: (scope: string) => SharedWorkerLike;
  createDedicatedWorker?: (
    scope: string,
    ownerEpoch: number
  ) => DedicatedWorkerLike;
  lockManager?: Pick<LockManager, 'request'>;
  onEngineReplaced?: (ownerEpoch: number) => void;
  onOwnerChanged?: (ownerEpoch: number | undefined) => void;
  onWorkerCreated?: (worker: DedicatedWorkerLike, ownerEpoch: number) => void;
  onWorkerTerminated?: (ownerEpoch: number, reason: string) => void;
  onProtocolError?: (error: Error) => void;
  gracefulTimeoutMs?: number;
}

export interface PageAdapterDisposeOptions {
  graceful?: boolean;
}

const DEFAULT_GRACEFUL_TIMEOUT_MS = 10_000;

const withVersion = <T extends { coordinatorVersion: 1 }>(
  value: T extends unknown ? Omit<T, 'coordinatorVersion'> : never
): T =>
  ({
    coordinatorVersion: CACHE_COORDINATOR_PROTOCOL_VERSION,
    ...value,
  }) as unknown as T;

const defaultSharedWorkerFactory = (scope: string): SharedWorkerLike =>
  new SharedWorker(
    new URL('./cache.coordinator.shared-worker.ts', import.meta.url),
    { type: 'module', name: `graphql-cache-coordinator:${scope}` }
  );

const defaultDedicatedWorkerFactory = (
  scope: string,
  ownerEpoch: number
): DedicatedWorkerLike =>
  new Worker(new URL('./cache.engine-worker.ts', import.meta.url), {
    type: 'module',
    name: `graphql-cache-engine:${scope}:${ownerEpoch}`,
  });

/**
 * Import-safe page endpoint for the browser coordinator topology.
 * SharedWorker creation starts on first use; DedicatedWorker creation starts
 * only after a current-epoch election.
 */
export class CacheCoordinatorPageAdapter {
  readonly tabId: string;
  onmessage: ((event: MessageEvent<WorkerMessage>) => void) | null = null;

  private readonly createSharedWorker: (scope: string) => SharedWorkerLike;
  private readonly createDedicatedWorker: (
    scope: string,
    ownerEpoch: number
  ) => DedicatedWorkerLike;
  private readonly lockManager: Pick<LockManager, 'request'> | undefined;
  private readonly gracefulTimeoutMs: number;
  private sharedWorker: SharedWorkerLike | undefined;
  private engineWorker: DedicatedWorkerLike | undefined;
  private ownerEpoch: number | undefined;
  private registered = false;
  private startPromise: Promise<void> | undefined;
  private resolveRegistration: (() => void) | undefined;
  private rejectRegistration: ((error: Error) => void) | undefined;
  private readonly queuedRequests: CacheRequest[] = [];
  private releaseLivenessLock: (() => void) | undefined;
  private disposePromise: Promise<void> | undefined;
  private resolveDispose: (() => void) | undefined;
  private pagehideRegistered = false;
  private closed = false;

  constructor(private readonly options: CacheCoordinatorPageAdapterOptions) {
    if (!options.scope) throw new Error('cache coordinator scope is required');
    if (
      options.hotCapacity !== undefined &&
      (!Number.isSafeInteger(options.hotCapacity) || options.hotCapacity <= 0)
    ) {
      throw new Error('hot capacity must be a positive integer');
    }
    this.tabId = options.tabId ?? crypto.randomUUID();
    this.createSharedWorker =
      options.createSharedWorker ?? defaultSharedWorkerFactory;
    this.createDedicatedWorker =
      options.createDedicatedWorker ?? defaultDedicatedWorkerFactory;
    this.lockManager = options.lockManager;
    this.gracefulTimeoutMs =
      options.gracefulTimeoutMs ?? DEFAULT_GRACEFUL_TIMEOUT_MS;
  }

  /** Acquires tab liveness and registers without constructing an engine. */
  start(): Promise<void> {
    if (this.closed) return Promise.reject(new Error('page adapter is closed'));
    if (this.startPromise) return this.startPromise;
    this.startPromise = new Promise<void>((resolve, reject) => {
      this.resolveRegistration = resolve;
      this.rejectRegistration = reject;
    });
    void this.connect().catch((error: unknown) => {
      this.failStartup(
        error instanceof Error ? error : new Error(String(error))
      );
    });
    return this.startPromise;
  }

  /** Queues unchanged cache RPC until registration and engine readiness. */
  postMessage(request: CacheRequest): void {
    if (!isCacheRequest(request)) {
      const error = new Error('invalid cache request');
      this.options.onProtocolError?.(error);
      if (
        typeof (request as { id?: unknown })?.id === 'number' &&
        Number.isSafeInteger((request as { id: number }).id)
      ) {
        this.emit({
          id: (request as { id: number }).id,
          ok: false,
          error: error.message,
        });
      }
      return;
    }
    if (this.closed) {
      this.emit({ id: request.id, ok: false, error: 'page adapter is closed' });
      return;
    }
    if (!this.registered) {
      this.queuedRequests.push(request);
      void this.start();
      return;
    }
    this.postCoordinator(
      withVersion<TabToCoordinatorEnvelope>({
        kind: 'cache-request',
        tabId: this.tabId,
        request,
      })
    );
  }

  /** Gracefully drains an owned engine, or immediately drops a standby tab. */
  dispose(options: PageAdapterDisposeOptions = {}): Promise<void> {
    if (this.disposePromise) return this.disposePromise;
    if (this.closed || !this.sharedWorker) {
      this.closed = true;
      this.releaseLiveness();
      return Promise.resolve();
    }
    this.disposePromise = new Promise<void>((resolve) => {
      this.resolveDispose = resolve;
    });

    if (options.graceful && this.ownerEpoch !== undefined) {
      const ownerEpoch = this.ownerEpoch;
      this.postCoordinator(
        withVersion<TabToCoordinatorEnvelope>({
          kind: 'graceful-departure',
          tabId: this.tabId,
          ownerEpoch,
        })
      );
      setTimeout(() => {
        if (!this.closed) {
          this.terminateEngine(ownerEpoch, 'graceful drain timed out', false);
          this.postCoordinator(
            withVersion<TabToCoordinatorEnvelope>({
              kind: 'disconnect-tab',
              tabId: this.tabId,
              reason: 'graceful page disposal timed out',
            })
          );
          this.finishDispose();
        }
      }, this.gracefulTimeoutMs);
    } else {
      if (this.ownerEpoch !== undefined) {
        this.terminateEngine(
          this.ownerEpoch,
          'page disposed without graceful drain',
          false
        );
      }
      this.postCoordinator(
        withVersion<TabToCoordinatorEnvelope>({
          kind: 'disconnect-tab',
          tabId: this.tabId,
          reason: 'page disposed without graceful drain',
        })
      );
      this.finishDispose();
    }
    return this.disposePromise;
  }

  private async connect(): Promise<void> {
    const lockManager = this.lockManager ?? navigator.locks;
    if (
      typeof SharedWorker !== 'function' &&
      this.options.createSharedWorker === undefined
    ) {
      throw new Error('SharedWorker is unavailable');
    }
    if (
      typeof Worker !== 'function' &&
      this.options.createDedicatedWorker === undefined
    ) {
      throw new Error('DedicatedWorker is unavailable');
    }
    if (typeof MessageChannel !== 'function') {
      throw new Error('MessageChannel is unavailable');
    }
    if (!lockManager) throw new Error('Web Locks are unavailable');

    const livenessLockName = tabLivenessLockName(
      this.options.scope,
      this.tabId
    );
    let acquired: (() => void) | undefined;
    let acquisitionFailed: ((error: Error) => void) | undefined;
    const acquiredPromise = new Promise<void>((resolve, reject) => {
      acquired = resolve;
      acquisitionFailed = reject;
    });
    const heldUntilReleased = new Promise<void>((resolve) => {
      this.releaseLivenessLock = resolve;
    });
    void lockManager
      .request(livenessLockName, { mode: 'exclusive' }, async (lock) => {
        if (!lock) {
          acquisitionFailed?.(new Error('tab liveness lock was not acquired'));
          return;
        }
        acquired?.();
        await heldUntilReleased;
      })
      .catch((error: unknown) => {
        acquisitionFailed?.(
          error instanceof Error ? error : new Error(String(error))
        );
      });
    await acquiredPromise;
    if (this.closed) return;

    const worker = this.createSharedWorker(this.options.scope);
    this.sharedWorker = worker;
    worker.port.onmessage = (event: MessageEvent<unknown>) => {
      this.handleCoordinatorMessage(event.data);
    };
    worker.port.onmessageerror = () => {
      this.options.onProtocolError?.(
        new Error('coordinator MessagePort messageerror')
      );
    };
    worker.port.start();
    this.postCoordinator(
      withVersion<TabToCoordinatorEnvelope>({
        kind: 'register-tab',
        scope: this.options.scope,
        tabId: this.tabId,
        livenessLockName,
        hotCapacity: this.options.hotCapacity,
      })
    );
  }

  private handleCoordinatorMessage(rawMessage: unknown): void {
    const parsed = validateCoordinatorToTabEnvelope(rawMessage);
    if (!parsed.ok) {
      this.options.onProtocolError?.(new Error(parsed.error));
      return;
    }
    const message = parsed.value;
    switch (message.kind) {
      case 'registered':
        if (message.tabId !== this.tabId || this.registered) {
          this.options.onProtocolError?.(
            new Error('invalid coordinator registration acknowledgement')
          );
          return;
        }
        this.registered = true;
        this.resolveRegistration?.();
        this.resolveRegistration = undefined;
        this.rejectRegistration = undefined;
        this.registerPagehide();
        for (const request of this.queuedRequests.splice(0)) {
          this.postMessage(request);
        }
        break;
      case 'become-owner':
        if (
          message.tabId !== this.tabId ||
          message.scope !== this.options.scope
        ) {
          this.options.onProtocolError?.(
            new Error('coordinator elected the wrong page or scope')
          );
          return;
        }
        this.spawnEngine(message);
        break;
      case 'cache-message':
        this.emit(message.message);
        break;
      case 'terminate-engine':
        if (message.tabId === this.tabId) {
          this.terminateEngine(message.ownerEpoch, message.reason, false);
        }
        break;
      case 'retire-complete':
        if (message.tabId === this.tabId) {
          this.terminateEngine(
            message.ownerEpoch,
            'graceful engine retirement completed',
            false
          );
          this.finishDispose();
        }
        break;
      case 'engine-replaced':
        this.options.onEngineReplaced?.(message.ownerEpoch);
        break;
      case 'protocol-error': {
        const error = new Error(message.error);
        if (!this.registered) this.failStartup(error);
        else this.options.onProtocolError?.(error);
        break;
      }
    }
  }

  private spawnEngine(
    election: Extract<CoordinatorToTabEnvelope, { kind: 'become-owner' }>
  ): void {
    if (this.engineWorker || this.ownerEpoch !== undefined) {
      this.options.onProtocolError?.(
        new Error('coordinator elected a page that already owns an engine')
      );
      return;
    }
    const worker = this.createDedicatedWorker(
      this.options.scope,
      election.ownerEpoch
    );
    this.engineWorker = worker;
    this.ownerEpoch = election.ownerEpoch;
    this.options.onOwnerChanged?.(election.ownerEpoch);
    this.options.onWorkerCreated?.(worker, election.ownerEpoch);

    worker.onerror = (event) => {
      event.preventDefault();
      const reason = event.message || 'dedicated engine worker error';
      this.terminateEngine(election.ownerEpoch, reason, true);
    };
    worker.onmessageerror = () => {
      this.terminateEngine(
        election.ownerEpoch,
        'dedicated engine worker messageerror',
        true
      );
    };

    const directChannel = new MessageChannel();
    this.postCoordinator(
      withVersion<TabToCoordinatorEnvelope>({
        kind: 'attach-engine-port',
        tabId: this.tabId,
        ownerEpoch: election.ownerEpoch,
      }),
      [directChannel.port1]
    );
    worker.postMessage(
      withVersion<PageToEngineEnvelope>({
        kind: 'activate-engine',
        scope: election.scope,
        tabId: this.tabId,
        ownerEpoch: election.ownerEpoch,
        databaseAction: election.databaseAction,
        ownerLockName: election.ownerLockName,
        hotCapacity: election.hotCapacity,
      }),
      [directChannel.port2]
    );
  }

  private terminateEngine(
    ownerEpoch: number,
    reason: string,
    reportLoss: boolean
  ): boolean {
    if (!this.engineWorker || this.ownerEpoch !== ownerEpoch) return false;
    const worker = this.engineWorker;
    worker.onerror = null;
    worker.onmessageerror = null;
    worker.terminate();
    this.engineWorker = undefined;
    this.ownerEpoch = undefined;
    this.options.onOwnerChanged?.(undefined);
    this.options.onWorkerTerminated?.(ownerEpoch, reason);
    if (reportLoss && this.sharedWorker && !this.closed) {
      this.postCoordinator(
        withVersion<TabToCoordinatorEnvelope>({
          kind: 'engine-lost',
          tabId: this.tabId,
          ownerEpoch,
          reason,
        })
      );
    }
    return true;
  }

  private emit(message: WorkerMessage): void {
    this.onmessage?.({ data: message } as MessageEvent<WorkerMessage>);
  }

  private postCoordinator(
    message: TabToCoordinatorEnvelope,
    transfer: Transferable[] = []
  ): void {
    this.sharedWorker?.port.postMessage(message, transfer);
  }

  private registerPagehide(): void {
    if (this.pagehideRegistered || typeof addEventListener !== 'function') {
      return;
    }
    this.pagehideRegistered = true;
    addEventListener(
      'pagehide',
      () => {
        void this.dispose({ graceful: false });
      },
      { once: true }
    );
  }

  private failStartup(error: Error): void {
    this.rejectRegistration?.(error);
    this.rejectRegistration = undefined;
    this.resolveRegistration = undefined;
    this.options.onProtocolError?.(error);
    for (const request of this.queuedRequests.splice(0)) {
      this.emit({ id: request.id, ok: false, error: error.message });
    }
    this.sharedWorker?.port.close();
    this.sharedWorker = undefined;
    this.releaseLiveness();
  }

  private finishDispose(): void {
    if (this.closed) return;
    this.closed = true;
    this.registered = false;
    this.sharedWorker?.port.close();
    this.sharedWorker = undefined;
    this.releaseLiveness();
    this.resolveDispose?.();
    this.resolveDispose = undefined;
  }

  private releaseLiveness(): void {
    this.releaseLivenessLock?.();
    this.releaseLivenessLock = undefined;
  }
}

/** Creates a lazy, import-safe page coordinator endpoint. */
export function createCacheCoordinatorPageAdapter(
  options: CacheCoordinatorPageAdapterOptions
): CacheCoordinatorPageAdapter {
  return new CacheCoordinatorPageAdapter(options);
}
