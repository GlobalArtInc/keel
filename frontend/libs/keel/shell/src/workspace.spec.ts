import { computed, signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { provideLocationMocks } from '@angular/common/testing';
import { Router, provideRouter, type Routes } from '@angular/router';
import { KeelBoot, KeelI18n } from '@globalart/keel/client';
import type { Boot, BootMenuGroup } from '@globalart/keel/meta';
import { KEEL_WORKSPACE, KeelNavigation } from '@globalart/keel/renderer';
import { KeelWorkspace } from '@globalart/keel/shell';
import { KEEL_BOOT } from '@globalart/keel/testing';
import { PhxTabsService, type PhxTab } from '@globalart/phoenix-ui';

const ADMIN = 'p-admin';
const OTHER = 'p-other';
const KEY = (id: string): string => `keel.workspace.v1.${id}`;
const WIDGETS = KEEL_BOOT.one.dashboard;
const WINDOW_WIDTH = window.innerWidth;

const MESSAGES: Record<string, string> = {
  'keel.shell.home.title': 'Overview',
  'dns.zone.list.title': 'Zones',
  'dns.zone.edit.title': 'Edit zone',
  'dns.record.list.title': 'Records',
};

const MENU: BootMenuGroup[] = [
  {
    id: 'dns',
    label: 'dns.group',
    icon: 'globe',
    items: [
      { path: 'zones', label: 'dns.zone.list.title', func: 'dns.zone.list' },
      { path: 'edit', label: 'dns.zone.edit.title', func: 'dns.zone.edit' },
      { path: 'records', label: 'dns.record.list.title', func: 'dns.record.list' },
    ],
  },
];

const ROUTES: Routes = [
  { path: '', pathMatch: 'full', children: [] },
  { path: 'login', children: [] },
  { path: 'f/:func', children: [] },
  { path: '**', children: [] },
];

const LIST = { search: 'a', sort: null, filters: [], page: 2 };

function principalAs(id: string): Boot['principal'] {
  const principal = KEEL_BOOT.one.principal;
  return principal === null || principal === undefined ? null : { ...principal, id };
}

function bootAs(id: string, dashboard: Boot['dashboard'] = []): Boot {
  return { ...KEEL_BOOT.one, principal: principalAs(id), menu: MENU, dashboard };
}

const payload = signal<Boot | null>(null);
const bootStub = {
  payload,
  principal: computed(() => payload()?.principal ?? null),
  menu: computed(() => payload()?.menu ?? []),
};
const i18nStub = { t: (key: string) => MESSAGES[key] ?? key };

function workspace(): KeelWorkspace {
  return TestBed.inject(KeelWorkspace);
}

function tabs(): PhxTabsService {
  return TestBed.inject(PhxTabsService);
}

function ids(): string[] {
  return tabs()
    .tabs()
    .map((tab) => tab.id);
}

function tab(id: string): PhxTab {
  const found = tabs()
    .tabs()
    .find((candidate) => candidate.id === id);
  if (found === undefined) {
    throw new Error(`no tab ${id}`);
  }
  return found;
}

interface StoredTabs {
  tabs: PhxTab[];
}

function storedIds(id: string): string[] {
  const raw = localStorage.getItem(KEY(id));
  const state: StoredTabs | null = raw === null ? null : JSON.parse(raw);
  return state?.tabs.map((item) => item.id) ?? [];
}

async function settle(): Promise<void> {
  TestBed.flushEffects();
  await new Promise<void>((done) => setTimeout(done, 0));
}

async function go(url: string): Promise<void> {
  await TestBed.inject(Router).navigateByUrl(url);
  await settle();
}

function url(): string {
  return TestBed.inject(Router).url;
}

describe('the open pages workspace', () => {
  beforeEach(() => {
    localStorage.clear();
    payload.set(bootAs(ADMIN));
    TestBed.configureTestingModule({
      providers: [
        provideRouter(ROUTES),
        provideLocationMocks(),
        { provide: KeelBoot, useValue: bootStub },
        { provide: KeelI18n, useValue: i18nStub },
        { provide: KEEL_WORKSPACE, useExisting: KeelWorkspace },
      ],
    });
  });

  afterEach(() => {
    TestBed.resetTestingModule();
    jest.restoreAllMocks();
    localStorage.clear();
    window.innerWidth = WINDOW_WIDTH;
  });

  it('a screen opens a tab keyed by its path and sorted query without selected, keeping the full url', async () => {
    workspace();
    await go('/f/dns.zone.edit?zone=z0&id=z1&selected=z2');
    expect(tabs().tabs()).toEqual([
      {
        id: '/f/dns.zone.edit?id=z1&zone=z0',
        label: 'Edit zone',
        data: {
          url: '/f/dns.zone.edit?zone=z0&id=z1&selected=z2',
          func: 'dns.zone.edit',
          lastActive: expect.any(Number),
        },
      },
    ]);
    expect(tabs().activeId()).toBe('/f/dns.zone.edit?id=z1&zone=z0');
  });

  it('a repeated navigation activates the tab instead of duplicating it', async () => {
    workspace();
    await go('/f/dns.zone.list');
    await go('/f/dns.zone.edit?id=z1');
    await go('/f/dns.zone.list');
    expect(ids()).toEqual(['/f/dns.zone.list', '/f/dns.zone.edit?id=z1']);
    expect(tabs().activeId()).toBe('/f/dns.zone.list');
  });

  it('a child opened through KeelNavigation takes the active tab as its parent and a menu navigation takes none', async () => {
    const keel = workspace();
    await go('/f/dns.zone.list');
    await keel.openFunc({
      func: 'dns.record.list',
      url: '/f/dns.record.list',
      label: 'Records',
      icon: null,
      problem: null,
    });
    await settle();
    expect(tab('/f/dns.record.list').parent).toBeUndefined();

    await TestBed.inject(KeelNavigation).object(
      { screen: 'dns.zone.edit', params: { id: 'z1' } },
      'a.org',
    );
    await settle();
    expect(tab('/f/dns.zone.edit?id=z1')).toMatchObject({ label: 'a.org', parent: 'Records' });
  });

  it('closing the active tab moves to its neighbour and closing the last one moves to the root', async () => {
    const keel = workspace();
    await go('/f/dns.zone.list');
    await go('/f/dns.zone.edit?id=z1');
    await go('/f/dns.record.list');
    await go('/f/dns.zone.edit?id=z1');

    keel.close(tab('/f/dns.zone.edit?id=z1'));
    await settle();
    expect(url()).toBe('/f/dns.record.list');

    keel.close(tab('/f/dns.record.list'));
    await settle();
    expect(url()).toBe('/f/dns.zone.list');

    keel.close(tab('/f/dns.zone.list'));
    await settle();
    expect(url()).toBe('/');
    expect(ids()).toEqual([]);
  });

  it('the twenty-first unpinned tab evicts the inactive one with the lowest lastActive together with its list state', async () => {
    let clock = 0;
    jest.spyOn(Date, 'now').mockImplementation(() => ++clock);
    const keel = workspace();
    for (let page = 1; page <= 20; page++) {
      await go(`/f/dns.zone.list?page=${page}`);
    }
    keel.saveListState('/f/dns.zone.list?page=2', LIST);
    await go('/f/dns.zone.list?page=1');
    await go('/f/dns.zone.list?page=21');
    expect(ids()).toHaveLength(20);
    expect(ids()).toContain('/f/dns.zone.list?page=1');
    expect(ids()).not.toContain('/f/dns.zone.list?page=2');
    expect(keel.listState('/f/dns.zone.list?page=2')).toBeNull();
  });

  it('the state is stored under the key of the principal id', async () => {
    workspace();
    await go('/f/dns.zone.list');
    expect(storedIds(ADMIN)).toEqual(['/f/dns.zone.list']);

    await go('/login');
    payload.set(bootAs(OTHER));
    await settle();
    await go('/f/dns.record.list');
    expect(storedIds(OTHER)).toEqual(['/f/dns.record.list']);
    expect(storedIds(ADMIN)).toEqual(['/f/dns.zone.list']);
  });

  it.each(['{', JSON.stringify({ v: 2, tabs: [], active: null, lists: {} })])(
    'a stored state that does not parse or has another version gives an empty workspace (%s)',
    async (raw) => {
      localStorage.setItem(KEY(ADMIN), raw);
      workspace();
      await settle();
      expect(ids()).toEqual([]);
    },
  );

  it('a restore makes the tab of the current url active', async () => {
    const stored = (id: string, func: string) => ({
      id,
      label: func,
      data: { url: id, func, lastActive: 1 },
    });
    localStorage.setItem(
      KEY(ADMIN),
      JSON.stringify({
        v: 1,
        tabs: [
          stored('/f/dns.zone.list', 'dns.zone.list'),
          stored('/f/dns.zone.edit?id=z1', 'dns.zone.edit'),
        ],
        active: '/f/dns.zone.list',
        lists: {},
      }),
    );
    await TestBed.inject(Router).navigateByUrl('/f/dns.zone.edit?id=z1');
    workspace();
    await settle();
    expect(ids()).toEqual(['/f/dns.zone.list', '/f/dns.zone.edit?id=z1']);
    expect(tabs().activeId()).toBe('/f/dns.zone.edit?id=z1');
  });

  it('the home tab is pinned on the root with a dashboard and survives closing all', async () => {
    payload.set(bootAs(ADMIN, WIDGETS));
    const keel = workspace();
    await go('/');
    await go('/f/dns.zone.list');
    expect(ids()).toEqual(['home', '/f/dns.zone.list']);
    expect(tab('home')).toMatchObject({
      label: 'Overview',
      pinned: true,
      icon: 'layout-dashboard',
    });

    keel.closeAll();
    await settle();
    expect(ids()).toEqual(['home']);
    expect(url()).toBe('/');
  });

  it('is visible with tabs open in a window wider than 640 px and follows a resize', async () => {
    const keel = workspace();
    expect(keel.visible()).toBe(false);
    await go('/f/dns.zone.list');
    expect(keel.visible()).toBe(true);
    window.innerWidth = 640;
    window.dispatchEvent(new Event('resize'));
    expect(keel.visible()).toBe(false);
    window.innerWidth = 641;
    window.dispatchEvent(new Event('resize'));
    expect(keel.visible()).toBe(true);
  });
});
