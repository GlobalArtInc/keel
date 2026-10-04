import type { Envelope } from '@globalart/keel/meta';
import { API_PREFIX, E2E_SCRIPT, expect, keelTest } from '@globalart/keel/testing/playwright';
import type { Locator, Page, Response } from '@playwright/test';

const TAG = '@panel';
const HOSTING_USER_VARIABLE = 'KEEL_E2E_HOSTING_USER';
const TAKEN_ZONE_VARIABLE = 'KEEL_E2E_DNS_ZONE';
const WIZARD_SITE_VARIABLE = 'KEEL_E2E_WIZARD_SITE';
const PLATFORM_BRAND = 'Keel';
const PRODUCTS = [
  { id: 'dns', brand: 'KeelDNS', item: 'Zones' },
  { id: 'panel', brand: 'KeelPanel', item: 'Sites' },
] as const;
const SITE_LIST = '/f/hosting.site.list';
const ZONE_LIST = '/f/dns.zone.list';
const SITE_CREATE = 'hosting.site.create';
const ZONE_TOGGLE = 'Create a DNS zone for the domain';
const OPEN_ZONE = 'Open in KeelDNS';

function variable(name: string): string {
  const value = process.env[name];
  if (value === undefined || value === '') {
    throw new Error(`${name} is not set; ${E2E_SCRIPT} sets it before the ${TAG} run`);
  }
  return value;
}

function field(scope: Locator, label: string): Locator {
  return scope.getByRole('group', { name: label, exact: true });
}

function cells(page: Page, list: string, column: string): Locator {
  return page.locator(
    `keel-list[data-list="${list}"] phx-list-table .body [role="row"] [data-column="${column}"]`,
  );
}

function created(page: Page): Promise<Response> {
  return page.waitForResponse(
    (response) =>
      response.url().endsWith(`${API_PREFIX}/call/${SITE_CREATE}`) &&
      response.request().method() === 'POST',
  );
}

async function openWizard(page: Page): Promise<Locator> {
  await page.goto(SITE_LIST);
  await page.locator('keel-list[data-list="hosting.site.list"] [data-action="create"]').click();
  const wizard = page.locator(`keel-wizard[data-wizard="${SITE_CREATE}"]`);
  await expect(wizard).toHaveAttribute('data-step', 'site');
  return wizard;
}

async function walkWithZone(page: Page, wizard: Locator, domain: string): Promise<void> {
  await field(wizard, 'Hosting user').getByRole('combobox').click();
  await page.getByRole('option', { name: variable(HOSTING_USER_VARIABLE), exact: true }).click();
  await field(wizard, 'Domain').getByRole('textbox').fill(domain);
  await wizard.locator('button.next').click();
  await expect(wizard).toHaveAttribute('data-step', 'php');
  await wizard.locator('button.next').click();
  await expect(wizard).toHaveAttribute('data-step', 'dns');
  await field(wizard, ZONE_TOGGLE).locator('phx-toggle label').click();
  await expect(field(wizard, ZONE_TOGGLE).getByRole('switch')).toBeChecked();
  await wizard.locator('button.next').click();
  await expect(wizard).toHaveAttribute('data-step', 'tls');
}

keelTest.describe('panel beside dns', { tag: TAG }, () => {
  keelTest.describe.configure({ mode: 'serial' });

  keelTest(
    'one login sees both products under the platform brand, each with its own group',
    async ({ page, keel }) => {
      await keel.asUser();
      const layout = page.locator('keel-layout');
      await expect(layout.locator('phx-brand-logo .brand')).toHaveText(PLATFORM_BRAND);
      for (const product of PRODUCTS) {
        const section = layout.locator(`phx-sidebar-group.section[data-product="${product.id}"]`);
        await expect(section).toHaveCount(1);
        await expect(section).toContainText(product.brand);
        await expect(section.getByRole('link', { name: product.item, exact: true })).toBeVisible();
        for (const other of PRODUCTS.filter((entry) => entry.id !== product.id)) {
          await expect(section.getByRole('link', { name: other.item, exact: true })).toHaveCount(0);
        }
      }
    },
  );

  keelTest(
    'the wizard creates a site whose zone dns-local creates in the same account',
    async ({ page, keel }) => {
      const domain = variable(WIZARD_SITE_VARIABLE);
      await keel.asUser();
      const wizard = await openWizard(page);
      await walkWithZone(page, wizard, domain);
      const answered = created(page);
      await wizard.locator('button.next').click();
      const envelope = (await (await answered).json()) as Envelope<unknown>;
      expect(envelope.ok).toBe(true);

      await page.goto(SITE_LIST);
      await expect(
        cells(page, 'hosting.site.list', 'domain').filter({ hasText: domain }),
      ).toHaveCount(1);
      await page.goto(ZONE_LIST);
      await expect(cells(page, 'dns.zone.list', 'name').filter({ hasText: domain })).toHaveCount(1);
    },
  );

  keelTest(
    'Open in KeelDNS on a site lands on the zone list of this installation narrowed to its domain',
    async ({ page, keel }) => {
      const domain = variable(WIZARD_SITE_VARIABLE);
      await keel.asUser();
      await page.goto(SITE_LIST);
      const site = page
        .locator('keel-list[data-list="hosting.site.list"] phx-list-table .body [role="row"]')
        .filter({ has: page.locator('[data-column="domain"]', { hasText: domain }) });
      await expect(site).toHaveCount(1);
      await site.locator('.cell_actions button').click();
      await page.getByRole('menuitem', { name: OPEN_ZONE, exact: true }).click();
      await expect(page).toHaveURL(
        `${new URL(page.url()).origin}${ZONE_LIST}?search=${encodeURIComponent(domain)}`,
      );
      await expect(cells(page, 'dns.zone.list', 'name')).toHaveText([domain]);
    },
  );

  keelTest(
    'a zone dns refuses stops the wizard with the key on the domain field',
    async ({ page, keel }) => {
      const taken = variable(TAKEN_ZONE_VARIABLE);
      await keel.asUser();
      const wizard = await openWizard(page);
      await walkWithZone(page, wizard, taken);
      const answered = created(page);
      await wizard.locator('button.next').click();
      const envelope = (await (await answered).json()) as Envelope<unknown>;
      expect(envelope.ok).toBe(false);

      await expect(wizard).toHaveAttribute('data-step', 'site');
      await expect(field(wizard, 'Domain').locator('phx-field-error')).toHaveText(
        `The zone ${taken} already exists`,
      );
      await page.goto(SITE_LIST);
      const domains = cells(page, 'hosting.site.list', 'domain');
      await expect(domains.filter({ hasText: variable(WIZARD_SITE_VARIABLE) })).toHaveCount(1);
      await expect(domains.filter({ hasText: taken })).toHaveCount(0);
    },
  );
});
