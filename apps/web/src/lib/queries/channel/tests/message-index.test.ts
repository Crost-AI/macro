import type { ApiChannelMessage } from '@service-storage/client';
import { createRoot } from 'solid-js';
import { describe, expect, it } from 'vitest';
import {
  type ChannelMessagesData,
  createMessageIndex,
} from '../channel-messages';

/**
 * The virtualized message list keys its rows on these ids, and Solid's `<For>`
 * compares them by value. Stable, unique, primitive keys are therefore the
 * premise the channel's render performance rests on: a duplicate id, or a key
 * that is not a primitive, makes every update rebuild every row — each one a
 * full subtree of hover cards, tooltips and reaction popovers.
 */

const message = (id: string): ApiChannelMessage =>
  ({ id }) as unknown as ApiChannelMessage;

const data = (...pages: string[][]): ChannelMessagesData =>
  ({
    pages: pages.map((ids) => ({ items: ids.map(message) })),
    pageParams: pages.map(() => null),
  }) as unknown as ChannelMessagesData;

const indexOf = (value: ChannelMessagesData | undefined) =>
  createRoot(() => createMessageIndex(() => value));

describe('createMessageIndex', () => {
  it('returns primitive keys', () => {
    const index = indexOf(data(['c', 'b', 'a']));

    // `<For>` reuses a row only when the key compares equal by value. Objects
    // would compare by reference and rebuild the list on every update.
    for (const key of index.keys) expect(typeof key).toBe('string');
  });

  it('orders pages and items oldest-first', () => {
    // Pages arrive newest-first and items within a page are newest-first, so
    // both layers are reversed.
    const index = indexOf(data(['d', 'c'], ['b', 'a']));

    expect(index.keys).toEqual(['a', 'b', 'c', 'd']);
  });

  it('drops ids that repeat across overlapping pages', () => {
    // Cursor pagination can hand back a message that a neighbouring page also
    // carried. Duplicated keys make `<For>` rebuild rows on every update.
    const index = indexOf(data(['c', 'b'], ['b', 'a']));

    expect(index.keys).toEqual(['a', 'b', 'c']);
    expect(new Set(index.keys).size).toBe(index.keys.length);
  });

  it('keeps keys aligned with items and the lookup map', () => {
    const index = indexOf(data(['c', 'b'], ['b', 'a']));

    expect(index.items.map((item) => item.id)).toEqual(index.keys);
    for (const key of index.keys) {
      expect(index.byId.get(key)?.id).toBe(key);
    }
  });

  it('is empty when there is no data', () => {
    const index = indexOf(undefined);

    expect(index.keys).toEqual([]);
    expect(index.items).toEqual([]);
  });
});
