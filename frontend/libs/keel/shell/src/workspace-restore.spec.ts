import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed } from '@angular/core/testing';
import { By } from '@angular/platform-browser';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { KEEL_EVENT_SOURCE, KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope } from '@globalart/keel/meta';
import { KEEL_WORKSPACE, KeelListComponent, KeelScreenComponent } from '@globalart/keel/renderer';
import { KeelMockClient, QuietEventSource } from '@globalart/keel/testing';
import { PhxTabsService, providePhoenixIcons } from '@globalart/phoenix-ui';
import { KeelWorkspace } from './workspace/workspace';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const BOOT = load<Boot>('boot.json');
const LIST = load<Descriptor>('descriptor-dns-zone-list.json');
const ZONES = [
  { id: 'z1', name: 'a.org', records: 3, status: 'active', dnssec: true, account: 'acme' },
];
const ROUTES = [
  { path: 'f/:func', component: KeelScreenComponent },
  { path: 'other', children: [] },
];

function ok(data: unknown): Envelope {
  return { ok: true, data, meta: { requestId: 'req-ok' } };
}

function must<T>(value: T | null | undefined, what: string): T {
  if (value === null || value === undefined) {
    throw new Error(`no ${what}`);
  }
  return value;
}

function settle(harness: RouterTestingHarness): Promise<void> {
  return new Promise((done) => setTimeout(done, 0)).then(() => {
    harness.fixture.detectChanges();
  });
}

describe('a reopened tab of the workspace', () => {
  beforeEach(() => {
    localStorage.clear();
    QuietEventSource.reset();
  });

  afterEach(() => {
    TestBed.resetTestingModule();
    localStorage.clear();
  });

  it('a list opened again in its tab takes the search that the tab saved', async () => {
    const mock = new KeelMockClient({
      boot: BOOT,
      descriptors: [LIST],
      catalogue: { lang: 'en', messages: {} },
      answers: {
        'dns.zone.list': ok({ data: ZONES, total: 60, totals: null }),
        'keel.account.options': ok({ data: [], more: false }),
      },
    });
    TestBed.configureTestingModule({
      providers: [
        providePhoenixIcons(),
        provideRouter(ROUTES, withComponentInputBinding()),
        { provide: KeelClient, useValue: mock },
        {
          provide: KEEL_EVENT_SOURCE,
          useValue: (url: string, init: EventSourceInit) => new QuietEventSource(url, init),
        },
        { provide: KEEL_WORKSPACE, useExisting: KeelWorkspace },
      ],
    });
    await TestBed.inject(KeelBoot).load();
    await TestBed.inject(KeelI18n).load('en');
    const workspace = TestBed.inject(KeelWorkspace);
    const harness = await RouterTestingHarness.create();

    await harness.navigateByUrl('/f/dns.zone.list');
    await settle(harness);
    const tab = must(TestBed.inject(PhxTabsService).activeId(), 'active tab');
    workspace.saveListState(tab, { search: 'a.org', sort: null, filters: [], page: 1 });

    await harness.navigateByUrl('/other');
    await harness.navigateByUrl('/f/dns.zone.list');
    await settle(harness);

    const list = harness.fixture.debugElement.query(By.directive(KeelListComponent))
      .componentInstance as KeelListComponent;
    expect(list.tableSearch()).toBe('a.org');
    expect(mock.callsOf('dns.zone.list').at(-1)?.params).toMatchObject({ search: 'a.org' });
  });
});
