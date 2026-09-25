import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { CdkDropList, type CdkDragDrop } from '@angular/cdk/drag-drop';
import { TestBed } from '@angular/core/testing';
import { By } from '@angular/platform-browser';
import { Router } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { KeelClient, PROBLEM_LIST, type Problem } from '@globalart/keel/client';
import type { Boot, BootWidget, Descriptor, Messages } from '@globalart/keel/meta';
import { hasDashboard, keelProviders } from '@globalart/keel/shell';
import { KEEL_BOOT, KeelMockClient, okEnvelope, pageEnvelope } from '@globalart/keel/testing';
import { PhxToastService, PhxWidgetGridComponent } from '@globalart/phoenix-ui';

const DUMP = resolve(__dirname, '../../../../../contracts/dump/admin/functions');
const ACCOUNT = '01J8A0000000000000000000A2';
const ZONES_WIDGET = 'dns.widget.zones';
const ACCOUNTS_WIDGET = 'keel.widget.accounts';
const PREF_SET = 'settings.pref.set';
const STATS_SERIES = 'stats.series';

const EN: Messages = {
  menu: { system: 'System', demo: 'Demo' },
  product: { demo: { brand: 'Keel Demo' } },
  keel: {
    brand: 'Keel',
    level: { admin: 'Administrator' },
    shell: {
      rail: { notifications: 'Notifications', tasks: 'Tasks', help: 'Help' },
      home: {
        title: 'Overview',
        open: 'Open',
        hide: 'Hide',
        restore: 'Restore widget',
        empty: 'Every widget is hidden',
        drag: 'Drag to reorder',
        sparkline: 'Last 7 days',
        unit: { count: 'Total' },
      },
      user: { language: 'Language', theme: 'Theme' },
    },
  },
  dns: { widget: { zones: { title: 'Zones' } } },
  notify: {
    problem: {
      list: { title: 'Problems' },
      empty: 'No problem is open',
      column: { key: 'Problem', severity: 'Severity', subject: 'Subject', count: 'Seen' },
      severity: { warning: 'Warning', critical: 'Critical' },
      open: { true: 'Open', false: 'Cleared' },
    },
  },
};

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(DUMP, name), 'utf8')) as T;
}

function descriptor(func: string): Descriptor {
  return load<Descriptor>(`${func}.json`);
}

function withWidgets(boot: Boot, dashboard: BootWidget[]): Boot {
  return { ...boot, dashboard };
}

function cardFuncs(element: HTMLElement): (string | null)[] {
  return [...element.querySelectorAll('phx-widget-card')].map((card) =>
    card.getAttribute('data-widget-func'),
  );
}

function overlay(selector: string): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>(`.cdk-overlay-container ${selector}`)];
}

async function settle(harness: RouterTestingHarness): Promise<void> {
  await harness.fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  harness.detectChanges();
}

function problem(key: string, count: number): Problem {
  return {
    id: `01J9Q3W6B1R8N5X2K4T7M0V9P${count}`,
    key,
    subject: ACCOUNT,
    severity: 'warning',
    params: { count },
    accountId: ACCOUNT,
    source: 'poll',
    count,
    open: true,
    raisedAt: '2026-09-20T09:00:00Z',
    updatedAt: '2026-09-20T09:00:00Z',
  };
}

async function render(boot: Boot, client: KeelMockClient): Promise<RouterTestingHarness> {
  TestBed.configureTestingModule({
    providers: [...keelProviders(), { provide: KeelClient, useValue: client }],
  });
  const harness = await RouterTestingHarness.create();
  await harness.navigateByUrl('/');
  await settle(harness);
  await settle(harness);
  return harness;
}

describe('the home dashboard', () => {
  afterEach(() => {
    TestBed.inject(PhxToastService).closeAll();
    TestBed.resetTestingModule();
  });

  it('a boot with a list widget renders the home with one card showing the first rows and a link to the screen', async () => {
    const boot = withWidgets(
      KEEL_BOOT.one,
      KEEL_BOOT.one.dashboard.filter((widget) => widget.kind === 'list'),
    );
    expect(hasDashboard(boot)).toBe(true);
    const rows = [1, 2, 3, 4, 5, 6, 7].map((count) => problem(`demo.problem.${count}`, count));
    const client = new KeelMockClient({
      boot,
      descriptors: [load<Descriptor>(`${PROBLEM_LIST}.json`)],
      catalogue: { lang: 'en', messages: EN },
      answers: {
        [PROBLEM_LIST]: pageEnvelope(rows.slice(0, 5)),
        'keel.account.options': okEnvelope({ data: [], more: false }),
      },
    });
    const harness = await render(boot, client);
    const element: HTMLElement = harness.fixture.nativeElement;
    expect(TestBed.inject(Router).url).toBe('/');
    const home = element.querySelector('keel-home');
    expect(home?.getAttribute('data-widgets')).toBe('1');
    const cards = element.querySelectorAll('phx-widget-card');
    expect(cards).toHaveLength(1);
    expect(cards[0].querySelector('.title__text')?.textContent?.trim()).toBe('Problems');
    const link = cards[0].querySelector<HTMLAnchorElement>('a.open');
    expect(link?.getAttribute('href')).toBe(`/f/${PROBLEM_LIST}`);
    expect(link?.textContent?.trim()).toBe('Open');
    const list = cards[0].querySelector('keel-list');
    expect(list?.getAttribute('data-compact')).toBe('5');
    expect(list?.querySelector('phx-page-header')).toBeNull();
    expect(list?.querySelector('phx-pagination')).toBeNull();
    expect(list?.querySelectorAll('.body [role="row"]:not(.row_header)')).toHaveLength(5);
    const sizes = client
      .callsOf(PROBLEM_LIST)
      .map((call) => (call.params as { pageSize?: number }).pageSize);
    expect(sizes).toContain(5);
    expect(
      [
        ...(list?.querySelectorAll('.body [role="row"]:not(.row_header) [data-column="key"]') ??
          []),
      ].map((cell) => cell.textContent?.trim()),
    ).toEqual(rows.slice(0, 5).map((row) => row.key));

    link?.click();
    await harness.fixture.whenStable();
    expect(TestBed.inject(Router).url).toBe(`/f/${PROBLEM_LIST}`);
  });

  it('a boot without widgets redirects to the first menu item', async () => {
    const boot = withWidgets(KEEL_BOOT.none, []);
    expect(hasDashboard(boot)).toBe(false);
    const client = new KeelMockClient({
      boot,
      descriptors: [],
      catalogue: { lang: 'en', messages: EN },
    });
    const harness = await render(boot, client);
    expect(TestBed.inject(Router).url).toBe(`/f/${boot.menu[0].items[0].func}`);
    expect(harness.fixture.nativeElement.querySelector('keel-home')).toBeNull();
  });

  it('a_stat_widget_shows_its_value_and_sparkline', async () => {
    const boot = withWidgets(KEEL_BOOT.one, [{ kind: 'stat', func: ZONES_WIDGET }]);
    const points = [9, 10, 12].map((value, day) => ({
      at: `2026-09-2${day}T00:00:00Z`,
      value,
      min: value,
      max: value,
      sum: value,
      count: 1,
    }));
    const client = new KeelMockClient({
      boot,
      descriptors: [descriptor(ZONES_WIDGET)],
      catalogue: { lang: 'en', messages: EN },
      answers: {
        [ZONES_WIDGET]: okEnvelope({ value: 1234, unit: 'count', series: 'dns.zones.total' }),
        [STATS_SERIES]: okEnvelope({
          key: 'dns.zones.total',
          unit: 'count',
          resolution: 'hour',
          points,
        }),
      },
    });
    const harness = await render(boot, client);
    const element: HTMLElement = harness.fixture.nativeElement;
    const card = element.querySelector(`phx-widget-card[data-widget-func="${ZONES_WIDGET}"]`);
    expect(card?.getAttribute('data-widget-kind')).toBe('stat');
    expect(card?.querySelector('.title__text')?.textContent?.trim()).toBe('Zones');
    expect(card?.querySelector('phx-stat .value__number')?.textContent?.trim()).toBe('1,234');
    expect(card?.querySelector('phx-stat .label')?.textContent?.trim()).toBe('Total');
    expect(card?.querySelector('a.open')).toBeNull();
    const sparkline = card?.querySelector('keel-sparkline');
    expect(sparkline?.getAttribute('data-points')).toBe('3');
    expect(sparkline?.querySelector('svg')?.getAttribute('aria-label')).toBe('Last 7 days');
    expect(sparkline?.querySelector('polyline')?.getAttribute('points')).toBe(
      '0,32 50,21.33 100,0',
    );
    const asked = client.callsOf(STATS_SERIES).map((call) => call.params);
    expect(asked).toHaveLength(1);
    expect(asked[0]).toMatchObject({ key: 'dns.zones.total', resolution: 'hour' });
    const { from, to } = asked[0] as { from: string; to: string };
    expect(Date.parse(to) - Date.parse(from)).toBe(7 * 86_400_000);
  });

  it('the_widget_order_follows_the_preference_and_a_drag_writes_it', async () => {
    const boot = withWidgets(KEEL_BOOT.one, [
      { kind: 'stat', func: ZONES_WIDGET },
      { kind: 'list', func: PROBLEM_LIST, limit: 5 },
      { kind: 'stat', func: ACCOUNTS_WIDGET, hidden: true },
    ]);
    const client = new KeelMockClient({
      boot,
      descriptors: [
        descriptor(ZONES_WIDGET),
        descriptor(PROBLEM_LIST),
        descriptor(ACCOUNTS_WIDGET),
      ],
      catalogue: { lang: 'en', messages: EN },
      answers: {
        [ZONES_WIDGET]: okEnvelope({ value: 3, unit: 'count' }),
        [ACCOUNTS_WIDGET]: okEnvelope({ value: 2, unit: 'count' }),
        [PROBLEM_LIST]: pageEnvelope([]),
        'keel.account.options': okEnvelope({ data: [], more: false }),
        [PREF_SET]: okEnvelope({ key: 'dashboard.layout', value: {} }),
      },
    });
    const harness = await render(boot, client);
    const element: HTMLElement = harness.fixture.nativeElement;
    expect(cardFuncs(element)).toEqual([ZONES_WIDGET, PROBLEM_LIST]);
    expect(element.querySelector('keel-home')?.getAttribute('data-order')).toBe(
      `${ZONES_WIDGET} ${PROBLEM_LIST}`,
    );

    const grid = harness.fixture.debugElement.query(By.directive(PhxWidgetGridComponent));
    grid.injector
      .get(CdkDropList)
      .dropped.emit({ previousIndex: 0, currentIndex: 1 } as CdkDragDrop<unknown>);
    await settle(harness);

    expect(cardFuncs(element)).toEqual([PROBLEM_LIST, ZONES_WIDGET]);
    expect(client.callsOf(PREF_SET).map((call) => call.params)).toEqual([
      {
        key: 'dashboard.layout',
        value: {
          order: [PROBLEM_LIST, ZONES_WIDGET, ACCOUNTS_WIDGET],
          hidden: [ACCOUNTS_WIDGET],
        },
      },
    ]);
  });

  it('a hidden widget leaves the grid for the restore menu and returns to its place', async () => {
    const boot = withWidgets(KEEL_BOOT.one, [
      { kind: 'stat', func: ZONES_WIDGET },
      { kind: 'stat', func: ACCOUNTS_WIDGET },
    ]);
    const client = new KeelMockClient({
      boot,
      descriptors: [descriptor(ZONES_WIDGET), descriptor(ACCOUNTS_WIDGET)],
      catalogue: { lang: 'en', messages: EN },
      answers: {
        [ZONES_WIDGET]: okEnvelope({ value: 3, unit: 'count' }),
        [ACCOUNTS_WIDGET]: okEnvelope({ value: 2, unit: 'count' }),
        [PREF_SET]: okEnvelope({ key: 'dashboard.layout', value: {} }),
      },
    });
    const harness = await render(boot, client);
    const element: HTMLElement = harness.fixture.nativeElement;
    expect(element.querySelector('button[data-action="restore"]')).toBeNull();

    element
      .querySelector<HTMLButtonElement>(
        `phx-widget-card[data-widget-func="${ZONES_WIDGET}"] button[data-action="hide"]`,
      )
      ?.click();
    await settle(harness);
    expect(cardFuncs(element)).toEqual([ACCOUNTS_WIDGET]);

    element.querySelector<HTMLButtonElement>('button[data-action="restore"]')?.click();
    await settle(harness);
    const items = overlay('phx-dropdown-item');
    expect(items.map((item) => item.getAttribute('data-widget-func'))).toEqual([ZONES_WIDGET]);
    expect(items[0]?.textContent?.trim()).toBe('Zones');
    items[0]?.click();
    await settle(harness);

    expect(cardFuncs(element)).toEqual([ZONES_WIDGET, ACCOUNTS_WIDGET]);
    expect(element.querySelector('button[data-action="restore"]')).toBeNull();
    expect(client.callsOf(PREF_SET).map((call) => call.params)).toEqual([
      {
        key: 'dashboard.layout',
        value: { order: [ZONES_WIDGET, ACCOUNTS_WIDGET], hidden: [ZONES_WIDGET] },
      },
      {
        key: 'dashboard.layout',
        value: { order: [ZONES_WIDGET, ACCOUNTS_WIDGET], hidden: [] },
      },
    ]);
  });
});
