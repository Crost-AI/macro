/**
 * jsdom has no layout engine, so a virtualized list measures every element as
 * zero and renders an empty range — any assertion about row mounting against
 * bare jsdom passes vacuously. This installs the minimum believable layout:
 * a ResizeObserver that reports sizes, and scroll geometry on the scroller.
 */

export type FakeLayoutOptions = {
  /** Height reported for the scroll viewport. */
  viewportSize?: number;
  /** Height reported for each virtualized row. */
  itemSize?: number;
};

const VIEWPORT_ATTRIBUTE = 'data-channel-scroll';

type Installed = {
  /** Total scrollable content height, driven by the current item count. */
  setContentSize: (size: number) => void;
  /** Deliver every queued resize measurement, repeatedly until quiescent. */
  flushResizes: () => void;
  uninstall: () => void;
};

export function installFakeLayout(options: FakeLayoutOptions = {}): Installed {
  const viewportSize = options.viewportSize ?? 600;
  const itemSize = options.itemSize ?? 96;

  const previousResizeObserver = globalThis.ResizeObserver;
  const scrollState = new WeakMap<Element, { scrollTop: number }>();
  let contentSize = 0;

  const isViewport = (el: Element) =>
    el instanceof HTMLElement && el.hasAttribute(VIEWPORT_ATTRIBUTE);

  const sizeOf = (el: Element) => (isViewport(el) ? viewportSize : itemSize);

  const pendingDeliveries: Array<() => void> = [];

  class FakeResizeObserver implements ResizeObserver {
    #callback: ResizeObserverCallback;
    #targets = new Set<Element>();

    constructor(callback: ResizeObserverCallback) {
      this.#callback = callback;
    }

    observe(target: Element): void {
      this.#targets.add(target);
      // Real ResizeObservers deliver an initial measurement for every observed
      // element, asynchronously. virtua depends on that to leave its zero-size
      // initial state, and on the delivery being late enough that its store
      // subscribers already exist.
      pendingDeliveries.push(() => {
        if (!this.#targets.has(target)) return;
        this.#callback(
          [
            {
              target,
              contentRect: {
                width: 800,
                height: sizeOf(target),
              } as DOMRectReadOnly,
            } as ResizeObserverEntry,
          ],
          this
        );
      });
    }

    unobserve(target: Element): void {
      this.#targets.delete(target);
    }

    disconnect(): void {
      this.#targets.clear();
    }
  }

  globalThis.ResizeObserver =
    FakeResizeObserver as unknown as typeof ResizeObserver;
  // virtua constructs its observer as `new window.ResizeObserver(...)`.
  (globalThis as { window?: Window }).window &&
    Object.defineProperty(globalThis.window, 'ResizeObserver', {
      value: FakeResizeObserver,
      configurable: true,
      writable: true,
    });

  // Scroll geometry: jsdom reports 0 for all of these, which makes
  // "distance from bottom" always 0 and hides real scrolling behaviour.
  const descriptors: Record<string, PropertyDescriptor> = {
    // virtua discards resize entries for elements it believes are display:none,
    // detected via `!target.offsetParent`. jsdom returns null for every
    // element, so without this every measurement is dropped and the list
    // renders an empty range.
    offsetParent: {
      get(this: HTMLElement) {
        return this.isConnected ? (this.ownerDocument.body ?? null) : null;
      },
      configurable: true,
    },
    clientHeight: {
      get(this: HTMLElement) {
        return isViewport(this) ? viewportSize : itemSize;
      },
      configurable: true,
    },
    scrollHeight: {
      get(this: HTMLElement) {
        return isViewport(this) ? contentSize : itemSize;
      },
      configurable: true,
    },
    scrollTop: {
      get(this: HTMLElement) {
        return scrollState.get(this)?.scrollTop ?? 0;
      },
      set(this: HTMLElement, value: number) {
        const max = Math.max(0, contentSize - viewportSize);
        const clamped = Math.max(0, Math.min(value, max));
        scrollState.set(this, { scrollTop: clamped });
        this.dispatchEvent(new Event('scroll'));
      },
      configurable: true,
    },
  };

  const originals = new Map<string, PropertyDescriptor | undefined>();
  for (const [name, descriptor] of Object.entries(descriptors)) {
    originals.set(
      name,
      Object.getOwnPropertyDescriptor(HTMLElement.prototype, name)
    );
    Object.defineProperty(HTMLElement.prototype, name, descriptor);
  }

  return {
    setContentSize: (size: number) => {
      contentSize = size;
    },
    flushResizes: () => {
      // Newly rendered rows register their own observers while earlier
      // measurements are delivered, so drain until the queue stops refilling.
      for (let pass = 0; pass < 20 && pendingDeliveries.length; pass++) {
        const batch = pendingDeliveries.splice(0, pendingDeliveries.length);
        for (const deliver of batch) deliver();
      }
    },
    uninstall: () => {
      globalThis.ResizeObserver = previousResizeObserver;
      for (const [name, descriptor] of originals) {
        if (descriptor) {
          Object.defineProperty(HTMLElement.prototype, name, descriptor);
        } else {
          Reflect.deleteProperty(HTMLElement.prototype, name);
        }
      }
    },
  };
}
