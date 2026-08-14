/** @vitest-environment jsdom */
import { render } from '@solidjs/testing-library';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { setTooltipsEnabled } from '../signals/signals';
import { Tooltip } from './Tooltip';

beforeEach(() => {
  setTooltipsEnabled(true);
  // The component renders bare children under MODE === 'test' so other suites
  // skip its machinery; this suite exists to test that machinery.
  vi.stubEnv('MODE', 'development');
});

afterEach(() => {
  setTooltipsEnabled(true);
  vi.unstubAllEnvs();
});

// jsdom ships no PointerEvent constructor and cannot drive Kobalte's tooltip
// far enough to render content (the unmodified component does not open here
// either), so these assert the thing that is observable: whether Kobalte's
// root exists. It brands its trigger with `data-closed`/`data-expanded`.
const pointerEnter = (el: Element) =>
  el.dispatchEvent(new Event('pointerenter', { bubbles: true }));

const hasTooltipRoot = (container: HTMLElement) =>
  container.querySelector('[data-closed],[data-expanded]') !== null;

function renderTooltip() {
  const result = render(() => (
    <Tooltip label="Reply" class="probe">
      <button type="button" data-child>
        Reply
      </button>
    </Tooltip>
  ));
  return {
    ...result,
    wrapper: result.container.querySelector('.probe') as HTMLElement,
    child: result.container.querySelector('[data-child]') as HTMLElement,
  };
}

describe('Tooltip', () => {
  it('renders its trigger without mounting a tooltip root', () => {
    const { child, container } = renderTooltip();

    expect(child.textContent).toBe('Reply');
    expect(hasTooltipRoot(container)).toBe(false);
  });

  it('mounts the tooltip root on hover', () => {
    const { wrapper, container } = renderTooltip();

    pointerEnter(wrapper);

    expect(hasTooltipRoot(container)).toBe(true);
  });

  it('mounts the tooltip root when the trigger takes focus', () => {
    const { wrapper, container } = renderTooltip();

    wrapper.dispatchEvent(new Event('focusin', { bubbles: true }));

    expect(hasTooltipRoot(container)).toBe(true);
  });

  it('keeps the same child element when the real trigger takes over', () => {
    const { child, wrapper, container } = renderTooltip();

    pointerEnter(wrapper);

    // The placeholder is replaced by Kobalte's trigger. If the children were
    // rebuilt rather than moved, every ref and listener a caller attached to
    // them — a Button's ref, the touch handlers on a reaction chip — would be
    // silently dropped on first hover.
    expect(container.querySelector('[data-child]')).toBe(child);
    expect(child.isConnected).toBe(true);
  });

  it('preserves the wrapper classes across activation', () => {
    const { wrapper, container } = renderTooltip();
    const before = wrapper.className;

    pointerEnter(wrapper);

    const after = container.querySelector('.probe') as HTMLElement;
    expect(after.className).toBe(before);
  });

  it('never mounts a tooltip root while tooltips are disabled', () => {
    setTooltipsEnabled(false);
    const { wrapper, container } = renderTooltip();

    pointerEnter(wrapper);

    expect(hasTooltipRoot(container)).toBe(false);
  });
});
