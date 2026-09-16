import { E2E_LOGINS, expect, keelTest } from '@globalart/keel/testing/playwright';
import { URI, type TOTP } from 'otpauth';

const CREDENTIALS_MESSAGE = 'The login or the password is wrong';
const NOTE_LIST = '/f/note.list';
const SECURITY = '/f/auth.totp.enroll';

const STEP_MARGIN_MS = 5_000;

class Codes {
  private last: number | null = null;

  constructor(private readonly totp: TOTP) {}

  async next(): Promise<string> {
    const period = this.totp.period * 1000;
    const left = period - (Date.now() % period);
    if (left < STEP_MARGIN_MS) {
      await new Promise((done) => setTimeout(done, left));
    }
    const now = Math.floor(Date.now() / period);
    const step = this.last === null ? now - 1 : Math.max(this.last + 1, now - 1);
    this.last = step;
    return this.totp.generate({ timestamp: step * period });
  }
}

keelTest.describe('login', () => {
  keelTest('a wrong password is refused with auth.error.credentials', async ({ page, keel }) => {
    await page.goto('/login');
    const form = page.locator('keel-login form[data-step="password"]');
    await form.locator('input[autocomplete="username"]').fill(E2E_LOGINS.user);
    await form.locator('input[autocomplete="current-password"]').fill('not-the-password');
    await form.locator('button[type="submit"]').click();
    await expect(form.getByRole('alert')).toContainText(CREDENTIALS_MESSAGE);
    await expect(page).toHaveURL(/\/login$/);
    await keel.asUser();
  });

  keelTest('alice logs in and out', async ({ page, keel }) => {
    await keel.asUser();
    await expect(page).toHaveURL(new RegExp(`${NOTE_LIST}$`));
    await page.locator('keel-rail [data-rail="user"] button').click();
    const menu = page.locator('phx-user-menu');
    await expect(menu).toContainText(E2E_LOGINS.user);
    await menu.getByRole('menuitem', { name: 'Log out' }).click();
    await expect(page).toHaveURL(/\/login$/);
    await expect(page.locator('keel-login form[data-step="password"]')).toBeVisible();
    await page.goto(NOTE_LIST);
    await expect(page).toHaveURL(/\/login\?returnTo=/);
  });

  keelTest('the reseller fixture signs res1 in', async ({ page, keel }) => {
    await keel.asReseller();
    await page.locator('keel-rail [data-rail="user"] button').click();
    const menu = page.locator('phx-user-menu');
    await expect(menu).toContainText(E2E_LOGINS.reseller);
    await expect(menu).toContainText('Reseller');
  });

  keelTest('a guest opening a screen returns there after the login', async ({ page, keelSite }) => {
    await page.goto(NOTE_LIST);
    await expect(page).toHaveURL(`/login?returnTo=${encodeURIComponent(NOTE_LIST)}`);
    const form = page.locator('keel-login form[data-step="password"]');
    await form.locator('input[autocomplete="username"]').fill(E2E_LOGINS.user);
    await form
      .locator('input[autocomplete="current-password"]')
      .fill(keelSite.logins.user.password);
    await form.locator('button[type="submit"]').click();
    await expect(page).toHaveURL(new RegExp(`${NOTE_LIST}$`));
    await expect(page.locator('keel-list[data-list="note.list"]')).toBeVisible();
  });

  keelTest(
    'root enrols the second factor, logs in through the code and disables it',
    async ({ page, keel, keelSite }) => {
      await keel.asAdmin();
      await page.goto(SECURITY);
      const totp = page.locator('keel-security [data-section="totp"]');
      await expect(totp).toHaveAttribute('data-enrolled', 'false');
      await totp.locator('button.enroll').click();

      const enroll = page.locator('keel-totp-enroll');
      const uri = await enroll.locator('[data-qr]').getAttribute('aria-label');
      const shownSecret = (await enroll.locator('[data-secret]').textContent()) ?? '';
      const parsed = URI.parse(uri ?? '');
      expect(parsed.constructor.name).toBe('TOTP');
      const codes = new Codes(parsed as TOTP);
      expect(shownSecret.replace(/\s+/g, '')).toContain(parsed.secret.base32);

      await enroll.locator('input[autocomplete="one-time-code"]').fill(await codes.next());
      await enroll.locator('button.confirm').click();
      await expect(enroll).toHaveAttribute('data-stage', 'recovery');
      await expect(enroll.locator('[data-recovery] li')).toHaveCount(10);
      await enroll.locator('button.finish').click();
      await expect(totp).toHaveAttribute('data-enrolled', 'true');

      await keel.logout();
      const password = page.locator('keel-login form[data-step="password"]');
      await password.locator('input[autocomplete="username"]').fill(keelSite.logins.admin.login);
      await password
        .locator('input[autocomplete="current-password"]')
        .fill(keelSite.logins.admin.password);
      await password.locator('button[type="submit"]').click();
      const code = page.locator('keel-login form[data-step="code"]');
      await code.locator('input[autocomplete="one-time-code"]').fill(await codes.next());
      await code.locator('button[type="submit"]').click();
      await expect(page.locator('keel-layout')).toBeVisible();

      await page.goto(SECURITY);
      await expect(totp).toHaveAttribute('data-enrolled', 'true');
      await totp.locator('button.disable').click();
      const disable = page.locator('keel-totp-disable');
      await disable.locator('input[autocomplete="one-time-code"]').fill(await codes.next());
      await disable.locator('button.disable').click();
      await expect(totp).toHaveAttribute('data-enrolled', 'false');
    },
  );
});
