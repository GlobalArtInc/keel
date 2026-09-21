import {
  E2E_LOGINS,
  SETUP_PATH,
  expect,
  keelTest,
  onSetupScreen,
  setupLink,
} from '@globalart/keel/testing/playwright';

const TAG = '@setup';

keelTest(
  `the setup link creates the first administrator, signs them in and dies with its use ${TAG}`,
  async ({ page, keel, keelSite }) => {
    const link = setupLink();
    expect(new URL(link).pathname).toBe(SETUP_PATH);
    const admin = keelSite.logins.admin;

    await page.goto(link);
    const form = page.locator('keel-setup form[data-step="setup"]');
    await expect(form).toBeVisible();
    await form.locator('input[autocomplete="username"]').fill(admin.login);
    await form.locator('input[autocomplete="new-password"]').fill('short');
    await form.locator('button[type="submit"]').click();
    await expect(page.locator('keel-setup').getByRole('alert')).toContainText(
      'The password must be at least 12 characters long',
    );

    await form.locator('input[autocomplete="new-password"]').fill(admin.password);
    await form.locator('button[type="submit"]').click();
    await page.waitForURL((url) => !onSetupScreen(url));
    await page.locator('keel-layout').waitFor();
    await page.locator('keel-rail [data-rail="user"] button').click();
    await expect(page.locator('phx-user-menu')).toContainText(E2E_LOGINS.admin);

    await keel.logout();
    await page.goto(link);
    await expect(page.locator('keel-setup form[data-step="setup"]')).toBeVisible();
    await page
      .locator('keel-setup form[data-step="setup"] input[autocomplete="username"]')
      .fill('second');
    await page
      .locator('keel-setup form[data-step="setup"] input[autocomplete="new-password"]')
      .fill(admin.password);
    await page.locator('keel-setup form[data-step="setup"] button[type="submit"]').click();
    await expect(page.locator('keel-setup')).toHaveAttribute('data-stage', 'done');
    await expect(page.locator('keel-setup').getByRole('status')).toContainText(
      'already has an administrator',
    );
    await page.locator('keel-setup a.to-login').click();
    await expect(page.locator('keel-login form[data-step="password"]')).toBeVisible();
  },
);
