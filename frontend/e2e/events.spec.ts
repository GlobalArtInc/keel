import { expect, keelTest, type KeelApiSession } from '@globalart/keel/testing/playwright';
import type { Locator, Page } from '@playwright/test';

const NOTE_LIST = '/f/note.list';
const NOTE_EDIT = 'note.edit';
const NOTE_DELETE = 'note.delete';
const NOTE_LIST_CALL = '/api/v1/call/note.list';
const OWN_TITLE = 'Live wire';
const OWN_EDITED = 'Live wire, edited over the API';
const OWN_FINAL = 'Live wire, edited once more';
const OTHER_TITLE = 'Reseller wire';
const OTHER_EDITED = 'Reseller wire, edited over the API';
const QUIET_MS = 1000;
const MARKER = 'live';

interface NoteRow {
  id: string;
  accountId: string;
  title: string;
}

function titles(page: Page): Locator {
  return page.locator('phx-list-table .body [role="row"] [data-column="title"]');
}

function mark(page: Page): Promise<void> {
  return page.evaluate((marker) => {
    document.documentElement.dataset['e2e'] = marker;
  }, MARKER);
}

function marker(page: Page): Promise<string | undefined> {
  return page.evaluate(() => document.documentElement.dataset['e2e']);
}

async function createNote(session: KeelApiSession, title: string): Promise<NoteRow> {
  const envelope = await session.call<NoteRow>(NOTE_EDIT, { title, body: `${title} body` });
  if (!envelope.ok) {
    throw new Error(`${NOTE_EDIT} refused ${title} for ${session.login}: ${envelope.error.key}`);
  }
  return envelope.data;
}

async function editAs(root: KeelApiSession, note: NoteRow, title: string): Promise<void> {
  const envelope = await root.call(NOTE_EDIT, {
    id: note.id,
    title,
    body: `${title} body`,
    account: note.accountId,
  });
  expect(envelope.ok).toBe(true);
}

async function deleteNote(session: KeelApiSession, note: NoteRow | null): Promise<void> {
  if (note !== null) {
    await session.call(NOTE_DELETE, { ids: [note.id] });
  }
}

function countListCalls(page: Page): () => number {
  let count = 0;
  page.on('request', (request) => {
    if (request.url().endsWith(NOTE_LIST_CALL)) {
      count += 1;
    }
  });
  return () => count;
}

keelTest.describe('the event stream', () => {
  keelTest(
    'an edit over the API reaches the open list of its account alone',
    async ({ page, keel, keelApi }) => {
      const alice = await keelApi.as('user');
      const res1 = await keelApi.as('reseller');
      const root = await keelApi.as('admin');
      let own: NoteRow | null = null;
      let other: NoteRow | null = null;
      try {
        own = await createNote(alice, OWN_TITLE);
        other = await createNote(res1, OTHER_TITLE);
        expect(own.accountId).not.toBe(other.accountId);

        await keel.asUser();
        const listCalls = countListCalls(page);
        await page.goto(NOTE_LIST);
        await expect(titles(page)).toHaveText([OWN_TITLE]);
        const listed = listCalls();
        await mark(page);

        await editAs(root, own, OWN_EDITED);
        await expect(titles(page)).toHaveText([OWN_EDITED]);
        expect(listCalls()).toBe(listed + 1);

        await editAs(root, other, OTHER_EDITED);
        await page.waitForTimeout(QUIET_MS);
        await expect(titles(page)).toHaveText([OWN_EDITED]);
        expect(listCalls()).toBe(listed + 1);

        await editAs(root, own, OWN_FINAL);
        await expect(titles(page)).toHaveText([OWN_FINAL]);
        expect(listCalls()).toBe(listed + 2);
        expect(await marker(page)).toBe(MARKER);
        await expect(page).toHaveURL(new RegExp(`${NOTE_LIST}$`));
      } finally {
        await deleteNote(alice, own);
        await deleteNote(res1, other);
        await Promise.all([alice.logout(), res1.logout(), root.logout()]);
      }
    },
  );
});
