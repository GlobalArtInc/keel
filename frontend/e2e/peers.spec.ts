import type { Envelope } from '@globalart/keel/meta';
import { API_PREFIX, expect, keelTest } from '@globalart/keel/testing/playwright';
import type { Locator, Page, Response } from '@playwright/test';

const TAG = '@peers';
const SCRIPT = 'hack/peers-e2e.sh';
const HOSTING_USER_VARIABLE = 'KEEL_E2E_HOSTING_USER';
const PEER_URL_VARIABLE = 'KEEL_E2E_PEER_URL';
const PEER_SITE_VARIABLE = 'KEEL_E2E_PEER_SITE';
const PEER_TAKEN_VARIABLE = 'KEEL_E2E_PEER_TAKEN';
const PEER_LOGIN_VARIABLE = 'KEEL_E2E_PEER_LOGIN';
const SITE_LIST = '/f/hosting.site.list';
const ZONE_LIST = '/f/dns.zone.list';
const SITE_CREATE = 'hosting.site.create';
const ZONE_TOGGLE = 'Create a DNS zone for the domain';
const OPEN_ZONE = 'Open in KeelDNS';

function variable(name: string): string {
  const value = process.env[name];
  if (value === undefined || value === '') {
    throw new Error(`${name} is not set; ${SCRIPT} sets it before the ${TAG} run`);
  }
  return value;
}

function field(scope: Locator, label: string): Locator {
  return scope.getByRole('group', { name: label, exact: true });
}

function rows(page: Page, list: string): Locator {
  return page.locator(`keel-list[data-list="${list}"] phx-list-table .body [role="row"]`);
}

function rowOf(page: Page, list: string, column: string, text: string): Locator {
  return rows(page, list).filter({
    has: page.locator(`[data-column="${column}"]`, { hasText: text }),
  });
}

function created(page: Page): Promise<Response> {
  return page.waitForResponse(
    (response) =>
      response.url().endsWith(`${API_PREFIX}/call/${SITE_CREATE}`) &&
      response.request().method() === 'POST',
  );
}

keelTest.describe('panel beside a dns neighbour', { tag: TAG }, () => {
  keelTest(
    'a zone the neighbour refuses stops the wizard with the key of dns-peer on the domain field',
    async ({ page, keel }) => {
      const taken = variable(PEER_TAKEN_VARIABLE);
      await keel.asUser();
      await page.goto(SITE_LIST);
      await page.locator('keel-list[data-list="hosting.site.list"] [data-action="create"]').click();
      const wizard = page.locator(`keel-wizard[data-wizard="${SITE_CREATE}"]`);
      await expect(wizard).toHaveAttribute('data-step', 'site');
      await field(wizard, 'Hosting user').getByRole('combobox').click();
      await page
        .getByRole('option', { name: variable(HOSTING_USER_VARIABLE), exact: true })
        .click();
      await field(wizard, 'Domain').getByRole('textbox').fill(taken);
      await wizard.locator('button.next').click();
      await expect(wizard).toHaveAttribute('data-step', 'php');
      await wizard.locator('button.next').click();
      await expect(wizard).toHaveAttribute('data-step', 'dns');
      await field(wizard, ZONE_TOGGLE).locator('phx-toggle label').click();
      await expect(field(wizard, ZONE_TOGGLE).getByRole('switch')).toBeChecked();
      await wizard.locator('button.next').click();
      await expect(wizard).toHaveAttribute('data-step', 'tls');
      const answered = created(page);
      await wizard.locator('button.next').click();
      const envelope = (await (await answered).json()) as Envelope<unknown>;
      expect(envelope).toMatchObject({
        ok: false,
        error: {
          code: 'peer',
          fields: [{ path: 'domain', key: 'dns_peer.error.exists' }],
        },
      });

      await expect(wizard).toHaveAttribute('data-step', 'site');
      await expect(field(wizard, 'Domain').locator('phx-field-error')).toHaveText(
        `The zone ${taken} already exists on the neighbouring installation`,
      );
      await page.goto(SITE_LIST);
      await expect(
        rowOf(page, 'hosting.site.list', 'domain', variable(PEER_SITE_VARIABLE)),
      ).toHaveCount(1);
      await expect(rowOf(page, 'hosting.site.list', 'domain', taken)).toHaveCount(0);
    },
  );

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
