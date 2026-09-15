import { expect, keelTest } from '@globalart/keel/testing/playwright';
import type { Locator, Page } from '@playwright/test';

const NOTE_LIST = '/f/note.list';
const NOTE_EDIT = '/f/note.edit';
const NOTE_IMPORT = '/f/note.import';
const TAG_LIST = '/f/note.tag.list';
const MAX_LINE = 200;
const DUE_AHEAD_DAYS = 7;
const IMPORTED_TAG = 'imported';
const IMPORTED = ['Delta', 'Epsilon', 'Zeta'];

interface NoteDraft {
  title: string;
  body: string;
  kind: 'memo' | 'todo';
  tags: string[];
}

const NOTES: readonly NoteDraft[] = [
  { title: 'Alpha', body: 'The first note', kind: 'memo', tags: ['one', 'shared'] },
  { title: 'Beta', body: 'The second note', kind: 'todo', tags: ['two', 'shared'] },
  { title: 'Gamma', body: 'The third note', kind: 'todo', tags: ['three'] },
];

const KIND_LABELS: Readonly<Record<NoteDraft['kind'], string>> = {
  memo: 'Memo',
  todo: 'To do',
};

function field(scope: Locator | Page, label: string): Locator {
  return scope.getByRole('group', { name: label, exact: true });
}

function rows(page: Page): Locator {
  return page.locator('phx-list-table .body [role="row"]');
}

function titles(page: Page): Locator {
  return rows(page).locator('[data-column="title"]');
}

function dueAhead(): string {
  const date = new Date();
  date.setUTCDate(date.getUTCDate() + DUE_AHEAD_DAYS);
  return new Intl.DateTimeFormat('en-US', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    timeZone: 'UTC',
  }).format(date);
}

async function saved(page: Page, func: string, save: () => Promise<void>): Promise<void> {
  const answered = page.waitForResponse(
    (response) =>
      response.url().endsWith(`/api/v1/call/${func}`) && response.request().method() === 'POST',
  );
  await save();
  expect((await answered).ok()).toBe(true);
}

async function openFilter(page: Page, column: string): Promise<Locator> {
  const header = page.locator(`[role="columnheader"][data-column="${column}"]`);
  await header.hover();
  await header.locator('button.cell_filter').click();
  return page.locator('phx-column-filter');
}

async function chooseKind(page: Page, kind: NoteDraft['kind']): Promise<void> {
  await field(page, 'Kind').getByRole('combobox').click();
  await page.getByRole('option', { name: KIND_LABELS[kind], exact: true }).click();
}

async function addTags(scope: Locator | Page, tags: readonly string[]): Promise<void> {
  const input = field(scope, 'Tags').getByRole('textbox');
  for (const tag of tags) {
    await input.fill(tag);
    await input.press('Enter');
  }
}

async function createNote(page: Page, draft: NoteDraft): Promise<void> {
  await page.goto(NOTE_EDIT);
  const form = page.locator('keel-form[data-form="note.edit"][data-mode="create"]');
  await expect(form).toBeVisible();
  await field(form, 'Title').getByRole('textbox').fill(draft.title);
  await field(form, 'Body').getByRole('textbox').fill(draft.body);
  await addTags(form, draft.tags);
  if (draft.kind === 'todo') {
    await expect(field(form, 'Due')).toHaveCount(0);
    await chooseKind(page, 'todo');
    await expect(field(form, 'Due').getByRole('textbox')).toHaveValue(dueAhead());
  }
  await saved(page, 'note.edit', () => form.locator('button.save').click());
}

keelTest.describe('notes', () => {
  keelTest.describe.configure({ mode: 'serial' });

  keelTest('alice creates notes through the form', async ({ page, keel }) => {
    await keel.asUser();
    await page.goto(NOTE_LIST);
    const list = page.locator('keel-list[data-list="note.list"]');
    await expect(list.getByRole('heading', { name: 'No notes yet' })).toBeVisible();
    await list.locator('[data-action="create"]').click();
    await expect(page).toHaveURL(new RegExp(`${NOTE_EDIT}$`));
    await expect(page.locator('keel-page[data-page="main"]')).toBeVisible();
    await expect(page.locator('keel-page[data-page="owner"]')).toHaveCount(0);
    await expect(page.getByRole('tab')).toHaveCount(0);
    for (const draft of NOTES) {
      await createNote(page, draft);
    }
    await page.goto(NOTE_LIST);
    await expect(rows(page)).toHaveCount(NOTES.length);
  });

  keelTest('the list searches, sorts, filters, selects and deletes', async ({ page, keel }) => {
    await keel.asUser();
    await page.goto(NOTE_LIST);
    await expect(rows(page)).toHaveCount(NOTES.length);

    const search = page.locator('keel-list input[type="search"]');
    await search.fill('Beta');
    await search.press('Enter');
    await expect(rows(page)).toHaveCount(1);
    await expect(titles(page)).toHaveText(['Beta']);
    await search.fill('');
    await search.press('Enter');
    await expect(rows(page)).toHaveCount(NOTES.length);

    const title = page.locator('[role="columnheader"][data-column="title"]');
    await title.getByRole('button').click();
    await expect(title).toHaveAttribute('aria-sort', 'ascending');
    await expect(titles(page)).toHaveText(['Alpha', 'Beta', 'Gamma']);
    await title.getByRole('button').click();
    await expect(title).toHaveAttribute('aria-sort', 'descending');
    await expect(titles(page)).toHaveText(['Gamma', 'Beta', 'Alpha']);

    const filter = await openFilter(page, 'kind');
    await filter.getByText(KIND_LABELS.todo, { exact: true }).click();
    await expect(filter.getByRole('checkbox', { name: KIND_LABELS.todo })).toBeChecked();
    await filter.locator('button[type="submit"]').click();
    await expect(rows(page)).toHaveCount(2);
    await expect(titles(page)).toHaveText(['Gamma', 'Beta']);

    for (const row of await rows(page).all()) {
      await row.locator('.cell_check label').click();
      await expect(row).toHaveAttribute('aria-selected', 'true');
    }
    const bar = page.locator('phx-selection-bar [role="toolbar"]');
    await expect(bar).toContainText('2');
    await bar.getByRole('button', { name: 'Delete' }).click();

    const confirm = page.getByRole('alertdialog');
    await expect(confirm).toContainText('Gamma');
    await expect(confirm).toContainText('Beta');
    const button = confirm.getByRole('button', { name: 'Delete' });
    await expect(button).toBeDisabled();
    await confirm.getByRole('textbox').fill('2');
    await expect(button).toBeEnabled();
    await button.click();
    await expect(confirm).toHaveCount(0);
    await expect(rows(page)).toHaveCount(0);

    const reopened = await openFilter(page, 'kind');
    await reopened.getByText(KIND_LABELS.todo, { exact: true }).click();
    await expect(reopened.getByRole('checkbox', { name: KIND_LABELS.todo })).not.toBeChecked();
    await reopened.locator('button[type="submit"]').click();
    await expect(titles(page)).toHaveText(['Alpha']);
  });

  keelTest('the form has the owner page for root alone', async ({ page, keel }) => {
    await keel.asAdmin();
    await page.goto(NOTE_EDIT);
    await expect(page.locator('keel-page[data-page="main"]')).toBeVisible();
    await expect(page.locator('keel-page[data-page="owner"]')).toHaveCount(1);
    await page.getByRole('tab', { name: 'Owner' }).click();
    await expect(field(page, 'Account').getByRole('combobox')).toBeVisible();
  });

  keelTest('a duplicate title is refused under the title', async ({ page, keel }) => {
    await keel.asUser();
    await page.goto(NOTE_EDIT);
    const form = page.locator('keel-form[data-form="note.edit"]');
    const title = field(form, 'Title');
    await title.getByRole('textbox').fill(NOTES[0].title);
    await form.locator('button.save').click();
    await expect(title.locator('phx-field-error')).toContainText(
      `A note titled ${NOTES[0].title} already exists`,
    );
  });

  keelTest('the import wizard refuses a long line and imports three', async ({ page, keel }) => {
    await keel.asUser();
    await page.goto(NOTE_LIST);
    await page.goto(NOTE_IMPORT);
    const wizard = page.locator('keel-wizard[data-wizard="note.import"]');
    await expect(wizard).toHaveAttribute('data-step', 'source');
    const lines = field(wizard, 'Lines').getByRole('textbox');
    await lines.fill('x'.repeat(MAX_LINE + 1));
    await wizard.locator('button.next').click();
    await expect(field(wizard, 'Lines').locator('phx-field-error')).toContainText(
      `Line 1 is longer than ${MAX_LINE} characters`,
    );
    await expect(wizard).toHaveAttribute('data-step', 'source');

    await lines.fill(IMPORTED.join('\n'));
    await wizard.locator('button.next').click();
    await expect(wizard).toHaveAttribute('data-step', 'options');
    await addTags(wizard, [IMPORTED_TAG]);
    await saved(page, 'note.import', () => wizard.locator('button.next').click());
    await expect(page).toHaveURL(new RegExp(`${NOTE_LIST}$`));
    await expect(rows(page)).toHaveCount(1 + IMPORTED.length);
  });

  keelTest('the tag cloud shows the tags of the notes', async ({ page, keel }) => {
    await keel.asUser();
    await page.goto(TAG_LIST);
    const cloud = page.locator('demo-note-tag-cloud[data-cloud="note.tag.list"]');
    await expect(cloud).toHaveAttribute('data-phase', 'loaded');
    const tags = cloud.locator('.tag');
    await expect(tags).toHaveCount(NOTES[0].tags.length + 1);
    for (const tag of NOTES[0].tags) {
      await expect(cloud.locator(`[data-tag="${tag}"]`)).toHaveAttribute('data-count', '1');
    }
    await expect(cloud.locator(`[data-tag="${IMPORTED_TAG}"]`)).toHaveAttribute(
      'data-count',
      String(IMPORTED.length),
    );
    await cloud.locator(`[data-tag="${IMPORTED_TAG}"]`).click();
    await expect(page).toHaveURL(new RegExp(`${NOTE_LIST}\\?search=${IMPORTED_TAG}$`));
  });
});
