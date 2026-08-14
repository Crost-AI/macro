/** @vitest-environment jsdom */
import { render } from '@solidjs/testing-library';
import { createSignal } from 'solid-js';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
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

  it('carries sizes on every snapshot, including mid-scroll ones', () => {
    // Mid-scroll snapshots reuse the last measured sizes rather than cloning
    // fresh ones. Reusing must not mean dropping: whichever snapshot teardown
    // happens to read still has to be restorable.
    const { snapshots, scroller } = renderList();

    scroller.scrollTop = 2_000;
    scroller.scrollTop = 5_000;

    expect(snapshots.length).toBeGreaterThan(1);
    for (const snapshot of snapshots) {
      expect(snapshot.virtualCache).toBeDefined();
    }
  });

  it('refreshes the sizes once scrolling settles', () => {
    const { snapshots, scroller } = renderList();

    scroller.scrollTop = 5_000;
    scroller.dispatchEvent(new Event('scrollend'));

    const last = snapshots.at(-1);
    expect(last?.virtualCache).toBeDefined();
    expect(last?.scrollOffset).toBe(5_000);
  });
});
