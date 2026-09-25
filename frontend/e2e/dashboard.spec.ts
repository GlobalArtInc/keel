import { expect, keelTest } from '@globalart/keel/testing/playwright';
import type { Locator, Page } from '@playwright/test';

const HOME = '/';
const PREF_SET = '/api/v1/call/settings.pref.set';
const ACCOUNTS_WIDGET = 'keel.widget.accounts';

function cards(page: Page): Locator {
  return page.locator('keel-home phx-widget-card');
}

function order(page: Page): Promise<string[]> {
  return cards(page).evaluateAll((elements) =>
    elements.map((element) => element.getAttribute('data-widget-func') ?? ''),
  );
}

async function centre(locator: Locator): Promise<{ x: number; y: number }> {
  const box = await locator.boundingBox();
  if (box === null) {
    throw new Error('the element to drag is not laid out');
  }
  return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
}

async function drag(page: Page, handle: Locator, target: Locator): Promise<void> {
  const from = await centre(handle);
  const to = await centre(target);
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(from.x + 8, from.y + 8, { steps: 4 });
  await page.mouse.move(to.x, to.y, { steps: 20 });
  await page.mouse.move(to.x + 4, to.y + 4, { steps: 4 });
  await page.mouse.up();
}

keelTest.describe('dashboard', () => {
  keelTest(
    'root drags a widget to a new place and it stays there after a reload',
    async ({ page, keel }) => {
      await keel.asAdmin();
      await page.goto(HOME);
      await expect(page).toHaveURL(/\/$/);
      const stat = page.locator(
        `keel-home phx-widget-card[data-widget-func="${ACCOUNTS_WIDGET}"] phx-stat .value__number`,
      );
      await expect(stat).toHaveText(/^\d+$/);
      const before = await order(page);
      expect(before.length).toBeGreaterThan(1);

      const saved = page.waitForResponse(
        (response) => response.url().endsWith(PREF_SET) && response.request().method() === 'POST',
      );
      await drag(page, cards(page).first().locator('.handle'), cards(page).last());
      expect((await saved).status()).toBe(200);
      const after = await order(page);
      expect(after).not.toEqual(before);
      expect([...after].sort()).toEqual([...before].sort());

      await page.reload();
      await expect(cards(page)).toHaveCount(after.length);
      await expect(stat).toHaveText(/^\d+$/);
      expect(await order(page)).toEqual(after);
    },
  );
});
