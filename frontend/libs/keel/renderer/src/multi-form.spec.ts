import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { ApplicationRef } from '@angular/core';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { KEEL_EVENT_SOURCE, KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope } from '@globalart/keel/meta';
import {
  KeelListComponent,
  groupBodyOf,
  groupPathOf,
  isMultiForm,
  multiDescriptorOf,
  type Row,
} from '@globalart/keel/renderer';
import { KeelMockClient, QuietEventSource } from '@globalart/keel/testing';
import { PhxModalService, PhxToastService, providePhoenixIcons } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const LIST = load<Descriptor>('descriptor-dns-zone-list.json');
const EDIT = load<Descriptor>('descriptor-dns-zone-edit.json');
const BOOT = load<Boot>('boot.json');
const MULTI: Descriptor = {
  ...EDIT,
  ui: { ...(EDIT.ui ?? { title: '' }), multi: ['kind', 'ttl'] },
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
      action: { create: 'New zone', edit: 'Edit', delete: 'Delete', export: 'Export' },
      kind: 'Kind',
      'kind.master': 'Master',
      'kind.slave': 'Slave',
      ttl: 'TTL',
    },
  },
  keel: {
    error: { request: '{message} (request {id})', multi_field: 'The field cannot be group-edited' },
    shell: {
      multi: {
        cancel: 'Cancel',
        change: 'Change',
        none: 'Switch on a field',
        save: 'Save',
        title: 'Edit {count} rows',
        updated: '{count} rows updated',
      },
    },
  },
  phx: { cancel: 'Cancel' },
};

function ok(data: unknown): Envelope {
  return { ok: true, data, meta: { requestId: 'req-ok' } };
}

function client(): KeelMockClient {
  return new KeelMockClient({
    boot: BOOT,
    descriptors: [LIST, MULTI],
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

async function openModal(fixture: ComponentFixture<KeelListComponent>): Promise<HTMLElement> {
  fixture.componentInstance.selection.set(new Set(['z1', 'z2']));
  await settle(fixture);
  const element: HTMLElement = fixture.nativeElement;
  must(
    element.querySelector<HTMLButtonElement>('phx-toolbar button[data-action="edit"]'),
    'edit in the toolbar pill',
  ).click();
  await settle(fixture);
  await settle(fixture);
  return must(overlay('keel-multi-form')[0], 'multi form modal');
}

function toggle(modal: HTMLElement, name: string): void {
  must(
    modal.querySelector<HTMLInputElement>(`[data-multi="${name}"] phx-toggle input`),
    `${name} toggle`,
  ).click();
}

describe('the multi-row form', () => {
  beforeEach(() => QuietEventSource.reset());

  afterEach(() => {
    TestBed.inject(PhxModalService).closeAll();
    TestBed.inject(PhxToastService).closeAll();
  });

  it('the toolbar pill offers edit when the form has ui.multi and the modal posts ids, fields and mask for the toggled fields alone', async () => {
    const mock = client().answer('dns.zone.edit', ok({ updated: 2 }));
    const fixture = await render(mock);
    expect(isMultiForm(MULTI)).toBe(true);
    expect(isMultiForm(EDIT)).toBe(true);
    expect(isMultiForm({ ...EDIT, ui: { ...(EDIT.ui ?? { title: '' }), multi: [] } })).toBe(false);
    expect(multiDescriptorOf(MULTI).ui?.pages).toEqual([]);
    expect(fixture.componentInstance.selectable()).toBe(true);
    expect(fixture.componentInstance.selectionActions().map((action) => action.id)).toEqual([
      'edit',
      'records',
      'suspend',
      'delete',
    ]);

    const modal = await openModal(fixture);
    expect(modal.getAttribute('data-count')).toBe('2');
    expect(
      [...modal.querySelectorAll<HTMLElement>('[data-multi]')].map((row) => row.dataset['multi']),
    ).toEqual(['kind', 'ttl']);
    const save = must(modal.querySelector<HTMLButtonElement>('button.save'), 'save');
    expect(save.disabled).toBe(true);
    const ttlInput = must(
      modal.querySelector<HTMLInputElement>('[data-multi="ttl"] phx-number-field input'),
      'ttl input',
    );
    expect(ttlInput.disabled).toBe(true);

    toggle(modal, 'ttl');
    await settle(fixture);
    expect(ttlInput.disabled).toBe(false);
    expect(save.disabled).toBe(false);
    ttlInput.value = '600';
    ttlInput.dispatchEvent(new Event('input'));
    await settle(fixture);
    const listed = mock.callsOf('dns.zone.list').length;
    save.click();
    await settle(fixture);
    await settle(fixture);

    const posted = mock.callsOf('dns.zone.edit');
    expect(posted).toHaveLength(1);
    expect(posted[0].params).toEqual({ ids: ['z1', 'z2'], fields: { ttl: 600 }, mask: ['ttl'] });
    expect(posted[0].options.idempotencyKey).toMatch(UUID);
    expect(overlay('keel-multi-form')).toHaveLength(0);
    expect(mock.callsOf('dns.zone.list')).toHaveLength(listed + 1);
    expect(fixture.componentInstance.selection().size).toBe(0);
    expect(
      TestBed.inject(PhxToastService)
        .toasts()
        .map((toast) => toast.text),
    ).toEqual(['2 rows updated']);
  });

  it('a 422 on mask lands on its row and a GroupResult reloads the table', async () => {
    const refused: Envelope = {
      ok: false,
      error: {
        code: 'validation',
        key: 'keel.error.validation',
        fields: [{ path: 'mask[0]', code: 'validation', key: 'keel.error.multi_field' }],
      },
    };
    const mock = client().answer('dns.zone.edit', [refused, ok({ updated: 2 })]);
    const fixture = await render(mock);
    const modal = await openModal(fixture);
    toggle(modal, 'kind');
    await settle(fixture);
    const save = must(modal.querySelector<HTMLButtonElement>('button.save'), 'save');
    save.click();
    await settle(fixture);
    expect(mock.callsOf('dns.zone.edit')).toHaveLength(0);
    expect(modal.querySelector('[data-multi="kind"] phx-field-error')).not.toBeNull();

    must(
      modal.querySelector<HTMLElement>('[data-multi="kind"] phx-select [role="combobox"]'),
      'kind select',
    ).click();
    await settle(fixture);
    const slave = must(
      overlay('[role="option"]').find((option) => option.textContent?.trim() === 'Slave'),
      'slave option',
    );
    slave.click();
    await settle(fixture);
    save.click();
    await settle(fixture);
    await settle(fixture);
    expect(mock.callsOf('dns.zone.edit')[0].params).toEqual({
      ids: ['z1', 'z2'],
      fields: { kind: 'slave' },
      mask: ['kind'],
    });
    expect(overlay('keel-multi-form')).toHaveLength(1);
    expect(modal.querySelector('[data-multi="kind"] phx-field-error')?.textContent?.trim()).toBe(
      'The field cannot be group-edited',
    );
    expect(modal.querySelector('[data-multi="ttl"] phx-field-error')).toBeNull();
    expect(groupPathOf('mask[1]', ['kind', 'ttl'])).toBe('ttl');
    expect(groupPathOf('fields.ttl', ['kind', 'ttl'])).toBe('ttl');
    expect(groupPathOf('ids', ['kind'])).toBe('ids');

    const listed = mock.callsOf('dns.zone.list').length;
    must(
      modal.querySelector<HTMLElement>('[data-multi="kind"] phx-select [role="combobox"]'),
      'kind select',
    ).click();
    await settle(fixture);
    must(
      overlay('[role="option"]').find((option) => option.textContent?.trim() === 'Master'),
      'master option',
    ).click();
    await settle(fixture);
    expect(modal.querySelector('[data-multi="kind"] phx-field-error')).toBeNull();
    save.click();
    await settle(fixture);
    await settle(fixture);
    expect(mock.callsOf('dns.zone.edit')).toHaveLength(2);
    expect(mock.callsOf('dns.zone.edit')[1].params).toEqual({
      ids: ['z1', 'z2'],
      fields: { kind: 'master' },
      mask: ['kind'],
    });
    expect(overlay('keel-multi-form')).toHaveLength(0);
    expect(mock.callsOf('dns.zone.list')).toHaveLength(listed + 1);
  });

  it('groupBodyOf keeps the toggled fields alone and nulls a blank nullable one', () => {
    const fields = [
      { name: 'kind', nullable: false, control: { value: 'slave' } },
      { name: 'ttl', nullable: false, control: { value: 300 } },
      { name: 'account', nullable: true, control: { value: '' } },
    ];
    expect(groupBodyOf(['a', 'b'], fields as never, new Set(['kind', 'account']))).toEqual({
      ids: ['a', 'b'],
      fields: { kind: 'slave', account: null },
      mask: ['kind', 'account'],
    });
  });
});
