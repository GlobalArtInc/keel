import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed } from '@angular/core/testing';
import { Router } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import {
  AUTH_SU,
  AUTH_SU_EXIT,
  KeelClient,
  KeelI18n,
  type Catalogue,
  type Fetched,
} from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope, Messages } from '@globalart/keel/meta';
import {
  KeelSu,
  LANG_PREF,
  PREF_SET,
  PRINCIPAL_LIST,
  THEME_PREF,
  keelProviders,
  languageOptions,
  userMenuItems,
} from '@globalart/keel/shell';
import { KeelMockClient } from '@globalart/keel/testing';
import { PhxModalService, PhxThemeService, PhxToastService } from '@globalart/phoenix-ui';
import { of, type Observable } from 'rxjs';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const ALICE = '01J9Q3W6B1R8N5X2K4T7M0V9A1';
const BOB = '01J9Q3W6B1R8N5X2K4T7M0V9B2';

const EN: Messages = {
  menu: { system: 'System' },
  keel: {
    brand: 'Keel',
    level: { reseller: 'Reseller', user: 'User', owner: 'Owner' },
    account: { list: { title: 'Accounts' } },
    principal: { list: { title: 'Principals' } },
    product: { list: { title: 'Products' } },
    error: { request: '{message} (request {id})' },
    shell: {
      acting_as: { banner: 'You are acting as {login}', exit: 'Return to your own session' },
      act_as: {
        title: 'Act as a principal',
        search: 'Search by login',
        enter: 'Enter',
        empty: 'No principal matches',
      },
      user: {
        language: 'Language',
        theme: 'Theme',
        act_as: 'Act as',
        sessions: 'Sessions',
        tokens: 'API tokens',
        security: 'Security',
        logout: 'Log out',
      },
      language: { en: 'English', ru: 'Russian' },
      rail: {
        notifications: 'Notifications',
        tasks: 'Tasks',
        help: 'Help',
        later: 'Available in a later release',
      },
    },
  },
  settings: { error: { pref_owner: 'The owner has no preferences' } },
  phx: { light: 'Light', dark: 'Dark', system: 'System' },
};

const RU: Messages = {
  ...EN,
  menu: { system: 'System (ru)' },
  keel: { ...(EN['keel'] as Messages), brand: 'Keel (ru)' },
};

const CATALOGUES: Record<string, Messages> = { en: EN, ru: RU };

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

function resellerBoot(): Boot {
  return load<Boot>('boot-none.json');
}

function delegatedBoot(): Boot {
  const boot = resellerBoot();
  boot.principal = {
    id: ALICE,
    login: 'alice',
    level: 'user',
    account: '01J8A0000000000000000000A1',
    actor: { id: boot.principal?.id ?? '', login: 'res1', level: 'reseller', via: 'su' },
    lang: 'en',
    tz: null,
    totp: false,
  };
  return boot;
}

const SU: Descriptor = {
  keel: 1,
  name: AUTH_SU,
  module: 'auth',
  kind: 'action',
  mode: 'write',
  access: { level: 'reseller', permission: 'auth.su' },
  params: { type: 'object' },
  result: { type: 'object' },
};

function ok(data: unknown): Envelope {
  return { ok: true, data, meta: { requestId: 'req-rail' } };
}

class RailClient extends KeelMockClient {
  current: Boot;

  constructor(
    boot: Boot,
    descriptors: Descriptor[],
    answers: Record<string, Envelope>,
    private readonly after: Partial<Record<string, Boot>> = {},
  ) {
    super({ boot, descriptors, answers });
    this.current = boot;
  }

  override boot(): Observable<Fetched<Boot> | null> {
    return of({ body: this.current, etag: `"${this.current.meta}"` });
  }

  override catalogue(lang: string): Observable<Fetched<Catalogue> | null> {
    return of({ body: { lang, messages: CATALOGUES[lang] ?? EN }, etag: null });
  }

  override call<T = unknown>(func: string, params?: unknown): Observable<Envelope<T>> {
    const answered = super.call<T>(func, params);
    const next = this.after[func];
    if (next !== undefined) {
      this.current = next;
    }
    return answered;
  }
}

interface Rendered {
  harness: RouterTestingHarness;
  client: RailClient;
  element: HTMLElement;
}

let rendered: Rendered | null = null;

async function render(client: RailClient): Promise<Rendered> {
  TestBed.configureTestingModule({
    providers: [...keelProviders(), { provide: KeelClient, useValue: client }],
  });
  const harness = await RouterTestingHarness.create();
  await harness.navigateByUrl('/f/keel.account.list');
  await settle(harness);
  rendered = { harness, client, element: harness.fixture.nativeElement };
  return rendered;
}

async function tearDown(): Promise<void> {
  if (rendered !== null) {
    const open = rendered.element.querySelector<HTMLElement>(
      'phx-rail-item[data-rail="user"] button[aria-expanded="true"]',
    );
    if (open !== null) {
      open.click();
      await settle(rendered.harness);
    }
    rendered = null;
  }
  TestBed.inject(PhxModalService).closeAll();
  TestBed.inject(PhxToastService).closeAll();
  TestBed.resetTestingModule();
}

async function settle(harness: RouterTestingHarness): Promise<void> {
  await harness.fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  harness.detectChanges();
}

function overlay(): HTMLElement {
  const container = document.querySelector<HTMLElement>('.cdk-overlay-container');
  if (container === null) {
    throw new Error('no overlay container');
  }
  return container;
}

function click(target: Element | null): void {
  if (!(target instanceof HTMLElement)) {
    throw new Error('nothing to click');
  }
  target.click();
}

async function openUserMenu(
  harness: RouterTestingHarness,
  element: HTMLElement,
): Promise<HTMLElement> {
  click(element.querySelector('phx-rail-item[data-rail="user"] button'));
  await settle(harness);
  const menu = overlay().querySelector<HTMLElement>('phx-user-menu');
  if (menu === null) {
    throw new Error('the user menu did not open');
  }
  return menu;
}

function menuItems(menu: HTMLElement): string[] {
  return [...menu.querySelectorAll('.list > phx-dropdown-item')].map(
    (item) => (item.querySelector('.item__label') ?? item).textContent?.trim() ?? '',
  );
}

function menuItem(menu: HTMLElement, label: string): HTMLElement {
  const found = [...menu.querySelectorAll<HTMLElement>('phx-dropdown-item')].find((item) =>
    item.textContent?.includes(label),
  );
  if (found === undefined) {
    throw new Error(`no menu item ${label}`);
  }
  return found;
}

describe('the rail and the user menu', () => {
  afterEach(tearDown);

  it('the language switch posts settings.pref.set { key: "lang", value: "ru" } and reloads the catalogue', async () => {
    const client = new RailClient(resellerBoot(), [], {
      [PREF_SET]: ok({ key: LANG_PREF, value: 'ru' }),
    });
    const { harness, element } = await render(client);
    expect(element.querySelector('phx-sidebar nav.menu')?.getAttribute('aria-label')).toBe(
      'System',
    );
    expect(
      element.querySelector('phx-rail-item[data-rail="user"] button')?.getAttribute('aria-label'),
    ).toBe('owner');
    const disabled = [...element.querySelectorAll('phx-rail-item[disabled] button')].map((button) =>
      button.getAttribute('aria-label'),
    );
    expect(disabled).toEqual([
      'Notifications (Available in a later release)',
      'Tasks (Available in a later release)',
      'Help (Available in a later release)',
    ]);

    const menu = await openUserMenu(harness, element);
    expect(menu.querySelector('.who__name')?.textContent).toBe('owner');
    expect(menu.querySelector('.who__role')?.textContent).toBe('Owner');
    expect(menuItems(menu)).toEqual([
      'Sessions',
      'API tokens',
      'Security',
      'Log out',
      'Language',
      'Theme',
    ]);
    expect(menuItem(menu, 'Language').querySelector('.hint')?.textContent?.trim()).toBe('English');

    click(menuItem(menu, 'Language'));
    await settle(harness);
    click(menuItem(menu, 'Russian'));
    await settle(harness);
    expect(client.callsOf(PREF_SET).map((call) => call.params)).toEqual([
      { key: LANG_PREF, value: 'ru' },
    ]);
    expect(TestBed.inject(KeelI18n).lang()).toBe('ru');
    expect(element.querySelector('phx-sidebar nav.menu')?.getAttribute('aria-label')).toBe(
      'System (ru)',
    );
    expect(element.querySelector('phx-brand-logo .brand')?.textContent?.trim()).toBe('Keel (ru)');
  });

  it('a refused preference is shown as a toast and the catalogue stays', async () => {
    const client = new RailClient(resellerBoot(), [], {
      [PREF_SET]: {
        ok: false,
        error: { code: 'forbidden', key: 'settings.error.pref_owner', requestId: 'req-no' },
      },
    });
    const { harness, element } = await render(client);
    const menu = await openUserMenu(harness, element);
    click(menuItem(menu, 'Language'));
    await settle(harness);
    click(menuItem(menu, 'Russian'));
    await settle(harness);
    expect(client.callsOf(PREF_SET)).toHaveLength(1);
    expect(TestBed.inject(KeelI18n).lang()).toBe('en');
    expect(overlay().querySelector('phx-toast')?.textContent).toContain(
      'The owner has no preferences (request req-no)',
    );
  });

  it('the theme switch posts settings.pref.set { key: "theme", value: "dark" } and sets PhxThemeService', async () => {
    const client = new RailClient(resellerBoot(), [], {
      [PREF_SET]: ok({ key: THEME_PREF, value: 'dark' }),
    });
    const { harness, element } = await render(client);
    const theme = TestBed.inject(PhxThemeService);
    theme.setScheme('system');
    const menu = await openUserMenu(harness, element);
    click(menuItem(menu, 'Theme'));
    await settle(harness);
    expect(menu.querySelector('phx-theme-settings phx-color-slider')).toBeNull();
    const dark = [...menu.querySelectorAll<HTMLElement>('phx-switcher button')].find(
      (button) => button.textContent?.trim() === 'Dark',
    );
    click(dark ?? null);
    await settle(harness);
    expect(theme.scheme()).toBe('dark');
    expect(client.callsOf(PREF_SET).map((call) => call.params)).toEqual([
      { key: THEME_PREF, value: 'dark' },
    ]);
  });

  it('the su banner appears with principal.actor and posts auth.su.exit', async () => {
    const client = new RailClient(
      delegatedBoot(),
      [],
      { [AUTH_SU_EXIT]: ok({ principal: { id: ALICE, login: 'res1', level: 'reseller' } }) },
      { [AUTH_SU_EXIT]: resellerBoot() },
    );
    const { harness, element } = await render(client);
    const banner = element.querySelector('[data-acting-as]');
    expect(banner?.querySelector('.text')?.textContent?.trim()).toBe('You are acting as alice');
    expect(
      element.querySelector('phx-rail-item[data-rail="user"] button')?.getAttribute('aria-label'),
    ).toBe('alice');

    click(banner?.querySelector('.exit') ?? null);
    await settle(harness);
    await settle(harness);
    expect(client.callsOf(AUTH_SU_EXIT).map((call) => call.params)).toEqual([{}]);
    expect(element.querySelector('[data-acting-as]')).toBeNull();
    expect(
      element.querySelector('phx-rail-item[data-rail="user"] button')?.getAttribute('aria-label'),
    ).toBe('owner');
    expect(TestBed.inject(Router).url).toBe('/f/keel.account.list');
  });

  it('"Act as" is absent when auth.su is not in the index and searches keel.principal.list when it is', async () => {
    const without = new RailClient(resellerBoot(), [], {});
    const first = await render(without);
    expect(first.element.querySelector('keel-rail')?.getAttribute('data-actable')).toBe('false');
    expect(menuItems(await openUserMenu(first.harness, first.element))[0]).toBe('Sessions');
    expect(userMenuItems((key) => key, false).map((item) => item.id)).toEqual([
      'sessions',
      'tokens',
      'security',
      'logout',
    ]);
    expect(userMenuItems((key) => key, true)[0]).toEqual({
      id: 'act_as',
      label: 'keel.shell.user.act_as',
      icon: 'user-cog',
    });
    expect(languageOptions((key) => key)).toEqual([
      { value: 'en', label: 'keel.shell.language.en' },
      { value: 'ru', label: 'keel.shell.language.ru' },
    ]);
    await tearDown();

    const client = new RailClient(
      resellerBoot(),
      [SU],
      {
        [PRINCIPAL_LIST]: ok({
          data: [
            { id: ALICE, login: 'alice', level: 'user' },
            { id: BOB, login: 'alina', level: 'user' },
          ],
          total: 2,
        }),
        [AUTH_SU]: ok({ principal: { id: ALICE, login: 'alice', level: 'user' } }),
      },
      { [AUTH_SU]: delegatedBoot() },
    );
    const { harness, element } = await render(client);
    expect(element.querySelector('keel-rail')?.getAttribute('data-actable')).toBe('true');
    expect(TestBed.inject(KeelSu).available()).toBe(true);
    const menu = await openUserMenu(harness, element);
    expect(menuItems(menu)[0]).toBe('Act as');
    click(menuItem(menu, 'Act as'));
    await settle(harness);

    const dialog = overlay().querySelector<HTMLElement>('phx-modal keel-act-as');
    expect(dialog).not.toBeNull();
    expect(overlay().querySelector('phx-modal .title')?.textContent).toBe('Act as a principal');
    const input = dialog?.querySelector<HTMLInputElement>('phx-searchbar input') ?? null;
    if (input === null) {
      throw new Error('no search input');
    }
    input.value = 'ali';
    input.dispatchEvent(new Event('input'));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }));
    await settle(harness);
    expect(client.callsOf(PRINCIPAL_LIST).map((call) => call.params)).toEqual([
      { page: 1, pageSize: 20, search: 'ali' },
    ]);
    const rows = [...(dialog?.querySelectorAll<HTMLElement>('.row') ?? [])];
    expect(
      rows.map(
        (row) =>
          `${row.dataset['principal']} ${row.querySelector('.login')?.textContent} ${row.querySelector('.level')?.textContent}`,
      ),
    ).toEqual([`${ALICE} alice User`, `${BOB} alina User`]);

    click(rows[0]);
    await settle(harness);
    await settle(harness);
    expect(client.callsOf(AUTH_SU).map((call) => call.params)).toEqual([{ principal: ALICE }]);
    expect(overlay().querySelector('keel-act-as')).toBeNull();
    expect(element.querySelector('[data-acting-as] .text')?.textContent?.trim()).toBe(
      'You are acting as alice',
    );
    expect(TestBed.inject(Router).url).toBe('/f/keel.account.list');
  });
});
