import { expect, keelTest } from '@globalart/keel/testing/playwright';
import type { Locator, Page } from '@playwright/test';

const NOTE_LIST = '/f/note.list';
const PACED_EXPORT = '/f/note.export?format=csv&pace=1000';
const CSV_HEADER = 'id,title,kind,due,tags';
const FILE_NAME = 'notes.csv';
const LOG_PREFIX = 'exported';
const EXPORT_TITLE = 'Export notes';
const JOBS = 3;

function modal(page: Page): Locator {
  return page.locator('keel-progress');
}

function drawer(page: Page): Locator {
  return page.locator('keel-task-drawer');
}

async function confirmExport(page: Page): Promise<void> {
  const confirm = page.getByRole('alertdialog');
  await expect(confirm).toBeVisible();
  await confirm.locator('.buttons button').first().click();
}

async function readDownload(page: Page, start: () => Promise<void>): Promise<string> {
  const pending = page.waitForEvent('download');
  await start();
  const download = await pending;
  expect(download.suggestedFilename()).toBe(FILE_NAME);
  const stream = await download.createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }
  return Buffer.concat(chunks).toString('utf8');
}

keelTest.describe('tasks', () => {
  keelTest.describe.configure({ mode: 'serial' });

  keelTest('alice exports the notes through the progress modal', async ({ page, keel }) => {
    await keel.asUser();
    await page.goto(NOTE_LIST);
    const list = page.locator('keel-list[data-list="note.list"]');
    await expect(list.locator('phx-list-table .body [role="row"]').first()).toBeVisible();
    const csv = await readDownload(page, () => list.locator('[data-action="export_task"]').click());
    const lines = csv.trim().split('\n');
    expect(lines[0]).toBe(CSV_HEADER);
    expect(lines.length).toBeGreaterThan(1);
    await expect(modal(page)).toHaveCount(0);
  });

  keelTest(
    'the paced export shows the titles, opens the details and cancels',
    async ({ page, keel }) => {
      await keel.asUser();
      await page.goto(PACED_EXPORT);
      await confirmExport(page);
      const progress = modal(page);
      await expect(progress).toBeVisible();
      await expect(progress.locator('.status')).not.toHaveText('');
      const status = await progress.locator('.status').textContent();
      expect(status?.trim()).toMatch(/\S/);
      await progress.locator('button.open').click();
      await expect(progress).toHaveCount(0);
      const details = drawer(page);
      await expect(details).toHaveAttribute('data-job', /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/);
      await expect(details.locator('phx-code-field textarea')).toHaveValue(
        new RegExp(`${LOG_PREFIX} ${status?.trim() ?? ''}`),
      );
      await details.locator('button.cancel').click();
      await expect(details.locator('button.cancel')).toHaveCount(0);
      await expect(details.locator('phx-prop-list')).toContainText('Cancelled');
    },
  );

  keelTest(
    'the paced export runs to the end and the drawer lists the jobs',
    async ({ page, keel }) => {
      await keel.asUser();
      await page.goto(PACED_EXPORT);
      const csv = await readDownload(page, () => confirmExport(page));
      expect(csv.trim().split('\n')[0]).toBe(CSV_HEADER);
      await expect(modal(page)).toHaveCount(0);
      await page.locator('keel-rail [data-rail="tasks"]').click();
      const jobs = drawer(page).locator('button.job');
      await expect(jobs).toHaveCount(JOBS);
      await expect(jobs.locator('phx-badge')).toHaveText(['Done', 'Cancelled', 'Done']);
      await jobs.first().click();
      await expect(drawer(page).locator('phx-prop-list')).toContainText(EXPORT_TITLE);
    },
  );
});
