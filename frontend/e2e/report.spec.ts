import { expect, keelTest } from '@globalart/keel/testing/playwright';

const REPORT = '/f/dns.report.zones';

keelTest.describe('report', () => {
  keelTest('the dns zones report opens from the menu and shows a chart', async ({ page, keel }) => {
    await keel.asUser();
    await page
      .locator('phx-sidebar')
      .getByRole('link', { name: 'Zone report', exact: true })
      .click();
    await expect(page).toHaveURL(new RegExp(`${REPORT.replace(/\./g, '\\.')}$`));

    const report = page.locator('keel-report[data-report="dns.report.zones"]');
    await expect(report).toHaveAttribute('data-phase', 'loaded');
    await expect(report.getByRole('heading', { name: 'Zone report' })).toBeVisible();
    const band = report.locator('[data-band="accounts"]');
    await expect(band.locator('phx-list-table .body [role="row"]')).toHaveCount(1);
    await expect(band.locator('[data-total="zones"]')).toContainText(
      'Zones at the end of the period',
    );

    const chart = report.locator('[data-chart="zones"]');
    await expect(chart).toHaveAttribute('data-phase', 'loaded');
    await expect(chart.locator('phx-chart')).toBeVisible();
    await expect(chart.getByRole('heading', { name: 'Zones over time' })).toBeVisible();

    await report.locator('[data-action="apply"]').click();
    await expect(report).toHaveAttribute('data-phase', 'loaded');
    await expect(band.locator('phx-list-table .body [role="row"]')).toHaveCount(1);
  });
});
