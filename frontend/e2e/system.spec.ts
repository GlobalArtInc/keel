import { E2E_LOGINS, expect, keelTest } from '@globalart/keel/testing/playwright';
import type { Locator, Page } from '@playwright/test';
import {
  SYSTEM_SCREENS,
  openSystemScreen,
  screenView,
  sectionTabs,
  sidebarLink,
  systemScreen,
} from './system-menu';

const MIN_LENGTH_KEY = 'auth.password.min_length';
const BRAND_HOST = 'panel.example';
const BRAND_LIST = '/f/settings.brand.list';
const PLAIN_LOGO =
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><rect width="8" height="8" fill="#315fcf"/></svg>';
const SCRIPTED_LOGO = '<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>';
const MIN_LENGTH = '10';
const TOKEN_NAME = 'browser suite';
const TOKEN_PREFIX = 'kat_';
const PRINCIPALS = 3;
const MAX_SIDEBAR_ITEMS = 14;

function field(scope: Locator | Page, label: string): Locator {
  return scope.getByRole('group', { name: label, exact: true });
}

function rows(page: Page): Locator {
  return page.locator('phx-list-table .body [role="row"]');
}

function sidebarItems(page: Page): Locator {
  return page.locator('phx-sidebar phx-sidebar-item');
}

function workspaceTabs(page: Page): Locator {
  return page.locator('phx-tab-bar [role="tab"]');
}

async function saved(page: Page, func: string, save: () => Promise<void>): Promise<void> {
  const answered = page.waitForResponse(
    (response) =>
      response.url().endsWith(`/api/v1/call/${func}`) && response.request().method() === 'POST',
  );
  await save();
  expect((await answered).ok()).toBe(true);
}

async function search(page: Page, text: string): Promise<void> {
  const input = page.locator('keel-list input[type="search"]');
  await input.fill(text);
  await input.press('Enter');
}

keelTest.describe('system', () => {
  keelTest('root opens every system screen from the menu', async ({ page, keel }) => {
    await keel.asAdmin();
    for (const screen of SYSTEM_SCREENS) {
      await openSystemScreen(page, screen);
      if (screen.tab !== undefined) {
        await expect(
          sectionTabs(page).getByRole('tab', { name: screen.tab, exact: true }),
        ).toHaveAttribute('aria-selected', 'true');
      }
      const view = screenView(page, screen);
      await expect(view.getByRole('heading', { level: 1 })).toHaveText(screen.title);
      if (screen.view === 'tree') {
        await expect(view.getByRole('tree', { name: screen.title })).toBeVisible();
      }
      if (screen.view === 'list') {
        await expect(view.getByText(/^Total: \d+$/)).toBeVisible();
      }
    }
  });

  keelTest(
    'the sections keep the sidebar within fourteen items, the users section shows its strip of tabs and a tab opens in the same workspace tab',
    async ({ page, keel }) => {
      await keel.asAdmin();
      await expect(sidebarItems(page).first()).toBeVisible();
      expect(await sidebarItems(page).count()).toBeLessThanOrEqual(MAX_SIDEBAR_ITEMS);

      const users = systemScreen('keel.principal.list');
      await openSystemScreen(page, users);
      await expect(sectionTabs(page)).toBeVisible();
      const opened = await workspaceTabs(page).count();
      const current = page.locator('phx-tab-bar [role="tab"].tab_active');
      const currentId = await current.getAttribute('id');
      expect(currentId).not.toBeNull();

      await sectionTabs(page).getByRole('tab', { name: 'Sessions', exact: true }).click();
      await expect(page).toHaveURL(/\/f\/auth\.session\.list$/);
      await expect(workspaceTabs(page)).toHaveCount(opened);
      await expect(current).toHaveAttribute('id', currentId ?? '');

      await page.goto('/f/auth.session.list');
      await expect(page.locator('phx-sidebar [aria-current="page"]')).toHaveCount(1);
      await expect(sidebarLink(page, users)).toHaveAttribute('aria-current', 'page');
      await expect(
        sectionTabs(page).getByRole('tab', { name: 'Sessions', exact: true }),
      ).toHaveAttribute('aria-selected', 'true');
    },
  );

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

  keelTest(
    'the white-label screen says white-label by host name, hue and chroma say from the product when empty, an uploaded svg logo shows after a reload and a scripted one is refused on the field',
    async ({ page, keel }) => {
      await keel.asAdmin();
      await openSystemScreen(page, systemScreen('settings.brand.list'));
      const list = page.locator('keel-list[data-list="settings.brand.list"]');
      await expect(list.getByRole('heading', { level: 1 })).toHaveText('White-label by host name');
      await list.locator('[data-action="create"]').click();

      const form = page.locator('keel-form[data-form="settings.brand.set"]');
      await expect(form).toBeVisible();
      await expect(form.getByRole('heading', { level: 1 })).toHaveText(
        'White-label of a host name',
      );
      await expect(field(form, 'Hue (0 to 359)').getByRole('textbox')).toHaveValue(
        'From the product',
      );
      await expect(field(form, 'Chroma (0 to 0.4)').getByRole('textbox')).toHaveValue(
        'From the product',
      );
      await field(form, 'Host name').getByRole('textbox').fill(BRAND_HOST);
      await saved(page, 'settings.brand.set', () => form.locator('button.save').click());
      await page.goto(BRAND_LIST);
      await search(page, BRAND_HOST);
      await expect(rows(page)).toHaveCount(1);
      await expect(rows(page).first().locator('[data-column="logo"] phx-icon')).toHaveAttribute(
        'aria-label',
        'No',
      );

      await rows(page).first().locator('.cell_actions button').click();
      await page.getByRole('menuitem', { name: 'Logo' }).click();
      await expect(page).toHaveURL(
        new RegExp(`/f/settings\\.brand\\.logo\\?host=${BRAND_HOST.replaceAll('.', '\\.')}$`),
      );
      const upload = page.locator('keel-form[data-form="settings.brand.logo"][data-mode="edit"]');
      await expect(upload).toBeVisible();
      await expect(field(upload, 'Host name').getByRole('textbox')).toHaveValue(BRAND_HOST);
      await expect(field(upload, 'Host name').getByRole('textbox')).toBeDisabled();
      const picker = upload.locator('input[type="file"]');
      await expect(picker).toHaveAttribute('accept', 'image/svg+xml');
      await picker.setInputFiles({
        name: 'scripted.svg',
        mimeType: 'image/svg+xml',
        buffer: Buffer.from(SCRIPTED_LOGO),
      });
      await upload.locator('button.save').click();
      await expect(field(upload, 'Logo (SVG)').locator('phx-field-error')).toContainText('script');

      await picker.setInputFiles({
        name: 'logo.svg',
        mimeType: 'image/svg+xml',
        buffer: Buffer.from(PLAIN_LOGO),
      });
      const answered = page.waitForResponse(
        (response) =>
          response.url().endsWith('/api/v1/x/settings/brand.logo') &&
          response.request().method() === 'POST',
      );
      await upload.locator('button.save').click();
      expect((await answered).ok()).toBe(true);
      await page.goto(BRAND_LIST);
      await page.reload();
      await search(page, BRAND_HOST);
      await expect(rows(page)).toHaveCount(1);
      await expect(rows(page).first().locator('[data-column="logo"] phx-icon')).toHaveAttribute(
        'aria-label',
        'Yes',
      );
    },
  );

  keelTest('root creates a token whose plaintext is shown once', async ({ page, keel }) => {
    await keel.asAdmin();
    await page.goto('/f/auth.token.list');
    await page.locator('keel-list phx-toolbar').getByRole('button', { name: 'More' }).click();
    await page.getByRole('menuitem', { name: 'New token', exact: true }).click();
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
    await search(page, TOKEN_NAME);
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
    await openSystemScreen(page, systemScreen('keel.principal.list'));
    await expect(rows(page)).toHaveCount(1);
    await expect(rows(page).first().locator('[data-column="login"]')).toHaveText(E2E_LOGINS.user);

    await banner.locator('button.exit').click();
    await expect(page.locator('[data-acting-as]')).toHaveCount(0);
    await page.goto('/f/keel.principal.list');
    await expect(rows(page)).toHaveCount(PRINCIPALS);
  });
});
