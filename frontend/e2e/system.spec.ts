import { E2E_LOGINS, expect, keelTest } from '@globalart/keel/testing/playwright';
import type { Locator, Page } from '@playwright/test';

interface SystemScreen {
  label: string;
  func: string;
}

const SCREENS: readonly SystemScreen[] = [
  { label: 'Accounts', func: 'keel.account.list' },
  { label: 'Principals', func: 'keel.principal.list' },
  { label: 'Products', func: 'keel.product.list' },
  { label: 'Settings', func: 'settings.list' },
  { label: 'Tasks', func: 'task.list' },
  { label: 'Scheduled jobs', func: 'cron.job.list' },
  { label: 'Webhooks', func: 'webhook.subscription.list' },
  { label: 'Audit trail', func: 'audit.list' },
  { label: 'Sessions', func: 'auth.session.list' },
  { label: 'API tokens', func: 'auth.token.list' },
  { label: 'Permission grants', func: 'auth.grant.list' },
];

const MIN_LENGTH_KEY = 'auth.password.min_length';
const MIN_LENGTH = '10';
const TOKEN_NAME = 'browser suite';
const TOKEN_PREFIX = 'kat_';
const PRINCIPALS = 3;

function field(scope: Locator | Page, label: string): Locator {
  return scope.getByRole('group', { name: label, exact: true });
}

function rows(page: Page): Locator {
  return page.locator('phx-list-table .body [role="row"]');
}

async function search(page: Page, text: string): Promise<void> {
  const input = page.locator('keel-list input[type="search"]');
  await input.fill(text);
  await input.press('Enter');
}

keelTest.describe('system', () => {
  keelTest('root opens every system screen from the menu', async ({ page, keel }) => {
    await keel.asAdmin();
    for (const screen of SCREENS) {
      await page.locator('phx-sidebar').getByRole('link', { name: screen.label }).click();
      await expect(page).toHaveURL(new RegExp(`/f/${screen.func.replaceAll('.', '\\.')}$`));
      const list = page.locator(`keel-list[data-list="${screen.func}"]`);
      await expect(list.getByRole('heading', { level: 1 })).toHaveText(screen.label);
      await expect(list.getByText(/^Total: \d+$/)).toBeVisible();
    }
  });

  keelTest('root sets auth.password.min_length through the value form', async ({ page, keel }) => {
    await keel.asAdmin();
    await page.goto('/f/settings.list');
    await search(page, MIN_LENGTH_KEY);
    await expect(rows(page)).toHaveCount(1);
    await rows(page).first().locator('.cell_actions button').click();
    await page.getByRole('menuitem', { name: 'Change' }).click();
    await expect(page).toHaveURL(new RegExp(`/f/settings\\.set\\?key=${MIN_LENGTH_KEY}$`));

    const form = page.locator(`keel-settings-value[data-setting="${MIN_LENGTH_KEY}"]`);
    await expect(form).toHaveAttribute('data-phase', 'ready');
    await expect(field(form, 'Key').getByRole('textbox')).toHaveValue(MIN_LENGTH_KEY);
    await expect(form.locator('[data-default]')).toHaveText('12');
    const value = field(form, 'Value').getByRole('textbox');
    await value.fill(MIN_LENGTH);
    await form.locator('button.save').click();

    await expect(page).toHaveURL(/\/f\/settings\.list/);
    await expect(page.locator('phx-toast')).toContainText(`Setting ${MIN_LENGTH_KEY} completed`);
    await search(page, MIN_LENGTH_KEY);
    await expect(rows(page)).toHaveCount(1);
    await expect(rows(page).first().locator('[data-column="value"]')).toHaveText(MIN_LENGTH);
  });

  keelTest('root creates a token whose plaintext is shown once', async ({ page, keel }) => {
    await keel.asAdmin();
    await page.goto('/f/auth.token.list');
    await page.locator('keel-list [data-action="create"]').click();
    await expect(page).toHaveURL(/\/f\/auth\.token\.create$/);
    const form = page.locator('keel-form[data-form="auth.token.create"]');
    await field(form, 'Name').getByRole('textbox').fill(TOKEN_NAME);
    await form.locator('button.save').click();

    const once = form.locator('keel-once');
    await expect(once.getByRole('alert')).toContainText('Shown only once');
    await expect(once).toContainText(TOKEN_PREFIX);
    const plaintext = (await once.textContent()) ?? '';
    const token = new RegExp(`${TOKEN_PREFIX}[0-9a-f]+`).exec(plaintext)?.[0];
    expect(token).toBeDefined();

    await page.goto('/f/auth.token.list');
    await expect(rows(page)).toHaveCount(1);
    await expect(rows(page).first().locator('[data-column="name"]')).toHaveText(TOKEN_NAME);
    await expect(page.locator('keel-list')).not.toContainText(token ?? TOKEN_PREFIX);
  });

  keelTest('root acts as alice, sees the narrowed list and returns', async ({ page, keel }) => {
    await keel.asAdmin();
    await page.goto('/f/keel.principal.list');
    await expect(rows(page)).toHaveCount(PRINCIPALS);
    await expect(page.locator('[data-acting-as]')).toHaveCount(0);

    await page.locator('keel-rail [data-rail="user"] button').click();
    await page.getByRole('menuitem', { name: 'Act as' }).click();
    const modal = page.locator('keel-act-as');
    const box = modal.locator('input[type="search"]');
    await box.fill(E2E_LOGINS.user);
    await box.press('Enter');
    await expect(modal).toHaveAttribute('data-state', 'found');
    await modal.locator('button.row', { hasText: E2E_LOGINS.user }).click();

    const banner = page.locator('[data-acting-as]');
    await expect(banner).toContainText(`You are acting as ${E2E_LOGINS.user}`);
    await page.locator('phx-sidebar').getByRole('link', { name: 'Principals' }).click();
    await expect(rows(page)).toHaveCount(1);
    await expect(rows(page).first().locator('[data-column="login"]')).toHaveText(E2E_LOGINS.user);

    await banner.locator('button.exit').click();
    await expect(page.locator('[data-acting-as]')).toHaveCount(0);
    await page.goto('/f/keel.principal.list');
    await expect(rows(page)).toHaveCount(PRINCIPALS);
  });
});
