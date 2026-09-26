import { LOGIN_PATH, expect, keelTest } from '@globalart/keel/testing/playwright';

const TAG = '@mail';
const SMTP_API_VARIABLE = 'KEEL_E2E_SMTP_API';
const LOGIN_VARIABLE = 'KEEL_E2E_RESET_LOGIN';
const PASSWORD_VARIABLE = 'KEEL_E2E_RESET_PASSWORD';
const ADDRESS_VARIABLE = 'KEEL_E2E_RESET_ADDRESS';
const RENEWED = 'a-renewed-password-from-the-mail';
const PATIENCE_MS = 30_000;
const LINK = /https?:\/\/\S+\/reset\?token=kpr_[0-9a-f]{64}/;

interface Listed {
  messages?: { ID: string }[];
}

interface Message {
  Text?: string;
}

function required(variable: string): string {
  const value = process.env[variable];
  if (value === undefined || value === '') {
    throw new Error(`${variable} is not set; hack/e2e.sh exports it for the ${TAG} run`);
  }
  return value;
}

async function mailIds(api: string, address: string): Promise<string[]> {
  const query = encodeURIComponent(`to:${address}`);
  const response = await fetch(`${api}/api/v1/search?query=${query}&limit=50`);
  expect(response.ok).toBe(true);
  const listed = (await response.json()) as Listed;
  return (listed.messages ?? []).map((message) => message.ID);
}

async function linkIn(api: string, id: string): Promise<string | null> {
  const response = await fetch(`${api}/api/v1/message/${id}`);
  expect(response.ok).toBe(true);
  const message = (await response.json()) as Message;
  return LINK.exec(message.Text ?? '')?.[0] ?? null;
}

async function newLink(api: string, address: string, seen: readonly string[]): Promise<string> {
  const started = Date.now();
  while (Date.now() - started < PATIENCE_MS) {
    for (const id of await mailIds(api, address)) {
      if (!seen.includes(id)) {
        const link = await linkIn(api, id);
        if (link !== null) {
          return link;
        }
      }
    }
    await new Promise((done) => setTimeout(done, 250));
  }
  throw new Error(`no reset mail reached ${address} within ${PATIENCE_MS} ms`);
}

keelTest(
  `a forgotten password is reset through the link the mail catcher received ${TAG}`,
  async ({ page, keel }) => {
    const api = required(SMTP_API_VARIABLE).replace(/\/+$/, '');
    const login = required(LOGIN_VARIABLE);
    const password = required(PASSWORD_VARIABLE);
    const address = required(ADDRESS_VARIABLE);
    const seen = await mailIds(api, address);

    await page.goto(LOGIN_PATH);
    await page.locator('keel-login a.forgot').click();
    const request = page.locator('keel-reset form[data-step="request"]');
    await expect(request).toBeVisible();
    await request.locator('input[autocomplete="username"]').fill(login);
    await request.locator('button[type="submit"]').click();
    await expect(page.locator('keel-reset')).toHaveAttribute('data-stage', 'sent');

    const link = await newLink(api, address, seen);
    await page.goto(link);
    const confirm = page.locator('keel-reset form[data-step="confirm"]');
    await expect(confirm).toBeVisible();
    await confirm.locator('input[autocomplete="new-password"]').fill(RENEWED);
    await confirm.locator('button[type="submit"]').click();
    await expect(page.locator('keel-reset')).toHaveAttribute('data-stage', 'done');
    await page.locator('keel-reset a.to-login').click();
    await expect(page.locator('keel-login form[data-step="password"]')).toBeVisible();

    const form = page.locator('keel-login form[data-step="password"]');
    await form.locator('input[autocomplete="username"]').fill(login);
    await form.locator('input[autocomplete="current-password"]').fill(password);
    await form.locator('button[type="submit"]').click();
    await expect(page.locator('keel-login').getByRole('alert')).toBeVisible();

    await keel.login(login, RENEWED);
    await expect(page.locator('keel-layout')).toBeVisible();

    await page.goto(link);
    await page
      .locator('keel-reset form[data-step="confirm"] input[autocomplete="new-password"]')
      .fill('yet-another-password-too');
    await page.locator('keel-reset form[data-step="confirm"] button[type="submit"]').click();
    await expect(page.locator('keel-reset').getByRole('alert')).toContainText(
      'The reset link is not valid any more',
    );
  },
);
