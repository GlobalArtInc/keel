import type { Envelope, Id } from '@globalart/keel/meta';
import { API_PREFIX, E2E_SCRIPT, expect, keelTest } from '@globalart/keel/testing/playwright';
import type { Locator, Page } from '@playwright/test';

interface ZoneRow {
  id: Id;
  name: string;
}

interface Issued {
  id: Id;
  token: string;
}

interface Revoked {
  revoked: boolean;
}

const ZONE_LIST = '/f/dns.zone.list';
const RECORD_LIST = '/f/dns.record.list';
const PARENT = 'zone_id';
const CLI_ZONE_VARIABLE = 'KEEL_E2E_DNS_ZONE';
const BRAND = 'KeelDNS';
const BROWSER_ZONE = 'browser.e2e.test';
const TOKEN_ZONE = 'token.e2e.test';
const TOKEN_NAME = 'dns over the API';
const TOKEN_PREFIX = 'kat_';
const RECORD = { type: 'A, IPv4 address', name: 'www', value: '192.0.2.10' };

function cliZone(): string {
  const zone = process.env[CLI_ZONE_VARIABLE];
  if (zone === undefined || zone === '') {
    throw new Error(
      `${CLI_ZONE_VARIABLE} is not set; ${E2E_SCRIPT} creates that zone over the CLI`,
    );
  }
  return zone;
}

function field(scope: Locator | Page, label: string): Locator {
  return scope.getByRole('group', { name: label, exact: true });
}

function rows(page: Page): Locator {
  return page.locator('phx-list-table .body [role="row"]');
}

function names(page: Page): Locator {
  return rows(page).locator('[data-column="name"]');
}

async function saved(page: Page, func: string, save: () => Promise<void>): Promise<void> {
  const answered = page.waitForResponse(
    (response) =>
      response.url().endsWith(`${API_PREFIX}/call/${func}`) &&
      response.request().method() === 'POST',
  );
  await save();
  expect((await answered).ok()).toBe(true);
}

async function openRecords(page: Page, zone: string): Promise<string> {
  await page.goto(ZONE_LIST);
  const row = rows(page).filter({ has: page.locator('[data-column="name"]', { hasText: zone }) });
  await expect(row).toHaveCount(1);
  await row.locator('.cell_actions button').click();
  await page.getByRole('menuitem', { name: 'Records' }).click();
  await expect(page).toHaveURL(new RegExp(`${RECORD_LIST.replace(/\./g, '\\.')}\\?${PARENT}=`));
  const id = new URL(page.url()).searchParams.get(PARENT);
  if (id === null) {
    throw new Error(`the Records action opened ${page.url()} without ${PARENT}`);
  }
  return id;
}

keelTest.describe('dns', () => {
  keelTest.describe.configure({ mode: 'serial' });

  keelTest(
    'alice creates a zone and its record through the list and the form',
    async ({ page, keel }) => {
      await keel.asUser();
      const sidebar = page.locator('phx-sidebar');
      await expect(sidebar).toContainText(BRAND);
      await sidebar.getByRole('link', { name: 'Zones' }).click();
      await expect(page).toHaveURL(new RegExp(`${ZONE_LIST.replace(/\./g, '\\.')}$`));
      const zones = page.locator('keel-list[data-list="dns.zone.list"]');
      await zones.locator('[data-action="create"]').click();

      const form = page.locator('keel-form[data-form="dns.zone.create"][data-mode="create"]');
      await expect(form).toBeVisible();
      await expect(page.locator('keel-page[data-page="owner"]')).toHaveCount(0);
      await expect(field(form, 'IPv4 address the template points to')).toHaveCount(0);
      await field(form, 'Name').getByRole('textbox').fill(BROWSER_ZONE);
      await saved(page, 'dns.zone.create', () => form.locator('button.save').click());

      const zone = await openRecords(page, BROWSER_ZONE);
      const records = page.locator('keel-list[data-list="dns.record.list"]');
      await expect(
        records.getByRole('heading', { name: 'No records in this zone yet' }),
      ).toBeVisible();
      await records.locator('[data-action="create"]').click();

      const record = page.locator('keel-form[data-form="dns.record.create"][data-mode="create"]');
      await expect(record).toBeVisible();
      await field(record, 'Zone').getByRole('textbox').fill(zone);
      await field(record, 'Type').getByRole('combobox').click();
      await page.getByRole('option', { name: RECORD.type, exact: true }).click();
      await expect(field(record, 'Priority')).toHaveCount(0);
      await field(record, 'Name, @ for the zone itself').getByRole('textbox').fill(RECORD.name);
      await field(record, 'Value').getByRole('textbox').fill(RECORD.value);
      await saved(page, 'dns.record.create', () => record.locator('button.save').click());

      await page.goto(`${RECORD_LIST}?${PARENT}=${zone}`);
      await expect(names(page)).toHaveText([RECORD.name]);
      await expect(rows(page).first().locator('[data-column="value"]')).toHaveText(RECORD.value);
    },
  );

  keelTest('alice creates a zone with an API token', async ({ keelApi, keelSite }) => {
    const alice = await keelApi.as('user');
    const issued = await alice.call<Issued>('auth.token.create', { name: TOKEN_NAME });
    if (!issued.ok) {
      await alice.logout();
      throw new Error(`auth.token.create refused alice: ${issued.error.key}`);
    }
    expect(issued.data.token.startsWith(TOKEN_PREFIX)).toBe(true);

    const response = await fetch(`${keelSite.baseURL}${API_PREFIX}/call/dns.zone.create`, {
      method: 'POST',
      headers: {
        authorization: `Bearer ${issued.data.token}`,
        'content-type': 'application/json',
      },
      body: JSON.stringify({ name: TOKEN_ZONE }),
    });
    const created = (await response.json()) as Envelope<ZoneRow>;
    const revoked = await alice.call<Revoked>('auth.token.revoke', { id: issued.data.id });
    await alice.logout();
    if (!created.ok) {
      throw new Error(`dns.zone.create refused the token: ${created.error.key}`);
    }
    expect(created.data.name).toBe(TOKEN_ZONE);
    expect(revoked.ok && revoked.data.revoked).toBe(true);
  });

  keelTest(
    'the zones of the browser, the CLI and the token land in one list',
    async ({ page, keel }) => {
      await keel.asUser();
      await page.goto(ZONE_LIST);
      await expect(names(page)).toHaveText([BROWSER_ZONE, cliZone(), TOKEN_ZONE].sort());
    },
  );
});
