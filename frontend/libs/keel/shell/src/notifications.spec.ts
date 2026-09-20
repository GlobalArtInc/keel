import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed } from '@angular/core/testing';
import { Router } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import {
  KEEL_EVENT_SOURCE,
  KeelClient,
  KeelEvents,
  KeelNotifications,
  KeelProblems,
  MESSAGE_CLEAR,
  MESSAGE_LIST,
  MESSAGE_MARK,
  PROBLEM_LIST,
  type Note,
  type Problem,
} from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope, Messages } from '@globalart/keel/meta';
import { keelProviders } from '@globalart/keel/shell';
import { KeelMockClient, QuietEventSource, pageEnvelope } from '@globalart/keel/testing';
import { PhxDrawerService, PhxToastService } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const RAISED = '01J9Q3W6B1R8N5X2K4T7M0V9N1';
const NOTICE = '01J9Q3W6B1R8N5X2K4T7M0V9N2';
const LIVE = '01J9Q3W6B1R8N5X2K4T7M0V9N3';
const PROBLEM = '01J9Q3W6B1R8N5X2K4T7M0V9P1';
const ACCOUNT = '01J8A0000000000000000000A2';
const OTHER_PROBLEM = '01J9Q3W6B1R8N5X2K4T7M0V9P2';
const OTHER_ACCOUNT = '01J8A0000000000000000000A3';

const EN: Messages = {
  menu: { system: 'System', dns: 'DNS', hosting: 'Hosting' },
  product: { dns: { brand: 'KeelDNS' }, panel: { brand: 'KeelPanel' } },
  dns: { zone: { list: { title: 'Zones' } } },
  hosting: { site: { list: { title: 'Sites' } } },
  keel: {
    brand: 'Keel',
    level: { reseller: 'Reseller' },
    account: { list: { title: 'Accounts' } },
    error: { request: '{message} (request {id})', forbidden: 'Not allowed' },
    shell: {
      rail: { notifications: 'Notifications', tasks: 'Tasks', help: 'Help' },
      notifications: {
        title: 'Notifications',
        unread: 'unread',
        dismiss: 'Dismiss',
        clear: 'Clear read notifications',
        problems: 'Open problems',
        no_problems: 'No problem is open',
        seen: 'Seen {count} times',
      },
      user: { language: 'Language', theme: 'Theme' },
    },
  },
  notify: {
    message: { level: { info: 'Info', warning: 'Warning', error: 'Error' } },
    problem: {
      raised: 'Problem {key} raised for {subject}',
      severity: { warning: 'Warning', critical: 'Critical' },
    },
  },
  dns_zone_expiring: 'The zone {name} expires soon',
  phx: {
    noNotifications: 'No notifications',
    markAllRead: 'Mark all as read',
    yesterday: 'Yesterday',
  },
};

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

function descriptor(name: string, kind: Descriptor['kind']): Descriptor {
  return {
    keel: 1,
    name,
    module: 'notify',
    kind,
    mode: kind === 'action' ? 'write' : 'read',
    access: { level: 'user' },
    params: {},
    result: {},
  };
}

function problem(overrides: Partial<Problem> = {}): Problem {
  return {
    id: PROBLEM,
    key: 'dns.zone.expiring',
    subject: ACCOUNT,
    severity: 'warning',
    params: { count: 2 },
    accountId: ACCOUNT,
    source: 'poll',
    count: 2,
    open: true,
    raisedAt: '2026-09-20T09:00:00Z',
    updatedAt: '2026-09-20T09:00:00Z',
    ...overrides,
  };
}

function note(id: string, key: string, unread: boolean, params: unknown = null): Note {
  return {
    id,
    key,
    params,
    level: 'warning',
    source: 'problem',
    problem: null,
    unread,
    createdAt: '2026-09-20T10:00:00Z',
  };
}

function ok<T>(data: T): Envelope<T> {
  return { ok: true, data, meta: { requestId: 'req-notify' } };
}

interface Rendered {
  harness: RouterTestingHarness;
  element: HTMLElement;
  stream: QuietEventSource;
}

async function render(client: KeelMockClient, url = '/f/keel.account.list'): Promise<Rendered> {
  TestBed.configureTestingModule({
    providers: [
      ...keelProviders(),
      { provide: KeelClient, useValue: client },
      {
        provide: KEEL_EVENT_SOURCE,
        useValue: (url: string, init: EventSourceInit) => new QuietEventSource(url, init),
      },
    ],
  });
  const harness = await RouterTestingHarness.create();
  await harness.navigateByUrl(url);
  TestBed.inject(KeelEvents).subscribe();
  await settle(harness);
  const stream = QuietEventSource.last();
  if (stream === null) {
    throw new Error('the event stream did not open');
  }
  stream.open();
  return { harness, element: harness.fixture.nativeElement, stream };
}

async function settle(harness: RouterTestingHarness): Promise<void> {
  await harness.fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  harness.detectChanges();
}

function centre(element: HTMLElement): HTMLElement | null {
  return element.ownerDocument.querySelector<HTMLElement>('keel-notification-centre');
}

function badge(element: HTMLElement): string | null {
  return (
    element.querySelector('phx-rail-item[data-rail="notifications"] .badge')?.textContent?.trim() ??
    null
  );
}

function rowsOf(open: HTMLElement | null): string[] {
  return [...(open?.querySelectorAll('phx-notification-center .row') ?? [])].map(
    (row) =>
      `${row.classList.contains('row_unread') ? '*' : ''}${row.querySelector('.row__text')?.textContent?.trim() ?? ''}`,
  );
}

function client(descriptors: Descriptor[], boot = load<Boot>('boot.json')): KeelMockClient {
  return new KeelMockClient({
    boot,
    descriptors,
    catalogue: { lang: 'en', messages: EN },
    answers: {
      [MESSAGE_LIST]: pageEnvelope([
        note(RAISED, 'notify.problem.raised', true, problem()),
        note(NOTICE, 'dns_zone_expiring', false, { name: 'a.org' }),
      ]),
      [PROBLEM_LIST]: pageEnvelope([problem()]),
    },
  });
}

const ALL = [
  descriptor(MESSAGE_LIST, 'list'),
  descriptor(MESSAGE_MARK, 'action'),
  descriptor(MESSAGE_CLEAR, 'action'),
  descriptor(PROBLEM_LIST, 'list'),
];

describe('the notification centre', () => {
  beforeEach(() => QuietEventSource.reset());

  afterEach(() => {
    TestBed.inject(PhxDrawerService).closeAll();
    TestBed.inject(PhxToastService).closeAll();
    TestBed.resetTestingModule();
  });

  it('the centre lists messages, marks read, clears, and the badge counts unread', async () => {
    const mock = client(ALL);
    mock.answer(MESSAGE_MARK, ok({ marked: 1 }));
    mock.answer(MESSAGE_CLEAR, ok({ deleted: 1 }));
    const { harness, element } = await render(mock);
    expect(badge(element)).toBe('1');
    expect(mock.callsOf(MESSAGE_LIST)).toHaveLength(1);

    element.querySelector<HTMLElement>('phx-rail-item[data-rail="notifications"] button')?.click();
    await settle(harness);
    const open = centre(element);
    expect(mock.callsOf(MESSAGE_LIST)).toHaveLength(2);
    expect(rowsOf(open)).toEqual([
      `*Problem dns.zone.expiring raised for ${ACCOUNT}`,
      'The zone a.org expires soon',
    ]);
    expect(open?.querySelector('[data-problems] [data-problem]')?.textContent).toContain(
      'dns.zone.expiring',
    );
    expect(open?.querySelector('[data-problems] .problem__count')?.textContent).toBe(
      'Seen 2 times',
    );

    const header = (label: string): HTMLButtonElement | undefined =>
      [...(open?.querySelectorAll<HTMLButtonElement>('.header__actions button') ?? [])].find(
        (button) => button.getAttribute('aria-label') === label,
      );
    header('Mark all as read')?.click();
    await settle(harness);
    expect(mock.callsOf(MESSAGE_MARK).map((call) => call.params)).toEqual([{ ids: [RAISED] }]);
    expect(rowsOf(open)).toEqual([
      `Problem dns.zone.expiring raised for ${ACCOUNT}`,
      'The zone a.org expires soon',
    ]);
    expect(badge(element)).toBeNull();

    header('Clear read notifications')?.click();
    await settle(harness);
    expect(mock.callsOf(MESSAGE_CLEAR)).toHaveLength(1);
    expect(rowsOf(open)).toEqual([]);

    open?.querySelector<HTMLElement>('[data-problems] [data-problem]')?.click();
    await settle(harness);
    expect(TestBed.inject(Router).url).toBe(`/f/${PROBLEM_LIST}?search=dns.zone.expiring`);
  });

  it('the rail item toggles the centre: a second press closes it and a third opens it again', async () => {
    const mock = client(ALL);
    const { harness, element } = await render(mock);
    const item = (): HTMLElement | null =>
      element.querySelector<HTMLElement>('phx-rail-item[data-rail="notifications"] button');
    item()?.click();
    await settle(harness);
    expect(centre(element)).not.toBeNull();
    item()?.click();
    await settle(harness);
    await new Promise((done) => setTimeout(done, 350));
    expect(centre(element)).toBeNull();
    item()?.click();
    await settle(harness);
    expect(element.ownerDocument.querySelectorAll('keel-notification-centre')).toHaveLength(1);
  });

  it('a notification frame prepends a message', async () => {
    const mock = client(ALL);
    const { harness, element, stream } = await render(mock);
    element.querySelector<HTMLElement>('phx-rail-item[data-rail="notifications"] button')?.click();
    await settle(harness);
    stream.emit('notification', {
      id: LIVE,
      key: 'dns_zone_expiring',
      params: { name: 'b.org' },
      level: 'error',
      createdAt: '2026-09-20T11:00:00Z',
    });
    await settle(harness);
    expect(rowsOf(centre(element))).toEqual([
      '*The zone b.org expires soon',
      `*Problem dns.zone.expiring raised for ${ACCOUNT}`,
      'The zone a.org expires soon',
    ]);
    expect(badge(element)).toBe('2');
    expect(TestBed.inject(KeelNotifications).messages()[0].id).toBe(LIVE);
  });

  it('open problems fill the sidebar badges by key for a principal who may list them and nothing for one who may not', async () => {
    const boot = load<Boot>('boot.json');
    boot.menu[0].items[0].badge = { problem: 'dns.zone.expiring' };
    const listing = client(ALL, boot);
    listing.answer(
      PROBLEM_LIST,
      pageEnvelope([
        problem({ count: 10 }),
        problem({ id: OTHER_PROBLEM, subject: OTHER_ACCOUNT }),
      ]),
    );
    const { element, harness, stream } = await render(listing);
    const item = (): string | null =>
      element.querySelector('phx-sidebar-item[data-func="dns.zone.list"] .badge')?.textContent ??
      null;
    expect(item()).toBe('2');
    expect(TestBed.inject(KeelProblems).open().get('dns.zone.expiring')).toBe(2);

    listing.answer(PROBLEM_LIST, pageEnvelope([]));
    stream.emit('data.invalidate', {
      id: 'e1',
      seq: 1,
      type: 'notify.problem.cleared',
      at: '2026-09-20T12:00:00Z',
      instanceId: 'i',
      module: 'notify',
    });
    await new Promise((done) => setTimeout(done, 300));
    await settle(harness);
    expect(item()).toBeNull();
    expect(listing.callsOf(PROBLEM_LIST)).toHaveLength(2);
    TestBed.resetTestingModule();

    const blind = client([descriptor(MESSAGE_LIST, 'list')], boot);
    const other = await render(blind);
    expect(
      other.element.querySelector('phx-sidebar-item[data-func="dns.zone.list"] .badge'),
    ).toBeNull();
    expect(blind.callsOf(PROBLEM_LIST)).toHaveLength(0);
    other.element
      .querySelector<HTMLElement>('phx-rail-item[data-rail="notifications"] button')
      ?.click();
    await settle(other.harness);
    expect(centre(other.element)?.querySelector('[data-problems]')).toBeNull();
  });
});
