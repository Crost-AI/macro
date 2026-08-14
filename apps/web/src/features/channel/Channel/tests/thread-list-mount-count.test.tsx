/** @vitest-environment jsdom */
import { render } from '@solidjs/testing-library';
import { createSignal, onCleanup } from 'solid-js';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { ThreadList } from '../ThreadList';
import { installFakeLayout } from './fake-layout';

const ITEM_SIZE = 96;
const VIEWPORT_SIZE = 600;

let layout: ReturnType<typeof installFakeLayout>;

beforeEach(() => {
  layout = installFakeLayout({
    itemSize: ITEM_SIZE,
    viewportSize: VIEWPORT_SIZE,
  });
});

afterEach(() => {
  layout.uninstall();
});

const keysOf = (count: number, prefix = 'm') =>
  Array.from({ length: count }, (_, i) => `${prefix}${i}`);

/**
 * Counts how many times each key's row factory runs. A virtualized list must
 * build a row once per key while that key stays in range; building it again
 * for a key that never left the range is a message mounted more times than
 * the viewport needs, and every one of those carries the row's full subtree
 * (hover cards, tooltips, reaction popovers) with it.
 */
function renderCountedList(initialKeys: string[]) {
  const [keys, setKeys] = createSignal(initialKeys);
  const mounts = new Map<string, number>();
  const disposals = new Map<string, number>();

  layout.setContentSize(initialKeys.length * ITEM_SIZE);

  const rendered = render(() => (
    <div style={{ height: `${VIEWPORT_SIZE}px` }}>
      <ThreadList keys={keys}>
        {(item) => {
          mounts.set(item.id, (mounts.get(item.id) ?? 0) + 1);
          onCleanup(() =>
            disposals.set(item.id, (disposals.get(item.id) ?? 0) + 1)
          );
          return <div data-key={item.id}>{item.id}</div>;
        }}
      </ThreadList>
    </div>
  ));

  layout.flushResizes();

  const setKeysAndSettle = (next: string[]) => {
    setKeys(next);
    layout.setContentSize(next.length * ITEM_SIZE);
    layout.flushResizes();
  };

  return { ...rendered, mounts, disposals, setKeysAndSettle };
}

const remountedSince = (
  before: Map<string, number>,
  mounts: Map<string, number>
) =>
  [...before.entries()]
    .filter(([key, count]) => (mounts.get(key) ?? 0) > count)
    .map(([key]) => key);

describe('ThreadList row mounting', () => {
  it('builds each in-range row exactly once, and only in-range rows', () => {
    const { mounts } = renderCountedList(keysOf(200));

    // Guards the harness itself: with no layout every element measures zero,
    // the range is empty, and every assertion below passes vacuously.
    expect(mounts.size).toBeGreaterThan(0);
    expect(mounts.size).toBeLessThan(200);
    expect([...mounts.values()].filter((count) => count > 1)).toEqual([]);
  });

  it('does not rebuild existing rows when newer messages arrive', () => {
    const { mounts, setKeysAndSettle } = renderCountedList(keysOf(200));
    const before = new Map(mounts);

    setKeysAndSettle([...keysOf(200), 'm200', 'm201']);

    expect(remountedSince(before, mounts)).toEqual([]);
  });

  it('does not rebuild existing rows when an older page is prepended', () => {
    const { mounts, setKeysAndSettle } = renderCountedList(keysOf(200));
    const before = new Map(mounts);

    // Older pages arrive at the head, shifting every existing index. Rows must
    // follow their key, not their position.
    setKeysAndSettle([...keysOf(20, 'older'), ...keysOf(200)]);

    expect(remountedSince(before, mounts)).toEqual([]);
  });

  it('does not rebuild rows when the key list is rebuilt with the same ids', () => {
    const { mounts, setKeysAndSettle } = renderCountedList(keysOf(200));
    const before = new Map(mounts);

    // Any message field changing rebuilds the key array; row identity must
    // survive it.
    setKeysAndSettle([...keysOf(200)]);

    expect(remountedSince(before, mounts)).toEqual([]);
  });
});
