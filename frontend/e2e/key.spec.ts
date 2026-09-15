import type { Id, Page } from '@globalart/keel/meta';
import { E2E_LOGINS, expect, keelTest } from '@globalart/keel/testing/playwright';

interface PrincipalRow {
  id: Id;
  login: string;
}

interface Issued {
  key: string;
  url: string;
}

const REDIRECT = '/f/note.tag.list';

keelTest(
  'root issues a login key for alice and the key lands her on the redirect',
  async ({ page, keelApi }) => {
    const root = await keelApi.as('admin');
    const principals = await root.call<Page<PrincipalRow>>('keel.principal.list', {
      page: 1,
      pageSize: 50,
      search: E2E_LOGINS.user,
    });
    if (!principals.ok) {
      throw new Error(`keel.principal.list refused root: ${principals.error.key}`);
    }
    const alice = principals.data.data.find((row) => row.login === E2E_LOGINS.user);
    expect(alice).toBeDefined();
    const issued = await root.call<Issued>('auth.key.issue', {
      principal: alice?.id,
      ttl: 60,
      redirect: REDIRECT,
    });
    await root.logout();
    if (!issued.ok) {
      throw new Error(`auth.key.issue refused root: ${issued.error.key}`);
    }
    expect(issued.data.url).toBe(`/login?key=${issued.data.key}`);

    await page.goto(issued.data.url);
    await expect(page).toHaveURL(new RegExp(`${REDIRECT.replace('.', '\\.')}$`));
    await expect(page.locator('demo-note-tag-cloud')).toBeVisible();
    await page.locator('keel-rail [data-rail="user"] button').click();
    await expect(page.locator('phx-user-menu')).toContainText(E2E_LOGINS.user);

    await page.goto(issued.data.url);
    await expect(page.locator('keel-login form[data-step="password"]')).toBeVisible();
    await expect(page.locator('keel-login').getByRole('alert')).toContainText(
      'The login key was not accepted',
    );
  },
);
