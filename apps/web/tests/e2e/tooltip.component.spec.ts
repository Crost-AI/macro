import { expect, test } from '@playwright/test';

/**
 * The deferred tooltip root, verified with real pointer input.
 *
 * The unit tests in src/components/ui/components/Tooltip.test.tsx cannot cover
 * this: jsdom never gets Kobalte's tooltip to render content (the component did
 * not open there before this change either), so they can only assert that the
 * root exists. The behaviour that actually matters is that a tooltip still
 * appears on the *first* hover, which depends on the pointer enter being
 * replayed into a trigger that mounts with the cursor already inside it.
 *
 * The fixture mounts the production path — `Button` with a `label` — which is
 * how roughly 800 call sites in the app reach this component.
 */

const HARNESS = '/tests/e2e/fixtures/tooltip-harness/index.html';

// Kobalte's openDelay is 400ms.
const OPEN_DELAY_MS = 400;

const tooltipRoot = '[data-closed], [data-expanded]';

test.describe('Tooltip deferred root', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto(HARNESS);
    await expect(page.getByTestId('hover-target')).toBeVisible();
  });

  test('mounts no tooltip root until the trigger is hovered', async ({
    page,
  }) => {
    await expect(page.locator(tooltipRoot)).toHaveCount(0);
  });

  test('shows the tooltip on the first hover', async ({ page }) => {
    await page.getByTestId('hover-target').hover();

    // Not a poll-until-pass: if the replayed pointer enter were dropped, no
    // amount of waiting would open this, because no further enter event fires
    // while the cursor sits still inside the trigger.
    const tip = page.getByRole('tooltip');
    await expect(tip).toBeVisible({ timeout: OPEN_DELAY_MS + 2000 });
    await expect(tip).toHaveText('Reply');
  });

  test('hides the tooltip again once the pointer leaves', async ({ page }) => {
    await page.getByTestId('hover-target').hover();
    await expect(page.getByRole('tooltip')).toBeVisible();

    await page.getByTestId('disabled-target').hover();

    await expect(page.getByRole('tooltip')).toHaveCount(0);
  });

  test('keeps the same trigger element when the real root takes over', async ({
    page,
  }) => {
    const target = page.getByTestId('hover-target');
    // Tag the live node, then confirm the tag survives activation. If the
    // children were rebuilt rather than moved, every ref and listener a caller
    // attached to them would be dropped on first hover.
    await target.evaluate((el) => {
      (el as HTMLElement & { __probe?: number }).__probe = 1;
    });

    await target.hover();
    await expect(page.getByRole('tooltip')).toBeVisible();

    const probe = await target.evaluate(
      (el) => (el as HTMLElement & { __probe?: number }).__probe ?? null
    );
    expect(probe).toBe(1);
  });

  test('never mounts a root for a disabled tooltip', async ({ page }) => {
    await page.getByTestId('disabled-target').hover();
    await page.waitForTimeout(OPEN_DELAY_MS + 400);

    await expect(page.getByRole('tooltip')).toHaveCount(0);
    await expect(
      page
        .getByTestId('disabled-target')
        .locator(`xpath=ancestor::*[${'@data-closed or @data-expanded'}]`)
    ).toHaveCount(0);
  });
});
