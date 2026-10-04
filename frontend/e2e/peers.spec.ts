import { E2E_SCRIPT, expect, keelTest } from '@globalart/keel/testing/playwright';
import type { Locator, Page } from '@playwright/test';

const TAG = '@peers';
const PEER_URL_VARIABLE = 'KEEL_E2E_PEER_URL';
const PEER_SITE_VARIABLE = 'KEEL_E2E_PEER_SITE';
const PEER_LOGIN_VARIABLE = 'KEEL_E2E_PEER_LOGIN';
const SITE_LIST = '/f/hosting.site.list';
const ZONE_LIST = '/f/dns.zone.list';
const OPEN_ZONE = 'Open in KeelDNS';

function variable(name: string): string {
  const value = process.env[name];
  if (value === undefined || value === '') {
    throw new Error(`${name} is not set; ${E2E_SCRIPT} sets it before the ${TAG} run`);
  }
  return value;
}

function rows(page: Page, list: string): Locator {
  return page.locator(`keel-list[data-list="${list}"] phx-list-table .body [role="row"]`);
}

function rowOf(page: Page, list: string, column: string, text: string): Locator {
  return rows(page, list).filter({
    has: page.locator(`[data-column="${column}"]`, { hasText: text }),
  });
}

keelTest.describe('panel beside a dns neighbour', { tag: TAG }, () => {
  keelTest(
    'Open in KeelDNS follows a one-time key onto the neighbour as the customer of the site',
    async ({ page, keel }) => {
      const neighbour = new URL(variable(PEER_URL_VARIABLE));
      const domain = variable(PEER_SITE_VARIABLE);
      await keel.asUser();
      await page.goto(SITE_LIST);
      const site = rowOf(page, 'hosting.site.list', 'domain', domain);
      await expect(site).toHaveCount(1);
      await site.locator('.cell_actions button').click();
      await page.getByRole('menuitem', { name: OPEN_ZONE, exact: true }).click();

      await page.waitForURL((url) => url.origin === neighbour.origin && url.pathname === ZONE_LIST);
      expect(new URL(page.url()).searchParams.get('search')).toBe(domain);
      await expect(rowOf(page, 'dns.zone.list', 'name', domain)).toHaveCount(1);
      await page.locator('keel-rail [data-rail="user"] button').click();
      await expect(page.locator('phx-user-menu')).toContainText(variable(PEER_LOGIN_VARIABLE));
    },
  );
});
