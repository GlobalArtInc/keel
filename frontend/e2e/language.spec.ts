import type { Catalogue } from '@globalart/keel/client';
import { lookup, type Envelope, type Messages } from '@globalart/keel/meta';
import { API_PREFIX, expect, keelBaseUrl, keelTest } from '@globalart/keel/testing/playwright';
import type { Locator, Page } from '@playwright/test';

type Lang = 'en' | 'ru';

const LANGUAGE_NAME_PREFIX = 'keel.shell.language';
const LANGUAGE_SECTION_KEY = 'keel.shell.user.language';
const NOTES_KEY = 'note.list.title';
const TITLE_COLUMN_KEY = 'note.column.title';
const KIND_COLUMN_KEY = 'note.column.kind';
const TITLE_FIELD_KEY = 'note.title';
const BODY_FIELD_KEY = 'note.body';

interface Words {
  names: Readonly<Record<Lang, string>>;
  section: string;
  notes: string;
  titleColumn: string;
  kindColumn: string;
  title: string;
  body: string;
}

let words: Readonly<Record<Lang, Words>>;

async function catalogue(lang: Lang): Promise<Messages> {
  const response = await fetch(`${keelBaseUrl()}${API_PREFIX}/i18n?lang=${lang}`);
  const envelope = (await response.json()) as Envelope<Catalogue>;
  if (!envelope.ok) {
    throw new Error(`the ${lang} catalogue was refused: ${envelope.error.key}`);
  }
  return envelope.data.messages;
}

function wordsOf(lang: Lang, messages: Messages): Words {
  const pick = (key: string): string => {
    const found = lookup(messages, key);
    if (found === undefined) {
      throw new Error(`the ${lang} catalogue has no ${key}`);
    }
    return found;
  };
  return {
    names: { en: pick(`${LANGUAGE_NAME_PREFIX}.en`), ru: pick(`${LANGUAGE_NAME_PREFIX}.ru`) },
    section: pick(LANGUAGE_SECTION_KEY),
    notes: pick(NOTES_KEY),
    titleColumn: pick(TITLE_COLUMN_KEY),
    kindColumn: pick(KIND_COLUMN_KEY),
    title: pick(TITLE_FIELD_KEY),
    body: pick(BODY_FIELD_KEY),
  };
}

function inEitherLanguage(
  menu: Locator,
  role: 'menuitem' | 'menuitemradio',
  name: (shown: Words) => string,
  exact: boolean,
): Locator {
  return menu
    .getByRole(role, { name: name(words.en), exact })
    .or(menu.getByRole(role, { name: name(words.ru), exact }));
}

async function switchLanguage(page: Page, lang: Lang): Promise<void> {
  await page.locator('keel-rail [data-rail="user"] button').click();
  const menu = page.locator('phx-user-menu');
  await inEitherLanguage(menu, 'menuitem', (shown) => shown.section, false).click();
  await inEitherLanguage(menu, 'menuitemradio', (shown) => shown.names[lang], true).click();
  await page.keyboard.press('Escape');
}

async function expectWords(page: Page, lang: Lang): Promise<void> {
  const shown = words[lang];
  const sidebar = page.locator('phx-sidebar');
  await expect(sidebar.getByRole('link', { name: shown.notes })).toBeVisible();
  await page.goto('/f/note.list');
  await expect(page.locator('[role="columnheader"][data-column="title"]')).toContainText(
    shown.titleColumn,
  );
  await expect(page.locator('[role="columnheader"][data-column="kind"]')).toContainText(
    shown.kindColumn,
  );
  await page.goto('/f/note.edit');
  const form = page.locator('keel-form[data-form="note.edit"]');
  await expect(form.getByRole('group', { name: shown.title, exact: true })).toBeVisible();
  await expect(form.getByRole('group', { name: shown.body, exact: true })).toBeVisible();
}

keelTest.describe('language', () => {
  keelTest.beforeAll(async () => {
    const [en, ru] = await Promise.all([catalogue('en'), catalogue('ru')]);
    words = { en: wordsOf('en', en), ru: wordsOf('ru', ru) };
  });

  keelTest.afterEach(async ({ page }) => {
    await switchLanguage(page, 'en');
    await expect(
      page.locator('phx-sidebar').getByRole('link', { name: words.en.notes }),
    ).toBeVisible();
  });

  keelTest('alice switches to ru and the choice survives a reload', async ({ page, keel }) => {
    await keel.asUser();
    await switchLanguage(page, 'en');
    await expectWords(page, 'en');

    await switchLanguage(page, 'ru');
    await expectWords(page, 'ru');

    await page.reload();
    await expect(page.locator('keel-layout')).toBeVisible();
    await expectWords(page, 'ru');
  });
});
