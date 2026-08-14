/** @vitest-environment jsdom */
import { render } from '@solidjs/testing-library';
import { createSignal } from 'solid-js';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ThreadList, type ThreadListScrollSnapshot } from '../ThreadList';
import { installFakeLayout } from './fake-layout';

const ITEM_SIZE = 96;
const VIEWPORT_SIZE = 600;
const COUNT = 200;

let layout: ReturnType<typeof installFakeLayout>;

beforeEach(() => {
  layout = installFakeLayout({
    itemSize: ITEM_SIZE,
    viewportSize: VIEWPORT_SIZE,
  });
});

afterEach(() => layout.uninstall());

/**
 * The snapshot feeds channel scroll restore: it is read at teardown by
 * `getMessagesStateSnapshot` and handed back as `initialScrollSnapshot` the
 * next time the channel opens. Refreshing virtua's measured-size array is the
 * expensive part, so it only happens when scrolling settles — these cover the
 * consequences of that, since a snapshot that never carries sizes, or one that
 * loses the offset between settles, restores the reader to the wrong place.
 */
function renderList() {
  const [keys] = createSignal(Array.from({ length: COUNT }, (_, i) => `m${i}`));
  layout.setContentSize(COUNT * ITEM_SIZE);

  const snapshots: ThreadListScrollSnapshot[] = [];
  const rendered = render(() => (
    <div style={{ height: `${VIEWPORT_SIZE}px` }}>
      <ThreadList
        keys={keys}
        onScrollSnapshotChange={(snapshot) => snapshots.push(snapshot)}
      >
        {(item) => <div data-key={item.id}>{item.id}</div>}
      </ThreadList>
    </div>
  ));
  layout.flushResizes();

  const scroller = rendered.container.querySelector(
    '[data-channel-scroll]'
  ) as HTMLElement;

  return { ...rendered, snapshots, scroller };
}

describe('ThreadList scroll snapshot', () => {
  it('emits a snapshot carrying virtua’s measured sizes', () => {
    const { snapshots } = renderList();

    expect(snapshots.length).toBeGreaterThan(0);
    // Without a refresh on completion the restore snapshot would permanently
    // carry `undefined` sizes, so a reopened channel re-measures every row.
    expect(snapshots.at(-1)?.virtualCache).toBeDefined();
  });

  it('keeps reporting the live scroll offset between settles', () => {
    const { snapshots, scroller } = renderList();
    const before = snapshots.length;

    scroller.scrollTop = 4_000;

    const emitted = snapshots.slice(before);
    expect(emitted.length).toBeGreaterThan(0);
    expect(emitted.at(-1)?.scrollOffset).toBe(4_000);
  });

  it('reuses one size array across a scroll instead of cloning per event', () => {
    // This is the optimization itself, so assert the mechanism rather than
    // truthiness: `handle.cache` copies virtua's per-item size array, and
    // reading it on every scroll event clones the whole measured list dozens
    // of times a second. Identical references prove no clone happened.
    const { snapshots, scroller } = renderList();

    scroller.scrollTop = 2_000;
    scroller.scrollTop = 5_000;

    const duringScroll = snapshots.slice(-2);
    expect(duringScroll).toHaveLength(2);
    expect(duringScroll[0]?.virtualCache).toBeDefined();
    expect(duringScroll[1]?.virtualCache).toBe(duringScroll[0]?.virtualCache);
  });

  it('takes a fresh size array once scrolling settles', () => {
    // The counterpart: reuse must not become staleness. Settling has to
    // re-read the sizes, or measurements taken during the scroll never reach
    // the snapshot that restore depends on.
    vi.useFakeTimers();
    try {
      const { snapshots, scroller } = renderList();

      // End the open-at-bottom pin deterministically through the path the
      // component actually exposes for it — a wheel-up aborts the settle loop.
      // Racing it with a timer advance makes this test order-dependent.
      scroller.dispatchEvent(
        Object.assign(new Event('wheel', { bubbles: true }), { deltaY: -1 })
      );

      scroller.scrollTop = 5_000;
      const midScroll = snapshots.at(-1)?.virtualCache;
      expect(midScroll).toBeDefined();

      // virtua decides scrolling has ended on a timer of its own; the DOM
      // `scrollend` event is not what drives `onScrollEnd`.
      vi.advanceTimersByTime(1_000);

      const settled = snapshots.at(-1);
      expect(settled?.virtualCache).toBeDefined();
      expect(settled?.virtualCache).not.toBe(midScroll);
    } finally {
      vi.useRealTimers();
    }
  });
});
