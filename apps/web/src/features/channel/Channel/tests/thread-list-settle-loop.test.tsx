/** @vitest-environment jsdom */
import { render } from '@solidjs/testing-library';
import { createSignal } from 'solid-js';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ThreadList } from '../ThreadList';
import { installFakeLayout } from './fake-layout';

const ITEM_SIZE = 96;
const VIEWPORT_SIZE = 600;

let layout: ReturnType<typeof installFakeLayout>;
let frames: FrameRequestCallback[];
let previousRaf: typeof requestAnimationFrame;
let previousCancelRaf: typeof cancelAnimationFrame;

beforeEach(() => {
  layout = installFakeLayout({
    itemSize: ITEM_SIZE,
    viewportSize: VIEWPORT_SIZE,
  });

  // A manually pumped frame queue: the settle loop reschedules itself every
  // frame, so counting scheduled frames is how "is it still polling" is
  // observed without waiting out real time.
  frames = [];
  previousRaf = globalThis.requestAnimationFrame;
  previousCancelRaf = globalThis.cancelAnimationFrame;
  globalThis.requestAnimationFrame = ((callback: FrameRequestCallback) =>
    frames.push(callback)) as unknown as typeof requestAnimationFrame;
  globalThis.cancelAnimationFrame = (() => {
    // Handles are not reused in this harness; pumping simply drains the queue.
  }) as unknown as typeof cancelAnimationFrame;
});

afterEach(() => {
  globalThis.requestAnimationFrame = previousRaf;
  globalThis.cancelAnimationFrame = previousCancelRaf;
  layout.uninstall();
});

/** Run every currently queued frame callback once, returning how many ran. */
function pumpFrame(): number {
  const queued = frames;
  frames = [];
  for (const callback of queued) callback(performance.now());
  return queued.length;
}

function renderList(count: number) {
  const [keys] = createSignal(Array.from({ length: count }, (_, i) => `m${i}`));
  layout.setContentSize(count * ITEM_SIZE);

  const rendered = render(() => (
    <div style={{ height: `${VIEWPORT_SIZE}px` }}>
      <ThreadList keys={keys}>
        {(item) => <div data-key={item.id}>{item.id}</div>}
      </ThreadList>
    </div>
  ));
  layout.flushResizes();
  return rendered;
}

describe('ThreadList scroll-to-bottom settle loop', () => {
  it('stops polling once the viewport rests at the bottom', () => {
    renderList(200);

    // Drain the mount-time frames, then confirm the loop winds down instead of
    // rescheduling itself for the full settle window. Each polled frame writes
    // scrollTop, and every write emits a scroll event that can move virtua's
    // range and churn a wave of message rows.
    let pumped = 0;
    for (let i = 0; i < 30 && frames.length > 0; i++) {
      pumpFrame();
      pumped++;
    }

    expect(frames).toHaveLength(0);
    expect(pumped).toBeLessThan(30);
  });

  it('re-pins to the bottom when content grows after the scroll landed', () => {
    // The whole point of the settle window: a late-loading image or embed
    // grows the content and pushes the bottom away after the initial scroll
    // has already landed. Stopping the polling early must not cost this — if
    // the settled-frame counter stopped resetting, or the ResizeObserver were
    // torn down with the polling, the last message would be left cut off.
    const { container } = renderList(200);
    for (let i = 0; i < 10 && frames.length > 0; i++) pumpFrame();

    const scroller = container.querySelector(
      '[data-channel-scroll]'
    ) as HTMLElement;
    const settledAt = scroller.scrollTop;

    layout.growContent(2_000);

    expect(scroller.scrollTop).toBeGreaterThan(settledAt);
    expect(scroller.scrollHeight - scroller.clientHeight).toBe(
      scroller.scrollTop
    );
  });

  it('stops re-pinning once the settle window has expired', () => {
    // The counterpart to the test above. Keeping the ResizeObserver alive past
    // the early polling exit is what makes late growth land, but it has to be
    // released when the window closes — otherwise every later content resize
    // drags a reader who has scrolled up back down to the newest message.
    // Only setTimeout is faked; the frame queue above stays under manual
    // control.
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    try {
      const { container } = renderList(200);
      for (let i = 0; i < 10 && frames.length > 0; i++) pumpFrame();

      const scroller = container.querySelector(
        '[data-channel-scroll]'
      ) as HTMLElement;

      // Close the window, then read away from the bottom.
      vi.advanceTimersByTime(2_000);
      scroller.scrollTop = 3_000;

      layout.growContent(2_000);

      expect(scroller.scrollTop).toBe(3_000);
    } finally {
      vi.useRealTimers();
    }
  });

  it('does not touch the scroller after unmount when the settle window ends', () => {
    // Ending the polling early moved the close of the settle window onto a
    // timeout. If that timeout outlives the mount it fires `stop()` against a
    // detached scroller a second after every channel switch. Asserting on the
    // effect rather than on a global timer count, since virtua keeps timers of
    // its own here.
    vi.useFakeTimers();
    try {
      const { unmount, container } = renderList(200);
      for (let i = 0; i < 10 && frames.length > 0; i++) pumpFrame();

      const scroller = container.querySelector(
        '[data-channel-scroll]'
      ) as HTMLElement;
      unmount();

      let touchedAfterUnmount = false;
      Object.defineProperty(scroller, 'scrollTop', {
        get: () => 0,
        set: () => {
          touchedAfterUnmount = true;
        },
        configurable: true,
      });

      vi.advanceTimersByTime(5_000);
      for (let i = 0; i < 10 && frames.length > 0; i++) pumpFrame();

      expect(touchedAfterUnmount).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });
});
