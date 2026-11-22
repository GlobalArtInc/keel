import type { Envelope, Id, Page } from '@globalart/keel/meta';
import { E2E_LOGINS, E2E_SCRIPT, expect, keelTest } from '@globalart/keel/testing/playwright';

interface ZoneRow {
  id: Id;
  name: string;
}

const ACCOUNTS = '/f/keel.account.list';
const ZONES = '/f/dns.zone.list';
const CLI_ZONE_VARIABLE = 'KEEL_E2E_DNS_ZONE';
const RECORD = { type: 'A', name: 'beside', value: '192.0.2.20' };

function cliZone(): string {
  const zone = process.env[CLI_ZONE_VARIABLE];
  if (zone === undefined || zone === '') {
    throw new Error(
      `${CLI_ZONE_VARIABLE} is not set; ${E2E_SCRIPT} creates that zone over the CLI`,
    );
  }
  return zone;
}

function zoneId(listed: Envelope<Page<ZoneRow>>, name: string): Id {
  const row = listed.ok ? listed.data.data.find((zone) => zone.name === name) : undefined;
  if (row === undefined) {
    throw new Error(`dns.zone.list does not show the zone ${name} to alice`);
  }
  return row.id;
}

keelTest.describe('layouts', () => {
  keelTest(
    'the account tree opens at the root and expands level by level',
    async ({ page, keel }) => {
      await keel.asAdmin();
      await page.goto(ACCOUNTS);
      const tree = page.locator('keel-tree[data-tree="keel.account.list"]');
      await expect(tree).toHaveAttribute('data-phase', 'loaded');
      const nodes = tree.locator('phx-tree-node');
      await expect(nodes).toHaveText([E2E_LOGINS.admin]);

      await nodes.first().locator('.chevron').click();
      await expect(nodes).toHaveCount(2);
      await nodes.nth(1).locator('.chevron').click();
      await expect(nodes).toHaveCount(3);

      await nodes.nth(2).click();
      const bar = tree.locator('phx-selection-bar');
      await expect(bar.getByRole('button', { name: 'Edit' })).toBeVisible();
    },
  );

  keelTest(
    'a zone chosen beside the zone list shows its records and survives a reload',
    async ({ page, keel, keelApi }) => {
      const name = cliZone();
      const alice = await keelApi.as('user');
      const zone = zoneId(await alice.call<Page<ZoneRow>>('dns.zone.list', { search: name }), name);
      const created = await alice.call('dns.record.create', { zone, ...RECORD });
      await alice.logout();
      expect(created.ok).toBe(true);

      await keel.asUser();
      await page.goto(ZONES);
      const split = page.locator('keel-master-detail[data-master="dns.zone.list"]');
      const row = split
        .locator('keel-list[data-list="dns.zone.list"] .body [role="row"]')
        .filter({ has: page.locator('[data-column="name"]', { hasText: name }) });
      await row.locator('[data-column="serial"]').click();
      await expect(page).toHaveURL(new RegExp(`\\?selected=${zone}$`));

      const records = split.locator('.detail keel-list[data-list="dns.record.list"]');
      const names = records.locator('.body [role="row"] [data-column="name"]');
      await expect(names).toHaveText([RECORD.name]);

      await page.reload();
      await expect(split.locator('.detail')).toHaveAttribute('data-selected', zone);
      await expect(names).toHaveText([RECORD.name]);
    },
  );
});
