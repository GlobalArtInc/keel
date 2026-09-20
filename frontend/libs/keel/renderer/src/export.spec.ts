import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { ApplicationRef } from '@angular/core';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import {
  KEEL_EVENT_SOURCE,
  KeelBoot,
  KeelClient,
  KeelI18n,
  type DownloadResult,
} from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope } from '@globalart/keel/meta';
import { EXPORT_FUNCTION, KeelListComponent, type Row } from '@globalart/keel/renderer';
import { KeelMockClient, QuietEventSource } from '@globalart/keel/testing';
import { PhxModalService, PhxToastService, providePhoenixIcons } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const TASK = '01J9Q3W6B1R8N5X2K4T7M0V9T1';

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const LIST = load<Descriptor>('descriptor-dns-zone-list.json');
const BOOT = load<Boot>('boot.json');
const EXPORTER: Descriptor = {
  ...LIST,
  name: EXPORT_FUNCTION,
  module: 'keel',
  kind: 'download',
  mode: 'read',
  key: null,
  ui: null,
};

const ZONES: Row[] = [
  { id: 'z1', name: 'a.org', records: 3, status: 'active', dnssec: true, account: 'acme' },
  { id: 'z2', name: 'b.org', records: 5, status: 'suspended', dnssec: false, account: 'acme' },
];

const MESSAGES = {
  dns: {
    zone: {
      list: { title: 'Zones' },
      column: { name: 'Name', records: 'Records', status: 'Status', dnssec: 'DNSSEC' },
      action: { create: 'New zone', export: 'Export' },
    },
  },
  keel: {
    export: { format: { csv: 'CSV', json: 'JSON' } },
    error: { request: '{message} (request {id})', unavailable: 'Temporarily unavailable' },
    shell: { tasks: { details: 'Details' } },
  },
  phx: { cancel: 'Cancel' },
};

function ok(data: unknown): Envelope {
  return { ok: true, data, meta: { requestId: 'req-ok' } };
}

function client(): KeelMockClient {
  return new KeelMockClient({
    boot: BOOT,
    descriptors: [LIST, EXPORTER],
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

async function render(mock: KeelMockClient): Promise<ComponentFixture<KeelListComponent>> {
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
  fixture.componentRef.setInput('descriptor', LIST);
  fixture.detectChanges();
  await settle(fixture);
  await settle(fixture);
  return fixture;
}

function overlay(selector: string): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>(`.cdk-overlay-container ${selector}`)];
}

function must<T>(value: T | null | undefined, what: string): T {
  if (value === null || value === undefined) {
    throw new Error(`no ${what}`);
  }
  return value;
}

describe('the export button', () => {
  beforeEach(() => QuietEventSource.reset());

  afterEach(() => {
    TestBed.inject(PhxModalService).closeAll();
    TestBed.inject(PhxToastService).closeAll();
  });

  it('the export dropdown lists the formats and posts keel.export with the table’s query and the visible columns in order', async () => {
    const mock = client().file(EXPORT_FUNCTION, {
      ok: true,
      file: { blob: new Blob(['name\n']), filename: 'zones.csv' },
    });
    URL.createObjectURL = jest.fn(() => 'blob:zones');
    URL.revokeObjectURL = jest.fn();
    const click = jest.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
    const fixture = await render(mock);
    const element: HTMLElement = fixture.nativeElement;
    const list = fixture.componentInstance;
    list.tableSearch.set('org');
    list.tableSort.set({ key: 'name', dir: 'desc' });
    list.tableFilters.set([{ key: 'status', value: ['active'] }]);
    list.order.set(['status', 'name', 'unknown']);
    await settle(fixture);

    must(
      element.querySelector<HTMLButtonElement>('button[data-action="export"]'),
      'button',
    ).click();
    await settle(fixture);
    const items = overlay('phx-dropdown-item');
    expect(items.map((item) => item.textContent?.trim())).toEqual(['CSV', 'JSON']);
    expect(items.map((item) => item.getAttribute('data-format'))).toEqual(['csv', 'json']);

    must(items[1], 'json item').click();
    await settle(fixture);
    await settle(fixture);
    expect(mock.downloads).toEqual([
      {
        func: EXPORT_FUNCTION,
        params: {
          func: 'dns.zone.list',
          query: {
            search: 'org',
            sort: { key: 'name', dir: 'desc' },
            filters: [{ key: 'status', value: ['active'] }],
          },
          columns: ['status', 'name'],
          format: 'json',
        },
      },
    ]);
    expect(overlay('phx-dropdown-item')).toHaveLength(0);
    click.mockRestore();
  });

  it('a TaskRef answer opens the progress modal and a file answer is handed to the browser', async () => {
    const task: DownloadResult = { ok: true, task: { task: { id: TASK } } };
    const mock = client().file(EXPORT_FUNCTION, task);
    const fixture = await render(mock);
    const pending = fixture.componentInstance.export(
      must(
        LIST.ui?.actions?.find((action) => action.role === 'export'),
        'export action',
      ),
      'csv',
    );
    await settle(fixture);
    expect(overlay('keel-progress')).toHaveLength(1);
    expect(overlay('keel-progress')[0].getAttribute('data-task')).toBe(TASK);
    TestBed.inject(PhxModalService).closeAll();
    await settle(fixture);
    await pending;
    expect(fixture.componentInstance.exporting()).toBe(false);
    TestBed.resetTestingModule();

    const file: DownloadResult = {
      ok: true,
      file: { blob: new Blob(['name\na.org\n']), filename: 'zones.csv' },
    };
    const again = client().file(EXPORT_FUNCTION, file);
    const clicks: string[] = [];
    URL.createObjectURL = jest.fn(() => 'blob:zones');
    URL.revokeObjectURL = jest.fn();
    const click = jest.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (
      this: HTMLAnchorElement,
    ) {
      clicks.push(`${this.getAttribute('href')} as ${this.download}`);
    });
    const second = await render(again);
    await second.componentInstance.export(
      must(
        LIST.ui?.actions?.find((action) => action.role === 'export'),
        'export action',
      ),
      'csv',
    );
    await settle(second);
    expect(again.downloads.map((download) => download.func)).toEqual([EXPORT_FUNCTION]);
    expect(clicks).toEqual(['blob:zones as zones.csv']);
    expect(overlay('keel-progress')).toHaveLength(0);
    click.mockRestore();
  });
});
