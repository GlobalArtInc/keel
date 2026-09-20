import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { ApplicationRef } from '@angular/core';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { KEEL_EVENT_SOURCE, KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import type { Boot, Column, Descriptor, Envelope } from '@globalart/keel/meta';
import {
  KeelListComponent,
  PREF_GET,
  PREF_SET,
  cellEditOf,
  columnsKeyOf,
  orderOf,
  reordered,
  type Row,
} from '@globalart/keel/renderer';
import { KeelMockClient, QuietEventSource } from '@globalart/keel/testing';
import { PhxToastService, providePhoenixIcons } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const LIST = load<Descriptor>('descriptor-dns-zone-list.json');
const BOOT = load<Boot>('boot.json');
const PREFS: Descriptor[] = [PREF_GET, PREF_SET].map((name) => ({
  ...LIST,
  name,
  module: 'settings',
  kind: name === PREF_GET ? 'get' : 'form',
  mode: name === PREF_GET ? 'read' : 'write',
  key: null,
  ui: null,
}));

const ZONES: Row[] = [
  { id: 'z1', name: 'a.org', records: 3, status: 'active', dnssec: true, account: 'acme' },
];

const MESSAGES = {
  dns: {
    zone: {
      list: { title: 'Zones' },
      column: { name: 'Name', records: 'Records', status: 'Status', dnssec: 'DNSSEC' },
      status: { active: 'Active', suspended: 'Suspended' },
      action: { create: 'New zone', export: 'Export' },
    },
  },
  keel: { error: { request: '{message} (request {id})' } },
  phx: { ok: 'OK', cancel: 'Cancel' },
};

function ok(data: unknown): Envelope {
  return { ok: true, data, meta: { requestId: 'req-ok' } };
}

function withStatusEdit(descriptor: Descriptor): Descriptor {
  const ui = descriptor.ui;
  if (!ui) {
    throw new Error('the descriptor has no ui');
  }
  return {
    ...descriptor,
    ui: {
      ...ui,
      columns: (ui.columns ?? []).map((column) =>
        column.key === 'status' ? { ...column, edit: { function: 'dns.zone.update' } } : column,
      ),
    },
  };
}

function client(descriptors: Descriptor[]): KeelMockClient {
  return new KeelMockClient({
    boot: BOOT,
    descriptors,
    catalogue: { lang: 'en', messages: MESSAGES },
    answers: {
      'dns.zone.list': ok({ data: ZONES, total: ZONES.length }),
      'keel.account.options': ok({ data: [], more: false }),
    },
  });
}

async function settle(fixture: ComponentFixture<unknown>): Promise<void> {
  await fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  fixture.detectChanges();
  TestBed.inject(ApplicationRef).tick();
}

async function render(
  mock: KeelMockClient,
  descriptor: Descriptor,
): Promise<ComponentFixture<KeelListComponent>> {
  TestBed.configureTestingModule({
    imports: [KeelListComponent],
    providers: [
      providePhoenixIcons(),
      provideRouter([]),
      { provide: KeelClient, useValue: mock },
      {
        provide: KEEL_EVENT_SOURCE,
        useValue: (url: string, init: EventSourceInit) => new QuietEventSource(url, init),
      },
    ],
  });
  await TestBed.inject(KeelBoot).load();
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelListComponent);
  fixture.componentRef.setInput('descriptor', descriptor);
  fixture.detectChanges();
  await settle(fixture);
  await settle(fixture);
  return fixture;
}

function headers(element: HTMLElement): string[] {
  return [...element.querySelectorAll<HTMLElement>('[role="columnheader"][data-column]')].map(
    (header) => header.dataset['column'] ?? '',
  );
}

function must<T>(value: T | null | undefined, what: string): T {
  if (value === null || value === undefined) {
    throw new Error(`no ${what}`);
  }
  return value;
}

describe('column order and cell edit', () => {
  beforeEach(() => QuietEventSource.reset());

  afterEach(() => TestBed.inject(PhxToastService).closeAll());

  it('the column order comes from columns.<function> and a reorder writes it back', async () => {
    const mock = client([LIST, ...PREFS])
      .answer(
        PREF_GET,
        ok({ key: columnsKeyOf('dns.zone.list'), value: { order: ['status', 'name', 'gone'] } }),
      )
      .answer(
        PREF_SET,
        ok({ key: columnsKeyOf('dns.zone.list'), value: { order: ['name', 'status'] } }),
      );
    const fixture = await render(mock, LIST);
    const element: HTMLElement = fixture.nativeElement;
    expect(mock.callsOf(PREF_GET).map((call) => call.params)).toEqual([
      { key: 'columns.dns.zone.list' },
    ]);
    expect(fixture.componentInstance.order()).toEqual(['status', 'name', 'gone']);
    expect(headers(element)).toEqual(['status', 'name']);
    expect(fixture.componentInstance.visibleColumns()).toEqual(['status', 'name']);

    const table = must(element.querySelector('phx-data-table'), 'table');
    const order = table.querySelector('phx-list-table');
    expect(order).not.toBeNull();
    fixture.componentInstance['reorder'](['name', 'status']);
    await settle(fixture);
    expect(headers(element)).toEqual(['name', 'status']);
    expect(mock.callsOf(PREF_SET).map((call) => call.params)).toEqual([
      { key: 'columns.dns.zone.list', value: { order: ['name', 'status'] } },
    ]);
    TestBed.resetTestingModule();

    const bare = client([LIST]);
    const plain = await render(bare, LIST);
    expect(bare.callsOf(PREF_GET)).toHaveLength(0);
    expect(plain.componentInstance.order()).toBeNull();
    expect(headers(plain.nativeElement as HTMLElement)).toEqual([
      'name',
      'records',
      'status',
      'dnssec',
      'account',
    ]);
    expect(orderOf({ order: ['a', 1] })).toBeNull();
    expect(orderOf('x')).toBeNull();
    expect(reordered(['a', 'b'], ['b', 'c', 'a'])).toEqual(['b', 'a']);
  });

  it('a select column with edit opens the cell editor with its options and posts the function', async () => {
    const descriptor = withStatusEdit(LIST);
    const mock = client([descriptor]).answer(
      'dns.zone.update',
      ok({ id: 'z1', status: 'suspended' }),
    );
    const fixture = await render(mock, descriptor);
    const element: HTMLElement = fixture.nativeElement;
    const status = must(
      (descriptor.ui?.columns ?? []).find((column) => column.key === 'status'),
      'status column',
    );
    expect(cellEditOf(status, 'dns.zone', MESSAGES)).toEqual({
      options: [
        { value: 'active', label: 'Active' },
        { value: 'suspended', label: 'Suspended' },
      ],
    });
    const records: Column = {
      key: 'records',
      type: 'number',
      edit: { function: 'dns.zone.update' },
    };
    expect(cellEditOf(records, 'dns.zone', MESSAGES)).toEqual({});
    const name: Column = { key: 'name', type: 'link', edit: { function: 'dns.zone.update' } };
    expect(cellEditOf(name, 'dns.zone', MESSAGES)).toBeNull();

    const listed = mock.callsOf('dns.zone.list').length;
    must(
      element.querySelector<HTMLButtonElement>('.body .row [data-column="status"] button.edit'),
      'edit button',
    ).click();
    await settle(fixture);
    const popup = must(
      document.querySelector<HTMLElement>('.cdk-overlay-container phx-cell-edit-popup'),
      'cell editor',
    );
    must(popup.querySelector<HTMLElement>('phx-select [role="combobox"]'), 'select').click();
    await settle(fixture);
    const options = [
      ...document.querySelectorAll<HTMLElement>('.cdk-overlay-container [role="option"]'),
    ];
    expect(options.map((option) => option.textContent?.trim())).toEqual(['Active', 'Suspended']);
    must(options[1], 'suspended option').click();
    await settle(fixture);
    must(popup.querySelector<HTMLButtonElement>('button[type="submit"], button.ok'), 'ok').click();
    await settle(fixture);
    await settle(fixture);

    const update = mock.callsOf('dns.zone.update');
    expect(update).toHaveLength(1);
    expect(update[0].params).toEqual({ id: 'z1', status: 'suspended' });
    expect(update[0].options.idempotencyKey).toMatch(UUID);
    expect(mock.callsOf('dns.zone.list')).toHaveLength(listed + 1);
  });
});
