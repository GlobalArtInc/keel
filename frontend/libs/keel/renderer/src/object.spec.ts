import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope } from '@globalart/keel/meta';
import {
  EMPTY_VALUE,
  KeelObjectComponent,
  KeelScreenComponent,
  objectRows,
} from '@globalart/keel/renderer';
import { KeelMockClient } from '@globalart/keel/testing';
import { providePhoenixIcons } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const GET = load<Descriptor>('descriptors/keel.account.get.json');
const ENABLE = load<Descriptor>('descriptors/keel.product.enable.json');
const FIXTURE = load<Boot>('boot.json');
const PRINCIPAL = FIXTURE.principal ?? null;
const BOOT: Boot = {
  ...FIXTURE,
  principal: PRINCIPAL === null ? null : { ...PRINCIPAL, lang: 'en-GB', tz: 'Asia/Tokyo' },
};
const ID = '01J8A0000000000000000000A2';

const ACCOUNT = {
  id: ID,
  kind: 'client',
  parentId: null,
  path: '01J8A0000000000000000000A1.01J8A0000000000000000000A2',
  state: 'active',
  externalId: 'crm-7',
  origin: null,
  createdAt: '2026-09-13T08:30:00Z',
  updatedAt: '2026-09-13T09:45:00Z',
};

const MESSAGES = {
  keel: {
    id: 'Id',
    kind: 'Kind',
    state: 'State',
    createdAt: 'Created',
    updatedAt: 'Updated',
    column: { origin: 'Origin' },
    value: { true: 'Yes', false: 'No' },
    action: { retry: 'Retry' },
    error: {
      not_found: 'Not found',
      forbidden: 'You are not allowed to do this',
      request: '{message} (request {id})',
    },
    account: {
      id: 'Account',
      column: { parentId: 'Parent', path: 'Path', externalId: 'External id' },
      state: { active: 'Active', suspended: 'Suspended' },
    },
    product: {
      state: { enabled: 'Enabled', disabled: 'Disabled' },
      column: { id: 'Product', state: 'State' },
    },
  },
};

function answer(data: unknown, requestId = 'req-get'): Envelope {
  return { ok: true, data, meta: { requestId } };
}

async function render(
  client: KeelMockClient,
  params: Record<string, unknown>,
): Promise<ComponentFixture<KeelObjectComponent>> {
  TestBed.configureTestingModule({
    imports: [KeelObjectComponent],
    providers: [providePhoenixIcons(), { provide: KeelClient, useValue: client }],
  });
  await TestBed.inject(KeelBoot).load();
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelObjectComponent);
  fixture.componentRef.setInput('descriptor', GET);
  fixture.componentRef.setInput('params', params);
  fixture.detectChanges();
  await fixture.whenStable();
  fixture.detectChanges();
  return fixture;
}

function props(element: HTMLElement): [string, string][] {
  return [...element.querySelectorAll('phx-prop')].map((prop) => [
    prop.querySelector('.label-text')?.textContent?.trim() ?? '',
    prop.querySelector('.value-text')?.textContent?.trim() ?? '',
  ]);
}

describe('keel-object', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('a get descriptor renders its result as a property list, the dates in the principal language and zone', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      catalogue: { lang: 'en', messages: MESSAGES },
      answers: { 'keel.account.get': answer(ACCOUNT) },
    });
    const fixture = await render(client, { id: ID });
    const element: HTMLElement = fixture.nativeElement;
    expect(client.calls).toEqual([{ func: 'keel.account.get', params: { id: ID }, options: {} }]);
    expect(element.getAttribute('data-phase')).toBe('loaded');
    expect(element.querySelector('phx-prop-list')).not.toBeNull();
    expect(props(element)).toEqual([
      ['Account', ID],
      ['Kind', 'client'],
      ['Parent', EMPTY_VALUE],
      ['Path', ACCOUNT.path],
      ['State', 'active'],
      ['External id', 'crm-7'],
      ['Origin', EMPTY_VALUE],
      ['Created', '13 Sept 2026, 17:30'],
      ['Updated', '13 Sept 2026, 18:45'],
    ]);
    expect(
      [...element.querySelectorAll('phx-prop')].map((prop) => prop.getAttribute('data-path')),
    ).toEqual([
      'id',
      'kind',
      'parentId',
      'path',
      'state',
      'externalId',
      'origin',
      'createdAt',
      'updatedAt',
    ]);
  });

  it('a failed read shows the error with its request id and retries on demand', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      catalogue: { lang: 'en', messages: MESSAGES },
      answers: {
        'keel.account.get': [
          {
            ok: false,
            error: { code: 'forbidden', key: 'keel.error.forbidden', requestId: 'req-7' },
          },
          answer(ACCOUNT),
        ],
      },
    });
    const fixture = await render(client, { id: ID });
    const element: HTMLElement = fixture.nativeElement;
    expect(element.getAttribute('data-phase')).toBe('failed');
    const error = element.querySelector('phx-error');
    expect(error?.querySelector('.title')).toBeNull();
    expect(error?.querySelector('.text')?.textContent?.trim()).toBe(
      'You are not allowed to do this (request req-7)',
    );
    const retry = error?.querySelector<HTMLButtonElement>('button');
    expect(retry?.textContent?.trim()).toBe('Retry');
    retry?.click();
    await fixture.whenStable();
    fixture.detectChanges();
    expect(client.callsOf('keel.account.get')).toHaveLength(2);
    expect(element.getAttribute('data-phase')).toBe('loaded');
    expect(props(element)[0]).toEqual(['Account', ID]);
  });

  it('the screen renders a get descriptor through the object renderer', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      descriptors: [GET],
      catalogue: { lang: 'en', messages: MESSAGES },
      answers: { 'keel.account.get': answer(ACCOUNT) },
    });
    TestBed.configureTestingModule({
      imports: [KeelScreenComponent],
      providers: [providePhoenixIcons(), { provide: KeelClient, useValue: client }],
    });
    await TestBed.inject(KeelBoot).load();
    await TestBed.inject(KeelI18n).load('en');
    const fixture = TestBed.createComponent(KeelScreenComponent);
    fixture.componentRef.setInput('func', 'keel.account.get');
    fixture.componentRef.setInput('params', { id: ID });
    fixture.detectChanges();
    await fixture.whenStable();
    fixture.detectChanges();
    await fixture.whenStable();
    fixture.detectChanges();
    const element: HTMLElement = fixture.nativeElement;
    expect(element.querySelector('keel-object')?.getAttribute('data-object')).toBe(
      'keel.account.get',
    );
    expect(props(element)).toHaveLength(9);
    expect(client.calls[0].params).toEqual({ id: ID });
  });
});

describe('objectRows', () => {
  it('walks the result schema in order, flattens nested objects and types the values', () => {
    const rows = objectRows(ENABLE, { id: 'dns', state: 'enabled', changed: true, restart: false });
    expect(rows).toEqual([
      {
        path: 'id',
        labelKey: 'keel.product.id',
        columnKey: 'keel.product.column.id',
        value: { kind: 'text', text: 'dns' },
      },
      {
        path: 'state',
        labelKey: 'keel.product.state',
        columnKey: 'keel.product.column.state',
        value: { kind: 'enum', value: 'enabled', labelKey: 'keel.product.state.enabled' },
      },
      {
        path: 'changed',
        labelKey: 'keel.product.changed',
        columnKey: 'keel.product.column.changed',
        value: { kind: 'boolean', value: true },
      },
      {
        path: 'restart',
        labelKey: 'keel.product.restart',
        columnKey: 'keel.product.column.restart',
        value: { kind: 'boolean', value: false },
      },
    ]);
    const nested: Descriptor = {
      ...ENABLE,
      name: 'dns.zone.get',
      result: {
        type: 'object',
        properties: {
          name: { type: 'string' },
          soa: {
            type: 'object',
            properties: {
              primary: { type: 'string' },
              serial: { type: 'integer' },
            },
          },
          tags: { type: 'array', items: { type: 'string' } },
          extra: { type: 'object' },
          missing: { type: 'string' },
        },
      },
    };
    expect(
      objectRows(nested, {
        name: 'a.org',
        soa: { primary: 'ns1.a.org', serial: 2026091301 },
        tags: ['prod', 'eu'],
        extra: { ttl: 300 },
        unknown: 'ignored',
      }).map((row) => [row.path, row.labelKey, row.value]),
    ).toEqual([
      ['name', 'dns.zone.name', { kind: 'text', text: 'a.org' }],
      ['soa.primary', 'dns.zone.soa.primary', { kind: 'text', text: 'ns1.a.org' }],
      ['soa.serial', 'dns.zone.soa.serial', { kind: 'text', text: '2026091301' }],
      ['tags', 'dns.zone.tags', { kind: 'text', text: 'prod, eu' }],
      ['extra', 'dns.zone.extra', { kind: 'json', text: '{"ttl":300}' }],
      ['missing', 'dns.zone.missing', { kind: 'empty' }],
    ]);
  });
});
