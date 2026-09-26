import { E2E_SCRIPT, expect, keelTest } from '@globalart/keel/testing/playwright';
import type { Locator, Page } from '@playwright/test';

const TAG = '@panel';
const SITE_VARIABLE = 'KEEL_E2E_SITE';
const DIRECTIVE_VARIABLE = 'KEEL_E2E_SITE_DIRECTIVE';
const REFUSED = 'keel_e2e_refused on;';

function variable(name: string): string {
  const value = process.env[name];
  if (value === undefined || value === '') {
    throw new Error(`${name} is not set; ${E2E_SCRIPT} creates the site over the CLI`);
  }
  return value;
}

function view(page: Page): Locator {
  return page.locator('panel-config-editor');
}

function editor(page: Page, side: 'override' | 'rendered'): Locator {
  return view(page).locator(`[data-part="server"] phx-code-editor.${side}`);
}

async function replace(page: Page, text: string): Promise<void> {
  await editor(page, 'override').locator('.cm-content').click();
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.type(text);
}

keelTest.describe('the configuration editor', { tag: TAG }, () => {
  keelTest.describe.configure({ mode: 'serial' });

  keelTest(
    'the operator edits the server block of a site, is refused by the check and sees the rendered block after a save',
    async ({ page, keel }) => {
      const domain = variable(SITE_VARIABLE);
      const directive = variable(DIRECTIVE_VARIABLE);
      await keel.asAdmin();
      await page
        .locator('phx-sidebar')
        .getByRole('link', { name: 'Site configuration', exact: true })
        .click();
      await expect(page).toHaveURL(/\/f\/hosting\.site\.config-editor$/);
      await view(page).locator('phx-select').getByRole('combobox').click();
      await page.getByRole('option', { name: domain, exact: true }).click();
      await expect(view(page)).toHaveAttribute('data-phase', 'loaded');
      await expect(editor(page, 'rendered')).toContainText(`server_name ${domain};`);
      await expect(editor(page, 'rendered')).not.toContainText(directive);

      await replace(page, REFUSED);
      await view(page).getByRole('button', { name: 'Save', exact: true }).click();
      await expect(editor(page, 'override')).toContainText(
        'nginx: [emerg] unknown directive "keel_e2e_refused"',
      );
      await expect(editor(page, 'rendered')).not.toContainText(REFUSED);

      await replace(page, directive);
      await view(page).getByRole('button', { name: 'Save', exact: true }).click();
      await expect(page.getByText(`The configuration of ${domain} is saved`)).toBeVisible();
      await expect(editor(page, 'rendered')).toContainText(directive);
      await expect(editor(page, 'rendered')).toContainText(`server_name ${domain};`);
      await expect(editor(page, 'override')).not.toContainText('unknown directive');
    },
  );
});
