import { expect, keelTest } from '@globalart/keel/testing/playwright';
import type { Page } from '@playwright/test';

const HOME = '/';
const PROBLEM_KEY = 'note.overdue';
const RAISED = /Problem note\.overdue raised for/;
const VERSION = /Version \d+\.\d+\.\d+/;

function rail(page: Page, item: string) {
  return page.locator(`keel-rail [data-rail="${item}"]`);
}

keelTest.describe('notifications', { tag: '@polled' }, () => {
  keelTest('root sees the problem on the dashboard and in the centre', async ({ page, keel }) => {
    await keel.asAdmin();
    await page.goto(HOME);
    const card = page.locator('keel-home phx-widget-card[data-widget-func="notify.problem.list"]');
    await expect(card).toBeVisible();
    await expect(card.locator('phx-list-table .body [role="row"]')).toHaveCount(1);
    await expect(card.locator('[role="cell"][data-column="key"]')).toHaveText(PROBLEM_KEY);
    await rail(page, 'notifications').click();
    const centre = page.locator('keel-notification-centre');
    await expect(centre.locator('phx-notification-center .row__main').first()).toContainText(
      RAISED,
    );
    await expect(centre.locator('[data-problems] [data-problem]')).toHaveText(
      new RegExp(PROBLEM_KEY),
    );
  });

  keelTest(
    'the notes item of alice carries the badge and the help item opens the about drawer with the version',
    async ({ page, keel }) => {
      await keel.asUser();
      await expect(
        page.locator('keel-layout phx-sidebar-item[data-func="note.list"] .badge'),
      ).toHaveText('1');
      await rail(page, 'help').click();
      const about = page.locator('keel-about');
      await expect(about).toBeVisible();
      await expect(about.locator('phx-prop-list phx-prop').first()).toContainText(VERSION);
    },
  );
});
