import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { ApplicationRef, signal, type Provider, type WritableSignal } from '@angular/core';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { Router, provideRouter } from '@angular/router';
import {
  KEEL_EVENT_SOURCE,
  KeelBoot,
  KeelClient,
  KeelEvents,
  KeelI18n,
} from '@globalart/keel/client';
import { OPTIONS_LIMIT } from '@globalart/keel/forms';
import type { Boot, Choices, Descriptor, Envelope, Event as KeelEvent } from '@globalart/keel/meta';
import {
  INVALIDATE_DEBOUNCE_MS,
  KEEL_SCREEN_DEPTH,
  KEEL_WORKSPACE,
  KeelActionRunner,
  KeelListComponent,
  maskMatches,
  scopeOf,
  screenParamsOf,
  searchOf,
  toColumn,
  usableZone,
  type KeelWorkspaceHost,
  type Row,
} from '@globalart/keel/renderer';
import { KeelMockClient, QuietEventSource } from '@globalart/keel/testing';
import { PhxTabsService, PhxToastService, providePhoenixIcons } from '@globalart/phoenix-ui';
import { EMPTY } from 'rxjs';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const LIST = load<Descriptor>('descriptor-dns-zone-list.json');
const BOOT = load<Boot>('boot.json');
const EVENT = load<KeelEvent>('event.json');
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

const ZONES: Row[] = [
  { id: 'z1', name: 'a.org', records: 3, status: 'active', dnssec: true, account: 'acme' },
  { id: 'z2', name: 'b.org', records: 5, status: 'suspended', dnssec: false, account: 'acme' },
];

const ACCOUNTS: Choices = {
  data: [
    { value: 'acme', label: 'ACME Ltd' },
    { value: 'globe', label: 'Globe Inc' },
  ],
  more: false,
};

const MESSAGES = {
  dns: {
    zone: {
      list: { title: 'Zones' },
      empty: 'No zones yet',
      column: {
        name: 'Name',
        records: 'Records',
        status: 'Status',
        dnssec: 'DNSSEC',
        account: 'Account',
      },
      status: { active: 'Active', suspended: 'Suspended' },
      action: {
        create: 'New zone',
        edit: 'Edit',
        records: 'Records',
        suspend: 'Suspend',
        delete: 'Delete',
        export: 'Export',
      },
    },
  },
  keel: {
    action: { create: 'New', delete: 'Delete', refresh: 'Refresh', export: 'Export' },
    confirm: {
      one: 'Continue with {action}?',
      many: 'Continue with {action} for {count} items?',
      count: 'Type {count} to confirm',
    },
    error: {
      forbidden: 'You are not allowed to do this',
      request: '{message} (request {id})',
      unavailable: 'Temporarily unavailable',
    },
    export: { format: { csv: 'CSV', json: 'JSON' } },
    list: {
      copyLink: 'Copy link',
      linkCopied: 'Link copied',
      linkFailed: 'The link could not be copied',
    },
    value: { true: 'Yes', false: 'No' },
  },
};

function source(): QuietEventSource {
  const last = QuietEventSource.last();
  if (last === null) {
    throw new Error('no event source was opened');
  }
  return last;
}

function audited(): Promise<void> {
  return new Promise((done) => setTimeout(done, INVALIDATE_DEBOUNCE_MS + 50));
}

function ok(data: unknown, requestId = 'req-ok'): Envelope {
  return { ok: true, data, meta: { requestId } };
}

function page(rows: readonly Row[], totals: Record<string, unknown> | null = null): Envelope {
  return ok({ data: rows, total: rows.length, totals });
}

function client(fixtures: { descriptors?: Descriptor[] } = {}): KeelMockClient {
  return new KeelMockClient({
    boot: BOOT,
    descriptors: fixtures.descriptors ?? [LIST],
    catalogue: { lang: 'en', messages: MESSAGES },
    answers: {
      'dns.zone.list': page(ZONES, { records: 8 }),
      'keel.account.options': ok(ACCOUNTS),
    },
  });
}

async function settle(fixture: ComponentFixture<unknown>): Promise<void> {
  await fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  fixture.detectChanges();
}

function workspaceOf(shown: WritableSignal<boolean>): KeelWorkspaceHost {
  return {
    activeId: () => null,
    visible: () => shown(),
    expectChild: () => undefined,
    retitle: () => undefined,
    refreshes: EMPTY,
    listState: () => null,
    saveListState: () => undefined,
  };
}

async function render(
  mock: KeelMockClient,
  descriptor: Descriptor = LIST,
  params?: Record<string, unknown>,
  extra: Provider[] = [],
): Promise<ComponentFixture<KeelListComponent>> {
  TestBed.configureTestingModule({
    imports: [KeelListComponent],
    providers: [
      providePhoenixIcons(),
      provideRouter([{ path: 'f/:func', children: [] }]),
      { provide: KeelClient, useValue: mock },
      {
        provide: KEEL_EVENT_SOURCE,
        useValue: (url: string, init: EventSourceInit) => new QuietEventSource(url, init),
      },
      ...extra,
    ],
  });
  await TestBed.inject(KeelBoot).load();
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelListComponent);
  fixture.componentRef.setInput('descriptor', descriptor);
  if (params !== undefined) {
    fixture.componentRef.setInput('params', params);
  }
  fixture.detectChanges();
  await settle(fixture);
  return fixture;
}

function refresh(fixture: ComponentFixture<unknown>): void {
  fixture.detectChanges();
  TestBed.inject(ApplicationRef).tick();
}

function overlay(selector: string): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>(`.cdk-overlay-container ${selector}`)];
}

function rows(element: HTMLElement): HTMLElement[] {
  return [...element.querySelectorAll<HTMLElement>('.body .row')];
}

function texts(nodes: Iterable<Element>): string[] {
  return [...nodes].map((node) => node.textContent?.trim() ?? '');
}

function must<T extends Element>(node: T | null | undefined, what: string): T {
  if (node === null || node === undefined) {
    throw new Error(`no ${what}`);
  }
  return node;
}

function withoutEdit(descriptor: Descriptor): Descriptor {
  const ui = descriptor.ui;
  if (!ui) {
    throw new Error('the descriptor has no ui');
  }
  return {
    ...descriptor,
    ui: {
      ...ui,
      columns: (ui.columns ?? []).map((column) =>
        column.key === 'dnssec' ? { ...column, edit: null } : column,
      ),
    },
  };
}

const NOTHING_SELECTED = ['create:on', 'edit:off', 'records:off', 'suspend:off', 'delete:off'];

function pill(element: HTMLElement): HTMLButtonElement[] {
  return [
    ...must(element.querySelector('phx-toolbar'), 'toolbar').querySelectorAll<HTMLButtonElement>(
      'button[data-action]',
    ),
  ];
}

function availability(element: HTMLElement): string[] {
  return pill(element).map(
    (button) => `${button.getAttribute('data-action')}:${button.disabled ? 'off' : 'on'}`,
  );
}

function recordsHiddenWhenSuspended(descriptor: Descriptor): Descriptor {
  const ui = descriptor.ui;
  const suspend = (ui?.actions ?? []).find((action) => action.id === 'suspend');
  if (!ui || suspend === undefined) {
    throw new Error('the descriptor has no suspend action');
  }
  return {
    ...descriptor,
    ui: {
      ...ui,
      actions: (ui.actions ?? []).map((action) =>
        action.id === 'records' ? { ...action, hidden_if: suspend.hidden_if } : action,
      ),
    },
  };
}

describe('the list renderer', () => {
  beforeEach(() => QuietEventSource.reset());

  afterEach(() => {
    TestBed.inject(PhxToastService).closeAll();
    TestBed.inject(KeelEvents).close();
    TestBed.resetTestingModule();
  });

  it('the header of a root list joins the tab bar only while the workspace is visible', async () => {
    const shown = signal(true);
    const fixture = await render(client(), LIST, undefined, [
      { provide: KEEL_SCREEN_DEPTH, useValue: 0 },
      { provide: KEEL_WORKSPACE, useValue: workspaceOf(shown) },
    ]);
    const header: HTMLElement = fixture.nativeElement.querySelector('phx-page-header');
    expect(header.classList.contains('attached')).toBe(true);
    shown.set(false);
    fixture.detectChanges();
    expect(header.classList.contains('attached')).toBe(false);
  });

  it('the header of a list nested under a screen never joins the tab bar', async () => {
    const fixture = await render(client(), LIST, undefined, [
      { provide: KEEL_SCREEN_DEPTH, useValue: 1 },
      { provide: KEEL_WORKSPACE, useValue: workspaceOf(signal(true)) },
    ]);
    const header: HTMLElement = fixture.nativeElement.querySelector('phx-page-header');
    expect(header.classList.contains('attached')).toBe(false);
  });

  it('the list renderer builds the table from the descriptor', async () => {
    const mock = client();
    const fixture = await render(mock);
    const element: HTMLElement = fixture.nativeElement;
    const list = fixture.componentInstance;

    expect(element.getAttribute('data-list')).toBe('dns.zone.list');
    expect(element.querySelector('phx-page-header .title__text')?.textContent).toBe('Zones');
    expect(availability(element)).toEqual(NOTHING_SELECTED);
    expect(mock.calls.map((call) => [call.func, call.params])).toEqual([
      ['keel.account.options', { limit: OPTIONS_LIMIT }],
      ['dns.zone.list', { page: 1, pageSize: 25 }],
    ]);

    expect(list.columns().map((column) => column.type)).toEqual([
      'link',
      'number',
      'status',
      'toggle',
      'text',
    ]);
    expect(list.columns().map((column) => [column.key, column.label])).toEqual([
      ['name', 'Name'],
      ['records', 'Records'],
      ['status', 'Status'],
      ['dnssec', 'DNSSEC'],
      ['account', 'Account'],
    ]);
    expect(
      list.columns().map((column) => [column.sortable ?? false, column.primary ?? false]),
    ).toEqual([
      [true, true],
      [true, false],
      [false, false],
      [false, false],
      [false, false],
    ]);
    expect(texts(element.querySelectorAll('.row_header [data-column] .sort'))).toEqual([
      'Name',
      'Records',
      'Status',
      'DNSSEC',
      'Account',
    ]);
    expect(element.querySelector('phx-searchbar')).not.toBeNull();

    expect(rows(element)).toHaveLength(2);
    expect(texts(element.querySelectorAll('.body .row [data-column="name"] a.link'))).toEqual([
      'a.org',
      'b.org',
    ]);
    expect(texts(element.querySelectorAll('.body .row [data-column="records"] .value'))).toEqual([
      '3',
      '5',
    ]);
    expect(texts(element.querySelectorAll('.body .row [data-column="status"] .value'))).toEqual([
      'Active',
      'Suspended',
    ]);
    expect(
      [...element.querySelectorAll('.body .row [data-column="status"] .status__dot')].map(
        (dot) => dot.className,
      ),
    ).toEqual(['status__dot status__dot_success', 'status__dot status__dot_warning']);
    expect(
      [
        ...element.querySelectorAll<HTMLInputElement>('.body .row [data-column="dnssec"] input'),
      ].map((input) => [input.checked, input.disabled]),
    ).toEqual([
      [true, false],
      [false, false],
    ]);
    expect(element.querySelector('[data-total="records"]')?.textContent).toBe('Records: 8');

    const status = list.columns()[2];
    expect(status.filterable).toBe(true);
    expect(status.filterOptions).toEqual([
      { value: 'active', label: 'Active' },
      { value: 'suspended', label: 'Suspended' },
    ]);
    const account = list.columns()[4];
    expect(account.filterable).toBe(true);
    expect(account.filterOptions).toEqual([
      { value: 'acme', label: 'ACME Ltd' },
      { value: 'globe', label: 'Globe Inc' },
    ]);
    expect(list.columns()[1].filterable).toBeUndefined();

    must(
      element.querySelector<HTMLButtonElement>('[data-column="status"] .cell_filter'),
      'status filter button',
    ).click();
    refresh(fixture);
    expect(texts(overlay('phx-column-filter phx-checkbox'))).toEqual(['Active', 'Suspended']);
    must(overlay('phx-column-filter phx-checkbox input')[1], 'suspended checkbox').click();
    refresh(fixture);
    must(overlay('phx-column-filter button[type="submit"]')[0], 'apply button').click();
    await settle(fixture);
    const filtered = mock.callsOf('dns.zone.list').at(-1);
    expect(filtered?.params).toEqual({
      page: 1,
      pageSize: 25,
      filters: [{ key: 'status', value: ['suspended'] }],
    });
  });

  it('a link column opens the target screen as an object tab and a toggle with edit calls its function', async () => {
    const mock = client().answer('dns.zone.update', ok({ id: 'z1', dnssec: false }));
    const fixture = await render(mock);
    const element: HTMLElement = fixture.nativeElement;
    const tabs = TestBed.inject(PhxTabsService);
    const router = TestBed.inject(Router);

    must(
      element.querySelector<HTMLAnchorElement>('.body .row [data-column="name"] a'),
      'link',
    ).click();
    await settle(fixture);
    expect(router.url).toBe('/f/dns.zone.edit?id=z1');
    expect(tabs.tabs()).toEqual([
      {
        id: '/f/dns.zone.edit?id=z1',
        label: 'a.org',
        data: { func: 'dns.zone.edit', params: { id: 'z1' } },
      },
    ]);
    expect(tabs.activeId()).toBe('/f/dns.zone.edit?id=z1');

    const listed = mock.callsOf('dns.zone.list').length;
    must(
      element.querySelector<HTMLInputElement>('.body .row [data-column="dnssec"] input'),
      'dnssec toggle',
    ).click();
    await settle(fixture);
    const update = mock.callsOf('dns.zone.update');
    expect(update).toHaveLength(1);
    expect(update[0].params).toEqual({ id: 'z1', dnssec: false });
    expect(update[0].options.idempotencyKey).toMatch(UUID);
    expect(mock.callsOf('dns.zone.list')).toHaveLength(listed + 1);
    expect(TestBed.inject(PhxToastService).toasts()).toHaveLength(0);
  });

  it('a toggle without edit is read-only', async () => {
    const mock = client();
    const fixture = await render(mock, withoutEdit(LIST));
    const element: HTMLElement = fixture.nativeElement;
    expect(fixture.componentInstance.readonlyToggles()).toEqual([
      { key: 'dnssec', label: 'DNSSEC' },
    ]);
    const toggles = [
      ...element.querySelectorAll<HTMLInputElement>('.body .row [data-column="dnssec"] input'),
    ];
    expect(toggles.map((input) => [input.checked, input.disabled])).toEqual([
      [true, true],
      [false, true],
    ]);
    toggles[0].click();
    await settle(fixture);
    expect(mock.calls.map((call) => call.func)).toEqual(['keel.account.options', 'dns.zone.list']);
  });

  it('row actions follow row_actions and hidden_if', async () => {
    const mock = client();
    const fixture = await render(mock);
    const element: HTMLElement = fixture.nativeElement;
    const list = fixture.componentInstance;

    expect(list.rowActions().map((action) => action.id)).toEqual([
      'edit',
      'records',
      'suspend',
      'delete',
    ]);
    const suspend = list.rowActions()[2];
    expect(suspend.hidden?.(ZONES[0])).toBe(false);
    expect(suspend.hidden?.(ZONES[1])).toBe(true);

    must(rows(element)[1].querySelector<HTMLButtonElement>('.kebab'), 'kebab').click();
    refresh(fixture);
    expect(texts(overlay('phx-dropdown-item'))).toEqual(['Edit', 'Records', 'Delete']);
    must(overlay('phx-dropdown-item')[1], 'records item').click();
    await settle(fixture);
    expect(TestBed.inject(Router).url).toBe('/f/dns.record.list?zone_id=z2');
    expect(TestBed.inject(PhxTabsService).tabs()).toEqual([]);

    must(rows(element)[0].querySelector<HTMLButtonElement>('.kebab'), 'kebab').click();
    refresh(fixture);
    expect(texts(overlay('phx-dropdown-item'))).toEqual(['Edit', 'Records', 'Suspend', 'Delete']);
    must(overlay('phx-dropdown-item')[2], 'suspend item').click();
    refresh(fixture);
    expect(overlay('phx-confirm')).toHaveLength(1);
    expect(overlay('phx-confirm .title')[0].textContent).toBe('Suspend');
    must(overlay('phx-confirm .buttons button')[1], 'cancel').click();
    await settle(fixture);
    expect(mock.callsOf('dns.zone.suspend')).toHaveLength(0);

    rows(element)[0].click();
    await settle(fixture);
    expect(TestBed.inject(Router).url).toBe('/f/dns.zone.edit?id=z1');
  });

  it('the toolbar pill enables the selection actions by the number of selected rows', async () => {
    const fixture = await render(client());
    const element: HTMLElement = fixture.nativeElement;

    expect(fixture.componentInstance.selectable()).toBe(true);
    expect(availability(element)).toEqual(NOTHING_SELECTED);

    must(
      rows(element)[0].querySelector<HTMLInputElement>('.cell_check input'),
      'first row',
    ).click();
    fixture.detectChanges();
    expect(availability(element)).toEqual([
      'create:on',
      'edit:on',
      'records:on',
      'suspend:on',
      'delete:on',
    ]);

    must(
      rows(element)[1].querySelector<HTMLInputElement>('.cell_check input'),
      'second row',
    ).click();
    fixture.detectChanges();
    expect(fixture.componentInstance.selection()).toEqual(new Set(['z1', 'z2']));
    expect(availability(element)).toEqual([
      'create:on',
      'edit:off',
      'records:off',
      'suspend:on',
      'delete:on',
    ]);
  });

  it('a sort chosen in the header sorts the fetch', async () => {
    const mock = client();
    const fixture = await render(mock);
    const element: HTMLElement = fixture.nativeElement;

    must(
      element.querySelector<HTMLButtonElement>('phx-sort-button button.button'),
      'sort button',
    ).click();
    await settle(fixture);
    refresh(fixture);
    must(overlay('.row__toggle')[0], 'name row').click();
    await settle(fixture);
    expect(fixture.componentInstance.tableSort()).toEqual({ key: 'name', dir: 'asc' });
    expect(mock.callsOf('dns.zone.list').at(-1)?.params).toEqual({
      page: 1,
      pageSize: 25,
      sort: { key: 'name', dir: 'asc' },
    });
  });

  it('a single action is off while the selected row hides it', async () => {
    const fixture = await render(
      client({ descriptors: [recordsHiddenWhenSuspended(LIST)] }),
      recordsHiddenWhenSuspended(LIST),
    );
    const element: HTMLElement = fixture.nativeElement;

    must(
      rows(element)[1].querySelector<HTMLInputElement>('.cell_check input'),
      'second row',
    ).click();
    fixture.detectChanges();
    expect(availability(element)).toContain('records:off');

    must(
      rows(element)[1].querySelector<HTMLInputElement>('.cell_check input'),
      'second row',
    ).click();
    must(
      rows(element)[0].querySelector<HTMLInputElement>('.cell_check input'),
      'first row',
    ).click();
    fixture.detectChanges();
    expect(availability(element)).toContain('records:on');
  });

  it('a delete from the toolbar pill hands the selected rows to the runner and reloads', async () => {
    const mock = client().answer('dns.zone.delete', ok({ deleted: 2 }));
    const fixture = await render(mock);
    const element: HTMLElement = fixture.nativeElement;
    const runner = TestBed.inject(KeelActionRunner);
    const run = jest.spyOn(runner, 'run');

    for (const row of rows(element)) {
      must(row.querySelector<HTMLInputElement>('.cell_check input'), 'row checkbox').click();
    }
    fixture.detectChanges();
    expect(fixture.componentInstance.selection()).toEqual(new Set(['z1', 'z2']));

    const listed = mock.callsOf('dns.zone.list').length;
    must(
      pill(element).find((button) => button.getAttribute('data-action') === 'delete'),
      'delete button',
    ).click();
    refresh(fixture);
    expect(run).toHaveBeenCalledTimes(1);
    expect(run.mock.calls[0][0]).toEqual({
      descriptor: LIST,
      action: LIST.ui?.actions?.find((action) => action.id === 'delete'),
      rows: ZONES,
    });
    expect(texts(overlay('phx-confirm .item'))).toEqual(['a.org', 'b.org']);
    const input = must(overlay('phx-confirm phx-input input')[0] as HTMLInputElement, 'count');
    input.value = '2';
    input.dispatchEvent(new Event('input'));
    refresh(fixture);
    must(overlay('phx-confirm .buttons button')[0], 'confirm').click();
    await settle(fixture);
    await settle(fixture);

    const deleted = mock.callsOf('dns.zone.delete');
    expect(deleted).toHaveLength(1);
    expect(deleted[0].params).toEqual({ ids: ['z1', 'z2'] });
    expect(mock.callsOf('dns.zone.list')).toHaveLength(listed + 1);
    expect(fixture.componentInstance.selection().size).toBe(0);
    expect(availability(element)).toEqual(NOTHING_SELECTED);
  });

  it('a burst of data.invalidate frames matching invalidate_on refetches the table once', async () => {
    const mock = client();
    const fixture = await render(mock);
    TestBed.inject(KeelEvents).subscribe();
    const stream = source();
    stream.open();
    const listed = mock.callsOf('dns.zone.list').length;

    for (let seq = 1; seq <= 5; seq += 1) {
      stream.emit('data.invalidate', { ...EVENT, seq, type: 'dns.zone.updated' }, String(seq));
    }
    stream.emit('data.invalidate', { ...EVENT, seq: 6, type: 'dns.record.created' }, '6');
    await settle(fixture);
    expect(mock.callsOf('dns.zone.list')).toHaveLength(listed);

    await audited();
    await settle(fixture);
    expect(mock.callsOf('dns.zone.list')).toHaveLength(listed + 1);
    expect(mock.callsOf('dns.zone.list').at(-1)?.params).toEqual({ page: 1, pageSize: 25 });
    expect(TestBed.inject(KeelEvents).state().lastEventId).toBe('6');

    stream.emit('data.invalidate', EVENT, String(EVENT.seq));
    await audited();
    await settle(fixture);
    expect(mock.callsOf('dns.zone.list')).toHaveLength(listed + 2);
  });

  it('a frame outside the masks refetches nothing', async () => {
    const mock = client();
    const fixture = await render(mock);
    TestBed.inject(KeelEvents).subscribe();
    const stream = source();
    stream.open();
    const listed = mock.callsOf('dns.zone.list').length;

    stream.emit('data.invalidate', { ...EVENT, type: 'dns.record.created' }, '1');
    stream.emit('data.invalidate', { ...EVENT, type: 'dns' }, '2');
    stream.emit('data.invalidate', { ...EVENT, type: 'dns.zones.created' }, '3');
    stream.emit('task.progress', { id: 't', function: 'dns.zone.import', state: 'done', done: 1 });
    stream.emit('notification', { id: 'n', key: 'k', level: 'info', createdAt: 'now' });
    await audited();
    await settle(fixture);
    expect(mock.callsOf('dns.zone.list')).toHaveLength(listed);
    expect(TestBed.inject(KeelEvents).state().lastEventId).toBe('3');
  });

  it('the export action is hidden while keel.export is absent', async () => {
    const fixture = await render(client());
    const element: HTMLElement = fixture.nativeElement;
    expect(fixture.componentInstance.exportable()).toBe(false);
    expect(element.querySelector('[data-action="export"]')).toBeNull();
    TestBed.resetTestingModule();

    const exporter: Descriptor = {
      ...LIST,
      name: 'keel.export',
      module: 'keel',
      kind: 'action',
      mode: 'read',
      key: null,
      ui: null,
    };
    const again = await render(client({ descriptors: [LIST, exporter] }));
    await settle(again);
    expect(again.componentInstance.exportable()).toBe(true);
    must(
      (again.nativeElement as HTMLElement).querySelector<HTMLButtonElement>(
        'phx-page-header .actions > button',
      ),
      'more button',
    ).click();
    await settle(again);
    expect(texts(overlay('phx-dropdown-item'))).toEqual(['CSV', 'JSON', 'Refresh', 'Copy link']);
  });

  it('the round search button sends its text to the fetch after the debounce', async () => {
    const mock = client();
    const fixture = await render(mock);
    const element: HTMLElement = fixture.nativeElement;

    must(element.querySelector<HTMLButtonElement>('phx-searchbar button'), 'search button').click();
    await settle(fixture);
    const input = must(
      element.querySelector<HTMLInputElement>('phx-searchbar input'),
      'search input',
    );
    input.value = 'b.org';
    input.dispatchEvent(new Event('input'));
    await new Promise((done) => setTimeout(done, 350));
    await settle(fixture);
    expect(mock.callsOf('dns.zone.list').at(-1)?.params).toEqual({
      page: 1,
      pageSize: 25,
      search: 'b.org',
    });
  });

  it('a search screen parameter fills the search box and stays out of the scope', async () => {
    const mock = client();
    const fixture = await render(mock, LIST, { search: 'a.org', view: 'mine' });
    const element: HTMLElement = fixture.nativeElement;
    expect(fixture.componentInstance.scope()).toEqual({ view: 'mine' });
    expect(fixture.componentInstance.search()).toBe('a.org');
    expect(element.querySelector<HTMLInputElement>('phx-searchbar input')?.value).toBe('a.org');
    expect(mock.callsOf('dns.zone.list').map((call) => call.params)).toEqual([
      { page: 1, pageSize: 25, search: 'a.org', scope: { view: 'mine' } },
    ]);
    expect(mock.callsOf('keel.account.options').map((call) => call.params)).toEqual([
      { scope: { view: 'mine' }, limit: OPTIONS_LIMIT },
    ]);
    expect(scopeOf({ search: 'x', id: 'z1' })).toEqual({ id: 'z1' });
    expect(searchOf({ search: ['a', 'b'] })).toBe('');

    fixture.componentRef.setInput('params', { search: 'b.org', view: 'mine' });
    await settle(fixture);
    expect(element.querySelector<HTMLInputElement>('phx-searchbar input')?.value).toBe('b.org');
    expect(mock.callsOf('dns.zone.list').at(-1)?.params).toEqual({
      page: 1,
      pageSize: 25,
      search: 'b.org',
      scope: { view: 'mine' },
    });
  });
});

describe('the column mapping', () => {
  const MESSAGES_ONLY = { keel: { value: { true: 'Yes', false: 'No' } } };
  const scope = {
    messages: MESSAGES_ONLY,
    zone: 'Europe/Budapest',
    locale: 'en',
    choices: () => null,
  };

  it('date and datetime become date columns in UTC and in the zone of the principal', () => {
    const day = toColumn(LIST, { key: 'due', type: 'date' }, scope);
    expect(day.type).toBe('date');
    expect(day.dateFormat).toEqual({ dateStyle: 'medium', timeZone: 'UTC' });
    const moment = toColumn(LIST, { key: 'updatedAt', type: 'datetime' }, scope);
    expect(moment.type).toBe('date');
    expect(moment.locale).toBe('en');
    expect(moment.dateFormat).toEqual({
      dateStyle: 'medium',
      timeStyle: 'short',
      timeZone: 'Europe/Budapest',
    });
    expect(
      toColumn(LIST, { key: 'at', type: 'datetime' }, { ...scope, zone: 'Mars/Olympus' })
        .dateFormat,
    ).toEqual({ dateStyle: 'medium', timeStyle: 'short', timeZone: 'UTC' });
    expect(usableZone('Mars/Olympus')).toBe('UTC');
  });

  it('a boolean becomes an icon column with the shared yes and no labels', () => {
    const flag = toColumn(LIST, { key: 'signed', type: 'boolean' }, scope);
    expect(flag.type).toBe('icon');
    expect(flag.value?.({ signed: true })).toEqual({
      icon: 'check',
      color: 'success',
      label: 'Yes',
    });
    expect(flag.value?.({ signed: false })).toEqual({ icon: 'minus', color: 'muted', label: 'No' });
    expect(flag.value?.({ signed: null })).toBeNull();
  });

  it('a status value outside the map falls back to the default tone and the raw value', () => {
    const status = toColumn(
      LIST,
      { key: 'status', type: 'status', map: { active: 'success', gone: 'neutral' } },
      scope,
    );
    expect(status.value?.({ status: 'active' })).toEqual({ status: 'success', label: 'active' });
    expect(status.value?.({ status: 'gone' })).toEqual({ status: 'default', label: 'gone' });
    expect(status.value?.({ status: 'odd' })).toEqual({ status: 'default', label: 'odd' });
    expect(status.value?.({ status: null })).toEqual({ status: 'default', label: '' });
  });
});

describe('the list helpers', () => {
  it('an event mask matches like the scope masks of the server', () => {
    expect(maskMatches('dns.zone.*', 'dns.zone.created')).toBe(true);
    expect(maskMatches('dns.zone.*', 'dns.zone.record.created')).toBe(true);
    expect(maskMatches('dns.zone.*', 'dns.zone.')).toBe(false);
    expect(maskMatches('dns.zone.*', 'dns.zone')).toBe(false);
    expect(maskMatches('dns.zone.*', 'dns.zones.created')).toBe(false);
    expect(maskMatches('dns.zone.created', 'dns.zone.created')).toBe(true);
    expect(maskMatches('dns.zone.created', 'dns.zone.deleted')).toBe(false);
  });

  it('a row action with a screen addresses the row by the key unless params say otherwise', () => {
    const edit = { id: 'edit', role: 'edit' as const, screen: 'dns.zone.edit' };
    expect(screenParamsOf(LIST, edit, ZONES[1])).toEqual({ id: 'z2' });
    expect(screenParamsOf(LIST, edit)).toEqual({});
    const child = {
      id: 'records',
      role: 'child' as const,
      screen: 'dns.record.list',
      params: { zone_id: '$id', view: 'all' },
    };
    expect(screenParamsOf(LIST, child, ZONES[0])).toEqual({ zone_id: 'z1', view: 'all' });
    expect(screenParamsOf(LIST, child)).toEqual({ zone_id: '$id', view: 'all' });
  });
});
