import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed } from '@angular/core/testing';
import { Router } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { KeelClient, PROBLEM_LIST, type Problem } from '@globalart/keel/client';
import type { Boot, Descriptor, Messages } from '@globalart/keel/meta';
import { hasDashboard, keelProviders } from '@globalart/keel/shell';
import { KEEL_BOOT, KeelMockClient, okEnvelope, pageEnvelope } from '@globalart/keel/testing';
import { PhxToastService } from '@globalart/phoenix-ui';

const DUMP = resolve(__dirname, '../../../../../contracts/dump/admin/functions');
const ACCOUNT = '01J8A0000000000000000000A2';

const EN: Messages = {
  menu: { system: 'System', demo: 'Demo' },
  product: { demo: { brand: 'Keel Demo' } },
  keel: {
    brand: 'Keel',
    level: { admin: 'Administrator' },
    shell: {
      rail: { notifications: 'Notifications', tasks: 'Tasks', help: 'Help' },
      home: { title: 'Overview', open: 'Open' },
      user: { language: 'Language', theme: 'Theme' },
    },
  },
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
  await harness.fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  harness.detectChanges();
  return harness;
}

describe('the home dashboard', () => {
  afterEach(() => {
    TestBed.inject(PhxToastService).closeAll();
    TestBed.resetTestingModule();
  });

  it('a boot with a list widget renders the home with one card showing the first rows and a link to the screen', async () => {
    const boot = KEEL_BOOT.one;
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
    const boot = KEEL_BOOT.none;
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
});
