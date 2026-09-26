import { E2E_SCRIPT, expect, keelTest } from '@globalart/keel/testing/playwright';
import type { Locator, Page } from '@playwright/test';

const TAG = '@panel';
const HOSTING_USER_VARIABLE = 'KEEL_E2E_HOSTING_USER';
const UPLOADED = 'hello.html';
const RENAMED = 'renamed.html';
const CONTENT = '<p>uploaded through the file manager</p>';

function hostingUser(): string {
  const user = process.env[HOSTING_USER_VARIABLE];
  if (user === undefined || user === '') {
    throw new Error(
      `${HOSTING_USER_VARIABLE} is not set; ${E2E_SCRIPT} creates that hosting user over the CLI`,
    );
  }
  return user;
}

function view(page: Page): Locator {
  return page.locator('panel-file-manager');
}

function entry(page: Page, name: string): Locator {
  return view(page).locator(`[data-entry="${name}"]`);
}

async function act(page: Page, name: string, label: string): Promise<void> {
  const row = view(page)
    .locator('phx-list-table .body [role="row"]')
    .filter({ has: page.locator(`[data-entry="${name}"]`) });
  await expect(row).toHaveCount(1);
  await row.locator('.cell_actions button').click();
  await page.getByRole('menuitem', { name: label, exact: true }).click();
}

keelTest.describe('the file manager', { tag: TAG }, () => {
  keelTest.describe.configure({ mode: 'serial' });

  keelTest(
    'alice uploads, renames, downloads and deletes a file in the home of her hosting user',
    async ({ page, keel }) => {
      await keel.asUser();
      await page.locator('phx-sidebar').getByRole('link', { name: 'Files' }).click();
      await expect(page).toHaveURL(/\/f\/hosting\.file-manager$/);
      await view(page).locator('phx-select').getByRole('combobox').click();
      await page.getByRole('option', { name: hostingUser(), exact: true }).click();
      await expect(view(page)).toHaveAttribute('data-phase', 'loaded');

      await view(page)
        .locator('input[data-picker="upload"]')
        .setInputFiles({ name: UPLOADED, mimeType: 'text/html', buffer: Buffer.from(CONTENT) });
      await expect(entry(page, UPLOADED)).toBeVisible();

      await act(page, UPLOADED, 'Rename');
      const prompt = page.locator('panel-file-prompt');
      await prompt.getByRole('textbox').fill(RENAMED);
      await prompt.getByRole('button', { name: 'Save' }).click();
      await expect(entry(page, RENAMED)).toBeVisible();
      await expect(entry(page, UPLOADED)).toHaveCount(0);

      const pending = page.waitForEvent('download');
      await act(page, RENAMED, 'Download');
      const download = await pending;
      expect(download.suggestedFilename()).toBe(RENAMED);
      const chunks: Buffer[] = [];
      for await (const chunk of await download.createReadStream()) {
        chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
      }
      expect(Buffer.concat(chunks).toString('utf8')).toBe(CONTENT);

      await act(page, RENAMED, 'Delete');
      await page.getByRole('alertdialog').getByRole('button', { name: 'Delete' }).click();
      await expect(entry(page, RENAMED)).toHaveCount(0);
      await expect(view(page)).toHaveAttribute('data-phase', 'loaded');
    },
  );
});
